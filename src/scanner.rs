//! Discovery of AppData folders and their current location state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;

use crate::config::MovedEntry;
use crate::fsutil::{self, TreeSize};
use crate::util;
use crate::winapi;

/// Which AppData branch a folder lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Local,
    Roaming,
}

impl Scope {
    /// Folder name inside `%USERPROFILE%\AppData`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Scope::Local => "Local",
            Scope::Roaming => "Roaming",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Local => "AppData\\Local",
            Scope::Roaming => "AppData\\Roaming",
        }
    }

    pub fn parse(value: &str) -> Scope {
        if value.eq_ignore_ascii_case("Roaming") {
            Scope::Roaming
        } else {
            Scope::Local
        }
    }
}

/// Where a folder currently lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderState {
    /// A normal directory on C: that LinkVault can relocate.
    Local,
    /// A junction pointing at another drive.
    Junction {
        target: PathBuf,
        /// The junction resolves to an existing directory.
        healthy: bool,
    },
    /// A junction whose target is gone.
    Broken { reason: String },
    /// Recorded in config.toml but the path no longer exists at all.
    Missing,
}

impl FolderState {
    pub fn short_label(&self) -> String {
        match self {
            FolderState::Local => "Na disku C:".to_string(),
            FolderState::Junction { target, healthy } => {
                if *healthy {
                    format!("Přesunuto → {}", util::drive_of(target).unwrap_or_else(|| "?".into()))
                } else {
                    "Odkaz nefunkční".to_string()
                }
            }
            FolderState::Broken { .. } => "Poškozený odkaz".to_string(),
            FolderState::Missing => "Chybí".to_string(),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, FolderState::Local)
    }

    pub fn is_moved(&self) -> bool {
        matches!(self, FolderState::Junction { .. })
    }
}

/// One scan result row.
#[derive(Debug, Clone)]
pub struct FolderEntry {
    pub name: String,
    pub scope: Scope,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub state: FolderState,
    /// Matching record from config.toml (LinkVault moved it).
    pub managed: Option<MovedEntry>,
}

impl FolderEntry {
    /// Can LinkVault move it right now?
    pub fn can_move(&self) -> bool {
        self.state.is_local()
    }

    /// Can LinkVault bring it back to C:?
    pub fn can_restore(&self) -> bool {
        self.managed.is_some() && self.state.is_moved()
    }
}

/// Progress snapshot emitted while scanning.
#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub folders_total: usize,
    pub folders_done: usize,
    pub bytes: u64,
    pub current: String,
}

pub type ProgressFn<'a> = &'a (dyn Fn(ScanProgress) + Sync);

/// `%LOCALAPPDATA%` and `%APPDATA%` (only the ones that exist).
pub fn appdata_roots() -> Vec<(Scope, PathBuf)> {
    let userprofile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let mut roots = Vec::new();

    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| userprofile.as_ref().map(|p| p.join("AppData").join("Local")));
    if let Some(p) = local {
        if p.is_dir() {
            roots.push((Scope::Local, p));
        }
    }

    let roaming = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| userprofile.as_ref().map(|p| p.join("AppData").join("Roaming")));
    if let Some(p) = roaming {
        if p.is_dir() {
            roots.push((Scope::Roaming, p));
        }
    }
    roots
}

/// True when `path` lives inside one of the AppData branches (safety guard:
/// LinkVault must never touch anything else).
pub fn is_under_appdata(path: &Path) -> bool {
    appdata_roots()
        .iter()
        .any(|(_, root)| util::starts_with_ci(path, root))
}

/// Determine the state of a single folder without measuring it.
pub fn classify(path: &Path) -> FolderState {
    if winapi::is_reparse_point(path) {
        return match winapi::junction_target(path) {
            Some(target) => {
                let healthy = target.is_dir();
                if healthy {
                    FolderState::Junction { target, healthy }
                } else {
                    FolderState::Broken {
                        reason: format!("cíl {} neexistuje", target.display()),
                    }
                }
            }
            None => FolderState::Broken {
                reason: "reparse point bez čitelného cíle".to_string(),
            },
        };
    }
    if !path.exists() {
        return FolderState::Missing;
    }
    FolderState::Local
}

