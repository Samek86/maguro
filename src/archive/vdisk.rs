//! Attach and detach disk images with the Windows Virtual Disk API.
//!
//! An ISO opens without elevation: `OpenVirtualDisk` with the ISO device type,
//! then `AttachVirtualDisk` with `READ_ONLY | PERMANENT_LIFETIME`. VHD and VHDX
//! use the same calls. Windows often returns access denied for those unless the
//! process is elevated; that becomes a localized error, not a crash.
//!
//! The new drive letter is the physical drive number from
//! `GetVirtualDiskPhysicalPath` matched against volumes, or a drive letter that
//! appeared after the attach.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetLogicalDrives, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::Storage::Vhd::{
    AttachVirtualDisk, DetachVirtualDisk, GetVirtualDiskPhysicalPath, OpenVirtualDisk, ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME,
    ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY, ATTACH_VIRTUAL_DISK_PARAMETERS, ATTACH_VIRTUAL_DISK_VERSION_1, DETACH_VIRTUAL_DISK_FLAG_NONE,
    OPEN_VIRTUAL_DISK_FLAG_NONE, OPEN_VIRTUAL_DISK_PARAMETERS, OPEN_VIRTUAL_DISK_VERSION_1, VIRTUAL_DISK_ACCESS_READ, VIRTUAL_STORAGE_TYPE,
    VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VIRTUAL_STORAGE_TYPE_DEVICE_VHD, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX, VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_NUMBER};

use super::cache::cache_root;
use super::kind::{disk_kind, DiskKind};

#[derive(Debug)]
pub(crate) enum MountError {
    AccessDenied,
    NoLetter,
    Failed(String),
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::AccessDenied => write!(f, "access denied"),
            MountError::NoLetter => write!(f, "no drive letter"),
            MountError::Failed(s) => write!(f, "{s}"),
        }
    }
}

/// Letters present in `after` that were not present in `before`.
pub(crate) fn new_letters(before: u32, after: u32) -> Vec<char> {
    let fresh = after & !before;
    (0..26u32).filter(|i| fresh & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).collect()
}

fn device_id(kind: DiskKind) -> u32 {
    match kind {
        DiskKind::Iso => VIRTUAL_STORAGE_TYPE_DEVICE_ISO,
        DiskKind::Vhd => VIRTUAL_STORAGE_TYPE_DEVICE_VHD,
        DiskKind::Vhdx => VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
    }
}

fn storage_type(kind: DiskKind) -> VIRTUAL_STORAGE_TYPE {
    VIRTUAL_STORAGE_TYPE { DeviceId: device_id(kind), VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT }
}

struct Disk {
    handle: HANDLE,
}

impl Drop for Disk {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe { let _ = CloseHandle(self.handle); }
        }
    }
}

fn open_disk(path: &Path, kind: DiskKind) -> Result<Disk, MountError> {
    let mut params = OPEN_VIRTUAL_DISK_PARAMETERS::default();
    params.Version = OPEN_VIRTUAL_DISK_VERSION_1;
    params.Anonymous.Version1.RWDepth = 1;
    let ty = storage_type(kind);
    let mut handle = HANDLE::default();
    let wide = HSTRING::from(path);
    let code = unsafe {
        OpenVirtualDisk(
            &ty,
            PCWSTR(wide.as_ptr()),
            VIRTUAL_DISK_ACCESS_READ,
            OPEN_VIRTUAL_DISK_FLAG_NONE,
            Some(&params),
            &mut handle,
        )
    };
    if code.0 != 0 || handle.is_invalid() {
        if code == ERROR_ACCESS_DENIED || code.0 == 5 {
            return Err(MountError::AccessDenied);
        }
        return Err(MountError::Failed(format!("OpenVirtualDisk {}", code.0)));
    }
    Ok(Disk { handle })
}

fn physical_number(disk: &Disk) -> Option<u32> {
    let mut chars = [0u16; 512];
    let mut bytes = (chars.len() * 2) as u32;
    let code = unsafe { GetVirtualDiskPhysicalPath(disk.handle, &mut bytes, windows::core::PWSTR(chars.as_mut_ptr())) };
    if code.0 != 0 {
        return None;
    }
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    let text = String::from_utf16_lossy(&chars[..end]);
    let lower = text.to_ascii_lowercase();
    let rest = lower.rsplit("physicaldrive").next()?;
    rest.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok()
}

