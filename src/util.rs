//! Small formatting / path helpers shared by the whole app.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 1024-based human readable size, e.g. `1.42 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Compact size used inside tables, e.g. `1.4G`.
pub fn short_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

/// `0.37` -> `37 %`
pub fn percent(part: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "před 3 dny" style relative timestamp, Czech pluralisation.
pub fn human_age(unix_ts: u64) -> String {
    if unix_ts == 0 {
        return "neznámo".to_string();
    }
    let now = unix_now();
    let secs = now.saturating_sub(unix_ts);
    match secs {
        0..=59 => "právě teď".to_string(),
        60..=3599 => {
            let m = secs / 60;
            format!("před {m} {}", plural(m, "minutou", "minutami", "minutami"))
        }
        3600..=86399 => {
            let h = secs / 3600;
            format!("před {h} {}", plural(h, "hodinou", "hodinami", "hodinami"))
        }
        _ => {
            let d = secs / 86400;
            if d < 30 {
                format!("před {d} {}", plural(d, "dnem", "dny", "dny"))
            } else {
                let mo = d / 30;
                format!("před {mo} {}", plural(mo, "měsícem", "měsíci", "měsíci"))
            }
        }
    }
}

fn plural(n: u64, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match n {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

/// `C:\Users\me\AppData\Local\Slack` -> `C:`; returns `None` for relative paths.
pub fn drive_of(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\?\UNC\{rest}"));
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest
    } else {
        &text
    };
    let un_prefixed = Path::new(text);
    let comps: Vec<_> = un_prefixed.components().take(1).collect();
    match comps.first() {
        Some(std::path::Component::Prefix(p)) => Some(p.as_os_str().to_string_lossy().trim_end_matches('\\').to_string()),
        _ => None,
    }
}

/// Root of the volume containing `path` (`D:\`), if it has a drive prefix.
pub fn volume_root(path: &Path) -> Option<PathBuf> {
    let drive = drive_of(path)?;
    Some(PathBuf::from(format!("{drive}\\")))
}

/// True when both paths live on the same volume.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    match (drive_of(a), drive_of(b)) {
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(&y),
        _ => false,
    }
}

/// Normalised path string for case-insensitive comparison on Windows.
///
/// Extended-length spellings (`\\?\C:\dir`, `\\?\UNC\srv\share`) are folded
/// onto their plain form so a canonicalised path compares equal to the path a
/// user picked in the UI.
pub fn normalize_path_str(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.to_string()
    };
    let trimmed = text.trim_end_matches(['\\', '/']);
    let mut out = trimmed.replace('/', "\\").to_lowercase();
    if out.is_empty() {
        out.push('\\');
    }
    out
}

/// `path` equals `base` or lives inside it (case-insensitive).
pub fn starts_with_ci(path: &Path, base: &Path) -> bool {
    let p = normalize_path_str(path);
    let b = normalize_path_str(base);
    p == b || p.starts_with(&format!("{b}\\"))
}

/// Case-insensitive path equality.
pub fn paths_eq(a: &Path, b: &Path) -> bool {
    normalize_path_str(a) == normalize_path_str(b)
}

/// Make a folder name safe to use as a single path component on another drive.
pub fn sanitize_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('_'),
            c if (c as u32) < 0x20 => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

/// Path shortened for display in narrow UI columns.
pub fn ellipsize_middle(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 8 {
        return text.to_string();
    }
    let keep = max_chars - 3;
    let head = keep / 2;
    let tail = keep - head;
    let mut s: String = chars[..head].iter().collect();
    s.push_str("...");
    s.extend(chars[chars.len() - tail..].iter());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_formatted() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.00 KB");
        assert_eq!(human_size(1536 * 1024), "1.50 MB");
        assert_eq!(short_size(1024 * 1024 * 1024), "1.0G");
    }

    #[test]
    fn volumes_are_compared() {
        assert!(same_volume(Path::new(r"C:\a\b"), Path::new(r"c:\x")));
        assert!(same_volume(Path::new(r"\\?\C:\a\b"), Path::new(r"c:\x")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\a")));
    }

    #[test]
    fn extended_prefixes_are_normalised() {
        assert!(paths_eq(Path::new(r"\\?\C:\Temp\"), Path::new(r"c:\temp")));
        assert!(paths_eq(
            Path::new(r"\\?\UNC\srv\share\x"),
            Path::new(r"\\srv\share\x")
        ));
        assert!(starts_with_ci(
            Path::new(r"\\?\C:\Users\me\AppData\Local\Slack"),
            Path::new(r"C:\Users\me\AppData")
        ));
    }

    #[test]
    fn components_are_sanitized() {
        assert_eq!(sanitize_component("a:b"), "a_b");
        assert_eq!(sanitize_component("  "), "unnamed");
    }
}