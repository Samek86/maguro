// Compare a pane with its folder after a delete, move, copy, or a burst of
// shell notifications. DefView usually inserts and removes rows itself, the
// way Explorer does, and a full IShellView::Refresh would jump back to the
// top and drop the selection.
//
// The check runs on the UI thread, and only for a local folder:
// - when our own operation returns (success, a partial failure, or cancel);
// - when a coalesced notification burst goes quiet (trailing edge, with a cap).
// A list that still contains a deleted name is stale. A copy, move destination,
// create, or rename is also stale when a name that should be visible is missing.
// The first stale result waits one grace period and is checked again, so a
// DefView that is still catching up is not refreshed. Only a list that is still
// stale is refreshed, and the top row, focus, and surviving selection are put back.
//
// A UNC or mapped share is not enumerated and not refreshed here.
//
// A shell label edit (F2, new folder, new file) pauses the check. The drift
// waits in `EditHold`, and one compare runs after the edit closes. The Refresh
// that follows a create is an echo for at least `CAP_MS`, so a late
// `SHCNE_UPDATEDIR` does not start a second burst.

use std::collections::HashSet;
use std::io::Write;
use std::os::windows::fs::MetadataExt;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;

use crate::app::*;
use crate::imports::*;
use crate::pane::*;
use crate::session::*;
use crate::ui::*;

/// Quiet time after the last notification before the pane is compared.
pub(crate) const QUIET_MS: u64 = 400;
/// Longest a burst may run before a compare. The next event starts another one.
pub(crate) const CAP_MS: u64 = 2_000;
/// Second look, after a stale compare, before Refresh.
pub(crate) const GRACE_MS: u64 = 300;
/// `SHCNE_UPDATEDIR` caused by our own `IShellView::Refresh`. Item events are not ignored.
const ECHO_MS: u64 = QUIET_MS;
/// How often to ask whether Refresh has finished filling the list.
const RESTORE_MS: u64 = 80;
/// Stop waiting and restore on this poll even if the count is still moving.
pub(crate) const RESTORE_POLLS: u32 = 8;
/// Polls of an unchanged pre-refresh count before it counts as settled.
const SAME_COUNT_POLLS: u32 = 6;
/// Names read on the UI thread. Past this the compare is skipped.
const NAME_CAP: usize = 8_192;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_DIR: u32 = 0x10;

/// `maguro-list.log` in the settings root: the exe directory when `portable.txt`
/// is beside the exe, otherwise `%APPDATA%\Maguro`.
pub(crate) const LIST_LOG: &str = "maguro-list.log";
/// The active log stays at this size. A larger file is renamed to `maguro-list.log.1`.
const LIST_LOG_MAX: u64 = 512 * 1024;

/// What a folder change can require of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drift {
    /// Rows that should have disappeared (delete, move source, a bare directory update).
    Gone,
    /// Also rows that should have appeared (copy or move destination, create, rename).
    Arrived,
}

impl Drift {
    pub(crate) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Drift::Arrived, _) | (_, Drift::Arrived) => Drift::Arrived,
            _ => Drift::Gone,
        }
    }
}

/// `true` when this event should arm the trailing compare.
/// A directory event that is only the echo of our own refresh must not arm another one.
pub(crate) fn keep_note(kind: ShellNote, echo: bool) -> bool {
    !(echo && kind == ShellNote::Dir)
}

/// How long `SHCNE_UPDATEDIR` counts as the echo of our own Refresh.
/// Listsync's Refresh keeps [`ECHO_MS`]. Creating a folder or file refreshes before the
/// rename box opens, and the shell can deliver that directory update about two seconds
/// later, so that path stays an echo for at least [`CAP_MS`].
pub(crate) fn echo_window_ms(own_create: bool) -> u64 {
    if own_create { CAP_MS } else { ECHO_MS }
}

/// `true` while a marked echo has not expired. Same cutoff as the echo list (`until > now`).
pub(crate) fn echo_live(until_ms: u64, now: u64) -> bool {
    until_ms > now
}

/// What one shell event says to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellNote {
    /// The path is a child. Compare its parent.
    Child,
    /// The path is the directory whose contents changed.
    Dir,
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

/// Create, mkdir, and rename can add a row. Delete and a bare directory update only remove.
pub(crate) fn drift_of(event: u32) -> Drift {
    if event == SHCNE_CREATE.0 || event == SHCNE_MKDIR.0 || event == SHCNE_RENAMEITEM.0 || event == SHCNE_RENAMEFOLDER.0 {
        Drift::Arrived
    } else {
        Drift::Gone
    }
}

/// File name used for the compare. Callers fold before `list_diff`.
pub(crate) fn name_key(name: &str) -> String {
    name.to_lowercase()
}

/// One log line. `view` and `disk` are -1 when that side was not read.
pub(crate) fn check_line(folder: &str, stale: bool, view: i64, disk: i64, refreshed: bool) -> String {
    format!("listsync check {folder} stale={stale} view={view} disk={disk} refreshed={refreshed}")
}

pub(crate) fn skip_line(folder: &str) -> String {
    format!("listsync skip {folder} share")
}

/// Explorer's rule for one directory entry. Hidden and system together stay hidden.
/// Other hidden names need `show_hidden`.
pub(crate) fn listed_name(name: &str, attrs: u32, show_hidden: bool) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    let hidden = attrs & ATTR_HIDDEN != 0;
    let system = attrs & ATTR_SYSTEM != 0;
    if hidden && system {
        return false;
    }
    !hidden || show_hidden
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListDiff {
    pub view: usize,
    pub disk: usize,
    pub stale: bool,
}

/// `view` is the shell list. `present` is every name on disk. `expected` is the
/// subset the view should show (hidden files and a name filter already applied).
/// Names are already folded. A cut-short read must not call this.
pub(crate) fn list_diff(view: &[String], present: &[String], expected: &[String], drift: Drift) -> ListDiff {
    let view_set: HashSet<&str> = view.iter().map(String::as_str).collect();
    let present_set: HashSet<&str> = present.iter().map(String::as_str).collect();
    let expected_set: HashSet<&str> = expected.iter().map(String::as_str).collect();
    let ghosts = view_set.iter().any(|n| !present_set.contains(n));
    let missing = drift == Drift::Arrived && expected_set.iter().any(|n| !view_set.contains(n));
    ListDiff { view: view_set.len(), disk: expected_set.len(), stale: ghosts || missing }
}

