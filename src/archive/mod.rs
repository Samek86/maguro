//! Open archives as folders, and mount disk images.
//!
//! A pane is a real Explorer view (`IExplorerBrowser`). A shell namespace
//! extension that pretends an archive is a folder would have to reimplement
//! icons, thumbnails, drag, copy, and "open with". That is a second file
//! manager. Instead the archive is extracted, once, into a cache directory
//! under `%LOCALAPPDATA%\Maguro\ArchiveCache`, and the pane browses that
//! ordinary folder. Copy-out, drag, the default verb, and thumbnails then
//! come from the shell. The address bar and the tab title show the archive
//! path (`D:\x\foo.7z\sub`), not the cache path. Up from the cache root
//! returns to the folder that contains the archive file.
//!
//! Zip stays with Explorer. Explorer already opens a zip as a folder, and
//! changing that would fight the shell.
//!
//! The "extract, then browse a real folder" shape is the same idea as
//! `browse_archives` in rustyfiles (MIT, https://github.com/gelenmedya/rustyfiles).
//! The readers here are Maguro's. The ISO reader follows ECMA-119 (primary
//! volume descriptor and Joliet). It does not read UDF or Rock Ridge.
//!
//! Mounting an `.iso` / `.img` / `.vhd` / `.vhdx` is separate: the Virtual
//! Disk API attaches the image and the pane navigates to the new drive letter.

mod cache;
mod extract;
mod iso;
mod kind;
mod names;

pub(crate) mod menu;
pub(crate) mod vdisk;

pub(crate) use cache::{
    display_path, history_path, parent_of_archive, plan, session_load_path, session_store_path, shutdown, startup, touch_path, up_from, Plan,
};

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use extract::ExtractError;

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

struct Live {
    pane: usize,
    /// Folder the pane was showing when the extract started.
    from: String,
    cancel: Arc<AtomicBool>,
}

fn lock_live() -> std::sync::MutexGuard<'static, Option<Live>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The user turned "Open archives as folders" on. Missing settings mean on.
pub(crate) fn enabled() -> bool {
    crate::app(|a| a.s.open_archives)
}

/// `path` is a file this feature opens as a folder, and the setting is on.
pub(crate) fn wants_folder(path: &str) -> bool {
    enabled() && kind::browse_kind(path).is_some() && Path::new(path).is_file()
}

/// Start an extract, or browse the cache when this archive is already there.
/// The extract runs off the UI thread. A later message navigates the pane.
pub(crate) unsafe fn begin_open(pane: usize, path: &str) {
    let path = path.trim();
    if path.is_empty() || !enabled() {
        return;
    }
    match plan(path) {
        Plan::Browse(dest) => {
            let _ = crate::navigate(pane, &dest);
        }
        Plan::Missing(parent) => {
            let _ = crate::navigate(pane, &parent);
        }
        Plan::Extract { archive, inner } => spawn_extract(pane, &archive, &inner),
        Plan::Normal => {}
    }
}

/// The pane left the folder the extract started from. The worker stops
/// copying and does not navigate when it finishes.
pub(crate) fn note_left(pane: usize, path: &str) {
    let live = lock_live();
    if let Some(job) = live.as_ref() {
        if job.pane == pane && !same_path(&job.from, path) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']))
}

fn spawn_extract(pane: usize, archive: &str, inner: &str) {
    let from = crate::app(|a| a.s.paths.get(pane).cloned().unwrap_or_default());
    let hwnd = crate::app(|a| a.main.0 as isize);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut live = lock_live();
        if let Some(old) = live.take() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        *live = Some(Live { pane, from: from.clone(), cancel: cancel.clone() });
    }
    post(hwnd, UiMsg::Progress(crate::tr(crate::Key::OpeningArchive).to_string()));
    let archive = archive.to_string();
    let inner = inner.to_string();
    std::thread::spawn(move || {
        let outcome = cache::open_into_cache(Path::new(&archive), &cancel, cache::MAX_EXTRACT_BYTES);
        let msg = match outcome {
            Ok(hit) => {
                let dest = cache::join_inner(&hit.files, &inner);
                if dest.is_dir() {
                    UiMsg::Done { pane, from, dest: dest.display().to_string() }
                } else {
                    UiMsg::Fail(crate::tf(crate::Key::ArchiveOpenFailed, &[("detail", crate::tr(crate::Key::CanTOpenThisFolder))]))
                }
            }
            Err(ExtractError::Cancelled) => UiMsg::Quiet,
            Err(ExtractError::Encrypted) => UiMsg::Fail(crate::tr(crate::Key::ArchiveEncrypted).to_string()),
            Err(ExtractError::TooLarge { limit, .. }) => UiMsg::Fail(crate::tf(crate::Key::ArchiveTooLarge, &[("limit", &human_bytes(limit))])),
            Err(e) => UiMsg::Fail(crate::tf(crate::Key::ArchiveOpenFailed, &[("detail", &trim_detail(&e.to_string()))])),
        };
        {
            let mut live = lock_live();
            if live.as_ref().is_some_and(|j| j.pane == pane && Arc::ptr_eq(&j.cancel, &cancel)) {
                *live = None;
            }
        }
        post(hwnd, msg);
    });
}

