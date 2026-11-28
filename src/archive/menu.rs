//! Maguro rows on the real Explorer menu: mount a disk image, or eject one.

use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{InsertMenuW, MF_BYPOSITION, MF_STRING, HMENU};

use super::kind::disk_kind;
use super::vdisk::{self, drive_root};

pub(crate) const ID_CTX_MOUNT: u32 = 0x800A;
pub(crate) const ID_CTX_UNMOUNT: u32 = 0x800B;
pub(crate) const ID_CTX_EJECT: u32 = 0x800C;

/// One selected disk image gets Mount, and Unmount when that image is attached.
/// A virtual drive root gets Eject / unmount.
pub(crate) unsafe fn insert(menu: HMENU, items: &[(String, bool)], background: bool, folder: &str) {
    if !crate::shell::ctxmenu::maguro_shown("mount") {
        return;
    }
    let at = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(menu)).max(0) as u32;
    let file = (!background && items.len() == 1).then(|| items[0].0.as_str());
    if let Some(path) = file {
        if disk_kind(path).is_some() && std::path::Path::new(path).is_file() {
            let label = HSTRING::from(crate::tr(crate::Key::MountAndOpen));
            let _ = InsertMenuW(menu, at, MF_STRING | MF_BYPOSITION, ID_CTX_MOUNT as usize, &label);
            if vdisk::attached_letter(std::path::Path::new(path)).is_some() {
                let label = HSTRING::from(crate::tr(crate::Key::DiskUnmount));
                let _ = InsertMenuW(menu, at.saturating_add(1), MF_STRING | MF_BYPOSITION, ID_CTX_UNMOUNT as usize, &label);
            }
            return;
        }
    }
    let drive = file.and_then(drive_root).or_else(|| background.then(|| drive_root(folder)).flatten());
    if let Some(letter) = drive {
        if vdisk::image_for_letter(letter).is_some() || vdisk::file_backed_drive(letter) {
            let label = HSTRING::from(crate::tr(crate::Key::EjectUnmount));
            let _ = InsertMenuW(menu, at, MF_STRING | MF_BYPOSITION, ID_CTX_EJECT as usize, &label);
        }
    }
}

pub(crate) fn handles(cmd: i32) -> bool {
    matches!(cmd as u32, ID_CTX_MOUNT | ID_CTX_UNMOUNT | ID_CTX_EJECT)
}

/// Run the chosen row. The pane navigates after a mount. Panes sitting on the
/// drive move to the image's parent before it is detached.
pub(crate) unsafe fn run(cmd: i32, owner: windows::Win32::Foundation::HWND, pane: usize, items: &[(String, bool)], folder: &str) {
    let path = items.first().map(|(p, _)| p.clone()).filter(|p| !p.is_empty()).or_else(|| (!folder.is_empty()).then(|| folder.to_string()));
    let Some(path) = path else { return };
    match cmd as u32 {
        ID_CTX_MOUNT => {
            let image = path.clone();
            // WM_ARCHIVE is handled by the main window, not by the menu owner (a pane or view).
            let _ = owner;
            let hwnd = crate::app(|a| a.main.0 as isize);
            std::thread::spawn(move || {
                let result = vdisk::mount(std::path::Path::new(&image));
                // A new CD-ROM volume can take a moment before its root is readable.
                if let Ok(letter) = result {
                    let root = format!(r"{letter}:\");
                    for _ in 0..50 {
                        if std::path::Path::new(&root).is_dir() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
                super::post_mount(hwnd, pane, result);
            });
        }
        ID_CTX_UNMOUNT => detach_image(&path),
        ID_CTX_EJECT => {
            if let Some(letter) = drive_root(&path).or_else(|| drive_root(folder)) {
                if let Some(image) = vdisk::image_for_letter(letter) {
                    move_panes_off(letter, &image);
                }
                if let Err(e) = vdisk::eject_letter(letter) {
                    fail_unmount(&e);
                }
            }
        }
        _ => {}
    }
}

fn detach_image(path: &str) {
    let letter = vdisk::attached_letter(std::path::Path::new(path));
    if let Some(letter) = letter {
        move_panes_off(letter, path);
    }
    if let Err(e) = vdisk::unmount(std::path::Path::new(path)) {
        fail_unmount(&e);
    }
}

fn move_panes_off(letter: char, image: &str) {
    let parent = super::parent_of_archive(image).unwrap_or_else(|| format!(r"{letter}:\"));
    let prefix = format!(r"{letter}:\");
    let panes: Vec<(usize, String)> = crate::app(|a| {
        a.s.paths.iter().enumerate().filter(|(_, p)| p.len() >= 2 && p.as_bytes()[0].eq_ignore_ascii_case(&prefix.as_bytes()[0]) && p.as_bytes().get(1) == Some(&b':')).map(|(i, p)| (i, p.clone())).collect()
    });
    for (i, path) in panes {
        if path.len() >= 2 && path.as_bytes()[0].eq_ignore_ascii_case(&(letter as u8)) {
            unsafe { let _ = crate::navigate(i, &parent); }
        }
    }
}

fn fail_unmount(err: &vdisk::MountError) {
    let text = match err {
        vdisk::MountError::AccessDenied => crate::tr(crate::Key::MountNeedsAdmin).to_string(),
        other => crate::tf(crate::Key::UnmountFailed, &[("detail", &other.to_string())]),
    };
    super::post_fail(text);
}