/// The anchor row if it is still on disk, otherwise the nearest row that is.
/// The same distance prefers the following row, which is where Explorer moves focus.
pub(crate) fn nearest_survivor(rows: &[String], alive: impl Fn(&str) -> bool, anchor: Option<usize>) -> Option<String> {
    let anchor = anchor?;
    if anchor >= rows.len() {
        return None;
    }
    if alive(&rows[anchor]) {
        return Some(rows[anchor].clone());
    }
    let n = rows.len();
    for dist in 1..n {
        let after = anchor + dist;
        if after < n && alive(&rows[after]) {
            return Some(rows[after].clone());
        }
        if dist <= anchor {
            let before = anchor - dist;
            if alive(&rows[before]) {
                return Some(rows[before].clone());
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeepPlan {
    pub top: Option<String>,
    pub focus: Option<String>,
    pub selected: Vec<String>,
}

/// What to put back after Refresh. `rows` is the view order, empty slots included.
/// `present` is the folded names that still exist. `selected` is the folded selection.
pub(crate) fn plan_keep(rows: &[String], present: &[String], top: Option<usize>, focus: Option<usize>, selected: &[String]) -> KeepPlan {
    let have: HashSet<&str> = present.iter().map(String::as_str).collect();
    let alive = |n: &str| !n.is_empty() && have.contains(n);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut kept = Vec::new();
    for n in selected {
        if alive(n) && seen.insert(n.as_str()) {
            kept.push(n.clone());
        }
    }
    KeepPlan { top: nearest_survivor(rows, alive, top), focus: nearest_survivor(rows, alive, focus), selected: kept }
}

/// `before` is the row count when Refresh was called. `prev` is the previous poll,
/// `now` is this poll, and `polls` counts polls so far, including this one.
/// Two equal polls mean the count held. A count that never leaves `before` is
/// accepted only late: the shell may not have started the new enumeration.
pub(crate) fn count_settled(before: i32, prev: Option<i32>, now: i32, polls: u32) -> bool {
    if prev != Some(now) {
        return false;
    }
    if now != before {
        return polls >= 2;
    }
    polls >= SAME_COUNT_POLLS
}

/// Folders a shell event should compare, normalized. A rename names both parents.
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
/// A copy does not name the source. The same folder is named once.
pub(crate) fn transfer_folders(srcs: &[String], dst: &str, mv: bool) -> Vec<String> {
    let mut out = if mv { source_folders(srcs) } else { Vec::new() };
    push_event(&mut out, ShellNote::Dir, dst);
    out
}

pub(crate) fn source_drift(paths: &[String]) -> Vec<(String, Drift)> {
    source_folders(paths).into_iter().map(|k| (k, Drift::Gone)).collect()
}

/// Move source is `Gone`. The destination is `Arrived`. The same folder is `Arrived`.
pub(crate) fn transfer_drift(srcs: &[String], dst: &str, mv: bool) -> Vec<(String, Drift)> {
    let dest = norm_key(dst);
    transfer_folders(srcs, dst, mv)
        .into_iter()
        .map(|k| {
            let drift = if k == dest { Drift::Arrived } else { Drift::Gone };
            (k, drift)
        })
        .collect()
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
    drift: Drift,
}

/// Trailing-edge coalesce. A note moves the deadline. `take_due` is empty until
/// `quiet_ms` has passed since the last note, or the burst has run for `cap_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Coalesce {
    slots: Vec<Slot>,
}

impl Coalesce {
    pub(crate) fn note(&mut self, now_ms: u64, key: &str, drift: Drift) {
        if key.is_empty() {
            return;
        }
        if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
            s.last_ms = now_ms;
            s.drift = s.drift.merge(drift);
            return;
        }
        self.slots.push(Slot { key: key.to_string(), started_ms: now_ms, last_ms: now_ms, drift });
    }

    pub(crate) fn take_due(&mut self, now_ms: u64, quiet_ms: u64, cap_ms: u64) -> Vec<(String, Drift)> {
        let mut due = Vec::new();
        self.slots.retain(|s| {
            let quiet = now_ms.saturating_sub(s.last_ms) >= quiet_ms;
            let cap = now_ms.saturating_sub(s.started_ms) >= cap_ms;
            if quiet || cap {
                due.push((s.key.clone(), s.drift));
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct GraceSlot {
    key: String,
    drift: Drift,
    until_ms: u64,
}

/// One delayed re-check per folder. Arming again merges the drift and does not move the deadline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Grace {
    slots: Vec<GraceSlot>,
}

impl Grace {
    fn arm(&mut self, now_ms: u64, key: &str, drift: Drift, grace_ms: u64) {
        if key.is_empty() {
            return;
        }
        if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
            s.drift = s.drift.merge(drift);
            return;
        }
        self.slots.push(GraceSlot { key: key.to_string(), drift, until_ms: now_ms.saturating_add(grace_ms) });
    }

    fn cancel(&mut self, key: &str) {
        self.slots.retain(|s| s.key != key);
    }

    fn take_due(&mut self, now_ms: u64) -> Vec<(String, Drift)> {
        let mut due = Vec::new();
        self.slots.retain(|s| {
            if s.until_ms <= now_ms {
                due.push((s.key.clone(), s.drift));
                false
            } else {
                true
            }
        });
        due
    }

    fn wait_ms(&self, now_ms: u64) -> Option<u64> {
        self.slots.iter().map(|s| s.until_ms.saturating_sub(now_ms).max(1)).min()
    }
}

/// What a due compare does with a shell label edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DueGate {
    /// No label edit: compare now. Nothing is stored.
    Run(Drift),
    /// Label edit is open. Compare, Refresh, calm_selection, and grace all wait.
    Held,
}

/// Drift parked until the shell label edit closes. The same folder merges.
/// This is not the hidden-tab queue (`Pending` / `pending`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditHold {
    slots: Vec<(String, Drift)>,
}

impl EditHold {
    pub(crate) fn note(&mut self, key: &str, drift: Drift) {
        if key.is_empty() {
            return;
        }
        if let Some(slot) = self.slots.iter_mut().find(|(k, _)| k == key) {
            slot.1 = slot.1.merge(drift);
            return;
        }
        self.slots.push((key.to_string(), drift));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub(crate) fn take(&mut self) -> Vec<(String, Drift)> {
        std::mem::take(&mut self.slots)
    }
}

/// A due folder while `editing` is the pane's shell label edit.
/// Held drift is not compared. A folder that is not being edited is returned and not stored.
pub(crate) fn gate_due(hold: &mut EditHold, key: &str, drift: Drift, editing: bool) -> DueGate {
    if editing && !key.is_empty() {
        hold.note(key, drift);
        return DueGate::Held;
    }
    DueGate::Run(drift)
}

/// Timer poll. Still editing, or nothing held: nothing to compare. Edit finished: the queue, once.
pub(crate) fn poll_edit_hold(hold: &mut EditHold, editing: bool) -> Vec<(String, Drift)> {
    if editing || hold.is_empty() {
        Vec::new()
    } else {
        hold.take()
    }
}

/// Posts the compare when dropped. Built on the worker so every return, including
/// cancel and a panic the worker catches, still reaches the UI thread.
pub(crate) struct RefreshLater {
    hwnds: Vec<isize>,
    folders: Vec<(String, Drift)>,
}

impl RefreshLater {
    pub(crate) fn new(hwnds: Vec<isize>, folders: Vec<(String, Drift)>) -> Self {
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
    folders: Vec<(String, Drift)>,
    left: usize,
}

struct Win {
    hwnd: isize,
    cookie: u32,
    /// PIDLs passed to `SHChangeNotifyRegister`. Valid until the cookie is dropped.
    pidls: Vec<isize>,
    slots: Coalesce,
    grace: Grace,
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
    slot: Option<usize>,
    sited: bool,
}

/// A hidden tab's browser has no site, so the compare waits until that tab is shown.
#[derive(Clone)]
struct Pending {
    pane: isize,
    key: String,
    drift: Drift,
}

struct Restore {
    hwnd: isize,
    pane: isize,
    key: String,
    top: Option<String>,
    focus: Option<String>,
    selected: Vec<String>,
    before: i32,
    prev: Option<i32>,
    polls: u32,
}

enum Look {
    Blind { view: i64, disk: i64 },
    Ready { diff: ListDiff, rows: Vec<String>, present: Vec<String>, selected: Vec<String>, top: Option<usize>, focus: Option<usize>, before: i32 },
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

/// Label-edit holds for one main window. Not the hidden-tab queue.
struct EditWatch {
    hwnd: isize,
    queue: EditHold,
}

fn held() -> &'static Mutex<EditWatch> {
    static HELD: Mutex<EditWatch> = Mutex::new(EditWatch { hwnd: 0, queue: EditHold { slots: Vec::new() } });
    &HELD
}

fn restores() -> &'static Mutex<Vec<Restore>> {
    static RESTORES: Mutex<Vec<Restore>> = Mutex::new(Vec::new());
    &RESTORES
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn mono_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn log_gate() -> std::sync::MutexGuard<'static, ()> {
    static GATE: Mutex<()> = Mutex::new(());
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// One older file. `maguro-list.log` larger than `max_bytes` becomes `maguro-list.log.1`.
fn rotate_list_log(path: &std::path::Path, max_bytes: u64) {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len <= max_bytes {
        return;
    }
    let bak = path.with_extension("log.1");
    let _ = std::fs::remove_file(&bak);
    let _ = std::fs::rename(path, &bak);
}

fn write_log(line: &str) {
    let path = crate::store::config_dir().join(LIST_LOG);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() && std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _gate = log_gate();
    rotate_list_log(&path, LIST_LOG_MAX);
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    let _ = writeln!(file, "{line}");
}

/// Tell every main window to compare `folders`. Safe on a worker thread.
pub(crate) fn post_refresh(hwnds: &[isize], folders: &[(String, Drift)]) {
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

fn take_job(id: u64) -> Option<Vec<(String, Drift)>> {
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
    g.retain(|e| echo_live(e.until_ms, now));
    g.iter().any(|e| e.key == key)
}

fn mark_echo(keys: &[String], now: u64, window_ms: u64) {
    let until = now.saturating_add(window_ms);
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
    g.retain(|e| echo_live(e.until_ms, now));
}

/// The folder Refresh that follows creating a file or folder.
/// Its `SHCNE_UPDATEDIR` can arrive after [`ECHO_MS`], still inside the burst cap.
pub(crate) fn note_own_refresh(path: &str) {
    let key = norm_key(path);
    if key.is_empty() {
        return;
    }
    mark_echo(std::slice::from_ref(&key), mono_ms(), echo_window_ms(true));
}

fn refreshing_pane(pane: isize) -> bool {
    guard(restores()).iter().any(|r| r.pane == pane)
}

/// Watch the folders on screen, not every drive. Non-recursive: a change in a child
/// folder does not alter this folder's rows. Called again after each navigation.
pub(crate) unsafe fn arm(hwnd: HWND) {
    retarget(hwnd);
}

pub(crate) unsafe fn retarget(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let mut seen = HashSet::new();
    let mut pidls = Vec::new();
    let mut entries = Vec::new();
    for path in paths_here() {
        if blocks(&path) {
            continue;
        }
        let key = norm_key(&path);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        let Some(pidl) = folder_pidl(&path) else { continue };
        entries.push(SHChangeNotifyEntry { pidl, fRecursive: false.into() });
        pidls.push(pidl as isize);
    }
    let watch = SHCNE_RENAMEITEM | SHCNE_CREATE | SHCNE_DELETE | SHCNE_MKDIR | SHCNE_RMDIR | SHCNE_UPDATEDIR | SHCNE_RENAMEFOLDER;
    // No recursive interrupt. Events outside these folders never reach the UI thread.
    let sources = SHCNRF_ShellLevel | SHCNRF_InterruptLevel | SHCNRF_NewDelivery;
    let cookie = if entries.is_empty() {
        0
    } else {
        SHChangeNotifyRegister(hwnd, sources, watch.0 as i32, WM_SHELL_CHANGE, entries.len() as i32, entries.as_ptr())
    };
    if cookie == 0 {
        free_pidls(&pidls);
        pidls.clear();
    }
    install_watch(hwnd, cookie, pidls);
}

unsafe fn folder_pidl(path: &str) -> Option<*mut ITEMIDLIST> {
    let mut pidl = std::ptr::null_mut();
    if SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None).is_err() || pidl.is_null() {
        return None;
    }
    Some(pidl)
}

unsafe fn free_pidls(pidls: &[isize]) {
    for p in pidls {
        if *p != 0 {
            ILFree(Some(*p as *const ITEMIDLIST));
        }
    }
}

unsafe fn install_watch(hwnd: HWND, cookie: u32, pidls: Vec<isize>) {
    let mut wins = guard(wins());
    if let Some(old) = wins.iter().find(|w| w.hwnd == hwnd.0 as isize) {
        let (old_cookie, old_pidls) = (old.cookie, old.pidls.clone());
        if old_cookie != 0 {
            let _ = SHChangeNotifyDeregister(old_cookie);
        }
        free_pidls(&old_pidls);
        if let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
            win.cookie = cookie;
            win.pidls = pidls;
        }
        return;
    }
    wins.push(Win { hwnd: hwnd.0 as isize, cookie, pidls, slots: Coalesce::default(), grace: Grace::default() });
}

pub(crate) unsafe fn disarm(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let _ = KillTimer(Some(hwnd), TIMER_LIST);
    let (cookie, pidls) = {
        let mut wins = guard(wins());
        let found = wins.iter().find(|w| w.hwnd == hwnd.0 as isize).map(|w| (w.cookie, w.pidls.clone())).unwrap_or((0, Vec::new()));
        wins.retain(|w| w.hwnd != hwnd.0 as isize);
        found
    };
    guard(restores()).retain(|r| r.hwnd != hwnd.0 as isize);
    {
        let mut h = guard(held());
        if h.hwnd == hwnd.0 as isize {
            let _ = h.queue.take();
            h.hwnd = 0;
        }
    }
    if cookie != 0 {
        let _ = SHChangeNotifyDeregister(cookie);
    }
    free_pidls(&pidls);
}

pub(crate) unsafe fn on_shell(hwnd: HWND, wp: WPARAM, lp: LPARAM) {
    let Some((event, item, item2)) = read_change(wp, lp) else { return };
    let event = event as u32;
    let Some(kind) = shell_note(event) else { return };
    let folders = event_folders(kind, &item, &item2);
    if folders.is_empty() {
        return;
    }
    let drift = drift_of(event);
    let shown = paths_here();
    let now = mono_ms();
    let add: Vec<String> = folders
        .into_iter()
        .filter(|f| keep_note(kind, is_echo(f, now)) && !panes_showing(&shown, std::slice::from_ref(f)).is_empty())
        .collect();
    if add.is_empty() {
        return;
    }
    {
        let mut wins = guard(wins());
        let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) else { return };
        for f in &add {
            // The burst is still going, so a grace re-check of this folder waits for the new quiet time.
            win.grace.cancel(f);
            win.slots.note(now, f, drift);
        }
    }
    arm_list_timer(hwnd);
}

pub(crate) unsafe fn on_timer(hwnd: HWND) {
    let _ = KillTimer(Some(hwnd), TIMER_LIST);
    let now = mono_ms();
    let (due, again) = {
        let mut wins = guard(wins());
        if let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
            (win.slots.take_due(now, QUIET_MS, CAP_MS), win.grace.take_due(now))
        } else {
            (Vec::new(), Vec::new())
        }
    };
    consider(hwnd, &due, false);
    consider(hwnd, &again, true);
    release_edit(hwnd);
    poll_restores(hwnd);
    arm_list_timer(hwnd);
}

pub(crate) unsafe fn on_finished(hwnd: HWND, wp: WPARAM) {
    let Some(folders) = take_job(wp.0 as u64) else { return };
    consider(hwnd, &folders, false);
    arm_list_timer(hwnd);
}

fn arm_list_timer(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let now = mono_ms();
    let wait = soonest(hwnd, now);
    let _ = unsafe { KillTimer(Some(hwnd), TIMER_LIST) };
    if let Some(ms) = wait {
        let _ = unsafe { SetTimer(Some(hwnd), TIMER_LIST, ms.max(1).min(u32::MAX as u64) as u32, None) };
    }
}

fn soonest(hwnd: HWND, now: u64) -> Option<u64> {
    let scheduled = {
        let wins = guard(wins());
        wins.iter().find(|w| w.hwnd == hwnd.0 as isize).map(|win| {
            let quiet = win.slots.wait_ms(now, QUIET_MS, CAP_MS);
            let grace = win.grace.wait_ms(now);
            quiet.into_iter().chain(grace).min()
        })
    };
    let restore = guard(restores()).iter().any(|r| r.hwnd == hwnd.0 as isize).then_some(RESTORE_MS);
    let edit = {
        let h = guard(held());
        (!h.queue.is_empty() && h.hwnd == hwnd.0 as isize).then_some(RESTORE_MS)
    };
    scheduled.flatten().into_iter().chain(restore).chain(edit).min()
}

fn drift_for(folders: &[(String, Drift)], path: &str) -> Drift {
    let key = norm_key(path);
    folders.iter().filter(|(k, _)| k == &key).fold(Drift::Gone, |d, (_, n)| d.merge(*n))
}

unsafe fn consider(hwnd: HWND, folders: &[(String, Drift)], again: bool) {
    if folders.is_empty() {
        return;
    }
    let keys: Vec<String> = folders.iter().map(|(k, _)| k.clone()).collect();
    let views = views_for(&keys);
    remember_hidden(&views, folders);
    let show_hidden = hidden();
    let mut touched = false;
    let again = again || hwnd.0.is_null();
    for v in views.into_iter().filter(|v| v.sited) {
        if blocks(&v.path) {
            write_log(&skip_line(&v.path));
            continue;
        }
        if refreshing_pane(v.pane) {
            continue;
        }
        let drift = drift_for(folders, &v.path);
        // The pane HWND is the parent, so it does not see the rename box.
        // Look under this pane's shell view. While that box is open, do not
        // compare: Refresh and calm_selection both close it. The held drift
        // is checked once, with grace, after the edit ends.
        let editing = if hwnd.0.is_null() {
            false
        } else if let Some(i) = v.slot {
            pane_shell_label_open(i)
        } else {
            false
        };
        if hold_due(hwnd, &norm_key(&v.path), drift, editing) {
            continue;
        }
        match look_at(&v, show_hidden, drift) {
            Look::Blind { view, disk } => {
                write_log(&check_line(&v.path, false, view, disk, false));
                if let Some(slot) = v.slot {
                    bump_status(slot);
                    touched = true;
                }
            }
            Look::Ready { diff, rows, present, selected, top, focus, before } => {
                if !diff.stale {
                    cancel_grace(hwnd, &norm_key(&v.path));
                    let (live, shell) = calm_selection(&v, &rows);
                    if let Some(slot) = v.slot {
                        hold_selection_count(slot, live, shell);
                        touched = true;
                    }
                    write_log(&check_line(&v.path, false, diff.view as i64, diff.disk as i64, false));
                    continue;
                }
                if !again {
                    arm_grace(hwnd, &norm_key(&v.path), drift);
                    continue;
                }
                let plan = plan_keep(&rows, &present, top, focus, &selected);
                start_refresh(hwnd, &v, &plan, before, diff.view as i64, diff.disk as i64);
            }
        }
    }
    if touched {
        update_status();
    }
}

/// `true` when the due folder was parked on the label-edit queue.
fn hold_due(hwnd: HWND, key: &str, drift: Drift, editing: bool) -> bool {
    let mut h = guard(held());
    if gate_due(&mut h.queue, key, drift, editing) != DueGate::Held {
        return false;
    }
    h.hwnd = hwnd.0 as isize;
    true
}

unsafe fn release_edit(hwnd: HWND) {
    let keys = {
        let h = guard(held());
        if h.hwnd != hwnd.0 as isize || h.queue.is_empty() {
            return;
        }
        h.queue.slots.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
    };
    let editing = edit_open_for(&keys);
    let released = {
        let mut h = guard(held());
        if h.hwnd != hwnd.0 as isize {
            return;
        }
        poll_edit_hold(&mut h.queue, editing)
    };
    if !released.is_empty() {
        consider(hwnd, &released, false);
    }
}

unsafe fn edit_open_for(keys: &[String]) -> bool {
    for v in views_for(keys) {
        if !v.sited {
            continue;
        }
        if let Some(i) = v.slot {
            if pane_shell_label_open(i) {
                return true;
            }
        }
    }
    false
}

fn arm_grace(hwnd: HWND, key: &str, drift: Drift) {
    if hwnd.0.is_null() || key.is_empty() {
        return;
    }
    let mut wins = guard(wins());
    let win = win_slot(&mut wins, hwnd);
    win.grace.arm(mono_ms(), key, drift, GRACE_MS);
}

fn cancel_grace(hwnd: HWND, key: &str) {
    if hwnd.0.is_null() {
        return;
    }
    let mut wins = guard(wins());
    if let Some(win) = wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
        win.grace.cancel(key);
    }
}

fn win_slot(wins: &mut Vec<Win>, hwnd: HWND) -> &mut Win {
    let id = hwnd.0 as isize;
    if !wins.iter().any(|w| w.hwnd == id) {
        wins.push(Win { hwnd: id, cookie: 0, pidls: Vec::new(), slots: Coalesce::default(), grace: Grace::default() });
    }
    wins.iter_mut().find(|w| w.hwnd == id).unwrap()
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
            out.push(View { eb: pane.eb.clone(), path, pane: pane.hwnd.0 as isize, slot: Some(i), sited: pane.cookie != 0 });
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
                out.push(View { eb: pane.eb.clone(), path, pane: pane.hwnd.0 as isize, slot: None, sited: pane.cookie != 0 });
            }
        }
        out
    })
}