fn human_bytes(n: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    if n >= GB && n % GB == 0 {
        format!("{} GB", n / GB)
    } else {
        format!("{n} B")
    }
}

fn trim_detail(s: &str) -> String {
    let s = s.replace(['\r', '\n'], " ");
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= 180 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

enum UiMsg {
    Progress(String),
    Done { pane: usize, from: String, dest: String },
    Fail(String),
    Quiet,
    Mounted { pane: usize, letter: char },
}

fn queue() -> &'static Mutex<Vec<UiMsg>> {
    static Q: Mutex<Vec<UiMsg>> = Mutex::new(Vec::new());
    &Q
}

fn post(hwnd: isize, msg: UiMsg) {
    if hwnd == 0 {
        return;
    }
    queue().lock().unwrap_or_else(|e| e.into_inner()).push(msg);
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(windows::Win32::Foundation::HWND(hwnd as *mut _)),
            crate::WM_ARCHIVE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
}

pub(crate) fn post_fail(text: String) {
    let hwnd = crate::app(|a| a.main.0 as isize);
    post(hwnd, UiMsg::Fail(text));
}

pub(crate) fn post_mount(hwnd: isize, pane: usize, result: Result<char, vdisk::MountError>) {
    let msg = match result {
        Ok(letter) => UiMsg::Mounted { pane, letter },
        Err(vdisk::MountError::AccessDenied) => UiMsg::Fail(crate::tr(crate::Key::MountNeedsAdmin).to_string()),
        Err(e) => UiMsg::Fail(crate::tf(crate::Key::MountFailed, &[("detail", &e.to_string())])),
    };
    post(hwnd, msg);
}

/// UI thread. Posted by the extract or mount worker.
pub(crate) unsafe fn on_ui() {
    let batch = std::mem::take(&mut *queue().lock().unwrap_or_else(|e| e.into_inner()));
    for msg in batch {
        match msg {
            UiMsg::Progress(text) => note(&text, 30),
            UiMsg::Quiet => {}
            UiMsg::Fail(text) => {
                clear_opening();
                let owner = crate::app(|a| a.main);
                let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    Some(owner),
                    &windows::core::HSTRING::from(&text),
                    windows::core::w!("Maguro"),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK | windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
                );
            }
            UiMsg::Done { pane, from, dest } => {
                let still = crate::app(|a| a.s.paths.get(pane).is_some_and(|p| same_path(p, &from)));
                if !still {
                    continue;
                }
                clear_opening();
                let _ = crate::navigate(pane, &dest);
                crate::focus_view(pane);
            }
            UiMsg::Mounted { pane, letter } => {
                clear_opening();
                let dest = format!(r"{letter}:\");
                let _ = crate::navigate(pane, &dest);
                crate::focus_view(pane);
            }
        }
    }
}

fn clear_opening() {
    note("", 0);
}

fn note(text: &str, secs: u64) {
    crate::app(|a| {
        if text.is_empty() || secs == 0 {
            if a.note.as_ref().is_some_and(|(s, _)| s == crate::tr(crate::Key::OpeningArchive)) {
                a.note = None;
            }
        } else {
            a.note = Some((text.to_string(), std::time::Instant::now() + std::time::Duration::from_secs(secs)));
        }
    });
    unsafe { crate::update_status() };
}

/// Paths the session is showing, so cache cleanup leaves them alone.
pub(crate) fn shown_paths(s: &crate::Session) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(s.paths.iter().cloned());
    for t in &s.tabs {
        out.extend(t.paths.iter().cloned());
        for folders in &t.pane_folders {
            out.extend(folders.iter().cloned());
        }
    }
    for folders in &s.pane_folders {
        out.extend(folders.iter().cloned());
    }
    out
}