fn volume_device(letter: char) -> Option<u32> {
    let name = HSTRING::from(format!(r"\\.\{letter}:"));
    let handle = unsafe {
        CreateFileW(
            &name,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()?
    };
    let mut info = STORAGE_DEVICE_NUMBER::default();
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some(&mut info as *mut _ as *mut _),
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            Some(&mut got),
            None,
        )
        .is_ok()
    };
    unsafe { let _ = CloseHandle(handle); }
    ok.then_some(info.DeviceNumber)
}

fn letter_for_device(mask: u32, device: u32) -> Option<char> {
    (0..26u32).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).find(|&ch| volume_device(ch) == Some(device))
}

fn wait_letter(disk: &Disk, before: u32) -> Result<char, MountError> {
    let device = physical_number(disk);
    for _ in 0..24 {
        let after = unsafe { GetLogicalDrives() };
        let fresh = new_letters(before, after);
        if let Some(device) = device {
            if let Some(ch) = fresh.iter().copied().find(|&ch| volume_device(ch) == Some(device)) {
                return Ok(ch);
            }
            if let Some(ch) = letter_for_device(after, device) {
                return Ok(ch);
            }
        }
        if fresh.len() == 1 {
            return Ok(fresh[0]);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(MountError::NoLetter)
}

/// Attach `path` read-only and return the drive letter. An image that is already
/// attached returns its current letter.
pub(crate) fn mount(path: &Path) -> Result<char, MountError> {
    let kind = disk_kind(&path.display().to_string()).ok_or_else(|| MountError::Failed("not a disk image".into()))?;
    if let Some(ch) = recorded_letter(path) {
        if volume_exists(ch) {
            return Ok(ch);
        }
        forget(path);
    }
    let disk = open_disk(path, kind)?;
    if let Some(device) = physical_number(&disk) {
        let mask = unsafe { GetLogicalDrives() };
        if let Some(ch) = letter_for_device(mask, device) {
            remember(path, ch);
            return Ok(ch);
        }
    }
    let before = unsafe { GetLogicalDrives() };
    let mut params = ATTACH_VIRTUAL_DISK_PARAMETERS::default();
    params.Version = ATTACH_VIRTUAL_DISK_VERSION_1;
    let flags = ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY | ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME;
    let code = unsafe { AttachVirtualDisk(disk.handle, None, flags, 0, Some(&params), None) };
    if code.0 != 0 {
        if code == ERROR_ACCESS_DENIED || code.0 == 5 {
            return Err(MountError::AccessDenied);
        }
        // Already attached: the physical path may show up on a second look.
        if let Some(device) = physical_number(&disk) {
            if let Some(ch) = letter_for_device(unsafe { GetLogicalDrives() }, device) {
                remember(path, ch);
                return Ok(ch);
            }
        }
        return Err(MountError::Failed(format!("AttachVirtualDisk {}", code.0)));
    }
    let letter = wait_letter(&disk, before)?;
    remember(path, letter);
    Ok(letter)
}

pub(crate) fn unmount(path: &Path) -> Result<(), MountError> {
    let kind = disk_kind(&path.display().to_string()).ok_or_else(|| MountError::Failed("not a disk image".into()))?;
    let disk = open_disk(path, kind)?;
    let code = unsafe { DetachVirtualDisk(disk.handle, DETACH_VIRTUAL_DISK_FLAG_NONE, 0) };
    forget(path);
    if code.0 != 0 && code.0 != 2 {
        if code == ERROR_ACCESS_DENIED || code.0 == 5 {
            return Err(MountError::AccessDenied);
        }
        return Err(MountError::Failed(format!("DetachVirtualDisk {}", code.0)));
    }
    Ok(())
}

fn volume_exists(letter: char) -> bool {
    let mask = unsafe { GetLogicalDrives() };
    let bit = (letter.to_ascii_uppercase() as u8).wrapping_sub(b'A');
    bit < 26 && mask & (1 << bit) != 0
}

/// A drive root (`E:\` or `E:`) whose disk is a virtual or file-backed disk,
/// or one Maguro attached.
pub(crate) fn drive_root(path: &str) -> Option<char> {
    let p = path.trim().trim_end_matches(['\\', '/']);
    let b = p.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some((b[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

pub(crate) fn image_for_letter(letter: char) -> Option<String> {
    load_mounts().into_iter().find(|m| m.letter.eq_ignore_ascii_case(&letter.to_string())).map(|m| m.image)
}

pub(crate) fn attached_letter(path: &Path) -> Option<char> {
    let ch = recorded_letter(path)?;
    volume_exists(ch).then_some(ch)
}

/// Bus type 15 (virtual) or 16 (file-backed virtual). A normal hard disk is neither.
pub(crate) fn file_backed_drive(letter: char) -> bool {
    if image_for_letter(letter).is_some() {
        return true;
    }
    let name = HSTRING::from(format!(r"\\.\{letter}:"));
    let Ok(handle) = (unsafe {
        CreateFileW(&name, 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None)
    }) else {
        return false;
    };
    let query = [0u8; 12];
    let mut out = [0u8; 512];
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(handle, IOCTL_STORAGE_QUERY_PROPERTY, Some(query.as_ptr() as *const _), query.len() as u32, Some(out.as_mut_ptr() as *mut _), out.len() as u32, Some(&mut got), None)
            .is_ok()
    };
    unsafe { let _ = CloseHandle(handle); }
    if !ok || got < 32 {
        return false;
    }
    let bus = u32::from_le_bytes([out[28], out[29], out[30], out[31]]);
    bus == 15 || bus == 16
}

pub(crate) fn eject_letter(letter: char) -> Result<(), MountError> {
    if let Some(image) = image_for_letter(letter) {
        return unmount(Path::new(&image));
    }
    let name = HSTRING::from(format!(r"\\.\{letter}:"));
    let handle = unsafe {
        CreateFileW(&name, 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None)
            .map_err(|e| MountError::Failed(e.to_string()))?
    };
    let mut got = 0u32;
    let ok = unsafe { DeviceIoControl(handle, IOCTL_STORAGE_EJECT_MEDIA, None, 0, None, 0, Some(&mut got), None).is_ok() };
    unsafe { let _ = CloseHandle(handle); }
    if ok {
        Ok(())
    } else {
        Err(MountError::Failed("eject failed".into()))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct MountItem {
    image: String,
    letter: String,
}

fn mounts_file() -> PathBuf {
    cache_root().join("mounts.json")
}

fn load_mounts() -> Vec<MountItem> {
    let text = std::fs::read_to_string(mounts_file()).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_mounts(items: &[MountItem]) {
    let _ = std::fs::create_dir_all(cache_root());
    if let Ok(text) = serde_json::to_string(items) {
        let _ = std::fs::write(mounts_file(), text);
    }
}

fn same_file(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']))
}

fn recorded_letter(path: &Path) -> Option<char> {
    let shown = path.display().to_string();
    let item = load_mounts().into_iter().find(|m| same_file(&m.image, &shown))?;
    item.letter.chars().next().map(|c| c.to_ascii_uppercase())
}

fn remember(path: &Path, letter: char) {
    let shown = path.display().to_string();
    let mut items = load_mounts();
    items.retain(|m| !same_file(&m.image, &shown) && !m.letter.eq_ignore_ascii_case(&letter.to_string()));
    items.push(MountItem { image: shown, letter: letter.to_ascii_uppercase().to_string() });
    save_mounts(&items);
}

fn forget(path: &Path) {
    let shown = path.display().to_string();
    let mut items = load_mounts();
    let before = items.len();
    items.retain(|m| !same_file(&m.image, &shown));
    if items.len() != before {
        save_mounts(&items);
    }
}

#[cfg(test)]
mod tests {
    use super::new_letters;

    #[test]
    fn new_letters_are_the_bits_that_appeared() {
        let before = 1 << 2; // C
        let after = (1 << 2) | (1 << 4) | (1 << 0); // C, E, A
        assert_eq!(new_letters(before, after), vec!['A', 'E']);
        assert!(new_letters(after, after).is_empty());
    }

    #[test]
    fn drive_root_is_a_letter() {
        assert_eq!(super::drive_root(r"E:\"), Some('E'));
        assert_eq!(super::drive_root("e:"), Some('E'));
        assert_eq!(super::drive_root(r"E:\folder"), None);
        assert_eq!(super::drive_root(r"\\server\share"), None);
    }
}