fn find_view(pane: isize) -> Option<View> {
    app(|a| {
        for (i, pane_slot) in a.panes.iter().enumerate() {
            let Some(p) = pane_slot.as_ref() else { continue };
            if p.hwnd.0 as isize != pane {
                continue;
            }
            let path = a.s.paths.get(i).cloned().unwrap_or_default();
            return Some(View { eb: p.eb.clone(), path, pane, slot: Some(i), sited: p.cookie != 0 });
        }
        let tab = a.s.tab;
        for (ti, set) in a.park.iter().enumerate() {
            if ti == tab {
                continue;
            }
            let paths = a.s.tabs.get(ti).map(|t| t.paths.clone()).unwrap_or_default();
            for (i, p) in set.iter().enumerate() {
                let Some(p) = p.as_ref() else { continue };
                if p.hwnd.0 as isize != pane {
                    continue;
                }
                let path = paths.get(i).cloned().unwrap_or_default();
                return Some(View { eb: p.eb.clone(), path, pane, slot: None, sited: p.cookie != 0 });
            }
        }
        None
    })
}

fn remember_hidden(views: &[View], folders: &[(String, Drift)]) {
    let mut q = guard(pending());
    for v in views.iter().filter(|v| !v.sited) {
        let key = norm_key(&v.path);
        if key.is_empty() {
            continue;
        }
        let drift = drift_for(folders, &v.path);
        if let Some(p) = q.iter_mut().find(|p| p.pane == v.pane && p.key == key) {
            p.drift = p.drift.merge(drift);
        } else {
            q.push(Pending { pane: v.pane, key, drift });
        }
    }
    let extra = q.len().saturating_sub(64);
    if extra > 0 {
        q.drain(0..extra);
    }
}

