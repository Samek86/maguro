//! Load shell context-menu handler DLLs before the first click, and keep them loaded.
//!
//! The menu the user sees is unchanged: `popup_ctx` still asks the shell view's own
//! `IContextMenu`. That `QueryContextMenu` is what loads each in-proc handler (7-Zip,
//! cloud sync, antivirus, …) the first time. COM's idle sweep (`CoFreeUnusedLibraries`,
//! about ten minutes with no outstanding objects) unloads them again, and Windows may
//! page the image out, so the next click pays the same cost.
//!
//! A few seconds after the first window is shown, and again when the window comes
//! forward after a long idle, this module builds that menu on a background STA and
//! drops it without showing it or running a verb. The handler DLLs that the query
//! loaded are pinned for the life of the process. Successful `IContextMenu` objects
//! are also kept on that STA so `DllCanUnloadNow` stays false. Nothing is written to
//! the registry or to the disk. `blocked_shell_extensions` is already applied
//! process-wide at startup; a listed DLL is not pinned if it loaded anyway.
//!
//! Network folders are skipped. A dead share must not block this thread, and it must
//! not block the UI. Failures and panics are swallowed.

use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64};
use std::sync::OnceLock;

use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::Win32::System::ProcessStatus::EnumProcessModules;
use windows::Win32::System::Threading::GetCurrentProcess;

use crate::app::*;
use crate::imports::*;

const START_MS: u32 = 3_000;
const POSTPONE_MS: u32 = 2_000;
const POSTPONE_CAP: u32 = 8;
/// Activate-time retries, so a skipped network folder is tried again without a storm.
const RETRY_GAP_MS: u64 = 30_000;
pub(crate) const IDLE_MS: u64 = 10 * 60 * 1000;
const SCAN_CAP: usize = 256;

static EPOCH: OnceLock<Instant> = OnceLock::new();
static ARMED_HWND: AtomicIsize = AtomicIsize::new(0);
static POSTPONES: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static WARMED: AtomicBool = AtomicBool::new(false);
static SKIPPED: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
static LAST_ENQ: AtomicU64 = AtomicU64::new(0);

struct Job {
    folder: String,
    blocked: Vec<String>,
    /// Registry catalogue only. The menu warm still waits for the startup timer.
    catalogue_only: bool,
}

enum Outcome {
    /// At least one `QueryContextMenu` succeeded.
    Warmed,
    /// Not a local filesystem folder. Try again when the pane changes.
    Skip,
    /// A menu was already open, so this pass stepped aside.
    Busy,
    /// The folder was local and every query failed.
    Failed,
    /// The handler catalogue is in memory. Warmup flags stay as they were.
    Listed,
}

struct Queue {
    job: Option<Job>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { job: None });
static WAKE: Condvar = Condvar::new();

thread_local! {
    static HELD: RefCell<Vec<IContextMenu>> = const { RefCell::new(Vec::new()) };
}

struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as _)) };
        }
    }
}

struct Popup(HMENU);
impl Drop for Popup {
    fn drop(&mut self) {
        unsafe { _ = DestroyMenu(self.0) };
    }
}

/// Pins handler DLLs even when a query panics halfway through.
struct PinLater {
    before: Vec<isize>,
    blocked: Vec<String>,
}
impl Drop for PinLater {
    fn drop(&mut self) {
        pin_new(&self.before, &self.blocked);
    }
}

fn mono_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// The startup timer owns the first pass. Later activations only re-warm.
pub(crate) fn activate_wants_warm(warmed: bool, skipped: bool, last_ms: u64, now_ms: u64) -> bool {
    if warmed {
        return last_ms != 0 && now_ms.saturating_sub(last_ms) >= IDLE_MS;
    }
    skipped
}

/// A local drive folder. `remote` is `GetDriveTypeW == DRIVE_REMOTE`, computed by the caller.
pub(crate) fn folder_ok(path: &str, remote: bool) -> bool {
    !path.is_empty() && crate::is_fs(path) && !crate::unc_path(path) && !remote
}

pub(crate) fn leaf_name(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let name = trimmed.rsplit(['\\', '/']).next().unwrap_or("");
    if plain_name(name) {
        Some(name)
    } else {
        None
    }
}

fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['\\', '/', ':'])
}

pub(crate) struct Sample {
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) link: bool,
}

