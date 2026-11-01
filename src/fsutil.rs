//! Filesystem primitives: measuring, copying and deleting directory trees
//! with progress reporting and cancellation.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use walkdir::WalkDir;

use crate::winapi;

/// Result of measuring a tree.
#[derive(Debug, Default, Clone, Copy)]
pub struct TreeSize {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
}

/// Recursively measure a directory without following junctions/symlinks.
pub fn dir_size(path: &Path) -> TreeSize {
    let mut size = TreeSize::default();
    for entry in WalkDir::new(winapi::extended(path))
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir() {
            size.dirs += 1;
        } else if entry.file_type().is_file() {
            if let Ok(md) = entry.metadata() {
                size.bytes += md.len();
                size.files += 1;
            }
        }
    }
    size
}

/// Aggregated result of a copy operation.
#[derive(Debug, Default)]
pub struct CopyOutcome {
    pub bytes: u64,
    pub files: u64,
    pub errors: Vec<String>,
    pub skipped_links: Vec<String>,
    pub cancelled: bool,
    pub aborted: Option<String>,
}

/// Errors that mean "stop now, the target volume cannot hold the data".
fn is_fatal_copy_error(err: &io::Error) -> Option<String> {
    match err.raw_os_error() {
        // ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL
        Some(112) | Some(39) => Some("na cílovém disku došlo místo".to_string()),
        _ => None,
    }
}

/// Copy `src` into `dst` recursively.
///
/// * `progress` receives `(bytes_done, current_relative_path)` every ~64 MiB
///   and at most a few hundred times per second.
/// * `cancel` is polled before every file; a cancelled copy returns
///   `outcome.cancelled == true` and the caller is expected to clean up `dst`.
pub fn copy_tree(
    src: &Path,
    dst: &Path,
    progress: &mut dyn FnMut(u64, &str),
    cancel: &AtomicBool,
) -> io::Result<CopyOutcome> {
    let mut outcome = CopyOutcome::default();
    std::fs::create_dir_all(winapi::extended(dst))?;

    let mut last_report_bytes: u64 = 0;
    let mut last_report = std::time::Instant::now();

    for entry in WalkDir::new(winapi::extended(src))
        .follow_links(false)
        .into_iter()
    {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            return Ok(outcome);
        }
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                outcome.errors.push(format!("{err}"));
                continue;
            }
        };
        // Never follow or duplicate nested junctions/symlinks: they are
        // reported so the UI can explain what was skipped.
        if entry.depth() > 0 && winapi::is_reparse_point(entry.path()) {
            outcome
                .skipped_links
                .push(entry.path().to_string_lossy().to_string());
            continue;
        }
        let rel = match entry.path().strip_prefix(winapi::extended(src)) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => entry
                .path()
                .strip_prefix(src)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| PathBuf::from(entry.file_name())),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(&rel);

        if entry.file_type().is_dir() {
            if let Err(err) = std::fs::create_dir_all(winapi::extended(&target)) {
                outcome
                    .errors
                    .push(format!("{}: {err}", target.display()));
            }
        } else if entry.file_type().is_file() {
            let ext_target = winapi::extended(&target);
            if let Some(parent) = ext_target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            match std::fs::copy(entry.path(), &ext_target) {
                Ok(_) => {
                    outcome.bytes += len;
                    outcome.files += 1;
                }
                Err(err) => {
                    if let Some(fatal) = is_fatal_copy_error(&err) {
                        outcome.aborted = Some(fatal);
                        return Ok(outcome);
                    }
                    outcome
                        .errors
                        .push(format!("{}: {err}", entry.path().display()));
                }
            }
            if outcome.bytes - last_report_bytes > 64 * 1024 * 1024
                || last_report.elapsed() > std::time::Duration::from_millis(120)
            {
                last_report_bytes = outcome.bytes;
                last_report = std::time::Instant::now();
                progress(outcome.bytes, &rel.to_string_lossy());
            }
        }
    }
    progress(outcome.bytes, "");
    Ok(outcome)
}

/// Compare two trees by byte count and file count (used to verify a copy).
pub fn verify_same(a: &Path, b: &Path) -> Result<(), String> {
    let sa = dir_size(a);
    let sb = dir_size(b);
    if sa.bytes != sb.bytes || sa.files != sb.files {
        return Err(format!(
            "ověření selhalo: zdroj {} / {}, kopie {} / {}",
            crate::util::human_size(sa.bytes),
            sa.files,
            crate::util::human_size(sb.bytes),
            sb.files
        ));
    }
    Ok(())
}