/// `IShellView::Refresh` and a directory read on a share can block the UI thread.
fn blocks(path: &str) -> bool {
    if path.is_empty() || !is_fs(path) {
        return true;
    }
    let drives = app(|a| a.drives.iter().map(|d| (d.0, d.1)).collect::<Vec<_>>());
    let live = crate::remote::path_on_live_mount(path);
    folder_is_share(path, cached_kind(&drives, path), live)
}

struct DiskNames {
    present: Vec<String>,
    expected: Vec<String>,
    cut: bool,
}

unsafe fn look_at(v: &View, show_hidden: bool, drift: Drift) -> Look {
    let Ok(fv) = v.eb.GetCurrentView::<IFolderView2>() else {
        return Look::Blind { view: -1, disk: -1 };
    };
    let before = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(-1);
    if before < 0 {
        return Look::Blind { view: -1, disk: -1 };
    }
    if before as usize > NAME_CAP {
        return Look::Blind { view: before as i64, disk: -1 };
    }
    let mut rows = Vec::with_capacity(before as usize);
    for k in 0..before {
        rows.push(item_key(&fv, k));
    }
    let Some(disk) = disk_names(&v.path, show_hidden, &spec_of(v)) else {
        return Look::Blind { view: before as i64, disk: -1 };
    };
    if disk.cut {
        return Look::Blind { view: before as i64, disk: -1 };
    }
    let view_names: Vec<String> = rows.iter().filter(|s| !s.is_empty()).cloned().collect();
    let diff = list_diff(&view_names, &disk.present, &disk.expected, drift);
    let selected = selected_keys(&fv);
    let top = first_visible(&fv).map(|i| i as usize);
    let focus = fv.GetFocusedItem().ok().filter(|&i| i >= 0).map(|i| i as usize);
    Look::Ready { diff, rows, present: disk.present, selected, top, focus, before }
}

