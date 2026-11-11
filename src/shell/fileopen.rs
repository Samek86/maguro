//! Open a file without blocking the UI thread.
//!
//! Double-click, Enter, the address bar, peek, and a mixed selection used to call
//! `ShellExecute` on the UI thread. For a document that goes through a context-menu
//! handler or a DDE server (Office), that call does not return until the handler is
//! loaded and the target application is ready. COM unloads those handlers after a
//! few minutes idle, so the next open pays the cost again and the window looks stuck.
//!
//! A filesystem document is launched here, on one background STA. Enter, the address
//! bar, peek, and a menu still use `ShellExecuteExW` with `SEE_MASK_INVOKEIDLIST |
//! SEE_MASK_ASYNCOK | SEE_MASK_FLAG_NO_UI` for a local path, so the default verb runs
//! off the UI thread. A double-click does not wait for that menu invoke: it starts
//! the registered `open` command first, and uses the id list only when that command
//! fails. The STA pumps messages, which is what lets a DDE conversation complete.
//! The working directory is the parent folder. `AllowSetForegroundWindow(ASFW_ANY)`
//! runs on the UI thread, while the click still owns the foreground, and again on
//! the worker just before the launch, so the opened app can come forward.
//!
//! Executables stay a direct `open` (no context-menu invoke): that is CreateProcess,
//! and a UAC or SmartScreen window still belongs to that path. A non-filesystem
//! item is not launched here. The caller keeps the existing shell path for those.
//! Folders, folder shortcuts, and zip-as-folder never reach this module.

use std::collections::VecDeque;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, Instant};

use crate::imports::*;

/// `CMINVOKECOMMANDINFO.fMask`. The windows crate does not export these two.
pub(crate) const CMIC_MASK_ASYNCOK: u32 = 0x0010_0000;
pub(crate) const CMIC_MASK_FLAG_NO_UI: u32 = 0x0000_0400;

/// Any process may take the foreground. The click handler calls this while it still can.
const ASFW_ANY: u32 = 0xFFFF_FFFF;
const QUEUE_CAP: usize = 32;

/// How the worker should start this file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Default verb. A local file goes through its pidl; a share uses the path.
    Verb,
    /// `open` on the path. Executables, so the shell runs the image itself.
    Run,
    /// Double-click. The registered `open` command runs before any context-menu invoke.
    First,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenFile {
    pub(crate) path: String,
    pub(crate) directory: String,
    pub(crate) mode: Mode,
}

struct Job {
    source: &'static str,
    hwnd: isize,
    files: Vec<OpenFile>,
}

struct Queue {
    jobs: VecDeque<Job>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    jobs: VecDeque::new(),
});
static WAKE: Condvar = Condvar::new();
static WORKER_OK: AtomicBool = AtomicBool::new(false);

/// `.exe`, `.com`, `.bat`, `.cmd`, `.msi`, `.msc`, `.scr`, `.cpl`. A shortcut is not one of these.
pub(crate) fn executable_path(path: &str) -> bool {
    let name = path.trim().rsplit(['\\', '/']).next().unwrap_or("");
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || ext.is_empty() {
        return false;
    }
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe" | "com" | "bat" | "cmd" | "msi" | "msc" | "scr" | "cpl"
    )
}

/// `Some` for a filesystem file this module can start. `None` for an empty path or a
/// shell item that is not a file path (`::{CLSID}`, `shell:`). The caller uses its
/// existing path for `None`.
pub(crate) fn route(path: &str) -> Option<Mode> {
    let path = path.trim();
    if path.is_empty() || !crate::is_fs(path) {
        return None;
    }
    if executable_path(path) {
        Some(Mode::Run)
    } else {
        Some(Mode::Verb)
    }
}

pub(crate) fn prepare(path: &str) -> Option<OpenFile> {
    let mode = route(path)?;
    let directory = crate::shell::parent_of(path.trim()).unwrap_or_default();
    Some(OpenFile {
        path: path.trim().to_string(),
        directory,
        mode,
    })
}

/// Flags added to `InvokeCommand` for the Open verb. Other verbs, including delete,
/// rename, and properties, get zero so they still finish on the UI thread before the
/// menu state is read. `NO_UI` is only on a verb that is literally `open`: an empty
/// default item can still be a chooser dialog.
pub(crate) fn open_invoke_mask(verb: &str, default_item: bool) -> u32 {
    let verb = verb.trim();
    if verb.eq_ignore_ascii_case("open") {
        return CMIC_MASK_ASYNCOK | CMIC_MASK_FLAG_NO_UI;
    }
    if verb.is_empty() && default_item {
        return CMIC_MASK_ASYNCOK;
    }
    0
}