/// Result of a recursive delete.
#[derive(Debug, Default)]
pub struct RemoveOutcome {
    pub bytes: u64,
    pub deleted: u64,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

/// Delete a directory tree, clearing read-only attributes when needed.
///
/// When `keep_root` is true the directory itself survives and only its
/// contents are removed (used by the cache cleaner).
pub fn remove_tree(
    path: &Path,
    keep_root: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, &str),
) -> RemoveOutcome {
    let mut outcome = RemoveOutcome::default();
    if !path.exists() && !winapi::is_reparse_point(path) {
        return outcome;
    }
    // A junction is a link, never delete through it.
    if winapi::is_reparse_point(path) {
        if let Err(err) = winapi::remove_junction(path) {
            outcome.errors.push(format!("{}: {err}", path.display()));
        }
        return outcome;
    }

    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(winapi::extended(path))
        .follow_links(false)
        .contents_first(true)
        .into_iter()
    {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            return outcome;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().is_dir() {
            dirs.push(entry.path().to_path_buf());
            continue;
        }
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let mut result = std::fs::remove_file(entry.path());
        if result.is_err() {
            // Locked file or read-only attribute: retry once after clearing it.
            let _ = winapi::make_writable(entry.path());
            result = std::fs::remove_file(entry.path());
        }
        match result {
            Ok(()) => {
                outcome.bytes += len;
                outcome.deleted += 1;
            }
            Err(err) => outcome
                .errors
                .push(format!("{}: {err}", entry.path().display())),
        }
        progress(outcome.bytes, "");
    }

    // Files were removed first (contents_first), so directories are now empty.
    // `contents_first` yields the deepest directory first - keep that order.
    for dir in dirs.iter() {
        let ext_dir = winapi::extended(dir);
        let mut result = std::fs::remove_dir(&ext_dir);
        if result.is_err() {
            let _ = winapi::make_writable(dir);
            result = std::fs::remove_dir(&ext_dir);
        }
        if let Err(err) = result {
            outcome.errors.push(format!("{}: {err}", dir.display()));
        }
    }

    if !keep_root {
        let ext_path = winapi::extended(path);
        let mut result = std::fs::remove_dir(&ext_path);
        if result.is_err() {
            let _ = winapi::make_writable(path);
            result = std::fs::remove_dir(&ext_path);
        }
        if let Err(err) = result {
            outcome.errors.push(format!("{}: {err}", path.display()));
        }
    }
    progress(outcome.bytes, "");
    outcome
}

/// Remove everything inside a directory but keep the directory itself.
pub fn empty_dir(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, &str),
) -> RemoveOutcome {
    let mut total = RemoveOutcome::default();
    if winapi::is_reparse_point(path) {
        total.errors.push(format!(
            "{} je odkaz (junction) - přeskakuji",
            path.display()
        ));
        return total;
    }
    let Ok(rd) = std::fs::read_dir(winapi::extended(path)) else {
        return total;
    };
    let children: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    for child in children {
        let mut out = remove_tree(&child, false, cancel, progress);
        total.bytes += out.bytes;
        total.deleted += out.deleted;
        total.errors.append(&mut out.errors);
        if out.cancelled {
            total.cancelled = true;
            break;
        }
    }
    total
}

/// Best-effort removal of a directory, ignoring all errors.
pub fn remove_tree_quiet(path: &Path) {
    let cancel = AtomicBool::new(false);
    let _ = remove_tree(path, false, &cancel, &mut |_, _| {});
}

/// Writes a temporary marker file inside `dir` to prove that a link works.
pub fn link_probe(dir: &Path) -> io::Result<()> {
    let probe = dir.join(".pathmover_probe");
    std::fs::write(&probe, b"pathmover")?;
    let ok = probe.is_file();
    let _ = std::fs::remove_file(&probe);
    if ok {
        Ok(())
    } else {
        Err(io::Error::other("soubor záložního testu nebyl nalezen"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_and_removing_trees() {
        let base = std::env::temp_dir().join("pathmover-test-copytree");
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(src.join("nested/deeper")).unwrap();
        std::fs::write(src.join("a.txt"), vec![b'x'; 1000]).unwrap();
        std::fs::write(src.join("nested/deeper/b.txt"), vec![b'y'; 2000]).unwrap();

        let dst = base.join("dst");
        let cancel = AtomicBool::new(false);
        let mut progress_calls = 0usize;
        let outcome = copy_tree(&src, &dst, &mut |_, _| progress_calls += 1, &cancel).unwrap();

        assert_eq!(outcome.files, 2);
        assert_eq!(outcome.bytes, 3000);
        assert!(progress_calls > 0);
        verify_same(&src, &dst).unwrap();

        let removed = remove_tree(&dst, false, &cancel, &mut |_, _| {});
        assert_eq!(removed.deleted, 2);
        assert!(!dst.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn covers_size_measurement() {
        let base = std::env::temp_dir().join("pathmover-test-size");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("x")).unwrap();
        std::fs::write(base.join("x/file"), vec![b'z'; 42]).unwrap();
        assert_eq!(dir_size(&base).bytes, 42);
        let _ = std::fs::remove_dir_all(&base);
    }
}