unsafe fn spec_of(v: &View) -> String {
    let from_view = v.eb.GetCurrentView::<IShellView>().ok().and_then(|sv| sv.GetWindow().ok()).and_then(filter_for_view);
    if let Some(spec) = from_view {
        return spec;
    }
    v.slot.map(filter_of).unwrap_or_default()
}

unsafe fn item_key(fv: &IFolderView2, index: i32) -> String {
    let Ok(it) = fv.GetItem::<IShellItem>(index) else { return String::new() };
    let name = it.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).ok().map(|p| pwstr(p)).unwrap_or_default();
    if name.is_empty() { String::new() } else { name_key(&name) }
}

fn disk_names(path: &str, show_hidden: bool, spec: &str) -> Option<DiskNames> {
    let opened = crate::winpath::win_long_path(path);
    let rd = std::fs::read_dir(opened).ok()?;
    let mut present = Vec::new();
    let mut expected = Vec::new();
    let mut seen = 0usize;
    for ent in rd {
        seen += 1;
        if seen > NAME_CAP {
            return Some(DiskNames { present, expected, cut: true });
        }
        let Ok(ent) = ent else { continue };
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let key = name_key(&name);
        let meta = ent.metadata().ok();
        let attrs = meta.as_ref().map(|m| m.file_attributes()).unwrap_or(0);
        present.push(key.clone());
        let visible = meta.is_some() && listed_name(&name, attrs, show_hidden);
        let is_dir = attrs & ATTR_DIR != 0;
        let spec_ok = spec.is_empty() || spec_hit(&name, spec);
        if visible && (is_dir || spec_ok) {
            expected.push(key);
        }
    }
    Some(DiskNames { present, expected, cut: false })
}

fn spec_hit(name: &str, spec: &str) -> bool {
    unsafe { PathMatchSpecW(&HSTRING::from(name), &HSTRING::from(spec)).as_bool() }
}

unsafe fn selected_keys(fv: &IFolderView2) -> Vec<String> {
    let Ok(sf) = fv.GetFolder::<IShellFolder>() else { return Vec::new() };
    let Ok(en) = fv.Items::<IEnumIDList>(SVGIO_SELECTION) else { return Vec::new() };
    let mut out = Vec::new();
    for _ in 0..10_000 {
        let mut slot = [std::ptr::null_mut()];
        let mut n = 0u32;
        let hr = en.Next(&mut slot, Some(&mut n));
        if !hr.is_ok() || n == 0 || slot[0].is_null() {
            break;
        }
        let pidl = slot[0];
        let key = name_key(&child_name(&sf, pidl));
        CoTaskMemFree(Some(pidl as _));
        if !key.is_empty() {
            out.push(key);
        }
    }
    out
}

/// Drop selection entries whose names are no longer in the view.
/// Returns `(names still selected, shell count before that cleanup)`.
unsafe fn calm_selection(v: &View, rows: &[String]) -> (i32, i32) {
    let Ok(fv) = v.eb.GetCurrentView::<IFolderView2>() else { return (0, 0) };
    let shell = fv.ItemCount(SVGIO_SELECTION).unwrap_or(0);
    // An empty list cannot still have a selection. The shell count often stays
    // until the next click; clear it here so the status line drops "N selected".
    // A non-empty list whose names could not be read is left to the walk below.
    if rows.is_empty() {
        if let Ok(sv) = fv.cast::<IShellView>() {
            let _ = sv.SelectItem(std::ptr::null(), SVSI_DESELECTOTHERS.0 as u32);
        }
        return (0, shell);
    }
    let have: HashSet<&str> = rows.iter().filter(|s| !s.is_empty()).map(String::as_str).collect();
    let Ok(sf) = fv.GetFolder::<IShellFolder>() else { return (shell, shell) };
    let Ok(en) = fv.Items::<IEnumIDList>(SVGIO_SELECTION) else { return (shell, shell) };
    let sv = fv.cast::<IShellView>().ok();
    let mut live = 0i32;
    let mut got = false;
    let mut drop_pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    for _ in 0..10_000 {
        let mut slot = [std::ptr::null_mut()];
        let mut n = 0u32;
        let hr = en.Next(&mut slot, Some(&mut n));
        if !hr.is_ok() || n == 0 || slot[0].is_null() {
            break;
        }
        got = true;
        let pidl = slot[0];
        let key = name_key(&child_name(&sf, pidl));
        if !key.is_empty() && have.contains(key.as_str()) {
            live += 1;
            CoTaskMemFree(Some(pidl as _));
        } else {
            drop_pidls.push(pidl);
        }
    }
    // The count still names rows, but the enumerator has no pidl. Those rows are gone.
    if !got && shell > 0 {
        for pidl in drop_pidls {
            CoTaskMemFree(Some(pidl as _));
        }
        if let Some(sv) = sv.as_ref() {
            let _ = sv.SelectItem(std::ptr::null(), SVSI_DESELECTOTHERS.0 as u32);
        }
        return (0, shell);
    }
    if let Some(sv) = sv {
        for pidl in &drop_pidls {
            let _ = sv.SelectItem(*pidl, SVSI_DESELECT.0 as u32);
        }
    }
    for pidl in drop_pidls {
        CoTaskMemFree(Some(pidl as _));
    }
    (live, shell)
}

unsafe fn child_name(sf: &IShellFolder, pidl: *const ITEMIDLIST) -> String {
    let mut sr = STRRET::default();
    let flags = SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0);
    if sf.GetDisplayNameOf(pidl, flags, &mut sr).is_err() {
        return String::new();
    }
    let mut wide = PWSTR::null();
    if StrRetToStrW(&mut sr, Some(pidl), &mut wide).is_err() || wide.0.is_null() {
        return String::new();
    }
    pwstr(wide)
}

unsafe fn start_refresh(hwnd: HWND, v: &View, plan: &KeepPlan, before: i32, view_n: i64, disk_n: i64) {
    let key = norm_key(&v.path);
    mark_echo(std::slice::from_ref(&key), mono_ms(), echo_window_ms(false));
    let Ok(sv) = v.eb.GetCurrentView::<IShellView>() else {
        write_log(&check_line(&v.path, true, view_n, disk_n, false));
        note_status(v.slot);
        return;
    };
    if sv.Refresh().is_err() {
        write_log(&check_line(&v.path, true, view_n, disk_n, false));
        note_status(v.slot);
        return;
    }
    write_log(&check_line(&v.path, true, view_n, disk_n, true));
    let job = Restore {
        hwnd: hwnd.0 as isize,
        pane: v.pane,
        key,
        top: plan.top.clone(),
        focus: plan.focus.clone(),
        selected: plan.selected.clone(),
        before,
        prev: None,
        polls: 0,
    };
    if hwnd.0.is_null() {
        if let Ok(fv) = v.eb.GetCurrentView::<IFolderView2>() {
            let _ = apply_keep(&fv, &job);
        }
        return;
    }
    let mut q = guard(restores());
    if q.len() >= 32 {
        q.remove(0);
    }
    q.push(job);
}

