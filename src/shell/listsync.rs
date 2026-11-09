// Re-list panes after a delete, move, or a burst of shell change notifications.
//
// The hosted Explorer view updates itself from SHCNE_* as the operation runs.
// A burst is folded inside DefView on the first events, so the enumeration can
// be the folder as it was in the middle of the delete. Later events in that
// burst do not schedule another pass, and the rows stay. PerformOperations
// finishing does not tell that view anything.
//
// Two replies, both on the UI thread:
// - when our own operation returns (success, a partial failure, or cancel),
//   re-list every pane and parked tab that is showing a folder the operation
//   changed;
// - shell notifications are trailing-edge. The timer is armed from the last
//   event, so the final re-list sees the folder after the burst. A long burst
//   also re-lists at a cap, and the next event starts a new burst.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, AtomicU64};

use crate::app::*;
use crate::imports::*;
use crate::pane::*;
use crate::session::*;
use crate::ui::*;

/// Quiet time after the last notification before the pane re-lists.
pub(crate) const QUIET_MS: u64 = 400;
/// Longest a burst may run before a re-list. The next event starts another one.
pub(crate) const CAP_MS: u64 = 2_000;
/// `SHCNE_UPDATEDIR` caused by our own `IShellView::Refresh`. Item events are not ignored.
const ECHO_MS: u64 = QUIET_MS;

/// What one shell event says to re-list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellNote {
    /// The path is a child. Re-list its parent.
    Child,
    /// The path is the directory whose contents changed.
    Dir,
}

/// `true` when this event should arm the trailing re-list.
/// A directory event that is only the echo of our own refresh must not arm another one.
pub(crate) fn keep_note(kind: ShellNote, echo: bool) -> bool {
    !(echo && kind == ShellNote::Dir)
}

pub(crate) fn shell_note(event: u32) -> Option<ShellNote> {
    if event == SHCNE_UPDATEDIR.0 {
        Some(ShellNote::Dir)
    } else if event == SHCNE_DELETE.0
        || event == SHCNE_CREATE.0
        || event == SHCNE_MKDIR.0
        || event == SHCNE_RMDIR.0
        || event == SHCNE_RENAMEITEM.0
        || event == SHCNE_RENAMEFOLDER.0
    {
        Some(ShellNote::Child)
    } else {
        None
    }
}

/// Folders a shell event should re-list, normalized. A rename names both parents.
pub(crate) fn event_folders(kind: ShellNote, item: &str, item2: &str) -> Vec<String> {
    let mut out = Vec::new();
    push_event(&mut out, kind, item);
    if !item2.is_empty() {
        push_event(&mut out, kind, item2);
    }
    out
}

fn push_event(out: &mut Vec<String>, kind: ShellNote, path: &str) {
    let key = match kind {
        ShellNote::Dir => {
            let k = norm_key(path);
            (!k.is_empty()).then_some(k)
        }
        ShellNote::Child => parent_key(&norm_key(path)),
    };
    if let Some(k) = key {
        if !out.iter().any(|e| e == &k) {
            out.push(k);
        }
    }
}

/// Parent folder of each path, once, in first-seen order.
pub(crate) fn source_folders(paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        push_event(&mut out, ShellNote::Child, p);
    }
    out
}

/// Destination, plus each source parent when this is a move.
/// A copy does not re-list the source. The same folder is named once.
pub(crate) fn transfer_folders(srcs: &[String], dst: &str, mv: bool) -> Vec<String> {
    let mut out = if mv { source_folders(srcs) } else { Vec::new() };
    push_event(&mut out, ShellNote::Dir, dst);
    out
}

/// Indexes into `pane_paths` whose folder is one of `folders`.
pub(crate) fn panes_showing(pane_paths: &[String], folders: &[String]) -> Vec<usize> {
    let keys: Vec<String> = folders.iter().map(|f| norm_key(f)).filter(|k| !k.is_empty()).collect();
    pane_paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let k = norm_key(p);
            (!k.is_empty() && keys.iter().any(|f| f == &k)).then_some(i)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    key: String,
    started_ms: u64,
    last_ms: u64,
}

