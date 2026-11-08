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

/// Which branch of the user profile a folder lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Local,
    Roaming,
    /// Top level of `%USERPROFILE%` (`.gradle`, `.cargo`, `Downloads`, ...).
    Profile,
}

impl Scope {
    /// Folder name used below the destination root and in `config.toml`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Scope::Local => "Local",
            Scope::Roaming => "Roaming",
            Scope::Profile => "Profile",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Local => "AppData\\Local",
            Scope::Roaming => "AppData\\Roaming",
            Scope::Profile => "Profil uživatele",
        }
    }

    /// Short badge text shown in the folder list.
    pub fn badge(self) -> &'static str {
        match self {
            Scope::Local => "Local",
            Scope::Roaming => "Roaming",
            Scope::Profile => "Profil",
        }
    }

    pub fn parse(value: &str) -> Scope {
        if value.eq_ignore_ascii_case("Roaming") {
            Scope::Roaming
        } else if value.eq_ignore_ascii_case("Profile") {
            Scope::Profile
        } else {
            Scope::Local
        }
    }
}

/// Top level of the current user profile, `%USERPROFILE%`.
pub fn user_profile_root() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Folders directly in `%USERPROFILE%` that LinkVault never moves.
///
/// Moving these either breaks the profile itself (AppData, registry hives),
/// breaks Known Folder redirection / cloud sync (Desktop, Documents, OneDrive)
/// or silently breaks credentials and shell integration (.ssh, .gnupg,
/// Favorites, ...).
const PROFILE_BLOCKED: &[(&str, &str)] = &[
    ("appdata", "AppData se přesouvá po částech ve scope Local/Roaming"),
    ("desktop", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("documents", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("downloads", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("pictures", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("music", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("videos", "složka je Known Folder a může být přesměrovaná do OneDrive"),
    ("contacts", "systémová složka profilu"),
    ("links", "systémová složka profilu"),
    ("favorites", "systémová složka profilu"),
    ("saved games", "systémová složka profilu"),
    ("searches", "systémová složka profilu"),
    ("application data", "kompatibilitní junction do AppData"),
    ("local settings", "kompatibilitní junction do AppData"),
    ("cookies", "kompatibilitní junction do AppData"),
    ("nethood", "kompatibilitní junction do AppData"),
    ("printer shortcuts", "kompatibilitní junction do AppData"),
    ("recent", "kompatibilitní junction do AppData"),
    ("sendto", "kompatibilitní junction do AppData"),
    ("start menu", "kompatibilitní junction do AppData"),
    ("templates", "kompatibilitní junction do AppData"),
    ("desktop.ini", "konfigurační soubor profilu"),
    (".ssh", "obsahuje klíče a konfiguraci SSH"),
    (".gnupg", "obsahuje klíče a konfiguraci GnuPG"),
];

/// Reason why a top-level profile folder must not be moved (`None` = movable).
pub fn profile_block_reason(name: &str) -> Option<String> {
    let lower = name.trim().to_lowercase();
    if lower.starts_with("onedrive") {
        return Some("synchronizovaná složka OneDrive - přesun by rozbil sync".to_string());
    }
    if lower.starts_with("ntuser") {
        return Some("registrové hives profilu (NTUSER.DAT)".to_string());
    }
    PROFILE_BLOCKED
        .iter()
        .find(|(blocked, _)| *blocked == lower)
        .map(|(_, reason)| (*reason).to_string())
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
    /// Lives in `%USERPROFILE%` but must stay there.
    Blocked { reason: String },
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
            FolderState::Blocked { reason } => format!("Vyloučeno: {reason}"),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, FolderState::Local)
    }

    #[allow(dead_code)] // used by tests and callers that filter the list
    pub fn is_blocked(&self) -> bool {
        matches!(self, FolderState::Blocked { .. })
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

/// True when `path` lives inside the current user profile.
pub fn is_under_profile(path: &Path) -> bool {
    user_profile_root()
        .map(|root| util::starts_with_ci(path, &root))
        .unwrap_or(false)
}

/// True when `path` is a folder LinkVault is allowed to move: inside AppData,
/// or a non-blocked top-level folder of the user profile.
pub fn is_movable_source(path: &Path) -> bool {
    if is_under_appdata(path) {
        return true;
    }
    let Some(root) = user_profile_root() else {
        return false;
    };
    if !is_under_profile(path) {
        return false;
    }
    // The profile root itself is never a move candidate.
    if util::paths_eq(path, &root) {
        return false;
    }
    profile_reason_relative(path, &root).is_none()
}

/// Block reason for `path` relative to the profile root, if any. Only the
/// first component is checked, so `.ssh` stays blocked but a nested folder is
/// not affected by the name of its parent.
fn profile_reason_relative(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let first = rel.components().next()?;
    let name = first.as_os_str().to_string_lossy();
    profile_block_reason(&name)
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

/// `classify` plus the user-profile blocklist, so a protected folder is
/// reported as `Blocked` instead of offering a move that would be refused.
pub fn classify_scoped(path: &Path, scope: Scope) -> FolderState {
    if scope == Scope::Profile {
        if let Some(root) = user_profile_root() {
            if let Some(reason) = profile_reason_relative(path, &root) {
                return FolderState::Blocked { reason };
            }
        }
    }
    classify(path)
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
    let mut roots: Vec<(Scope, PathBuf)> = appdata_roots();
    if let Some(profile) = user_profile_root() {
        roots.push((Scope::Profile, profile));
    }
    for (scope, root) in roots {
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
            let state = classify_scoped(path, *scope);
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
        assert_eq!(Scope::parse("Profile"), Scope::Profile);
        assert_eq!(Scope::parse("profile"), Scope::Profile);
        assert_eq!(Scope::parse("Local"), Scope::Local);
        assert_eq!(Scope::parse("Weird"), Scope::Local);
    }

    #[test]
    fn profile_blocklist_protects_system_folders() {
        for name in [
            "AppData",
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            "Contacts",
            "Favorites",
            ".ssh",
            ".gnupg",
            "OneDrive",
            "OneDrive - Contoso",
            "NTUSER.DAT",
        ] {
            assert!(
                profile_block_reason(name).is_some(),
                "{name} must be blocked"
            );
        }
        for name in [
            ".gradle",
            ".cargo",
            ".vscode",
            ".android",
            "Zalohy",
            "Projects",
        ] {
            assert!(
                profile_block_reason(name).is_none(),
                "{name} must stay movable"
            );
        }
    }

    #[test]
    fn blocked_profile_folders_are_classified_as_blocked() {
        let Some(root) = user_profile_root() else {
            return;
        };
        let state = classify_scoped(&root.join(".ssh"), Scope::Profile);
        assert!(state.is_blocked());
        assert!(!state.is_local());
        // The same name outside the profile keeps its normal classification.
        let elsewhere = std::env::temp_dir().join("no-such-folder");
        assert_eq!(
            classify_scoped(&elsewhere, Scope::Local),
            FolderState::Missing
        );
    }

    #[test]
    fn movable_source_accepts_appdata_and_safe_profile_folders() {
        let Some(root) = user_profile_root() else {
            return;
        };
        assert!(is_movable_source(&root.join("AppData").join("Local")));
        assert!(!is_movable_source(&root.join("Downloads")));
        assert!(!is_movable_source(&root.join("Documents")));
        assert!(!is_movable_source(&root.join(".ssh")));
        // The profile root itself is never movable.
        assert!(!is_movable_source(&root));
        assert!(!is_movable_source(Path::new(r"C:\Windows")));
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