/// First ordinary file and first child folder. Links are not samples: opening one can leave the drive.
pub(crate) fn pick_samples(entries: &[Sample]) -> (Option<String>, Option<String>) {
    let mut file = None;
    let mut dir = None;
    for entry in entries.iter().take(SCAN_CAP) {
        if entry.link || !plain_name(&entry.name) {
            continue;
        }
        if entry.dir {
            if dir.is_none() {
                dir = Some(entry.name.clone());
            }
        } else if file.is_none() {
            file = Some(entry.name.clone());
        }
        if file.is_some() && dir.is_some() {
            break;
        }
    }
    (file, dir)
}

/// Other files in the same folder, one per extension, skipping the file already warmed.
pub(crate) fn pick_more(entries: &[Sample], skip: Option<&str>, cap: usize) -> Vec<String> {
    let mut seen = Vec::new();
    if let Some(name) = skip {
        if let Some(ext) = ext_of(name) {
            seen.push(ext);
        }
    }
    let mut out = Vec::new();
    for entry in entries.iter().take(SCAN_CAP) {
        if entry.link || entry.dir || !plain_name(&entry.name) {
            continue;
        }
        if skip.is_some_and(|s| s.eq_ignore_ascii_case(&entry.name)) {
            continue;
        }
        let Some(ext) = ext_of(&entry.name) else { continue };
        if seen.iter().any(|e| e == &ext) {
            continue;
        }
        seen.push(ext);
        out.push(entry.name.clone());
        if out.len() == cap {
            break;
        }
    }
    out
}

fn ext_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.contains(['\\', '/']) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Third-party modules only. System DLLs and a blocked shell extension stay unpinned.
pub(crate) fn pin_path(path: &str, blocked: &[String]) -> bool {
    super::blockext::blockable(path) && !super::blockext::names_match(blocked, path)
}

/// One timer on the first window, a few seconds after it is shown.
pub(crate) fn arm(h: HWND) {
    if h.0.is_null() || STARTED.load(Ordering::Relaxed) || WARMED.load(Ordering::Relaxed) {
        return;
    }
    let id = h.0 as isize;
    if ARMED_HWND
        .compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    unsafe {
        if SetTimer(Some(h), TIMER_CTX_WARM, START_MS, None) == 0 {
            ARMED_HWND.store(0, Ordering::Relaxed);
        }
    }
}

pub(crate) fn disarm(h: HWND) {
    if ARMED_HWND.load(Ordering::Relaxed) != h.0 as isize {
        return;
    }
    unsafe { _ = KillTimer(Some(h), TIMER_CTX_WARM) };
    if !STARTED.load(Ordering::Relaxed) {
        ARMED_HWND.store(0, Ordering::Relaxed);
    }
}

pub(crate) fn on_timer(h: HWND) {
    unsafe { _ = KillTimer(Some(h), TIMER_CTX_WARM) };
    let folder = app(|a| a.s.paths.get(a.s.active).cloned().unwrap_or_default());
    // Empty or a shell place (This PC) is not ready. A UNC path is a decision, not a delay.
    let pending = folder.is_empty() || (!crate::is_fs(&folder) && !crate::unc_path(&folder));
    let n = POSTPONES.load(Ordering::Relaxed);
    if (pending || ui_busy()) && n < POSTPONE_CAP {
        POSTPONES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            if SetTimer(Some(h), TIMER_CTX_WARM, POSTPONE_MS, None) == 0 {
                STARTED.store(true, Ordering::Relaxed);
                request();
            }
        }
        return;
    }
    STARTED.store(true, Ordering::Relaxed);
    request();
}

/// Forward again after a long idle, or after a pass that found nothing local to warm.
pub(crate) fn on_activate() {
    if !STARTED.load(Ordering::Relaxed) {
        return;
    }
    let now = mono_ms();
    if !activate_wants_warm(
        WARMED.load(Ordering::Relaxed),
        SKIPPED.load(Ordering::Relaxed),
        LAST.load(Ordering::Relaxed),
        now,
    ) {
        return;
    }
    let enq = LAST_ENQ.load(Ordering::Relaxed);
    if enq != 0 && now.saturating_sub(enq) < RETRY_GAP_MS {
        return;
    }
    if ui_busy() {
        return;
    }
    request();
}

/// A real menu just loaded the handlers. The idle clock starts now.
pub(crate) fn note_menu() {
    LAST.store(mono_ms().max(1), Ordering::Relaxed);
}

fn ui_busy() -> bool {
    if crate::CTX_OPEN.load(Ordering::Acquire) {
        return true;
    }
    app(|a| a.drag.is_some() || a.sizing)
}

fn request() {
    let (folder, blocked) = app(|a| {
        let folder = a.s.paths.get(a.s.active).cloned().unwrap_or_default();
        (folder, a.s.blocked_shell_extensions.clone())
    });
    LAST_ENQ.store(mono_ms().max(1), Ordering::Relaxed);
    enqueue(Job { folder, blocked, catalogue_only: false });
}