/// Trailing-edge coalesce. A note moves the deadline. `take_due` is empty until
/// `quiet_ms` has passed since the last note, or the burst has run for `cap_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Coalesce {
    slots: Vec<Slot>,
}

impl Coalesce {
    pub(crate) fn note(&mut self, now_ms: u64, key: &str) {
        if key.is_empty() {
            return;
        }
        if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
            s.last_ms = now_ms;
            return;
        }
        self.slots.push(Slot { key: key.to_string(), started_ms: now_ms, last_ms: now_ms });
    }

    pub(crate) fn take_due(&mut self, now_ms: u64, quiet_ms: u64, cap_ms: u64) -> Vec<String> {
        let mut due = Vec::new();
        self.slots.retain(|s| {
            let quiet = now_ms.saturating_sub(s.last_ms) >= quiet_ms;
            let cap = now_ms.saturating_sub(s.started_ms) >= cap_ms;
            if quiet || cap {
                due.push(s.key.clone());
                false
            } else {
                true
            }
        });
        due
    }

    /// Milliseconds until the next slot is due. `None` when nothing is waiting.
    /// Already-due slots report 1, so the caller leaves the notification handler first.
    pub(crate) fn wait_ms(&self, now_ms: u64, quiet_ms: u64, cap_ms: u64) -> Option<u64> {
        let mut best: Option<u64> = None;
        for s in &self.slots {
            let quiet_left = quiet_ms.saturating_sub(now_ms.saturating_sub(s.last_ms));
            let cap_left = cap_ms.saturating_sub(now_ms.saturating_sub(s.started_ms));
            let left = quiet_left.min(cap_left).max(1);
            best = Some(best.map(|b| b.min(left)).unwrap_or(left));
        }
        best
    }
}

/// Posts the re-list when dropped. Built on the worker so every return, including
/// cancel and a panic the worker catches, still reaches the UI thread.
pub(crate) struct RefreshLater {
    hwnds: Vec<isize>,
    folders: Vec<String>,
}

impl RefreshLater {
    pub(crate) fn new(hwnds: Vec<isize>, folders: Vec<String>) -> Self {
        Self { hwnds, folders }
    }
}

impl Drop for RefreshLater {
    fn drop(&mut self) {
        post_refresh(&self.hwnds, &self.folders);
    }
}

/// Main windows, captured on the UI thread. The worker must not call `app()`.
pub(crate) fn list_hwnds() -> Vec<isize> {
    app(|a| a.desks.iter().map(|d| d.main.0 as isize).filter(|&p| p != 0).collect())
}

struct Job {
    id: u64,
    folders: Vec<String>,
    left: usize,
}

struct Win {
    hwnd: isize,
    cookie: u32,
    slots: Coalesce,
}

struct Echo {
    key: String,
    until_ms: u64,
}

#[derive(Clone)]
struct View {
    eb: IExplorerBrowser,
    path: String,
    pane: isize,
    sited: bool,
}

/// A hidden tab's browser has no site, so `Refresh` waits until that tab is shown.
#[derive(Clone)]
struct Pending {
    pane: isize,
    key: String,
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
    &JOBS
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn wins() -> &'static Mutex<Vec<Win>> {
    static WINS: Mutex<Vec<Win>> = Mutex::new(Vec::new());
    &WINS
}

fn echoes() -> &'static Mutex<Vec<Echo>> {
    static ECHO: Mutex<Vec<Echo>> = Mutex::new(Vec::new());
    &ECHO
}

