//! Decisions shared by Quick Look and the docked preview for a shell preview handler.
//!
//! The host thread owns `IPreviewHandler::Unload` and the host HWND. The UI thread
//! asks for that unload and waits, pumping messages, so the handler's child windows
//! are not destroyed out from under an out-of-process server such as `EXCEL.EXE -Embedding`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::imports::*;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Shell::IPreviewHandler;

/// Posted to the host thread. It unloads the handler, then destroys the host window.
pub(crate) const WM_HOST_UNLOAD: u32 = WM_APP + 90;
/// How long the UI waits for `Unload` before it detaches the host window and moves on.
pub(crate) const HOST_UNLOAD_WAIT_MS: u32 = 400;
/// A failed handler is not tried again for this path and mtime.
pub(crate) const HANDLER_FAIL_TTL_MS: u64 = 10 * 60 * 1000;

const RPC_S_SERVER_UNAVAILABLE: i32 = 0x8007_06BA_u32 as i32;
const RPC_S_CALL_FAILED: i32 = 0x8007_06BE_u32 as i32;
const RPC_S_CALL_FAILED_DNE: i32 = 0x8007_06BF_u32 as i32;
const CO_E_SERVER_EXEC_FAILURE: i32 = 0x8008_0005_u32 as i32;

/// Shared flags for one host thread. The UI and the thread each hold an `Arc`.
pub(crate) struct HostCtl {
    pub unload: AtomicBool,
    pub done: AtomicBool,
    pub handler_pid: AtomicU32,
    /// True from startup until the UI has accepted a successful `DoPreview`.
    pub loading: AtomicBool,
}

impl HostCtl {
    pub(crate) fn fresh() -> Arc<Self> {
        Arc::new(Self {
            unload: AtomicBool::new(false),
            done: AtomicBool::new(false),
            handler_pid: AtomicU32::new(0),
            loading: AtomicBool::new(true),
        })
    }
}

/// Sets `done` when the host thread leaves, including on panic, so the UI wait ends.
pub(crate) struct HostDone(pub Arc<HostCtl>);

impl Drop for HostDone {
    fn drop(&mut self) {
        self.0.loading.store(false, Ordering::Release);
        self.0.done.store(true, Ordering::Release);
    }
}

/// `Unload` then destroy, dropped before COM is uninitialized.
pub(crate) struct HandlerSession {
    pub ph: Option<IPreviewHandler>,
    pub host: HWND,
}

impl Drop for HandlerSession {
    fn drop(&mut self) {
        unsafe {
            if let Some(ph) = self.ph.take() {
                _ = ph.Unload();
            }
            if !self.host.0.is_null() && IsWindow(Some(self.host)).as_bool() {
                _ = DestroyWindow(self.host);
            }
        }
    }
}

/// Balances `CoInitializeEx` after the handler has been unloaded.
pub(crate) struct ComUninit(pub bool);

impl Drop for ComUninit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandlerSignal {
    /// Call succeeded. Do not try another init interface.
    Ok,
    /// Initialize or `DoPreview` failed, and the server is still there.
    TryNextInit,
    /// The out-of-process server is gone. Fall back to Maguro's own preview.
    Fallback,
}

/// `hr` is an `HRESULT`. Success is non-negative. Server-death codes are the RPC and
/// COM values a preview handler returns after its local server crashes or disconnects.
pub(crate) fn handler_signal(hr: i32) -> HandlerSignal {
    if hr >= 0 {
        return HandlerSignal::Ok;
    }
    if handler_server_dead(hr) {
        HandlerSignal::Fallback
    } else {
        HandlerSignal::TryNextInit
    }
}

pub(crate) fn handler_server_dead(hr: i32) -> bool {
    matches!(
        hr,
        x if x == RPC_E_DISCONNECTED.0
            || x == RPC_E_SERVER_DIED.0
            || x == RPC_E_SERVER_DIED_DNE.0
            || x == RPC_E_CONNECTION_TERMINATED.0
            || x == RPC_E_SERVERFAULT.0
            || x == CO_E_OBJNOTCONNECTED.0
            || x == RPC_S_SERVER_UNAVAILABLE
            || x == RPC_S_CALL_FAILED
            || x == RPC_S_CALL_FAILED_DNE
            || x == CO_E_SERVER_EXEC_FAILURE
    )
}