/// Read the shell-extension catalogue before the first click. The visible menu is unchanged.
pub(crate) fn prime() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let blocked = app(|a| a.s.blocked_shell_extensions.clone());
        enqueue(Job { folder: String::new(), blocked, catalogue_only: true });
    });
}

fn enqueue(job: Job) {
    {
        let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        q.job = Some(job);
    }
    ensure_worker();
    WAKE.notify_one();
}

fn ensure_worker() {
    static WORKER: Once = Once::new();
    WORKER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("ctx-warm".into())
            .spawn(worker);
    });
}

fn recv() -> Job {
    loop {
        let job = {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if q.job.is_none() {
                let waited = WAKE.wait_timeout(q, std::time::Duration::from_millis(200));
                q = match waited {
                    Ok((guard, _)) => guard,
                    Err(e) => e.into_inner().0,
                };
            }
            q.job.take()
        };
        if let Some(job) = job {
            return job;
        }
        // The STA has to dispatch. Held handler objects can receive calls only on this thread.
        unsafe { pump_once() };
    }
}

unsafe fn pump_once() {
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
        _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn worker() {
    // OLE as well as COM: the shell builds an `IDataObject` while it creates the menu.
    let ole = unsafe { OleInitialize(None).is_ok() };
    loop {
        let job = recv();
        if !ole {
            apply(Outcome::Failed);
            continue;
        }
        let outcome =
            match crate::crash::guard(std::panic::AssertUnwindSafe(|| unsafe { execute(&job) })) {
                Ok(v) => v,
                Err(_) => Outcome::Failed,
            };
        apply(outcome);
    }
}

fn apply(outcome: Outcome) {
    match outcome {
        Outcome::Listed => {}
        Outcome::Warmed | Outcome::Failed => {
            WARMED.store(true, Ordering::Relaxed);
            SKIPPED.store(false, Ordering::Relaxed);
            LAST.store(mono_ms().max(1), Ordering::Relaxed);
        }
        Outcome::Skip | Outcome::Busy => {
            if !WARMED.load(Ordering::Relaxed) {
                SKIPPED.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn menu_idle() -> bool {
    !crate::CTX_OPEN.load(Ordering::Acquire)
}

unsafe fn execute(job: &Job) -> Outcome {
    // The first click used to scan the registry on the UI thread. Do it here instead.
    super::ctxmenu::ensure_catalogue();
    if job.catalogue_only {
        return Outcome::Listed;
    }
    if !menu_idle() {
        return Outcome::Busy;
    }
    let remote = crate::share_path(&job.folder);
    if !folder_ok(&job.folder, remote) {
        super::ctxmenu::log_phase("warm-skip", Instant::now());
        return Outcome::Skip;
    }
    let t_all = Instant::now();
    let pin = PinLater {
        before: module_bases(),
        blocked: job.blocked.clone(),
    };
    let mut held = Vec::new();
    let mut tried = false;
    let mut ok_any = false;

    if menu_idle() {
        tried = true;
        if let Some(cm) = guarded("warm-bg", || unsafe { background_menu(&job.folder) }) {
            ok_any = true;
            held.push(cm);
        }
    }

    let samples = read_samples(&job.folder);
    let (file, dir) = pick_samples(&samples);
    let more = pick_more(&samples, file.as_deref(), 4);

    if menu_idle() {
        if let Some(name) = file {
            tried = true;
            if let Some(cm) = guarded("warm-file", || unsafe { item_menu(&job.folder, &name) }) {
                ok_any = true;
                held.push(cm);
            }
        } else if let Some(cm) = warm_fallback_file() {
            tried = true;
            ok_any = true;
            held.push(cm);
        }
    }

    for name in more {
        if !menu_idle() {
            break;
        }
        tried = true;
        if let Some(cm) = guarded("warm-ext", || unsafe { item_menu(&job.folder, &name) }) {
            ok_any = true;
            held.push(cm);
        }
    }

    if menu_idle() {
        if let Some(name) = dir {
            tried = true;
            if let Some(cm) = guarded("warm-folder", || unsafe { item_menu(&job.folder, &name) }) {
                ok_any = true;
                held.push(cm);
            }
        } else if let Some(cm) = warm_self_as_folder(&job.folder) {
            tried = true;
            ok_any = true;
            held.push(cm);
        }
    }

    if !held.is_empty() {
        // Replacing the previous set releases it on this STA. The new objects keep the DLLs locked.
        HELD.with(|h| *h.borrow_mut() = held);
    }
    drop(pin);
    super::ctxmenu::log_phase("warm", t_all);
    if ok_any {
        Outcome::Warmed
    } else if tried {
        Outcome::Failed
    } else {
        Outcome::Busy
    }
}

/// No file in the pane folder. The running executable is a stand-in so `*` handlers still load.
fn warm_fallback_file() -> Option<IContextMenu> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.into_os_string().into_string().ok()?;
    if crate::unc_path(&path) || !crate::is_fs(&path) {
        return None;
    }
    let remote = unsafe { crate::share_path(&path) };
    let parent = crate::parent_of(&path)?;
    if !folder_ok(&parent, remote) {
        return None;
    }
    let leaf = leaf_name(&path)?.to_string();
    guarded("warm-file", move || unsafe { item_menu(&parent, &leaf) })
}

/// No child folder. The pane folder itself, asked from its parent, loads Directory / Folder handlers.
fn warm_self_as_folder(folder: &str) -> Option<IContextMenu> {
    let parent = crate::parent_of(folder)?;
    let remote = unsafe { crate::share_path(&parent) };
    if !folder_ok(&parent, remote) {
        return None;
    }
    let leaf = leaf_name(folder)?.to_string();
    guarded("warm-folder", move || unsafe { item_menu(&parent, &leaf) })
}

fn guarded(label: &str, f: impl FnOnce() -> Option<IContextMenu>) -> Option<IContextMenu> {
    let t0 = Instant::now();
    let found = crate::crash::guard(std::panic::AssertUnwindSafe(f)).unwrap_or(None);
    super::ctxmenu::log_phase(label, t0);
    found
}

fn read_samples(folder: &str) -> Vec<Sample> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(folder) else {
        return out;
    };
    for entry in rd.take(SCAN_CAP) {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        out.push(Sample {
            name,
            dir: kind.is_dir(),
            link: kind.is_symlink(),
        });
    }
    out
}

unsafe fn background_menu(path: &str) -> Option<IContextMenu> {
    if !menu_idle() {
        return None;
    }
    let (folder, _pidl) = bind_folder(path)?;
    let cm = folder
        .CreateViewObject::<IContextMenu>(HWND::default())
        .ok()?;
    // Build the menu and release the HMENU. The object is what we keep.
    query(cm, false)
}

unsafe fn item_menu(folder_path: &str, name: &str) -> Option<IContextMenu> {
    if !menu_idle() || !plain_name(name) {
        return None;
    }
    let (folder, _pidl) = bind_folder(folder_path)?;
    let mut child = std::ptr::null_mut();
    let wide = HSTRING::from(name);
    if folder
        .ParseDisplayName(
            HWND::default(),
            None,
            &wide,
            None,
            &mut child,
            std::ptr::null_mut(),
        )
        .is_err()
        || child.is_null()
    {
        return None;
    }
    let child = Pidl(child);
    let refs = [child.0 as *const ITEMIDLIST];
    let cm = folder
        .GetUIObjectOf::<IContextMenu>(HWND::default(), &refs, None)
        .ok()?;
    query(cm, true)
}

/// Same flags as a click without Shift. No verb is invoked. The menu is destroyed here.
unsafe fn query(cm: IContextMenu, item: bool) -> Option<IContextMenu> {
    let Ok(menu) = CreatePopupMenu() else {
        return None;
    };
    let popup = Popup(menu);
    let mut flags = CMF_EXPLORE | CMF_CANRENAME;
    if item {
        flags |= CMF_ITEMMENU;
    }
    let hr = cm.QueryContextMenu(popup.0, 0, 1, 0x7FFF, flags);
    drop(popup);
    if hr.is_ok() {
        Some(cm)
    } else {
        None
    }
}

unsafe fn bind_folder(path: &str) -> Option<(IShellFolder, Pidl)> {
    let mut raw = std::ptr::null_mut();
    if SHParseDisplayName(&HSTRING::from(path), None, &mut raw, 0, None).is_err() || raw.is_null() {
        return None;
    }
    let pidl = Pidl(raw);
    let desktop = SHGetDesktopFolder().ok()?;
    let folder: IShellFolder = desktop.BindToObject(pidl.0, None).ok()?;
    Some((folder, pidl))
}

fn module_bases() -> Vec<isize> {
    unsafe { modules() }
        .into_iter()
        .map(|m| m.0 as isize)
        .filter(|p| *p != 0)
        .collect()
}

fn pin_new(before: &[isize], blocked: &[String]) {
    for hm in unsafe { modules() } {
        let base = hm.0 as isize;
        if base == 0 || before.contains(&base) {
            continue;
        }
        let Some(path) = module_path(hm) else {
            continue;
        };
        if !pin_path(&path, blocked) {
            continue;
        }
        unsafe {
            let mut out = HMODULE::default();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN;
            let _ = GetModuleHandleExW(flags, PCWSTR(hm.0 as *const u16), &mut out);
        }
    }
}

unsafe fn modules() -> Vec<HMODULE> {
    let proc = GetCurrentProcess();
    let mut need = 0u32;
    let _ = EnumProcessModules(proc, std::ptr::null_mut(), 0, &mut need);
    let n = (need as usize / std::mem::size_of::<HMODULE>()).clamp(1, 1024);
    let mut mods = vec![HMODULE::default(); n];
    let cb = (mods.len() * std::mem::size_of::<HMODULE>()) as u32;
    if EnumProcessModules(proc, mods.as_mut_ptr(), cb, &mut need).is_err() {
        return Vec::new();
    }
    let count = (need as usize / std::mem::size_of::<HMODULE>()).min(mods.len());
    mods.truncate(count);
    mods
}

fn module_path(hm: HMODULE) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(Some(hm), &mut buf);
        if n == 0 {
            return None;
        }
        let n = (n as usize).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_rearm_waits_ten_minutes() {
        assert!(!activate_wants_warm(false, false, 0, 0));
        assert!(!activate_wants_warm(
            true,
            false,
            1_000,
            1_000 + IDLE_MS - 1
        ));
        assert!(activate_wants_warm(true, false, 1_000, 1_000 + IDLE_MS));
        assert!(activate_wants_warm(false, true, 0, 0));
        assert!(!activate_wants_warm(true, true, 5_000, 5_000 + 60_000));
    }

    #[test]
    fn samples_are_one_file_and_one_child_folder() {
        let entries = vec![
            Sample {
                name: "link.txt".into(),
                dir: false,
                link: true,
            },
            Sample {
                name: ".".into(),
                dir: true,
                link: false,
            },
            Sample {
                name: "notes.txt".into(),
                dir: false,
                link: false,
            },
            Sample {
                name: "sub".into(),
                dir: true,
                link: false,
            },
            Sample {
                name: "later.txt".into(),
                dir: false,
                link: false,
            },
            Sample {
                name: r"odd\name".into(),
                dir: false,
                link: false,
            },
        ];
        let (file, dir) = pick_samples(&entries);
        assert_eq!(file.as_deref(), Some("notes.txt"));
        assert_eq!(dir.as_deref(), Some("sub"));
        let more = pick_more(&entries, file.as_deref(), 4);
        assert!(more.is_empty());
    }

    #[test]
    fn more_samples_are_other_extensions() {
        let entries = vec![
            Sample { name: "a.txt".into(), dir: false, link: false },
            Sample { name: "b.txt".into(), dir: false, link: false },
            Sample { name: "c.png".into(), dir: false, link: false },
            Sample { name: "d.docx".into(), dir: false, link: false },
            Sample { name: "link.zip".into(), dir: false, link: true },
            Sample { name: "sub".into(), dir: true, link: false },
        ];
        assert_eq!(pick_more(&entries, Some("a.txt"), 4), vec!["c.png".to_string(), "d.docx".to_string()]);
        assert_eq!(pick_more(&entries, None, 1), vec!["a.txt".to_string()]);
    }

    #[test]
    fn local_folders_only() {
        assert!(folder_ok(r"C:\Work", false));
        assert!(!folder_ok(r"C:\Work", true));
        assert!(!folder_ok(r"\\server\share", false));
        assert!(!folder_ok(
            r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            false
        ));
        assert!(!folder_ok("", false));
        assert_eq!(leaf_name(r"C:\Work\Docs"), Some("Docs"));
        assert_eq!(leaf_name(r"C:\"), None);
        assert_eq!(leaf_name(r"C:\Work\a.txt"), Some("a.txt"));
    }

    #[test]
    fn pin_skips_system_and_blocked_dlls() {
        assert!(pin_path(r"C:\Program Files\7-Zip\7-zip.dll", &[]));
        assert!(!pin_path(r"C:\Windows\System32\shell32.dll", &[]));
        assert!(!pin_path(r"C:\Windows\System32\7-zip.dll", &[]));
        assert!(!pin_path(r"D:\tools\propsys.dll", &[]));
        assert!(!pin_path(
            r"C:\Program Files\7-Zip\7-zip.dll",
            &["7-zip.dll".into()]
        ));
        assert!(pin_path(
            r"C:\Program Files\7-Zip\7-zip.dll",
            &["other.dll".into()]
        ));
    }
}