fn pending() -> &'static Mutex<Vec<Pending>> {
    static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
    &PENDING
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn mono_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Tell every main window to re-list `folders`. Safe on a worker thread.
pub(crate) fn post_refresh(hwnds: &[isize], folders: &[String]) {
    if folders.is_empty() {
        return;
    }
    let live: Vec<isize> = hwnds.iter().copied().filter(|&h| h != 0).collect();
    if live.is_empty() {
        return;
    }
    let id = next_id();
    {
        let mut q = guard(jobs());
        if q.len() >= 32 {
            q.remove(0);
        }
        q.push(Job { id, folders: folders.to_vec(), left: live.len() });
    }
    let mut failed = 0usize;
    for h in live {
        let ok = unsafe { PostMessageW(Some(HWND(h as _)), WM_LIST_NOW, WPARAM(id as usize), LPARAM(0)).is_ok() };
        if !ok {
            failed += 1;
        }
    }
    for _ in 0..failed {
        let _ = take_job(id);
    }
}

fn take_job(id: u64) -> Option<Vec<String>> {
    let mut q = guard(jobs());
    let i = q.iter().position(|j| j.id == id)?;
    q[i].left = q[i].left.saturating_sub(1);
    let folders = q[i].folders.clone();
    if q[i].left == 0 {
        q.remove(i);
    }
    Some(folders)
}

fn is_echo(key: &str, now: u64) -> bool {
    let mut g = guard(echoes());
    g.retain(|e| e.until_ms > now);
    g.iter().any(|e| e.key == key)
}

fn mark_echo(keys: &[String], now: u64) {
    let until = now.saturating_add(ECHO_MS);
    let mut g = guard(echoes());
    for k in keys {
        if k.is_empty() {
            continue;
        }
        if let Some(e) = g.iter_mut().find(|e| e.key == *k) {
            e.until_ms = until;
        } else {
            g.push(Echo { key: k.clone(), until_ms: until });
        }
    }
    g.retain(|e| e.until_ms > now);
}

/// Virtual Desktop, not the user's Desktop directory. Recursive from here is every folder.
fn root_pidl() -> *mut ITEMIDLIST {
    static ONCE: Once = Once::new();
    static PIDL: AtomicIsize = AtomicIsize::new(0);
    ONCE.call_once(|| {
        // CSIDL_DESKTOP. The known-folder Desktop is only `...\Desktop`, which would miss other drives.
        let p = unsafe { SHGetSpecialFolderLocation(None, 0).unwrap_or(std::ptr::null_mut()) };
        PIDL.store(p as isize, Ordering::Relaxed);
    });
    PIDL.load(Ordering::Relaxed) as *mut ITEMIDLIST
}

pub(crate) unsafe fn arm(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let entry = SHChangeNotifyEntry { pidl: root_pidl(), fRecursive: true.into() };
    let watch = SHCNE_RENAMEITEM | SHCNE_CREATE | SHCNE_DELETE | SHCNE_MKDIR | SHCNE_RMDIR | SHCNE_UPDATEDIR | SHCNE_RENAMEFOLDER;
    let sources = SHCNRF_ShellLevel | SHCNRF_InterruptLevel | SHCNRF_NewDelivery | SHCNRF_RecursiveInterrupt;
    let cookie = SHChangeNotifyRegister(hwnd, sources, watch.0 as i32, WM_SHELL_CHANGE, 1, std::ptr::addr_of!(entry));
    if cookie == 0 {
        return;
    }
    let mut wins = guard(wins());
    if let Some(old) = wins.iter().find(|w| w.hwnd == hwnd.0 as isize).map(|w| w.cookie) {
        if old != 0 {
            let _ = SHChangeNotifyDeregister(old);
        }
        wins.retain(|w| w.hwnd != hwnd.0 as isize);
    }
    wins.push(Win { hwnd: hwnd.0 as isize, cookie, slots: Coalesce::default() });
}

pub(crate) unsafe fn disarm(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let _ = KillTimer(Some(hwnd), TIMER_LIST);
    let cookie = {
        let mut wins = guard(wins());
        let cookie = wins.iter().find(|w| w.hwnd == hwnd.0 as isize).map(|w| w.cookie).unwrap_or(0);
        wins.retain(|w| w.hwnd != hwnd.0 as isize);
        cookie
    };
    if cookie != 0 {
        let _ = SHChangeNotifyDeregister(cookie);
    }
}

