//! Persistent application state stored in `%APPDATA%\LinkVault\config.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util;

pub const CONFIG_VERSION: u32 = 1;

/// One folder that LinkVault moved from C: to another drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovedEntry {
    /// `Local` or `Roaming`.
    pub scope: String,
    /// Display name (the folder name inside AppData).
    pub name: String,
    /// Original location on C: (now a junction).
    pub source: String,
    /// Real data location on the target drive.
    pub target: String,
    /// Size in bytes at the time of the move.
    #[serde(default)]
    pub bytes: u64,
    /// Unix timestamp of the move.
    #[serde(default)]
    pub moved_at: u64,
}

impl MovedEntry {
    pub fn source_path(&self) -> PathBuf {
        PathBuf::from(&self.source)
    }

    pub fn target_path(&self) -> PathBuf {
        PathBuf::from(&self.target)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    /// Root on the destination drive, e.g. `D:\AppDataMoved`.
    pub target_root: String,
    /// Keep the `.bak` copy instead of deleting it after a successful move.
    pub keep_backup: bool,
    /// Include `logs` folders when cleaning caches.
    pub clean_logs: bool,
    pub entries: Vec<MovedEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            target_root: default_target_root(),
            keep_backup: false,
            clean_logs: true,
            entries: Vec::new(),
        }
    }
}

/// Pick a sensible default destination: the largest non-system fixed drive.
pub fn default_target_root() -> String {
    let mut best: Option<(u64, String)> = None;
    for drive in fixed_drives() {
        if drive.eq_ignore_ascii_case("C:") {
            continue;
        }
        let root = format!("{drive}\\");
        if let Ok((avail, _)) = crate::winapi::free_space(Path::new(&root)) {
            if best.as_ref().map(|(a, _)| avail > *a).unwrap_or(true) {
                best = Some((avail, format!("{root}AppDataMoved")));
            }
        }
    }
    best.map(|(_, p)| p)
        .unwrap_or_else(|| r"D:\AppDataMoved".to_string())
}

/// Letters of all fixed drives (`C:`, `D:`, ...).
pub fn fixed_drives() -> Vec<String> {
    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let name = format!("{}:", letter as char);
        let root = format!("{name}\\");
        // GetDriveTypeW == DRIVE_FIXED (3)
        if drive_type(&root) == 3 {
            drives.push(name);
        }
    }
    drives
}

#[cfg(windows)]
fn drive_type(root: &str) -> u32 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let mut wide: Vec<u16> = std::ffi::OsStr::new(root).encode_wide().collect();
    wide.push(0);
    unsafe { GetDriveTypeW(wide.as_ptr()) }
}

#[cfg(not(windows))]
fn drive_type(_root: &str) -> u32 {
    0
}

impl Config {
    /// `%APPDATA%\LinkVault\config.toml`, falling back to the exe folder.
    pub fn path() -> PathBuf {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("LinkVault").join("config.toml");
        }
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkvault-config.toml")
    }

    /// Load the config, returning defaults plus a warning on failure.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    cfg.version = CONFIG_VERSION;
                    (cfg, None)
                }
                Err(err) => (
                    Config::default(),
                    Some(format!("config.toml nelze přečíst ({err}), používám výchozí nastavení")),
                ),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(err) => (
                Config::default(),
                Some(format!("config.toml nelze načíst: {err}")),
            ),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("nelze vytvořit {}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| format!("serializace selhala: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("zápis {} selhal: {e}", path.display()))
    }

    pub fn add_entry(&mut self, entry: MovedEntry) {
        self.entries.retain(|e| !e.source.eq_ignore_ascii_case(&entry.source));
        self.entries.push(entry);
        self.entries.sort_by_key(|a| a.name.to_lowercase());
    }

    pub fn remove_entry(&mut self, source: &str) {
        self.entries.retain(|e| !e.source.eq_ignore_ascii_case(source));
    }

    pub fn entry_for(&self, source: &Path) -> Option<&MovedEntry> {
        let source = source.to_string_lossy();
        self.entries
            .iter()
            .find(|e| e.source.eq_ignore_ascii_case(source.as_ref()))
    }

    /// Total bytes currently parked on other drives.
    pub fn total_moved_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }

    /// Resolve the destination path for a folder of the given scope/name.
    /// Kept in sync with `relocator::preview_target`.
    pub fn target_for(&self, scope: &str, name: &str) -> PathBuf {
        PathBuf::from(&self.target_root)
            .join(scope)
            .join(util::sanitize_component(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_toml() {
        let mut cfg = Config::default();
        cfg.target_root = r"D:\AppDataMoved".to_string();
        cfg.add_entry(MovedEntry {
            scope: "Local".into(),
            name: "Slack".into(),
            source: r"C:\Users\me\AppData\Local\Slack".into(),
            target: r"D:\AppDataMoved\Local\Slack".into(),
            bytes: 1234,
            moved_at: 42,
        });
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.total_moved_bytes(), 1234);
        assert!(back.entry_for(Path::new(r"c:\users\ME\appdata\local\slack")).is_some());
    }

    #[test]
    fn target_paths_are_sanitized() {
        let cfg = Config::default();
        let p = cfg.target_for("Roaming", "a/b");
        assert!(p.ends_with(Path::new("Roaming").join("a_b")));
    }
}