unsafe fn poll_restores(hwnd: HWND) {
    let mine = {
        let mut q = guard(restores());
        let mut mine = Vec::new();
        let mut rest = Vec::new();
        for job in q.drain(..) {
            if job.hwnd == hwnd.0 as isize {
                mine.push(job);
            } else {
                rest.push(job);
            }
        }
        *q = rest;
        mine
    };
    let mut keep = Vec::new();
    for mut job in mine {
        if !poll_one(&mut job) {
            keep.push(job);
        }
    }
    if keep.is_empty() {
        return;
    }
    let mut q = guard(restores());
    q.extend(keep);
    let extra = q.len().saturating_sub(32);
    if extra > 0 {
        q.drain(0..extra);
    }
}

unsafe fn poll_one(job: &mut Restore) -> bool {
    let Some(found) = find_view(job.pane) else { return true };
    if norm_key(&found.path) != job.key {
        return true;
    }
    let Ok(fv) = found.eb.GetCurrentView::<IFolderView2>() else {
        job.polls = job.polls.saturating_add(1);
        return job.polls >= RESTORE_POLLS;
    };
    let now = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(job.before);
    job.polls = job.polls.saturating_add(1);
    if !count_settled(job.before, job.prev, now, job.polls) && job.polls < RESTORE_POLLS {
        job.prev = Some(now);
        return false;
    }
    let shell = fv.ItemCount(SVGIO_SELECTION).unwrap_or(0);
    match apply_keep(&fv, job) {
        Some(n) => {
            if let Some(slot) = found.slot {
                hold_selection_count(slot, n, shell);
                update_status();
            }
        }
        None => {
            if let Some(slot) = found.slot {
                bump_status(slot);
                update_status();
            }
        }
    }
    true
}