pub(crate) unsafe fn on_shell(hwnd: HWND, wp: WPARAM, lp: LPARAM) {
    let Some((event, item, item2)) = read_change(wp, lp) else { return };
    let Some(kind) = shell_note(event as u32) else { return };
    let folders = event_folders(kind, &item, &item2);
    if folders.is_empty() {
        return;
    }
    let shown = paths_here();
    let now = mono_ms();
    let add: Vec<String> = folders
        .into_iter()
        .filter(|f| keep_note(kind, is_echo(f, now)) && !panes_showing(&shown, std::slice::from_ref(f)).is_empty())
        .collect();
    if add.is_empty() {
        return;
    }
    let wait = {
        let mut wins = guard(wins());
        let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) else { return };
        for f in &add {
            win.slots.note(now, f);
        }
        win.slots.wait_ms(now, QUIET_MS, CAP_MS)
    };
    if let Some(ms) = wait {
        let _ = KillTimer(Some(hwnd), TIMER_LIST);
        let _ = SetTimer(Some(hwnd), TIMER_LIST, ms.min(u32::MAX as u64) as u32, None);
    }
}

pub(crate) unsafe fn on_timer(hwnd: HWND) {
    let _ = KillTimer(Some(hwnd), TIMER_LIST);
    let now = mono_ms();
    let (due, again) = {
        let mut wins = guard(wins());
        let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) else { return };
        let due = win.slots.take_due(now, QUIET_MS, CAP_MS);
        let again = win.slots.wait_ms(now, QUIET_MS, CAP_MS);
        (due, again)
    };
    if let Some(ms) = again {
        let _ = SetTimer(Some(hwnd), TIMER_LIST, ms.min(u32::MAX as u64) as u32, None);
    }
    refresh_folders(&due);
}

pub(crate) unsafe fn on_finished(wp: WPARAM) {
    let Some(folders) = take_job(wp.0 as u64) else { return };
    refresh_folders(&folders);
}

unsafe fn read_change(wp: WPARAM, lp: LPARAM) -> Option<(i32, String, String)> {
    let mut pidls: *mut *mut ITEMIDLIST = std::ptr::null_mut();
    let mut event = 0i32;
    let lock = SHChangeNotification_Lock(HANDLE(wp.0 as *mut _), lp.0 as u32, Some(&mut pidls), Some(&mut event));
    if lock.0.is_null() {
        return None;
    }
    let item = path_at(pidls, 0);
    let rename = event as u32 == SHCNE_RENAMEITEM.0 || event as u32 == SHCNE_RENAMEFOLDER.0;
    let item2 = if rename { path_at(pidls, 1) } else { String::new() };
    let _ = SHChangeNotification_Unlock(lock);
    Some((event, item, item2))
}

unsafe fn path_at(pidls: *mut *mut ITEMIDLIST, index: usize) -> String {
    if pidls.is_null() {
        return String::new();
    }
    let p = *pidls.add(index);
    if p.is_null() {
        return String::new();
    }
    name(p, SIGDN_FILESYSPATH)
}

fn paths_here() -> Vec<String> {
    app(|a| {
        let mut out = Vec::new();
        let n = a.s.panes();
        for i in 0..n {
            if a.panes.get(i).and_then(|p| p.as_ref()).is_none() {
                continue;
            }
            if let Some(p) = a.s.paths.get(i) {
                if !p.is_empty() {
                    out.push(p.clone());
                }
            }
        }
        let tab = a.s.tab;
        for (ti, set) in a.park.iter().enumerate() {
            if ti == tab {
                continue;
            }
            let Some(paths) = a.s.tabs.get(ti).map(|t| &t.paths) else { continue };
            for (i, pane) in set.iter().enumerate() {
                if pane.is_none() {
                    continue;
                }
                if let Some(p) = paths.get(i) {
                    if !p.is_empty() {
                        out.push(p.clone());
                    }
                }
            }
        }
        out
    })
}