/// Scan AppData and return every top-level folder, sorted by size (largest
/// first). Sizes of already-moved folders are read from config.toml to avoid
/// re-walking the other drive.
pub fn scan(
    managed: &[MovedEntry],
    progress: ProgressFn<'_>,
    cancel: &AtomicBool,
) -> Vec<FolderEntry> {
    let mut work: Vec<(Scope, PathBuf, String)> = Vec::new();
    for (scope, root) in appdata_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Include junctions even though `is_dir()` is false for broken ones.
            let is_dir = no_follow_is_dir(&path);
            if is_dir {
                work.push((scope, path, name));
            }
        }
    }

    let total = work.len();
    let done = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let current = Mutex::new(String::new());

    let managed_map: HashMap<String, MovedEntry> = managed
        .iter()
        .map(|m| (util::normalize_path_str(Path::new(&m.source)), m.clone()))
        .collect();

    let mut entries: Vec<FolderEntry> = work
        .par_iter()
        .filter_map(|(scope, path, name)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let state = classify(path);
            let key = util::normalize_path_str(path);
            let record = managed_map.get(&key).cloned();

            let size = match &state {
                FolderState::Local => {
                    let TreeSize { bytes, files, .. } = fsutil::dir_size(path);
                    *current.lock().unwrap() = name.clone();
                    Some((bytes, files))
                }
                FolderState::Junction { .. } => record.as_ref().map(|r| (r.bytes, 0)),
                _ => None,
            };

            let (size_bytes, files) = size.unwrap_or((0, 0));
            let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
            bytes.fetch_add(size_bytes, Ordering::Relaxed);
            progress(ScanProgress {
                folders_total: total,
                folders_done: processed,
                bytes: bytes.load(Ordering::Relaxed),
                current: current.lock().map(|c| c.clone()).unwrap_or_default(),
            });

            Some(FolderEntry {
                name: name.clone(),
                scope: *scope,
                path: path.clone(),
                size: size_bytes,
                files,
                state,
                managed: record,
            })
        })
        .collect();

    // Surface config entries whose folder vanished entirely, so the user can
    // see stale records instead of silently losing track of the data.
    if !cancel.load(Ordering::Relaxed) {
        let known: Vec<String> = entries
            .iter()
            .map(|e| util::normalize_path_str(&e.path))
            .collect();
        for record in managed {
            let key = util::normalize_path_str(Path::new(&record.source));
            if !known.contains(&key) {
                entries.push(FolderEntry {
                    name: record.name.clone(),
                    scope: Scope::parse(&record.scope),
                    path: PathBuf::from(&record.source),
                    size: record.bytes,
                    files: 0,
                    state: FolderState::Missing,
                    managed: Some(record.clone()),
                });
            }
        }
    }

    entries.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    entries
}

/// `path` is a directory, following junctions only one level.
fn no_follow_is_dir(path: &Path) -> bool {
    if winapi::is_reparse_point(path) {
        // Junctions and symlinks to directories count as folders.
        winapi::junction_target(path)
            .map(|target| target.is_dir())
            .unwrap_or(true)
    } else {
        path.is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parsing_is_lenient() {
        assert_eq!(Scope::parse("Roaming"), Scope::Roaming);
        assert_eq!(Scope::parse("roaming"), Scope::Roaming);
        assert_eq!(Scope::parse("Local"), Scope::Local);
        assert_eq!(Scope::parse("Weird"), Scope::Local);
    }

    #[test]
    fn local_directories_are_classified() {
        let dir = std::env::temp_dir().join("linkvault-test-classify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(classify(&dir), FolderState::Local);
        assert_eq!(classify(&dir.join("nope")), FolderState::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appdata_roots_exist_on_windows() {
        let roots = appdata_roots();
        assert!(!roots.is_empty(), "expected at least one AppData root");
        assert!(roots.iter().all(|(_, p)| p.is_dir()));
    }
}