/// Explorer's prevhost order. A failed attempt uses the next one on a new server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InitKind {
    Stream,
    Item,
    File,
}

pub(crate) fn init_kinds() -> [InitKind; 3] {
    [InitKind::Stream, InitKind::Item, InitKind::File]
}

/// Spreadsheet types Maguro parses. These never go to the Excel preview handler,
/// even when a route asks for a registered handler first.
pub(crate) fn maguro_spreadsheet(ext: &str) -> bool {
    matches!(ext_key(ext).as_str(), "xlsx" | "xlsm" | "xltx" | "ods" | "csv" | "tsv")
}

/// Legacy Excel binaries. A registered handler may show them; Maguro does not parse them.
pub(crate) fn legacy_excel(ext: &str) -> bool {
    matches!(ext_key(ext).as_str(), "xls" | "xlsb")
}

fn ext_key(ext: &str) -> String {
    ext.to_ascii_lowercase()
}

/// `maguro_draws` is `maguro_owns`. `handlers_first` tries a handler before Maguro
/// for types that are not parsed spreadsheets.
pub(crate) fn allow_shell_handler(ext: &str, maguro_draws: bool, handlers_first: bool) -> bool {
    if maguro_spreadsheet(ext) {
        return false;
    }
    // Legacy .xls / .xlsb stay on the handler even if a route also marks them as drawn here.
    if legacy_excel(ext) {
        return true;
    }
    if handlers_first {
        return true;
    }
    !maguro_draws
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FailHit {
    path: String,
    mtime: u128,
    until_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FailMemo {
    hits: Vec<FailHit>,
}

fn norm_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

impl FailMemo {
    pub(crate) fn note(&mut self, path: &str, mtime: u128, now_ms: u64) {
        self.prune(now_ms);
        let path = norm_path(path);
        let until_ms = now_ms.saturating_add(HANDLER_FAIL_TTL_MS);
        if let Some(hit) = self.hits.iter_mut().find(|h| h.path == path && h.mtime == mtime) {
            hit.until_ms = until_ms;
            return;
        }
        self.hits.push(FailHit { path, mtime, until_ms });
    }

    pub(crate) fn blocks(&mut self, path: &str, mtime: u128, now_ms: u64) -> bool {
        self.prune(now_ms);
        let path = norm_path(path);
        self.hits.iter().any(|h| h.path == path && h.mtime == mtime && now_ms < h.until_ms)
    }

    fn prune(&mut self, now_ms: u64) {
        self.hits.retain(|h| now_ms < h.until_ms);
    }
}

pub(crate) fn mtime_key(modified: Option<SystemTime>) -> u128 {
    modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn fail_memo() -> std::sync::MutexGuard<'static, FailMemo> {
    FAIL_MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

static FAIL_MEMO: std::sync::Mutex<FailMemo> = std::sync::Mutex::new(FailMemo { hits: Vec::new() });

/// Remember a handler failure so the next request for this file skips the handler.
pub(crate) fn note_handler_failure(path: &str) {
    if path.is_empty() {
        return;
    }
    let mtime = mtime_key(std::fs::metadata(path).ok().and_then(|m| m.modified().ok()));
    fail_memo().note(path, mtime, now_ms());
}

pub(crate) fn handler_failure_blocks(path: &str, mtime: u128) -> bool {
    fail_memo().blocks(path, mtime, now_ms())
}

/// A handler child that disappears after it was shown. One empty check can be a
/// recreate; the second means the server's window is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChildWatch {
    seen: bool,
    misses: u8,
}

impl ChildWatch {
    pub(crate) fn new() -> Self {
        Self { seen: false, misses: 0 }
    }

    pub(crate) fn lost_after(mut self, has_child: bool) -> (Self, bool) {
        if has_child {
            self.seen = true;
            self.misses = 0;
            return (self, false);
        }
        if !self.seen {
            return (self, false);
        }
        self.misses = self.misses.saturating_add(1);
        let lost = self.misses >= 2;
        (self, lost)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostRun {
    /// Generation changed, or the UI asked for unload.
    Stop,
    /// RPC death, or the handler child vanished after it was shown.
    Failed,
}

pub(crate) unsafe fn drive_host(
    ph: &IPreviewHandler,
    host: HWND,
    size_msg: u32,
    mut keep: impl FnMut() -> bool,
    mut on_size: impl FnMut(&IPreviewHandler) -> Result<()>,
) -> HostRun {
    let mut child = ChildWatch::new();
    let mut msg = MSG::default();
    loop {
        if !keep() {
            return HostRun::Stop;
        }
        if !IsWindow(Some(host)).as_bool() {
            return if keep() { HostRun::Failed } else { HostRun::Stop };
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_HOST_UNLOAD || msg.message == WM_QUIT {
                return HostRun::Stop;
            }
            if msg.message == size_msg {
                if size_dead(&mut on_size, ph) {
                    return HostRun::Failed;
                }
            } else {
                _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if size_dead(&mut on_size, ph) {
            return HostRun::Failed;
        }
        let has = host_has_child(host);
        let lost;
        (child, lost) = child.lost_after(has);
        if lost {
            return HostRun::Failed;
        }
        _ = MsgWaitForMultipleObjects(None, false, 100, QS_ALLINPUT);
    }
}

fn size_dead(on_size: &mut impl FnMut(&IPreviewHandler) -> Result<()>, ph: &IPreviewHandler) -> bool {
    match on_size(ph) {
        Ok(()) => false,
        Err(e) => handler_server_dead(e.code().0),
    }
}

pub(crate) unsafe fn host_has_child(host: HWND) -> bool {
    if host.0.is_null() || !IsWindow(Some(host)).as_bool() {
        return false;
    }
    match GetWindow(host, GW_CHILD) {
        Ok(child) => !child.0.is_null() && IsWindow(Some(child)).as_bool(),
        Err(_) => false,
    }
}

/// Hide, ask the host thread to unload, and wait. On timeout the window is detached
/// so destroying the parent does not destroy the handler's children. The UI never
/// destroys a live host window itself while the thread may still be inside `Unload`.
pub(crate) unsafe fn release_host_window(ctl: Option<Arc<HostCtl>>, host: HWND, tid: u32) {
    let alive = !host.0.is_null() && IsWindow(Some(host)).as_bool();
    if alive {
        _ = ShowWindow(host, SW_HIDE);
    }
    let Some(ctl) = ctl else {
        if alive {
            _ = DestroyWindow(host);
        }
        return;
    };
    // Idle -> Showing -> UnloadAsked. The host HWND stays until the thread finishes or we abandon.
    let showing = host_step(HostPhase::Idle, HostEvent::Shown);
    let asked = host_step(showing, HostEvent::AskUnload);
    ctl.unload.store(true, Ordering::Release);
    ctl.loading.store(false, Ordering::Release);
    if tid != 0 {
        _ = PostThreadMessageW(tid, WM_HOST_UNLOAD, WPARAM(0), LPARAM(0));
    }
    let finished = wait_until(&ctl.done, HOST_UNLOAD_WAIT_MS);
    let phase = if finished {
        host_step(asked, HostEvent::UnloadDone)
    } else {
        host_step(asked, HostEvent::TimedOut)
    };
    // `phase` is Unloaded or Abandoned. The UI still does not destroy a window the
    // thread is responsible for, except an empty host the thread never took.
    let _ = parent_may_close(phase) && !ui_destroys_host(phase);
    if finished {
        // The thread destroys the window after Unload. If it never ran, the window is still here.
        if alive && IsWindow(Some(host)).as_bool() {
            _ = DestroyWindow(host);
        }
        return;
    }
    if alive && IsWindow(Some(host)).as_bool() {
        detach_abandoned(host);
    }
}

pub(crate) unsafe fn wait_until(done: &AtomicBool, timeout_ms: u32) -> bool {
    let start = std::time::Instant::now();
    while !done.load(Ordering::Acquire) {
        let elapsed = start.elapsed().as_millis() as u32;
        if elapsed >= timeout_ms {
            return false;
        }
        let slice = timeout_ms.saturating_sub(elapsed).min(50);
        let mut msg = MSG::default();
        _ = MsgWaitForMultipleObjects(None, false, slice, QS_ALLINPUT);
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

pub(crate) unsafe fn detach_abandoned(host: HWND) {
    if host.0.is_null() || !IsWindow(Some(host)).as_bool() {
        return;
    }
    _ = ShowWindow(host, SW_HIDE);
    _ = SetParent(host, Some(HWND_MESSAGE));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostPhase {
    Idle,
    Showing,
    UnloadAsked,
    Unloaded,
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostEvent {
    Shown,
    AskUnload,
    UnloadDone,
    TimedOut,
}

/// The parent window may close once unload finished or the wait was abandoned
/// and the host window was detached. Destroying it from `Showing` is not a step.
pub(crate) fn host_step(phase: HostPhase, ev: HostEvent) -> HostPhase {
    use HostEvent::*;
    use HostPhase::*;
    match (phase, ev) {
        (Idle, Shown) => Showing,
        (Showing, AskUnload) => UnloadAsked,
        (UnloadAsked, AskUnload) => UnloadAsked,
        (UnloadAsked, UnloadDone) => Unloaded,
        (UnloadAsked, TimedOut) => Abandoned,
        (Abandoned, UnloadDone) => Unloaded,
        (Abandoned, AskUnload) => Abandoned,
        (phase, _) => phase,
    }
}

pub(crate) fn parent_may_close(phase: HostPhase) -> bool {
    matches!(phase, HostPhase::Idle | HostPhase::Unloaded | HostPhase::Abandoned)
}

/// The UI thread does not destroy the host window. The host thread does, after `Unload`.
pub(crate) fn ui_destroys_host(_phase: HostPhase) -> bool {
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivatedWin {
    Peek,
    Descendant,
    /// A top-level (or other) window whose process is the handler server.
    HandlerProcess,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CloseLook {
    pub win: ActivatedWin,
    pub same_process: bool,
}

pub(crate) fn classify_activate(is_peek: bool, is_descendant: bool, handler_process: bool, top_level: bool) -> ActivatedWin {
    if is_peek {
        ActivatedWin::Peek
    } else if is_descendant {
        ActivatedWin::Descendant
    } else if handler_process && top_level {
        ActivatedWin::HandlerProcess
    } else {
        ActivatedWin::Other
    }
}

/// Close only when the user left. A handler window during load is not "left".
/// A foreign window whose process we have not identified yet, while the handler
/// is still loading, is treated the same: the server is the process that just
/// took activation. The main window is same-process and still closes.
pub(crate) fn peek_should_close(hold: bool, win: ActivatedWin, loading: bool, same_process: bool) -> bool {
    if hold {
        return false;
    }
    match win {
        ActivatedWin::Peek | ActivatedWin::Descendant => false,
        ActivatedWin::HandlerProcess if loading => false,
        ActivatedWin::Other if loading && !same_process => false,
        _ => true,
    }
}

/// `foreground` is `GetForegroundWindow` after the deactivate message was deferred.
/// It wins over the `WM_ACTIVATE` lParam when both are known.
pub(crate) fn close_after_deferral(hold: bool, lparam: Option<CloseLook>, foreground: Option<CloseLook>, loading: bool) -> bool {
    if hold {
        return false;
    }
    let Some(look) = foreground.or(lparam) else {
        return false;
    };
    peek_should_close(false, look.win, loading, look.same_process)
}

pub(crate) fn should_reclaim(foreground_pid: u32, handler_pid: u32, our_pid: u32) -> bool {
    handler_pid != 0 && foreground_pid == handler_pid && foreground_pid != our_pid
}

pub(crate) unsafe fn child_process(host: HWND) -> u32 {
    if host.0.is_null() || !IsWindow(Some(host)).as_bool() {
        return 0;
    }
    let Ok(child) = GetWindow(host, GW_CHILD) else {
        return 0;
    };
    if child.0.is_null() {
        return 0;
    }
    let mut pid = 0u32;
    _ = GetWindowThreadProcessId(child, Some(&mut pid));
    pid
}

pub(crate) unsafe fn foreground_process() -> u32 {
    let fg = GetForegroundWindow();
    if fg.0.is_null() {
        return 0;
    }
    let mut pid = 0u32;
    _ = GetWindowThreadProcessId(fg, Some(&mut pid));
    pid
}

pub(crate) unsafe fn remember_handler_pid(host: HWND, ctl: &HostCtl) -> u32 {
    let ours = GetCurrentProcessId();
    let mut pid = child_process(host);
    if pid == 0 || pid == ours {
        let fg = foreground_process();
        if fg != 0 && fg != ours {
            pid = fg;
        }
    }
    if pid != 0 && pid != ours {
        ctl.handler_pid.store(pid, Ordering::Release);
        pid
    } else {
        ctl.handler_pid.load(Ordering::Acquire)
    }
}

pub(crate) unsafe fn reclaim_foreground(hwnd: HWND) {
    if hwnd.0.is_null() || !IsWindow(Some(hwnd)).as_bool() {
        return;
    }
    let fg = GetForegroundWindow();
    let fore = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
    let ours = GetCurrentThreadId();
    let attached = fore != 0 && fore != ours && AttachThreadInput(fore, ours, true).as_bool();
    _ = SetForegroundWindow(hwnd);
    _ = SetActiveWindow(hwnd);
    _ = SetFocus(Some(hwnd));
    if attached {
        _ = AttachThreadInput(fore, ours, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_death_falls_back_and_other_errors_try_the_next_init() {
        assert_eq!(handler_signal(0), HandlerSignal::Ok);
        assert_eq!(handler_signal(1), HandlerSignal::Ok); // S_FALSE
        for hr in [
            RPC_E_DISCONNECTED.0,
            RPC_E_SERVER_DIED.0,
            RPC_E_SERVER_DIED_DNE.0,
            RPC_E_CONNECTION_TERMINATED.0,
            RPC_E_SERVERFAULT.0,
            CO_E_OBJNOTCONNECTED.0,
            RPC_S_SERVER_UNAVAILABLE,
            RPC_S_CALL_FAILED,
            RPC_S_CALL_FAILED_DNE,
            CO_E_SERVER_EXEC_FAILURE,
        ] {
            assert_eq!(handler_signal(hr), HandlerSignal::Fallback, "{hr:#010x}");
            assert!(handler_server_dead(hr));
        }
        for hr in [E_FAIL.0, E_NOINTERFACE.0, E_ACCESSDENIED.0, E_UNEXPECTED.0] {
            assert_eq!(handler_signal(hr), HandlerSignal::TryNextInit, "{hr:#010x}");
            assert!(!handler_server_dead(hr));
        }
    }

    #[test]
    fn init_order_matches_explorer() {
        assert_eq!(init_kinds(), [InitKind::Stream, InitKind::Item, InitKind::File]);
    }

    #[test]
    fn parsed_spreadsheets_never_use_a_handler_even_when_handlers_are_first() {
        for ext in ["xlsx", "XLSM", "xltx", "ods", "csv", "tsv"] {
            assert!(maguro_spreadsheet(ext), "{ext}");
            assert!(!legacy_excel(ext), "{ext}");
            assert!(!allow_shell_handler(ext, true, false), "{ext}");
            assert!(!allow_shell_handler(ext, true, true), "{ext}");
            assert!(!allow_shell_handler(ext, false, true), "{ext}");
        }
        for ext in ["xls", "xlsb"] {
            assert!(legacy_excel(ext), "{ext}");
            assert!(!maguro_spreadsheet(ext), "{ext}");
            assert!(allow_shell_handler(ext, false, false), "{ext}");
            assert!(allow_shell_handler(ext, false, true), "{ext}");
        }
        assert!(!allow_shell_handler("docx", true, false));
        assert!(allow_shell_handler("docx", true, true));
        assert!(allow_shell_handler("html", false, false));
        assert!(!allow_shell_handler("png", true, false));
    }

    #[test]
    fn failure_memo_blocks_the_same_path_and_mtime_until_it_expires() {
        let mut memo = FailMemo::default();
        let path = r"C:\Work\Budget.XLS";
        memo.note(path, 10, 1_000);
        assert!(memo.blocks(r"c:\work\budget.xls", 10, 1_000));
        assert!(memo.blocks(r"C:/Work/Budget.xls", 10, 1_000 + HANDLER_FAIL_TTL_MS - 1));
        assert!(!memo.blocks(path, 11, 1_500), "a new mtime may try the handler again");
        assert!(!memo.blocks(r"C:\Work\other.xls", 10, 1_500));
        assert!(!memo.blocks(path, 10, 1_000 + HANDLER_FAIL_TTL_MS));
        memo.note(path, 10, 5_000);
        assert!(memo.blocks(path, 10, 5_000));
    }

    #[test]
    fn child_loss_needs_a_child_that_was_shown_and_then_stays_gone() {
        let w = ChildWatch::new();
        let (w, lost) = w.lost_after(false);
        assert!(!lost);
        let (w, lost) = w.lost_after(true);
        assert!(!lost && w.seen);
        let (w, lost) = w.lost_after(false);
        assert!(!lost, "one miss can be a recreate");
        let (w, lost) = w.lost_after(true);
        assert!(!lost && w.misses == 0);
        let (w, lost) = w.lost_after(false);
        assert!(!lost);
        let (_, lost) = w.lost_after(false);
        assert!(lost);
    }

    #[test]
    fn teardown_unloads_before_the_parent_may_close() {
        let mut phase = HostPhase::Idle;
        phase = host_step(phase, HostEvent::Shown);
        assert_eq!(phase, HostPhase::Showing);
        assert!(!parent_may_close(phase));
        assert!(!ui_destroys_host(phase));
        // The old bug destroyed the HWND here. That is not a transition.
        assert_eq!(host_step(phase, HostEvent::UnloadDone), HostPhase::Showing);
        phase = host_step(phase, HostEvent::AskUnload);
        assert_eq!(phase, HostPhase::UnloadAsked);
        assert!(!parent_may_close(phase));
        phase = host_step(phase, HostEvent::UnloadDone);
        assert_eq!(phase, HostPhase::Unloaded);
        assert!(parent_may_close(phase));
    }

    #[test]
    fn teardown_abandons_a_stuck_handler_without_blocking() {
        let phase = host_step(host_step(HostPhase::Showing, HostEvent::AskUnload), HostEvent::TimedOut);
        assert_eq!(phase, HostPhase::Abandoned);
        assert!(parent_may_close(phase));
        assert!(!ui_destroys_host(phase));
        assert_eq!(host_step(phase, HostEvent::UnloadDone), HostPhase::Unloaded);
    }

    fn look(win: ActivatedWin, same_process: bool) -> CloseLook {
        CloseLook { win, same_process }
    }

    #[test]
    fn peek_stays_open_when_the_handler_takes_activation_during_load() {
        assert!(!peek_should_close(false, ActivatedWin::HandlerProcess, true, false));
        assert!(!peek_should_close(false, ActivatedWin::Peek, true, true));
        assert!(!peek_should_close(false, ActivatedWin::Descendant, false, true));
        assert!(!peek_should_close(true, ActivatedWin::Other, false, true));
        // Not identified yet, but not our process, and the handler is still starting.
        assert!(!peek_should_close(false, ActivatedWin::Other, true, false));
        assert_eq!(
            classify_activate(false, false, true, true),
            ActivatedWin::HandlerProcess
        );
        assert_eq!(classify_activate(true, true, false, true), ActivatedWin::Peek);
        assert_eq!(classify_activate(false, true, true, false), ActivatedWin::Descendant);
    }

    #[test]
    fn peek_closes_when_the_user_leaves_for_the_main_window_or_another_app() {
        assert!(peek_should_close(false, ActivatedWin::Other, true, true), "clicking the main window closes even during load");
        assert!(peek_should_close(false, ActivatedWin::Other, false, true));
        assert!(peek_should_close(false, ActivatedWin::Other, false, false));
        assert!(peek_should_close(false, ActivatedWin::HandlerProcess, false, false), "after load, activating the server is leaving");
    }

    #[test]
    fn deferred_foreground_decides_the_close() {
        let excel = Some(look(ActivatedWin::HandlerProcess, false));
        let peek = Some(look(ActivatedWin::Peek, true));
        let main = Some(look(ActivatedWin::Other, true));
        let other = Some(look(ActivatedWin::Other, false));
        assert!(!close_after_deferral(false, main, peek, true));
        assert!(close_after_deferral(false, excel, other, false));
        assert!(!close_after_deferral(false, other, excel, true));
        assert!(close_after_deferral(false, main, None, true));
        assert!(!close_after_deferral(true, main, other, false));
        assert!(!close_after_deferral(false, None, None, true));
    }

    #[test]
    fn reclaim_only_when_the_handler_process_is_in_front() {
        assert!(should_reclaim(9, 9, 1));
        assert!(!should_reclaim(1, 9, 1));
        assert!(!should_reclaim(4, 9, 1));
        assert!(!should_reclaim(9, 0, 1));
        assert!(!should_reclaim(0, 9, 1));
    }
}