unsafe fn apply_keep(fv: &IFolderView2, job: &Restore) -> Option<i32> {
    let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(-1);
    if n < 0 || n as usize > NAME_CAP {
        return None;
    }
    let mut rows = Vec::with_capacity(n as usize);
    for k in 0..n {
        rows.push(item_key(fv, k));
    }
    let focus = job.focus.as_deref().and_then(|name| find_row(&rows, name));
    let top = job.top.as_deref().and_then(|name| find_row(&rows, name));
    let mut picked = Vec::new();
    for name in &job.selected {
        if let Some(i) = find_row(&rows, name) {
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
    }
    // EnsureVisible can clear the selection, so it runs only before the select.
    // The second pass only scrolls, and leaves the selection in place.
    scroll_top(fv, top, true);
    select_indexes(fv, &picked, focus);
    scroll_top(fv, top, false);
    Some(picked.len() as i32)
}

fn find_row(rows: &[String], name: &str) -> Option<i32> {
    rows.iter().position(|n| n == name).map(|i| i as i32)
}

unsafe fn select_indexes(fv: &IFolderView2, selected: &[i32], focus: Option<i32>) {
    if selected.is_empty() && focus.is_none() {
        if let Ok(sv) = fv.cast::<IShellView>() {
            let _ = sv.SelectItem(std::ptr::null(), SVSI_DESELECTOTHERS.0 as u32);
        }
        return;
    }
    let mut first = true;
    let mut saw_focus = false;
    let mut mark = |index: i32, select: bool, focused: bool| {
        let mut flags = SVSI_NOTAKEFOCUS.0;
        if select {
            flags |= SVSI_SELECT.0;
        }
        if focused {
            flags |= SVSI_FOCUSED.0;
        }
        if first {
            flags |= SVSI_DESELECTOTHERS.0;
            first = false;
        }
        let _ = fv.SelectItem(index, flags as u32);
    };
    for &index in selected {
        let focused = focus == Some(index);
        if focused {
            saw_focus = true;
        }
        mark(index, true, focused);
    }
    if let Some(index) = focus {
        if !saw_focus {
            mark(index, false, true);
        }
    }
}

unsafe fn first_visible(fv: &IFolderView2) -> Option<i32> {
    let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(0);
    if n <= 0 {
        return None;
    }
    if let Ok(i) = fv.GetVisibleItem(-1, false) {
        if (0..n).contains(&i) {
            return Some(i);
        }
    }
    let lv = view_list(fv);
    if is_sys_list(lv) {
        let i = SendMessageW(lv, LVM_GETTOPINDEX, None, None).0 as i32;
        if (0..n).contains(&i) {
            return Some(i);
        }
    }
    if let Ok(i) = fv.GetVisibleItem(0, true) {
        if (0..n).contains(&i) {
            return Some(i);
        }
    }
    Some(0)
}

const LVM_GETTOPINDEX: u32 = 0x1027;
const LVM_SCROLL: u32 = 0x1014;
const LVM_GETITEMRECT: u32 = 0x100E;

unsafe fn scroll_top(fv: &IFolderView2, index: Option<i32>, allow_ensure: bool) {
    let Some(index) = index else { return };
    let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(0);
    if index < 0 || n <= 0 {
        return;
    }
    let index = index.min(n - 1);
    let mode = fv.GetCurrentViewMode().unwrap_or(0) as i32;
    let lv = view_list(fv);
    if is_sys_list(lv) && theme_scroll_rows(mode) {
        let cur = SendMessageW(lv, LVM_GETTOPINDEX, None, None).0 as i32;
        if let Some(rows) = scroll_rows(cur, index, n) {
            let px = row_px(lv).max(1);
            let _ = SendMessageW(lv, LVM_SCROLL, Some(WPARAM(0)), Some(LPARAM(i32::saturating_mul(rows, px) as isize)));
        }
        return;
    }
    let host = fv.cast::<IShellView>().and_then(|sv| sv.GetWindow()).unwrap_or_default();
    for _ in 0..40 {
        let cur = first_visible(fv).unwrap_or(index);
        if cur == index {
            return;
        }
        let gap = (index - cur).abs();
        let cmd = if index > cur {
            if gap > 8 { SB_PAGEDOWN.0 } else { SB_LINEDOWN.0 }
        } else if gap > 8 {
            SB_PAGEUP.0
        } else {
            SB_LINEUP.0
        };
        let targets = [host, lv];
        if targets.iter().all(|h| h.0.is_null()) {
            break;
        }
        for h in targets {
            if !h.0.is_null() {
                let _ = SendMessageW(h, WM_VSCROLL, Some(WPARAM(cmd as usize)), Some(LPARAM(0)));
            }
        }
        let next = first_visible(fv).unwrap_or(cur);
        if next == cur {
            break;
        }
    }
    // Scroll messages did not move a DirectUI list. EnsureVisible can.
    // The caller selects afterwards, because this flag can clear a selection.
    if allow_ensure {
        let flags = (SVSI_ENSUREVISIBLE.0 | SVSI_NOTAKEFOCUS.0) as u32;
        let _ = fv.SelectItem(index, flags);
    }
}

unsafe fn row_px(lv: HWND) -> i32 {
    let mut rc = RECT::default();
    if SendMessageW(lv, LVM_GETITEMRECT, Some(WPARAM(0)), Some(LPARAM(&mut rc as *mut _ as isize))).0 == 0 {
        return 1;
    }
    (rc.bottom - rc.top).max(1)
}

/// A tab that was hidden during the delete is sited again. Compare it once, then forget it.
pub(crate) unsafe fn relist_shown() {
    let queued: Vec<Pending> = guard(pending()).clone();
    if queued.is_empty() {
        return;
    }
    let mut folders: Vec<(String, Drift)> = Vec::new();
    for p in &queued {
        if let Some(slot) = folders.iter_mut().find(|(k, _)| k == &p.key) {
            slot.1 = slot.1.merge(p.drift);
        } else {
            folders.push((p.key.clone(), p.drift));
        }
    }
    let main = app(|a| a.main);
    consider(main, &folders, false);
    let keys: Vec<String> = folders.iter().map(|(k, _)| k.clone()).collect();
    let views = views_for(&keys);
    let mine = pane_hwnds_here();
    let mut q = guard(pending());
    q.retain(|p| {
        if !mine.contains(&p.pane) {
            return true;
        }
        views.iter().any(|v| !v.sited && v.pane == p.pane && norm_key(&v.path) == p.key)
    });
    arm_list_timer(main);
}

fn note_status(slot: Option<usize>) {
    if let Some(slot) = slot {
        bump_status(slot);
        unsafe { update_status() };
    }
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

    fn due(pairs: &[(&str, Drift)]) -> Vec<(String, Drift)> {
        pairs.iter().map(|(k, d)| ((*k).to_string(), *d)).collect()
    }

    #[test]
    fn a_burst_waits_for_the_last_note() {
        let mut c = Coalesce::default();
        // Twenty deletes, 10ms apart. A timer armed only on the first note would be
        // due at t = 400, while files are still being removed.
        for n in 0..20 {
            c.note(n * 10, r"c:\work", Drift::Gone);
        }
        assert!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS).is_empty());
        let last = 19 * 10;
        assert!(c.take_due(last + QUIET_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(last + QUIET_MS, QUIET_MS, CAP_MS), due(&[(r"c:\work", Drift::Gone)]));
        // The event after that compare starts another burst. The last one is not dropped.
        c.note(last + QUIET_MS + 5, r"c:\work", Drift::Gone);
        assert!(c.take_due(last + QUIET_MS + 5, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(last + QUIET_MS + 5 + QUIET_MS, QUIET_MS, CAP_MS), due(&[(r"c:\work", Drift::Gone)]));
    }

    #[test]
    fn a_long_burst_relists_at_the_cap_and_the_next_note_still_counts() {
        let mut c = Coalesce::default();
        c.note(0, r"c:\work", Drift::Gone);
        c.note(CAP_MS - 10, r"c:\work", Drift::Gone);
        assert!(c.take_due(CAP_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(CAP_MS, QUIET_MS, CAP_MS), due(&[(r"c:\work", Drift::Gone)]));
        c.note(CAP_MS, r"c:\work", Drift::Arrived);
        assert!(c.take_due(CAP_MS + QUIET_MS - 1, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(CAP_MS + QUIET_MS, QUIET_MS, CAP_MS), due(&[(r"c:\work", Drift::Arrived)]));
    }

    #[test]
    fn an_arrived_note_is_not_downgraded_by_a_later_delete() {
        let mut c = Coalesce::default();
        c.note(0, r"c:\work", Drift::Arrived);
        c.note(10, r"c:\work", Drift::Gone);
        assert_eq!(c.take_due(QUIET_MS + 10, QUIET_MS, CAP_MS), due(&[(r"c:\work", Drift::Arrived)]));
    }

    #[test]
    fn two_folders_wait_on_their_own_last_note() {
        let mut c = Coalesce::default();
        c.note(0, r"c:\a", Drift::Gone);
        c.note(100, r"c:\b", Drift::Arrived);
        assert_eq!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS), due(&[(r"c:\a", Drift::Gone)]));
        assert!(c.take_due(QUIET_MS, QUIET_MS, CAP_MS).is_empty());
        assert_eq!(c.take_due(100 + QUIET_MS, QUIET_MS, CAP_MS), due(&[(r"c:\b", Drift::Arrived)]));
    }

    #[test]
    fn grace_waits_once_and_a_second_arm_does_not_move_the_deadline() {
        let mut g = Grace::default();
        g.arm(0, r"c:\work", Drift::Gone, GRACE_MS);
        assert!(g.take_due(GRACE_MS - 1).is_empty());
        g.arm(GRACE_MS - 1, r"c:\work", Drift::Arrived, GRACE_MS);
        assert_eq!(g.wait_ms(GRACE_MS - 1), Some(1));
        assert_eq!(g.take_due(GRACE_MS), due(&[(r"c:\work", Drift::Arrived)]));
        assert!(g.take_due(GRACE_MS).is_empty());
        g.arm(0, r"c:\other", Drift::Gone, GRACE_MS);
        g.cancel(r"c:\other");
        assert_eq!(g.wait_ms(0), None);
    }

    #[test]
    fn source_and_transfer_folders_name_each_place_once() {
        let files = vec![r"C:\Work\a.txt".into(), r"C:\Work\b.txt".into(), r"C:\Other\c.txt".into()];
        assert_eq!(source_folders(&files), vec![r"c:\work".to_string(), r"c:\other".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\B", false), vec![r"c:\b".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\B", true), vec![r"c:\a".to_string(), r"c:\b".to_string()]);
        assert_eq!(transfer_folders(&[r"C:\A\f.txt".into()], r"C:\A\", true), vec![r"c:\a".to_string()]);
        assert_eq!(source_drift(&files), due(&[(r"c:\work", Drift::Gone), (r"c:\other", Drift::Gone)]));
        assert_eq!(transfer_drift(&[r"C:\A\f.txt".into()], r"C:\B", false), due(&[(r"c:\b", Drift::Arrived)]));
        assert_eq!(transfer_drift(&[r"C:\A\f.txt".into()], r"C:\B", true), due(&[(r"c:\a", Drift::Gone), (r"c:\b", Drift::Arrived)]));
        assert_eq!(transfer_drift(&[r"C:\A\f.txt".into()], r"C:\A\", true), due(&[(r"c:\a", Drift::Arrived)]));
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
        assert_eq!(drift_of(SHCNE_DELETE.0), Drift::Gone);
        assert_eq!(drift_of(SHCNE_RMDIR.0), Drift::Gone);
        assert_eq!(drift_of(SHCNE_UPDATEDIR.0), Drift::Gone);
        assert_eq!(drift_of(SHCNE_CREATE.0), Drift::Arrived);
        assert_eq!(drift_of(SHCNE_MKDIR.0), Drift::Arrived);
        assert_eq!(drift_of(SHCNE_RENAMEITEM.0), Drift::Arrived);
        assert_eq!(drift_of(SHCNE_RENAMEFOLDER.0), Drift::Arrived);
        assert_eq!(Drift::Gone.merge(Drift::Arrived), Drift::Arrived);
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

    fn names(all: &[&str]) -> Vec<String> {
        all.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn staleness_is_a_ghost_or_a_missing_arrival() {
        let same = list_diff(&names(&["a", "b"]), &names(&["a", "b", "desktop.ini"]), &names(&["a", "b"]), Drift::Arrived);
        assert_eq!(same, ListDiff { view: 2, disk: 2, stale: false });
        // A delete that the view has already applied. Extra names on disk are not stale.
        let gone_ok = list_diff(&names(&["a"]), &names(&["a", "b"]), &names(&["a", "b"]), Drift::Gone);
        assert!(!gone_ok.stale);
        // The view still shows a name that is gone.
        let ghost = list_diff(&names(&["a", "b"]), &names(&["a"]), &names(&["a"]), Drift::Gone);
        assert!(ghost.stale);
        assert_eq!(ghost.view, 2);
        assert_eq!(ghost.disk, 1);
        // Copy destination: the new file is not in the view yet.
        let missing = list_diff(&names(&["a"]), &names(&["a", "b"]), &names(&["a", "b"]), Drift::Arrived);
        assert!(missing.stale);
        assert_eq!(missing.disk, 2);
        // Folded names compare equal. The caller folds; mixed case here is a different key.
        assert_eq!(name_key("A.TXT"), "a.txt");
        assert_eq!(name_key("사진"), "사진");
        let folded = list_diff(&names(&["a.txt"]), &names(&["a.txt"]), &names(&["a.txt"]), Drift::Arrived);
        assert!(!folded.stale);
    }

    #[test]
    fn hidden_and_system_names_are_not_expected_in_the_view() {
        assert!(!listed_name("", 0, true));
        assert!(!listed_name(".", 0, true));
        assert!(listed_name("a.txt", 0, false));
        assert!(!listed_name("a.txt", ATTR_HIDDEN, false));
        assert!(listed_name("a.txt", ATTR_HIDDEN, true));
        assert!(!listed_name("desktop.ini", ATTR_HIDDEN | ATTR_SYSTEM, true));
        assert!(listed_name("folder", ATTR_DIR, false));
    }

    #[test]
    fn the_nearest_survivor_keeps_the_row_or_the_following_neighbour() {
        let rows = names(&["a", "b", "c", "d", "e"]);
        let alive = |n: &str| n != "c";
        assert_eq!(nearest_survivor(&rows, alive, Some(1)).as_deref(), Some("b"));
        // c is gone. d and b are the same distance; the following row wins.
        assert_eq!(nearest_survivor(&rows, alive, Some(2)).as_deref(), Some("d"));
        assert_eq!(nearest_survivor(&rows, |n| n != "e", Some(4)).as_deref(), Some("d"));
        assert_eq!(nearest_survivor(&rows, |n| n != "a", Some(0)).as_deref(), Some("b"));
        assert_eq!(nearest_survivor(&rows, |_| false, Some(2)), None);
        assert_eq!(nearest_survivor(&rows, alive, None), None);
        assert_eq!(nearest_survivor(&rows, alive, Some(9)), None);
        let plan = plan_keep(&rows, &names(&["a", "b", "d", "e"]), Some(2), Some(2), &names(&["b", "c", "e"]));
        assert_eq!(plan.top.as_deref(), Some("d"));
        assert_eq!(plan.focus.as_deref(), Some("d"));
        assert_eq!(plan.selected, names(&["b", "e"]));
    }

    #[test]
    fn the_refresh_count_settles_when_it_holds_still() {
        assert!(!count_settled(10, None, 10, 1));
        assert!(!count_settled(10, Some(10), 8, 2));
        assert!(count_settled(10, Some(8), 8, 2));
        assert!(!count_settled(10, Some(8), 8, 1));
        assert!(!count_settled(10, Some(10), 10, SAME_COUNT_POLLS - 1));
        assert!(count_settled(10, Some(10), 10, SAME_COUNT_POLLS));
        assert_eq!(RESTORE_POLLS, 8);
    }

    #[test]
    fn the_log_line_names_the_compare() {
        assert_eq!(
            check_line(r"C:\Work", true, 150, 140, true),
            r"listsync check C:\Work stale=true view=150 disk=140 refreshed=true"
        );
        assert_eq!(check_line(r"C:\Work", false, 10, -1, false), r"listsync check C:\Work stale=false view=10 disk=-1 refreshed=false");
        assert_eq!(skip_line(r"\\server\share"), r"listsync skip \\server\share share");
        assert_eq!(LIST_LOG, "maguro-list.log");
        assert_eq!(super::LIST_LOG_MAX, 512 * 1024);
    }

    #[test]
    fn the_list_log_rotates_past_512_kb_and_keeps_one_backup() {
        let dir = std::env::temp_dir().join(format!("maguro-listlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LIST_LOG);
        assert_eq!(path.with_extension("log.1").file_name().and_then(|s| s.to_str()), Some("maguro-list.log.1"));
        std::fs::write(&path, vec![b'a'; 40]).unwrap();
        super::rotate_list_log(&path, 40);
        assert!(path.exists(), "a file at the cap stays");
        std::fs::write(&path, vec![b'a'; 41]).unwrap();
        super::rotate_list_log(&path, 40);
        let bak = path.with_extension("log.1");
        assert_eq!(std::fs::read(&bak).unwrap().len(), 41);
        assert!(!path.exists());
        std::fs::write(&path, vec![b'b'; 50]).unwrap();
        super::rotate_list_log(&path, 40);
        assert_eq!(std::fs::read(&bak).unwrap(), vec![b'b'; 50]);
        assert!(!path.exists());
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "only the one rotated file is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn label_edit_holds_a_due_compare() {
        let mut h = EditHold::default();
        assert_eq!(gate_due(&mut h, r"c:\work", Drift::Arrived, true), DueGate::Held);
        assert_eq!(h.take(), due(&[(r"c:\work", Drift::Arrived)]));
    }

    #[test]
    fn label_edit_hold_merges_drift_and_arrived_wins() {
        let mut h = EditHold::default();
        assert_eq!(gate_due(&mut h, r"c:\work", Drift::Gone, true), DueGate::Held);
        assert_eq!(gate_due(&mut h, r"c:\work", Drift::Arrived, true), DueGate::Held);
        assert_eq!(h.take(), due(&[(r"c:\work", Drift::Arrived)]));
    }

    #[test]
    fn label_edit_hold_is_taken_once_when_the_edit_ends() {
        let mut h = EditHold::default();
        gate_due(&mut h, r"c:\work", Drift::Arrived, true);
        assert_eq!(poll_edit_hold(&mut h, false), due(&[(r"c:\work", Drift::Arrived)]));
        assert!(h.is_empty());
        assert!(poll_edit_hold(&mut h, false).is_empty());
    }

    #[test]
    fn label_edit_hold_stays_while_the_edit_continues() {
        let mut h = EditHold::default();
        gate_due(&mut h, r"c:\work", Drift::Gone, true);
        assert!(poll_edit_hold(&mut h, true).is_empty());
        assert_eq!(poll_edit_hold(&mut h, false), due(&[(r"c:\work", Drift::Gone)]));
    }

    #[test]
    fn label_edit_hold_is_empty_when_nothing_is_waiting() {
        let mut h = EditHold::default();
        assert!(poll_edit_hold(&mut h, false).is_empty());
        assert!(h.is_empty());
    }

    #[test]
    fn label_edit_due_without_an_edit_compares_now() {
        let mut h = EditHold::default();
        assert_eq!(gate_due(&mut h, r"c:\work", Drift::Gone, false), DueGate::Run(Drift::Gone));
        assert!(h.is_empty());
        assert!(poll_edit_hold(&mut h, false).is_empty());
    }

    #[test]
    fn label_edit_create_echo_lasts_at_least_the_burst_cap() {
        assert!(echo_window_ms(true) >= CAP_MS);
        assert_eq!(echo_window_ms(false), ECHO_MS);
        assert_eq!(ECHO_MS, QUIET_MS);
        let now = 50u64;
        let until = now.saturating_add(echo_window_ms(true));
        let during = now + CAP_MS - 1;
        assert!(echo_live(until, during));
        assert!(!keep_note(ShellNote::Dir, echo_live(until, during)));
        assert_eq!(shell_note(SHCNE_CREATE.0), Some(ShellNote::Child));
        assert_eq!(shell_note(SHCNE_MKDIR.0), Some(ShellNote::Child));
        assert!(keep_note(ShellNote::Child, echo_live(until, during)));
        let after = now.saturating_add(echo_window_ms(true));
        assert!(!echo_live(until, after));
        assert!(keep_note(ShellNote::Dir, echo_live(until, after)));
    }
}
