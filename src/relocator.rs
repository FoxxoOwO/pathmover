//! Moving AppData folders to another drive (and back) with NTFS junctions.
//!
//! The move is deliberately conservative:
//!
//! 1. preflight validation (source, target, free space, same-volume check)
//! 2. copy the tree to the destination drive
//! 3. verify the copy byte-for-byte (size + file count)
//! 4. rename the original to `<name>.bak`
//! 5. create the junction `<original>` -> `<destination>`
//! 6. verify the junction works (resolve target + write a probe file through it)
//! 7. delete `<name>.bak`
//!
//! Any failure triggers a rollback that removes the junction, restores the
//! `.bak` folder and deletes the partial copy - the original folder always
//! ends up usable.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::MovedEntry;
use crate::fsutil::{self, TreeSize};
use crate::scanner;
use crate::util;
use crate::winapi;

/// Keep this much headroom on the target volume instead of filling it up.
pub const FREE_SPACE_MARGIN: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preflight,
    Copy,
    Verify,
    Backup,
    Link,
    VerifyLink,
    Cleanup,
    Restore,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Preflight => "Kontrola",
            Phase::Copy => "Kopírování",
            Phase::Verify => "Ověření kopie",
            Phase::Backup => "Záloha originálu",
            Phase::Link => "Vytváření junction",
            Phase::VerifyLink => "Ověření odkazu",
            Phase::Cleanup => "Úklid",
            Phase::Restore => "Navracení na C:",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Good,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub enum Event {
    Phase(Phase),
    Detail(String),
    Progress { done: u64, total: u64 },
    Log(Level, String),
}

pub type Emitter<'a> = &'a mut dyn FnMut(Event);

/// Successful move result.
#[derive(Debug, Clone)]
pub struct Moved {
    pub entry: MovedEntry,
    pub backup: Option<PathBuf>,
    pub warnings: Vec<String>,
}

fn emit_log(emit: Emitter<'_>, level: Level, message: impl Into<String>) {
    emit(Event::Log(level, message.into()));
}

/// Destination path for a folder, without touching the filesystem.
pub fn preview_target(target_root: &Path, scope: &str, name: &str) -> PathBuf {
    target_root
        .join(scope)
        .join(util::sanitize_component(name))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("operace byla zrušena".to_string())
    } else {
        Ok(())
    }
}

/// Pick a non-existing `.bak` sibling for the original folder.
fn backup_path(source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "folder".to_string());
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let mut candidate = parent.join(format!("{name}.bak"));
    if candidate.exists() {
        candidate = parent.join(format!("{name}.{}.bak", util::unix_now()));
    }
    candidate
}

