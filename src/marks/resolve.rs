//! Turn a shell parsing name into a [`super::toggle::Target`].
//! `GetDriveTypeW` is not called: a dead mapped drive can block the UI thread.
//! A letter missing from `GetLogicalDrives` is not mounted. Drive type 1 comes
//! from the drive-button cache, which was filled when the letter appeared.

use windows::Win32::Storage::FileSystem::{GetFileAttributesW, GetLogicalDrives, FILE_ATTRIBUTE_DIRECTORY, INVALID_FILE_ATTRIBUTES};
use windows::Win32::UI::Shell::{SHCreateItemFromParsingName, SIGDN_FILESYSPATH, IShellItem};
use windows::core::HSTRING;

use super::toggle::{archive_ancestor, archive_named, parent_path, Target};

/// `parsing` is the name the view already has. `folder` is the shell's folder bit.
pub(crate) unsafe fn from_parsing(parsing: &str, folder: bool) -> Target {
    let fs = filesystem_name(parsing);
    let path = fs.unwrap_or_default();
    let mut t = if folder { Target::folder(&path) } else { Target::file(&path) };
    t.library = library_hint(parsing, &path);
    t.phone = phone_hint(parsing, &path);
    t.unmounted = unmounted_drive(&path);
    t.in_archive = in_archive(&path);
    t
}

unsafe fn filesystem_name(parsing: &str) -> Option<String> {
    let parsing = parsing.trim();
    if parsing.is_empty() {
        return None;
    }
    // A plain filesystem path is already `SIGDN_FILESYSPATH`. Asking the shell
    // again for every row of a large selection is the slow part of opening the menu.
    if crate::is_fs(parsing) && !library_hint(parsing, parsing) && !phone_hint(parsing, parsing) {
        return Some(parsing.to_string());
    }
    let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(parsing), None).ok()?;
    let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let text = crate::pwstr(name);
    (crate::is_fs(&text)).then_some(text)
}

fn library_hint(parsing: &str, fs: &str) -> bool {
    let p = parsing.to_ascii_lowercase();
    let f = fs.to_ascii_lowercase();
    p.contains(".library-ms") || f.ends_with(".library-ms") || p.contains("::{031e4825-7b94-4dc3-b131-e946b44c8dd5}")
}

fn phone_hint(parsing: &str, fs: &str) -> bool {
    let p = parsing.to_ascii_lowercase();
    let f = fs.to_ascii_lowercase();
    p.contains("mtp") || f.contains("mtp") || f.contains(r"\\?\usb")
}

fn letter_of(path: &str) -> Option<u8> {
    match path.as_bytes() {
        [c, b':', ..] if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

fn letter_present(letter: u8) -> bool {
    if !(b'A'..=b'Z').contains(&letter) {
        return false;
    }
    (unsafe { GetLogicalDrives() }) >> (letter - b'A') & 1 != 0
}

/// Drive-button cache. `1` is `DRIVE_NO_ROOT_DIR` (a letter with no volume).
fn cached_type(letter: u8) -> Option<u32> {
    crate::app_try(|a| a.drives.iter().find(|d| d.0 == letter).map(|d| d.1)).flatten()
}

pub(crate) unsafe fn unmounted_drive(path: &str) -> bool {
    let Some(letter) = letter_of(path) else { return false };
    if !letter_present(letter) {
        return true;
    }
    cached_type(letter) == Some(1)
}

fn stat_safe(path: &str) -> bool {
    if path.starts_with(r"\\") || path.starts_with("//") {
        return false;
    }
    let Some(letter) = letter_of(path) else { return false };
    if !letter_present(letter) {
        return false;
    }
    !matches!(cached_type(letter), Some(1) | Some(4))
}

fn is_dir(path: &str) -> bool {
    let attr = unsafe { GetFileAttributesW(&HSTRING::from(path)) };
    attr != INVALID_FILE_ATTRIBUTES && attr & FILE_ATTRIBUTE_DIRECTORY.0 != 0
}

/// A child of an archive file. A directory that merely ends in `.zip` is a folder.
pub(crate) unsafe fn in_archive(path: &str) -> bool {
    if !archive_ancestor(path) {
        return false;
    }
    if !stat_safe(path) {
        return true;
    }
    let mut cur = path.trim();
    while let Some(parent) = parent_path(cur) {
        if archive_named(parent) && !is_dir(parent) {
            return true;
        }
        cur = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_named_zip_is_not_an_archive_and_a_missing_zip_is() {
        let root = std::env::temp_dir().join(format!("maguro-mark-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("folder.zip");
        std::fs::create_dir_all(&dir).unwrap();
        let child = dir.join("note.txt");
        std::fs::write(&child, b"x").unwrap();
        let path = child.to_string_lossy().replace('/', "\\");
        assert!(!unsafe { in_archive(&path) }, "{path}");
        let fake = root.join("real.zip").join("inner.txt");
        let fake_s = fake.to_string_lossy().replace('/', "\\");
        assert!(unsafe { in_archive(&fake_s) }, "{fake_s}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_letter_that_is_not_in_the_drive_mask_is_unmounted() {
        let mask = unsafe { GetLogicalDrives() };
        let free = (b'A'..=b'Z').find(|c| mask >> (c - b'A') & 1 == 0);
        if let Some(c) = free {
            let path = format!(r"{}:\not-mounted\a.txt", c as char);
            assert!(unsafe { unmounted_drive(&path) }, "{path}");
        }
        if mask >> (b'C' - b'A') & 1 != 0 {
            assert!(!unsafe { unmounted_drive(r"C:\Windows") });
        }
    }
}
