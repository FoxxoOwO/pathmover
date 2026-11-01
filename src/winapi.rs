//! Thin, safe wrappers around the Windows APIs PathMover needs:
//! NTFS directory junctions (reparse points), elevation and free space.
//!
//! Junctions (mount points) do **not** require administrator rights, unlike
//! ordinary symlinks, which is why PathMover builds them itself with
//! `FSCTL_SET_REPARSE_POINT` instead of calling `CreateSymbolicLinkW`.

#![cfg(windows)]

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &OsStr) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_wide().collect();
    v.push(0);
    v
}

fn wide_str(s: &str) -> Vec<u16> {
    wide(OsStr::new(s))
}

/// Windows paths longer than 260 characters need the `\\?\` prefix.
pub fn extended(path: &Path) -> PathBuf {
    use std::path::Component;
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        return path.to_path_buf();
    }
    if path.is_absolute() {
        if let Some(Component::Prefix(prefix)) = path.components().next() {
            let p = prefix.as_os_str().to_string_lossy();
            if p.starts_with(r"\\") {
                // UNC: `\\server\share\dir` -> `\\?\UNC\server\share\dir`
                return PathBuf::from(format!(r"\\?\UNC\{}", &text[2..]));
            }
        }
        return PathBuf::from(format!(r"\\?\{text}"));
    }
    path.to_path_buf()
}

/// Strip the `\\?\` / `\\?\UNC\` prefix a canonicalised path carries, so the
/// result can be used in user-facing text and in reparse point buffers.
pub fn plain_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.to_string()
    }
}

/// True when the path itself is a reparse point (junction, symlink, ...).
pub fn is_reparse_point(path: &Path) -> bool {
    match std::fs::symlink_metadata(extended(path)) {
        Ok(md) => {
            use std::os::windows::fs::MetadataExt;
            md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        }
        Err(_) => false,
    }
}

/// Resolve where a junction/symlink points to. `None` when it is not a
/// reparse point or it cannot be read.
pub fn reparse_target(path: &Path) -> Option<PathBuf> {
    if !is_reparse_point(path) {
        return None;
    }
    let mut buffer = vec![0u8; 16 * 1024];
    let mut returned: u32 = 0;

    unsafe {
        let handle = CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let ok = DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        );
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
    }

    parse_reparse_buffer(&buffer[..returned as usize])
}

fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn utf16_at(buf: &[u8], off: usize, len_bytes: usize) -> Option<String> {
    let end = off.checked_add(len_bytes)?;
    if end > buf.len() || !len_bytes.is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = buf[off..end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let s = String::from_utf16_lossy(&units);
    // Substitute names are prefixed with the NT namespace: \??\C:\dir
    let s = s
        .strip_prefix(r"\??\")
        .or_else(|| s.strip_prefix(r"\\?\"))
        .unwrap_or(&s)
        .to_string();
    Some(s)
}

fn parse_reparse_buffer(buf: &[u8]) -> Option<PathBuf> {
    if buf.len() < 16 {
        return None;
    }
    let tag = u32_at(buf, 0);
    if tag != IO_REPARSE_TAG_MOUNT_POINT && tag != IO_REPARSE_TAG_SYMLINK {
        return None;
    }
    // REPARSE_DATA_BUFFER: 4 tag + 2 len + 2 reserved, then the payload
    // (MountPointReparseBuffer / SymlinkReparseBuffer) starting at offset 8.
    let subst_off = u16_at(buf, 8) as usize;
    let subst_len = u16_at(buf, 10) as usize;
    let print_off = u16_at(buf, 12) as usize;
    let print_len = u16_at(buf, 14) as usize;
    let path_buffer = 16;

    let subst = utf16_at(buf, path_buffer + subst_off, subst_len).filter(|s| !s.is_empty());
    let print = utf16_at(buf, path_buffer + print_off, print_len).filter(|s| !s.is_empty());
    subst.or(print).map(PathBuf::from)
}

/// Create an NTFS directory junction `link` -> `target`.
///
/// `link` must not exist yet; it is created as an empty directory and then
/// converted into a reparse point. Falls back to `cmd /c mklink /J` when the
/// direct DeviceIoControl call is refused (e.g. by a filter driver).
pub fn create_junction(link: &Path, target: &Path) -> io::Result<()> {
    let target_abs = std::fs::canonicalize(extended(target)).unwrap_or_else(|_| target.to_path_buf());
    if !target_abs.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cílová složka neexistuje: {}", target_abs.display()),
        ));
    }
    if link.exists() || is_reparse_point(link) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("odkaz už existuje: {}", link.display()),
        ));
    }
    std::fs::create_dir_all(extended(link))?;

    match set_mount_point(link, &target_abs) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            // Clean up the empty directory before trying the fallback.
            let _ = std::fs::remove_dir(extended(link));
            match mklink_fallback(link, &target_abs) {
                Ok(()) => Ok(()),
                Err(_) => Err(first_error),
            }
        }
    }
}