/// Perform a full move. Returns the config record on success.
pub fn move_folder(
    source: &Path,
    scope: &str,
    target_root: &Path,
    keep_backup: bool,
    emit: Emitter<'_>,
    cancel: &AtomicBool,
) -> Result<Moved, String> {
    emit(Event::Phase(Phase::Preflight));
    let mut warnings: Vec<String> = Vec::new();

    // ---- preflight -------------------------------------------------------
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "neplatná zdrojová cesta".to_string())?;

    if !winapi::is_reparse_point(source) {
        if !source.exists() {
            return Err(format!("zdroj neexistuje: {}", source.display()));
        }
        if !source.is_dir() {
            return Err(format!("zdroj není složka: {}", source.display()));
        }
    } else {
        return Err(format!(
            "{} už je odkaz (junction), nelze přesunout znovu",
            source.display()
        ));
    }

    if !scanner::is_movable_source(source) {
        return Err(format!(
            "{} není v AppData ani v povolené části profilu - LinkVault toto nepodporuje",
            source.display()
        ));
    }

    if target_root.as_os_str().is_empty() {
        return Err("není nastaven cílový adresář".to_string());
    }
    if util::same_volume(source, target_root) {
        return Err(format!(
            "cíl {} je na stejném disku jako zdroj - přesun nemá smysl",
            target_root.display()
        ));
    }

    let TreeSize { bytes, files, .. } = fsutil::dir_size(source);
    if files == 0 {
        warnings.push("složka je prázdná".to_string());
    }
    emit(Event::Progress {
        done: 0,
        total: bytes,
    });
    emit_log(
        emit,
        Level::Info,
        format!(
            "{}: {} v {} souborech",
            name,
            util::human_size(bytes),
            files
        ),
    );

    let target = preview_target(target_root, scope, &name);
    if target.exists() || winapi::is_reparse_point(&target) {
        return Err(format!("cíl už existuje: {}", target.display()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(winapi::extended(parent))
            .map_err(|e| format!("nelze vytvořit {}: {e}", parent.display()))?;
    }

    let available = winapi::available_space(target_root);
    if available > 0 && available < bytes + FREE_SPACE_MARGIN {
        return Err(format!(
            "na cílovém disku není dost místa (potřeba {}, volno {})",
            util::human_size(bytes + FREE_SPACE_MARGIN),
            util::human_size(available)
        ));
    }
    check_cancel(cancel)?;

    // ---- copy ------------------------------------------------------------
    emit(Event::Phase(Phase::Copy));
    let copy = fsutil::copy_tree(
        source,
        &target,
        &mut |done, rel| {
            emit(Event::Progress {
                done,
                total: bytes,
            });
            if !rel.is_empty() {
                emit(Event::Detail(rel.to_string()));
            }
        },
        cancel,
    )
    .map_err(|e| format!("kopírování selhalo: {e}"))?;

    if copy.cancelled {
        fsutil::remove_tree_quiet(&target);
        return Err("kopírování bylo zrušeno".to_string());
    }
    if let Some(fatal) = copy.aborted.clone() {
        fsutil::remove_tree_quiet(&target);
        emit_log(emit, Level::Error, format!("kopírování selhalo: {fatal}"));
        return Err(fatal);
    }
    if !copy.skipped_links.is_empty() {
        warnings.push(format!(
            "{} vnořených odkazů bylo přeskočeno (nebyly zkopírovány)",
            copy.skipped_links.len()
        ));
    }
    if !copy.errors.is_empty() {
        let first: Vec<String> = copy.errors.iter().take(3).cloned().collect();
        fsutil::remove_tree_quiet(&target);
        return Err(format!(
            "{} souborů se nepodařilo zkopírovat (např. {}); cíl byl uklizen",
            copy.errors.len(),
            first.join("; ")
        ));
    }

    // ---- verify copy -----------------------------------------------------
    emit(Event::Phase(Phase::Verify));
    if let Err(err) = fsutil::verify_same(source, &target) {
        fsutil::remove_tree_quiet(&target);
        return Err(format!("{err} - cíl byl uklizen, originál zůstal nedotčen"));
    }
    emit_log(emit, Level::Good, "kopie ověřena (velikost i počet souborů)");
    check_cancel(cancel)?;

    // ---- rename original to .bak ----------------------------------------
    emit(Event::Phase(Phase::Backup));
    let backup = backup_path(source);
    std::fs::rename(winapi::extended(source), winapi::extended(&backup)).map_err(|e| {
        fsutil::remove_tree_quiet(&target);
        format!("nelze přejmenovat originál na {}: {e}", backup.display())
    })?;

    // ---- create junction -------------------------------------------------
    emit(Event::Phase(Phase::Link));
    if let Err(err) = winapi::create_junction(source, &target) {
        // Restore the original: nothing was lost yet.
        let _ = std::fs::rename(winapi::extended(&backup), winapi::extended(source));
        fsutil::remove_tree_quiet(&target);
        return Err(format!("vytvoření junction selhalo: {err}"));
    }

    // ---- verify junction -------------------------------------------------
    emit(Event::Phase(Phase::VerifyLink));
    let verify = (|| -> Result<(), String> {
        let resolved = winapi::junction_target(source)
            .ok_or_else(|| "junction nelze přečíst".to_string())?;
        let expected = std::fs::canonicalize(winapi::extended(&target)).unwrap_or(target.clone());
        if !util::paths_eq(&resolved, &expected) {
            return Err(format!(
                "junction ukazuje na {}, očekáváno {}",
                resolved.display(),
                expected.display()
            ));
        }
        fsutil::link_probe(source).map_err(|e| format!("odkaz nefunguje: {e}"))?;
        if !source.is_dir() {
            return Err("přes odkaz nelze vstoupit do složky".to_string());
        }
        Ok(())
    })();

    if let Err(err) = verify {
        // Roll the whole thing back.
        let _ = winapi::remove_junction(source);
        let _ = std::fs::remove_dir(winapi::extended(source));
        let _ = std::fs::rename(winapi::extended(&backup), winapi::extended(source));
        fsutil::remove_tree_quiet(&target);
        return Err(format!("{err} - vše vráceno zpět, originál obnoven"));
    }
    emit_log(emit, Level::Good, "junction aktivní a funkční");

    // ---- cleanup ---------------------------------------------------------
    emit(Event::Phase(Phase::Cleanup));
    let kept_backup = if keep_backup {
        emit_log(
            emit,
            Level::Info,
            format!("záloha ponechána: {}", backup.display()),
        );
        Some(backup.clone())
    } else {
        let outcome = fsutil::remove_tree(&backup, false, cancel, &mut |done, _| {
            emit(Event::Detail(format!(
                "mažu zálohu: {}",
                util::human_size(done)
            )));
        });
        if !outcome.errors.is_empty() {
            warnings.push(format!(
                "zálohu {} nelze celou smazat ({} chyb) - smažte ji ručně",
                backup.display(),
                outcome.errors.len()
            ));
            Some(backup.clone())
        } else {
            None
        }
    };

    let entry = MovedEntry {
        scope: scope.to_string(),
        name: name.clone(),
        source: source.to_string_lossy().to_string(),
        target: target.to_string_lossy().to_string(),
        bytes,
        moved_at: util::unix_now(),
    };

    emit_log(
        emit,
        Level::Good,
        format!(
            "hotovo: {} uvolněno na C:, data jsou v {}",
            util::human_size(bytes),
            target.display()
        ),
    );
    Ok(Moved {
        entry,
        backup: kept_backup,
        warnings,
    })
}

/// Move a folder back to its original location on C:.
///
/// * `delete_target` also removes the data left on the other drive.
pub fn restore(
    entry: &MovedEntry,
    delete_target: bool,
    emit: Emitter<'_>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    emit(Event::Phase(Phase::Restore));
    let source = entry.source_path();
    let target = entry.target_path();

    // ---- validation ------------------------------------------------------
    let resolved = winapi::junction_target(&source).ok_or_else(|| {
        format!(
            "{} není junction - nelze vrátit (možná už bylo vráceno)",
            source.display()
        )
    })?;
    if !util::paths_eq(&resolved, &target) {
        return Err(format!(
            "junction ukazuje na {}, ale config.toml říká {} - přerušuji",
            resolved.display(),
            target.display()
        ));
    }
    if !target.is_dir() {
        return Err(format!("data na cíli chybí: {}", target.display()));
    }
    if !scanner::is_movable_source(&source) {
        return Err(format!("{} není v AppData ani v povolené části profilu", source.display()));
    }

    let TreeSize { bytes, files, .. } = fsutil::dir_size(&target);
    emit(Event::Progress {
        done: 0,
        total: bytes,
    });
    let available = winapi::available_space(&source);
    if available > 0 && available < bytes + FREE_SPACE_MARGIN {
        return Err(format!(
            "na C: není dost místa (potřeba {}, volno {})",
            util::human_size(bytes + FREE_SPACE_MARGIN),
            util::human_size(available)
        ));
    }

    // ---- copy data to a sibling temp folder ------------------------------
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(
        ".linkvault-restore-{}",
        util::sanitize_component(&entry.name)
    ));
    fsutil::remove_tree_quiet(&temp);
    std::fs::create_dir_all(winapi::extended(&temp))
        .map_err(|e| format!("nelze vytvořit {}: {e}", temp.display()))?;

    emit_log(
        emit,
        Level::Info,
        format!("kopíruji {} zpět na C:", util::human_size(bytes)),
    );
    let copy = fsutil::copy_tree(
        &target,
        &temp,
        &mut |done, rel| {
            emit(Event::Progress {
                done,
                total: bytes,
            });
            if !rel.is_empty() {
                emit(Event::Detail(rel.to_string()));
            }
        },
        cancel,
    )
    .map_err(|e| format!("kopírování zpět selhalo: {e}"))?;

    let abort = if copy.cancelled {
        Some("navracení bylo zrušeno".to_string())
    } else if let Some(fatal) = copy.aborted.clone() {
        Some(fatal)
    } else if !copy.errors.is_empty() {
        Some(format!(
            "{} souborů se nepodařilo zkopírovat zpět (např. {})",
            copy.errors.len(),
            copy.errors.first().cloned().unwrap_or_default()
        ))
    } else {
        fsutil::verify_same(&target, &temp).err()
    };

    if let Some(reason) = abort {
        fsutil::remove_tree_quiet(&temp);
        return Err(format!(
            "{reason}; odkaz na {} zůstal nedotčen",
            target.display()
        ));
    }

    // ---- swap the junction for the restored folder -----------------------
    winapi::remove_junction(&source)
        .map_err(|e| format!("nelze odstranit junction: {e}"))?;

    if let Err(err) = std::fs::rename(winapi::extended(&temp), winapi::extended(&source)) {
        // Put the junction back so the application keeps working.
        let _ = winapi::create_junction(&source, &target);
        fsutil::remove_tree_quiet(&temp);
        return Err(format!(
            "nelze obnovit {}: {err} - odkaz byl obnoven, data zůstala na cíli",
            source.display()
        ));
    }

    if !source.is_dir() {
        return Err(format!("{} po obnovení není složka", source.display()));
    }
    emit_log(
        emit,
        Level::Good,
        format!(
            "obnoveno: {} ({} souborů) na {}",
            util::human_size(bytes),
            files,
            source.display()
        ),
    );

    // ---- optionally drop the data on the other drive ---------------------
    if delete_target {
        let outcome = fsutil::remove_tree(&target, false, cancel, &mut |done, _| {
            emit(Event::Detail(format!(
                "mažu data na cíli: {}",
                util::human_size(done)
            )));
        });
        if outcome.errors.is_empty() {
            emit_log(emit, Level::Info, "data na cílovém disku smazána");
        } else {
            emit_log(
                emit,
                Level::Warn,
                format!(
                    "{} souborů na cíli nešlo smazat - odstraňte {} ručně",
                    outcome.errors.len(),
                    target.display()
                ),
            );
        }
    } else {
        emit_log(
            emit,
            Level::Info,
            format!("data na cíli ponechána: {}", target.display()),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_previewed_without_touching_disk() {
        let p = preview_target(Path::new(r"D:\Moved"), "Local", "Some App");
        assert_eq!(p, PathBuf::from(r"D:\Moved\Local\Some App"));
    }

    #[test]
    fn backup_paths_avoid_collisions() {
        let base = std::env::temp_dir().join("linkvault-test-backup");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("App")).unwrap();
        assert_eq!(backup_path(&base.join("App")), base.join("App.bak"));
        std::fs::create_dir_all(base.join("App.bak")).unwrap();
        let second = backup_path(&base.join("App"));
        assert_ne!(second, base.join("App.bak"));
        assert!(second.to_string_lossy().ends_with(".bak"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_paths_outside_appdata() {
        let out = move_folder(
            Path::new(r"C:\Windows"),
            "Local",
            Path::new(r"D:\Moved"),
            false,
            &mut |_| {},
            &AtomicBool::new(false),
        );
        assert!(out.is_err());
    }
}