fn views_for(folders: &[String]) -> Vec<View> {
    app(|a| {
        let mut out = Vec::new();
        let n = a.s.panes();
        for i in 0..n {
            let Some(pane) = a.panes.get(i).and_then(|p| p.as_ref()) else { continue };
            let path = a.s.paths.get(i).cloned().unwrap_or_default();
            if panes_showing(std::slice::from_ref(&path), folders).is_empty() {
                continue;
            }
            out.push(View { eb: pane.eb.clone(), path, pane: pane.hwnd.0 as isize, sited: pane.cookie != 0 });
        }
        let tab = a.s.tab;
        for (ti, set) in a.park.iter().enumerate() {
            if ti == tab {
                continue;
            }
            let paths = a.s.tabs.get(ti).map(|t| t.paths.clone()).unwrap_or_default();
            for (i, pane) in set.iter().enumerate() {
                let Some(pane) = pane.as_ref() else { continue };
                let path = paths.get(i).cloned().unwrap_or_default();
                if panes_showing(std::slice::from_ref(&path), folders).is_empty() {
                    continue;
                }
                out.push(View { eb: pane.eb.clone(), path, pane: pane.hwnd.0 as isize, sited: pane.cookie != 0 });
            }
        }
        out
    })
}

fn remember_hidden(views: &[View]) {
    let mut q = guard(pending());
    for v in views.iter().filter(|v| !v.sited) {
        let key = norm_key(&v.path);
        if key.is_empty() {
            continue;
        }
        let exists = q.iter().any(|p| p.pane == v.pane && p.key == key);
        if !exists {
            q.push(Pending { pane: v.pane, key });
        }
    }
    let extra = q.len().saturating_sub(64);
    if extra > 0 {
        q.drain(0..extra);
    }
}

/// `IShellView::Refresh` on a share can block the UI thread until the redirector gives up.
fn blocks(path: &str) -> bool {
    if path.is_empty() || !is_fs(path) {
        return true;
    }
    let drives = app(|a| a.drives.iter().map(|d| (d.0, d.1)).collect::<Vec<_>>());
    let live = crate::remote::path_on_live_mount(path);
    folder_is_share(path, cached_kind(&drives, path), live)
}

fn refresh_folders(folders: &[String]) {
    if folders.is_empty() {
        return;
    }
    let views: Vec<View> = views_for(folders).into_iter().filter(|v| !blocks(&v.path)).collect();
    remember_hidden(&views);
    let live: Vec<View> = views.into_iter().filter(|v| v.sited).collect();
    refresh_views(&live);
}

fn refresh_views(views: &[View]) {
    if views.is_empty() {
        return;
    }
    let keys: Vec<String> = views.iter().map(|v| norm_key(&v.path)).collect();
    mark_echo(&keys, mono_ms());
    for v in views {
        if let Ok(sv) = unsafe { v.eb.GetCurrentView::<IShellView>() } {
            let _ = unsafe { sv.Refresh() };
        }
    }
}

/// A tab that was hidden during the delete is sited again. Re-list it once, then forget it.
pub(crate) unsafe fn relist_shown() {
    let queued: Vec<Pending> = guard(pending()).clone();
    if queued.is_empty() {
        return;
    }
    let keys: Vec<String> = queued.iter().map(|p| p.key.clone()).collect();
    let views = views_for(&keys);
    let live: Vec<View> = views
        .iter()
        .filter(|v| v.sited && !blocks(&v.path) && queued.iter().any(|p| p.pane == v.pane && p.key == norm_key(&v.path)))
        .cloned()
        .collect();
    refresh_views(&live);
    let mine = pane_hwnds_here();
    let mut q = guard(pending());
    q.retain(|p| {
        // Another window's hidden tab is not in this pass.
        if !mine.contains(&p.pane) {
            return true;
        }
        let done = live.iter().any(|v| v.pane == p.pane && norm_key(&v.path) == p.key);
        let still = views.iter().any(|v| !v.sited && v.pane == p.pane && norm_key(&v.path) == p.key);
        !done && still
    });
}