fn set_mount_point(link: &Path, target_abs: &Path) -> io::Result<()> {
    // The reparse buffer must hold a plain (non-`\\?\`) path; the NT
    // namespace variant is used for the substitute name.
    let plain = plain_path(target_abs);
    let subst: Vec<u16> = OsStr::new(&format!(r"\??\{plain}"))
        .encode_wide()
        .collect();
    let print: Vec<u16> = OsStr::new(&plain).encode_wide().collect();

    let subst_bytes = subst.len() * 2;
    let print_bytes = print.len() * 2;
    let reparse_data_len = 8 + subst_bytes + 2 + print_bytes + 2;

    let mut buf: Vec<u8> = Vec::with_capacity(8 + reparse_data_len);
    buf.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend_from_slice(&(reparse_data_len as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    // MountPointReparseBuffer
    buf.extend_from_slice(&0u16.to_le_bytes()); // SubstituteNameOffset
    buf.extend_from_slice(&(subst_bytes as u16).to_le_bytes());
    buf.extend_from_slice(&((subst_bytes + 2) as u16).to_le_bytes()); // PrintNameOffset
    buf.extend_from_slice(&(print_bytes as u16).to_le_bytes());
    // PathBuffer
    for u in &subst {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf.extend_from_slice(&0u16.to_le_bytes());
    for u in &print {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf.extend_from_slice(&0u16.to_le_bytes());

    let mut returned: u32 = 0;
    unsafe {
        let handle = CreateFileW(
            wide(extended(link).as_os_str()).as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let ok = DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buf.as_ptr() as *const _,
            buf.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
        let err = io::Error::last_os_error();
        CloseHandle(handle);
        if ok == 0 {
            return Err(err);
        }
    }
    Ok(())
}

fn mklink_fallback(link: &Path, target_abs: &Path) -> io::Result<()> {
    let plain_target = plain_path(target_abs);
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(&plain_target)
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() && is_reparse_point(link) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "mklink /J selhalo (exit {:?})",
            status.code()
        )))
    }
}

/// The directory a junction points to, if `path` is a junction/mount point.
pub fn junction_target(path: &Path) -> Option<PathBuf> {
    if !is_reparse_point(path) {
        return None;
    }
    reparse_target(path)
}

/// Delete the junction itself. The link target and its data are untouched.
pub fn remove_junction(path: &Path) -> io::Result<()> {
    std::fs::remove_dir(extended(path))
}

/// Clear the read-only / hidden / system attributes so a delete can succeed.
pub fn make_writable(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let md = std::fs::symlink_metadata(extended(path))?;
    let attrs = md.file_attributes();
    let stripped = attrs & !(FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
    if stripped != attrs {
        set_attributes(path, stripped)?;
    }
    Ok(())
}

fn set_attributes(path: &Path, attrs: u32) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
    unsafe {
        let ok = SetFileAttributesW(wide(path.as_os_str()).as_ptr(), attrs);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// `(available, total)` bytes on the volume containing `path`.
pub fn free_space(path: &Path) -> io::Result<(u64, u64)> {
    let root = crate::util::volume_root(path).unwrap_or_else(|| path.to_path_buf());
    let mut avail: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    unsafe {
        let ok = GetDiskFreeSpaceExW(
            wide(root.as_os_str()).as_ptr(),
            &mut avail,
            &mut total,
            &mut free,
        );
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((avail, total))
}

/// Available bytes on the volume holding `path` (0 when unknown).
pub fn available_space(path: &Path) -> u64 {
    free_space(path).map(|(a, _)| a).unwrap_or(0)
}

/// Is the process running elevated?
pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Relaunch PathMover through the UAC prompt. Returns true when the new
/// process was started (the caller should exit afterwards).
pub fn relaunch_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let verb = wide_str("runas");
    let file = wide(exe.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    result as isize > 32
}

/// Open a folder in Windows Explorer.
pub fn open_in_explorer(path: &Path) -> io::Result<()> {
    std::process::Command::new("explorer")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// `HANDLE` is not `Send`; a tiny helper so tasks can hold handles if needed.
pub fn _assert_handle_type(_h: HANDLE) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junction_roundtrip() {
        let base = std::env::temp_dir().join("pathmover-test-junction");
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("hello.txt"), b"hi").unwrap();

        create_junction(&link, &target).unwrap();
        assert!(is_reparse_point(&link));
        let got = junction_target(&link).expect("target");
        let expected = std::fs::canonicalize(&target).unwrap();
        assert_eq!(
            got.to_string_lossy().to_lowercase(),
            plain_path(&expected).to_lowercase()
        );
        // Both spellings must compare equal through the shared helper.
        assert!(crate::util::paths_eq(&got, &expected));
        // Data is reachable through the link.
        assert_eq!(std::fs::read(link.join("hello.txt")).unwrap(), b"hi");

        remove_junction(&link).unwrap();
        assert!(!link.exists());
        assert!(target.exists(), "target data must survive");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn extended_paths_are_prefixed() {
        assert_eq!(extended(Path::new(r"C:\temp")).to_string_lossy(), r"\\?\C:\temp");
        assert_eq!(
            extended(Path::new(r"\\server\share")).to_string_lossy(),
            r"\\?\UNC\server\share"
        );
    }
}