/// A double-click document uses [`Mode::First`]. An executable stays [`Mode::Run`].
pub(crate) fn for_double_click(mut file: OpenFile) -> OpenFile {
    if file.mode == Mode::Verb {
        file.mode = Mode::First;
    }
    file
}

/// Queue `files` and return. False when there is nothing to do or the worker did not start.
/// The caller then keeps its previous launch path.
pub(crate) fn launch(source: &'static str, hwnd: isize, files: Vec<OpenFile>) -> bool {
    enqueue(source, hwnd, files, false)
}

/// Same as [`launch`], placed ahead of opens that have not started yet.
pub(crate) fn launch_soon(source: &'static str, hwnd: isize, files: Vec<OpenFile>) -> bool {
    enqueue(source, hwnd, files, true)
}

fn enqueue(source: &'static str, hwnd: isize, files: Vec<OpenFile>, ahead: bool) -> bool {
    if files.is_empty() {
        return false;
    }
    unsafe { _ = AllowSetForegroundWindow(ASFW_ANY) };
    if !ensure_worker() {
        super::openlog::trace("error", || format!("source={source} err=worker"));
        return false;
    }
    {
        let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        if q.jobs.len() >= QUEUE_CAP {
            super::openlog::trace("error", || format!("source={source} err=queue-full"));
            return false;
        }
        let job = Job { source, hwnd, files };
        if ahead {
            q.jobs.push_front(job);
        } else {
            q.jobs.push_back(job);
        }
    }
    WAKE.notify_one();
    true
}

fn ensure_worker() -> bool {
    static WORKER: Once = Once::new();
    WORKER.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("open-file".into())
            .spawn(worker);
        WORKER_OK.store(spawned.is_ok(), Ordering::Relaxed);
    });
    WORKER_OK.load(Ordering::Relaxed)
}

fn worker() {
    // Apartment STA, as ShellExecuteEx with an id list requires. OLE as well: the
    // invoke builds a data object. The thread stays up, so COM is not torn down.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    if com {
        let _ = unsafe { OleInitialize(None) };
    }
    loop {
        let job = recv();
        if !com {
            super::openlog::trace("error", || format!("source={} err=com-init", job.source));
            unsafe { _ = MessageBeep(MB_ICONWARNING) };
            continue;
        }
        for file in &job.files {
            unsafe { launch_one(job.source, job.hwnd, file) };
        }
    }
}

fn recv() -> Job {
    loop {
        let job = {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if q.jobs.is_empty() {
                let waited = WAKE.wait_timeout(q, Duration::from_millis(50));
                q = match waited {
                    Ok((guard, _)) => guard,
                    Err(e) => e.into_inner().0,
                };
            }
            q.jobs.pop_front()
        };
        if let Some(job) = job {
            return job;
        }
        // DDE after SEE_MASK_ASYNCOK completes only if this STA keeps pumping.
        unsafe { pump_all() };
    }
}