fn pane_hwnds_here() -> Vec<isize> {
    app(|a| {
        let mut out = Vec::new();
        for pane in a.panes.iter().flatten() {
            out.push(pane.hwnd.0 as isize);
        }
        for set in &a.park {
            for pane in set.iter().flatten() {
                out.push(pane.hwnd.0 as isize);
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_waits_for_the_last_note() {
        let mut c = Coalesce::default();
        // Twenty deletes, 10ms apart. A timer armed only on the first note would be
        // due at t = 400, while files are still being removed.
        for n in 0..20 {
            c.note(n * 10, r"c:\work");
        }
        assert!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS).is_empty());
        let last = 19 * 10;
        assert!(c.take_due(last + QUIET_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(last + QUIET_MS, QUIET_MS, CAP_MS), vec![r"c:\work".to_string()]);
        // The event after that re-list starts another burst. The last one is not dropped.
        c.note(last + QUIET_MS + 5, r"c:\work");
        assert!(c.take_due(last + QUIET_MS + 5, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(last + QUIET_MS + 5 + QUIET_MS, QUIET_MS, CAP_MS), vec![r"c:\work".to_string()]);
    }

    #[test]
    fn a_long_burst_relists_at_the_cap_and_the_next_note_still_counts() {
        let mut c = Coalesce::default();
        c.note(0, r"c:\work");
        c.note(CAP_MS - 10, r"c:\work");
        assert!(c.take_due(CAP_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(CAP_MS, QUIET_MS, CAP_MS), vec![r"c:\work".to_string()]);
        c.note(CAP_MS, r"c:\work");
        assert!(c.take_due(CAP_MS + QUIET_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(CAP_MS + QUIET_MS, QUIET_MS, CAP_MS), vec![r"c:\work".to_string()]);
    }

    #[test]
    fn two_folders_wait_on_their_own_last_note() {
        let mut c = Coalesce::default();
        c.note(0, r"c:\a");
        c.note(100, r"c:\b");
        assert_eq!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS), vec![r"c:\a".to_string()]);
        assert!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(100 + QUIET_MS, QUIET_MS, CAP_MS), vec![r"c:\b".to_string()]);
    }

    #[test]
    fn source_and_transfer_folders_name_each_place_once() {
        let files = vec![r"C:\Work\a.txt".into(), r"C:\Work\b.txt".into(), r"C:\Other\c.txt".into()];
        assert_eq!(source_folders(&files), vec![r"c:\work".to_string(), r"c:\other".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\B", false), vec![r"c:\b".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\B", true), vec![r"c:\a".to_string(), r"c:\b".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\A\", true), vec![r"c:\a".to_string()]);
    }

    #[test]
    fn both_panes_on_the_same_folder_are_listed() {
        let paths = vec![r"C:\Work\".into(), r"D:\".into(), r"c:\work".into()];
        assert_eq!(panes_showing(&paths, &[r"c:\work".into()]), vec![0, 2]);
        assert!(panes_showing(&paths, &[r"c:\other".into()]).is_empty());
    }

    #[test]
    fn shell_events_name_the_folder_to_relist() {
        assert_eq!(shell_note(SHCNE_DELETE.0), Some(ShellNote::Child));
        assert_eq!(shell_note(SHCNE_RMDIR.0), Some(ShellNote::Child));
        assert_eq!(shell_note(SHCNE_UPDATEDIR.0), Some(ShellNote::Dir));
        assert_eq!(shell_note(SHCNE_ATTRIBUTES.0), None);
        assert_eq!(shell_note(SHCNE_UPDATEITEM.0), None);
        assert_eq!(event_folders(ShellNote::Child, r"C:\Work\a.txt", ""), vec![r"c:\work".to_string()]);
        assert_eq!(event_folders(ShellNote::Dir, r"C:\Work\", ""), vec![r"c:\work".to_string()]);
        assert_eq!(
            event_folders(ShellNote::Child, r"C:\A\old.txt", r"D:\B\new.txt"),
            vec![r"c:\a".to_string(), r"d:\b".to_string()]
        );
        assert!(keep_note(ShellNote::Child, true));
        assert!(!keep_note(ShellNote::Dir, true));
        assert!(keep_note(ShellNote::Dir, false));
    }
}
