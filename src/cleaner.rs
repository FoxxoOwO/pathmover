//! Electron/Chromium cache discovery and cleaning.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::fsutil;
use crate::scanner::{self, ScanProgress};
use crate::util;
use crate::winapi;

/// A single cache directory found inside AppData.
#[derive(Debug, Clone)]
pub struct CacheTarget {
    /// Owning application, e.g. `Code`.
    pub app: String,
    /// Cache kind, e.g. `Code Cache`.
    pub kind: String,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    /// Selected for cleaning in the UI.
    pub selected: bool,
    /// The folder is a junction - never delete through it.
    pub is_link: bool,
}

impl CacheTarget {
    pub fn display_path(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

/// Cache directory names recognised inside AppData (case-insensitive).
fn cache_kind(name: &str, include_logs: bool) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let kind = match lower.as_str() {
        "cache" => "Cache",
        "code cache" => "Code Cache",
        "gpucache" => "GPUCache",
        "dawncache" => "DawnCache",
        "dawngraphitecache" => "DawnGraphiteCache",
        "dawnwebgpucache" => "DawnWebGPUCache",
        "grshadercache" => "GrShaderCache",
        "shadercache" | "shader_cache" => "ShaderCache",
        "cacheddata" => "CachedData",
        "cachestorage" => "CacheStorage",
        "crashpad" | "crashpad\\pending" => "Crashpad",
        "blob_storage" => "Blob Storage",
        "logs" | "log" if include_logs => "Logs",
        _ => return None,
    };
    Some(kind)
}

/// Reuse a single walk per AppData branch, stopping at cache directories.
pub fn scan(
    include_logs: bool,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancel: &AtomicBool,
) -> Vec<CacheTarget> {
    // Phase 1: find candidate directories (cheap, no sizes yet).
    let mut candidates: Vec<(PathBuf, String, String)> = Vec::new();
    for (_, root) in scanner::appdata_roots() {
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(4)
            .into_iter()
            .filter_entry(|e| {
                // Never descend into a cache folder or into a junction.
                if e.depth() == 0 {
                    return true;
                }
                if winapi::is_reparse_point(e.path()) {
                    return false;
                }
                !(e.file_type().is_dir()
                    && cache_kind(&e.file_name().to_string_lossy(), true).is_some())
            })
            .filter_map(Result::ok)
        {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            if !entry.file_type().is_dir() || entry.depth() == 0 {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(kind) = cache_kind(&name, include_logs) else {
                continue;
            };
            let Some(app) = app_owner(&root, entry.path()) else {
                continue;
            };
            candidates.push((entry.path().to_path_buf(), app, kind.to_string()));
        }
    }

    // Phase 2: measure them in parallel.
    let total = candidates.len();
    let done = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let current = Mutex::new(String::new());

    let mut targets: Vec<CacheTarget> = candidates
        .par_iter()
        .filter_map(|(path, app, kind)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let size = fsutil::dir_size(path);
            let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
            bytes.fetch_add(size.bytes, Ordering::Relaxed);
            if let Ok(mut c) = current.lock() {
                *c = app.clone();
            }
            progress(ScanProgress {
                folders_total: total,
                folders_done: processed,
                bytes: bytes.load(Ordering::Relaxed),
                current: app.clone(),
            });
            Some(CacheTarget {
                app: app.clone(),
                kind: kind.clone(),
                path: path.clone(),
                size: size.bytes,
                files: size.files,
                selected: true,
                is_link: winapi::is_reparse_point(path),
            })
        })
        .collect();

    targets.sort_by_key(|a| std::cmp::Reverse(a.size));
    targets
}

/// The first path component below an AppData root, e.g. `Code`.
fn app_owner(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let first = rel.components().next()?;
    let name = first.as_os_str().to_string_lossy().to_string();
    // Skip `.git`, `.config` and similar hidden bookkeeping folders: use the
    // next component so the label stays meaningful.
    if name.starts_with('.') || name.eq_ignore_ascii_case("Packages") {
        let second = rel.components().nth(1)?;
        return Some(format!(
            "{}\\{}",
            name,
            second.as_os_str().to_string_lossy()
        ));
    }
    Some(name)
}

/// Summary of a cleaning run.
#[derive(Debug, Default, Clone)]
pub struct CleanReport {
    pub freed: u64,
    pub deleted: u64,
    pub errors: Vec<String>,
    pub skipped_links: Vec<String>,
    pub cancelled: bool,
    /// Fields kept for the UI summary line.
    pub dirs: usize,
}

impl CleanReport {
    pub fn success(&self) -> bool {
        self.errors.is_empty() && !self.cancelled
    }
}

/// Delete the contents of the selected cache directories.
///
/// Locked files (an Electron app is running) are reported but never fatal.
pub fn clean(
    targets: &[PathBuf],
    progress: &(dyn Fn(u64, u64, &str) + Sync),
    cancel: &AtomicBool,
) -> CleanReport {
    let mut report = CleanReport {
        dirs: targets.len(),
        ..Default::default()
    };
    let mut seen: HashSet<String> = HashSet::new();

    for (index, path) in targets.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let key = util::normalize_path_str(path);
        if !seen.insert(key) {
            continue;
        }
        if !scanner::is_under_appdata(path) {
            report
                .errors
                .push(format!("{}: mimo AppData, přeskakuji", path.display()));
            continue;
        }
        if winapi::is_reparse_point(path) {
            report.skipped_links.push(path.display().to_string());
            continue;
        }
        let mut freed_in_dir = 0u64;
        let outcome = fsutil::empty_dir(path, cancel, &mut |freed, _| {
            freed_in_dir = freed;
            progress(report.freed + freed, index as u64, &path.display().to_string());
        });
        report.freed += outcome.bytes.max(freed_in_dir);
        report.deleted += outcome.deleted;
        report.errors.extend(outcome.errors);
        report.cancelled |= outcome.cancelled;
        progress(report.freed, (index + 1) as u64, &path.display().to_string());
    }

    // Keep the error list readable.
    if report.errors.len() > 40 {
        let hidden = report.errors.len() - 40;
        report.errors.truncate(40);
        report
            .errors
            .push(format!("... a {hidden} dalších chyb (soubory jsou pravděpodobně zamčené)"));
    }
    report
}

/// Total size of the selected targets.
pub fn total_selected(targets: &[CacheTarget]) -> u64 {
    targets.iter().filter(|t| t.selected).map(|t| t.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_cache_names() {
        assert_eq!(cache_kind("Code Cache", true), Some("Code Cache"));
        assert_eq!(cache_kind("gpucache", true), Some("GPUCache"));
        assert_eq!(cache_kind("Cache", true), Some("Cache"));
        assert_eq!(cache_kind("logs", true), Some("Logs"));
        assert_eq!(cache_kind("logs", false), None);
        assert_eq!(cache_kind("Local Storage", true), None);
        assert_eq!(cache_kind("IndexedDB", true), None);
    }

    #[test]
    fn empties_directory_but_keeps_it() {
        let base = std::env::temp_dir().join("linkvault-test-empty-dir");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub/a"), b"12345").unwrap();
        let cancel = AtomicBool::new(false);
        let out = fsutil::empty_dir(&base, &cancel, &mut |_, _| {});
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(base.is_dir());
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}