unsafe fn pump_all() {
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
        _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe fn pump_pending() {
    let mut msg = MSG::default();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(30)
        && PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool()
    {
        _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe fn launch_one(source: &str, hwnd: isize, file: &OpenFile) {
    _ = AllowSetForegroundWindow(ASFW_ANY);
    let owner = HWND(hwnd as *mut core::ffi::c_void);
    let directory = (!file.directory.is_empty()).then(|| HSTRING::from(&file.directory));
    let dir = directory
        .as_ref()
        .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));
    // A double-click must start the program before any context-menu handler loads.
    if file.mode == Mode::First {
        if try_path(source, owner, dir, &file.path, "first", false) {
            return;
        }
        if !crate::unc_path(&file.path) && try_idlist(source, owner, dir, &file.path) {
            return;
        }
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    if file.mode == Mode::Verb
        && !crate::unc_path(&file.path)
        && try_idlist(source, owner, dir, &file.path)
    {
        return;
    }
    let mode = if file.mode == Mode::Run { "run" } else { "path" };
    try_path(source, owner, dir, &file.path, mode, true);
}

unsafe fn try_idlist(source: &str, owner: HWND, dir: PCWSTR, path: &str) -> bool {
    super::openlog::trace("launch-before", || {
        format!(
            "source={source} mode=invoke-idlist dir={} file={path}",
            dir_text(dir)
        )
    });
    let mut pidl = std::ptr::null_mut();
    let parsed = SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None).is_ok()
        && !pidl.is_null();
    if !parsed {
        if !pidl.is_null() {
            CoTaskMemFree(Some(pidl as _));
        }
        super::openlog::trace("error", || {
            format!("source={source} mode=invoke-idlist err=pidl file={path}")
        });
        return false;
    }
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_ASYNCOK | SEE_MASK_FLAG_NO_UI,
        hwnd: owner,
        lpIDList: pidl as _,
        lpDirectory: dir,
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let hr = ShellExecuteExW(&mut sei);
    // The shell copies the pidl during the call. Pump once so a DDE post is dispatched
    // before the memory goes away.
    pump_pending();
    CoTaskMemFree(Some(pidl as _));
    let ok = hr.is_ok();
    super::openlog::trace("launch-after", || {
        let err = hr.as_ref().err().map(|e| e.code().0).unwrap_or(0);
        format!(
            "source={source} mode=invoke-idlist ok={} err={err} file={path}",
            ok as u8
        )
    });
    // A failed id list is retried as a path open. Beep only if that retry fails too.
    ok
}

unsafe fn try_path(source: &str, owner: HWND, dir: PCWSTR, path: &str, mode: &str, beep: bool) -> bool {
    super::openlog::trace("launch-before", || {
        format!(
            "source={source} mode={mode} dir={} file={path}",
            dir_text(dir)
        )
    });
    let file = HSTRING::from(path);
    let verb = HSTRING::from("open");
    // No NO_UI here. A missing association can show Open With, and an executable can
    // show its own error. ASYNCOK still returns before a DDE server is idle.
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_ASYNCOK,
        hwnd: owner,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpDirectory: dir,
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let hr = ShellExecuteExW(&mut sei);
    pump_pending();
    let ok = hr.is_ok();
    super::openlog::trace("launch-after", || {
        let err = hr.as_ref().err().map(|e| e.code().0).unwrap_or(0);
        format!(
            "source={source} mode={mode} ok={} err={err} file={path}",
            ok as u8
        )
    });
    if !ok && beep {
        _ = MessageBeep(MB_ICONWARNING);
    }
    ok
}

fn dir_text(dir: PCWSTR) -> String {
    if dir.0.is_null() {
        return String::new();
    }
    unsafe {
        let mut n = 0usize;
        while *dir.0.add(n) != 0 {
            n += 1;
            if n > 1024 {
                break;
            }
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(dir.0, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_use_the_verb_and_executables_run() {
        assert_eq!(route(r"C:\Work\a.docx"), Some(Mode::Verb));
        assert_eq!(route(r"D:\notes\a.PDF"), Some(Mode::Verb));
        assert_eq!(route(r"\\server\share\a.pdf"), Some(Mode::Verb));
        assert_eq!(route(r"C:\Work\setup.EXE"), Some(Mode::Run));
        assert_eq!(route(r"C:\Work\install.msi"), Some(Mode::Run));
        assert_eq!(route(r"C:\Work\go.bat"), Some(Mode::Run));
        assert_eq!(route(r"C:\Work\tool.cmd"), Some(Mode::Run));
        // A shortcut is a file the shell resolves. It is not the executable image.
        assert_eq!(route(r"C:\Work\setup.exe.lnk"), Some(Mode::Verb));
        assert_eq!(route(r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), None);
        assert_eq!(route("shell:Downloads"), None);
        assert_eq!(route(""), None);
        assert_eq!(route("   "), None);
    }

    #[test]
    fn open_verb_is_async_and_other_verbs_stay_sync() {
        let open = open_invoke_mask("open", false);
        assert_eq!(open & CMIC_MASK_ASYNCOK, CMIC_MASK_ASYNCOK);
        assert_eq!(open & CMIC_MASK_FLAG_NO_UI, CMIC_MASK_FLAG_NO_UI);
        assert_eq!(
            open_invoke_mask("Open", true) & CMIC_MASK_ASYNCOK,
            CMIC_MASK_ASYNCOK
        );
        let unnamed = open_invoke_mask("", true);
        assert_eq!(unnamed, CMIC_MASK_ASYNCOK);
        assert_eq!(open_invoke_mask("", false), 0);
        assert_eq!(open_invoke_mask("properties", true), 0);
        assert_eq!(open_invoke_mask("delete", false), 0);
        assert_eq!(open_invoke_mask("rename", false), 0);
        assert_eq!(open_invoke_mask("explore", false), 0);
        assert_eq!(open_invoke_mask("openas", false), 0);
        assert_eq!(open_invoke_mask("openwith", false), 0);
        assert_eq!(open_invoke_mask("opennew", false), 0);
    }

    #[test]
    fn a_double_click_document_starts_with_the_open_command() {
        let doc = OpenFile {
            path: r"C:\Work\note.md".into(),
            directory: r"C:\Work".into(),
            mode: Mode::Verb,
        };
        assert_eq!(for_double_click(doc).mode, Mode::First);
        let exe = OpenFile {
            path: r"C:\Work\setup.EXE".into(),
            directory: r"C:\Work".into(),
            mode: Mode::Run,
        };
        assert_eq!(for_double_click(exe).mode, Mode::Run);
    }
}
