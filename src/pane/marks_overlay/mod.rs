//! Gold stars on marked files inside a folder pane.
//!
//! One layered click-through window per pane. UI Automation runs on a worker thread,
//! and only after this folder's mark index is non-empty. Scrolling hides the stars
//! at once; one read runs 130 ms after the last scroll event.

mod geom;
mod overlay;
mod star_draw;
mod uia;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::imports::*;
use crate::app::*;
use crate::shell::*;
use crate::ui::*;

use geom::{assign_stars, estimate_icon, file_name, icon_on_row, large_badge, needs_folder_walk, path_for_rect, MarkRef, Placed, Row};
use overlay::{place_stars, Layer};

pub(crate) const WM_STAR: u32 = WM_APP + 96;
/// `TIMER_STAR + pane`. One settle timer per pane on that pane's main window.
pub(crate) const TIMER_STAR: usize = 120;
const SETTLE_MS: u32 = 130;
const STAR_SUB: usize = 8;

static LAST_REAPPEAR_MS: AtomicU64 = AtomicU64::new(0);

fn slog(msg: &str) {
    let Ok(path) = std::env::var("MAGURO_STAR_LOG") else { return };
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return };
    use std::io::Write;
    let _ = writeln!(file, "{msg}");
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cover {
    menu: bool,
    drag: bool,
    peek: bool,
    rename: bool,
    sizing: bool,
}

impl Cover {
    fn blocks(self) -> bool {
        self.menu || self.drag || self.peek || self.rename || self.sizing
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refresh {
    Hide,
    HideUntilSettle,
    Show,
}

/// What the overlay should do. `Show` is the only variant that may call UIA.
fn refresh_action(show_stars: bool, ok_marks: usize, pane_shown: bool, cover: Cover, scrolling: bool) -> Refresh {
    if !show_stars || ok_marks == 0 || !pane_shown || cover.blocks() {
        Refresh::Hide
    } else if scrolling {
        Refresh::HideUntilSettle
    } else {
        Refresh::Show
    }
}

trait StarHost {
    fn hide(&mut self);
    fn arm_settle(&mut self);
    fn request_uia(&mut self);
}

fn dispatch(action: Refresh, host: &mut impl StarHost) {
    match action {
        Refresh::Hide => host.hide(),
        Refresh::HideUntilSettle => {
            host.hide();
            host.arm_settle();
        }
        Refresh::Show => host.request_uia(),
    }
}

#[derive(Default)]
struct Effect {
    did_hide: bool,
    did_settle: bool,
    did_query: bool,
}

impl StarHost for Effect {
    fn hide(&mut self) {
        self.did_hide = true;
    }
    fn arm_settle(&mut self) {
        self.did_settle = true;
    }
    fn request_uia(&mut self) {
        self.did_query = true;
    }
}

fn drive(show_stars: bool, ok_marks: usize, pane_shown: bool, cover: Cover, scrolling: bool, host: &mut impl StarHost) {
    dispatch(refresh_action(show_stars, ok_marks, pane_shown, cover, scrolling), host);
}

/// Marks in `folder` whose state is still ok. A folder mark is the folder row in its parent,
/// not a mark on the files inside it.
fn ok_in(store: &crate::marks::Store, folder: &str) -> Vec<(String, String)> {
    store
        .in_folder(folder)
        .into_iter()
        .filter(|m| m.state == "ok")
        .map(|m| (m.path.clone(), file_name(&m.path).to_string()))
        .collect()
}

fn live_ok(folder: &str) -> Vec<(String, String)> {
    crate::marks::with_store(|store| ok_in(store, folder)).unwrap_or_default()
}

struct PaneState {
    epoch: u64,
    scrolling: bool,
    drag: bool,
    rename: bool,
    shown: bool,
    retries: u8,
    folder: String,
    host: isize,
    layer: Option<Layer>,
    hooked: Vec<isize>,
    press: Option<(i32, i32)>,
    hide_at: Option<Instant>,
}

impl PaneState {
    fn new() -> Self {
        Self {
            epoch: 1,
            scrolling: false,
            drag: false,
            rename: false,
            shown: false,
            retries: 0,
            folder: String::new(),
            host: 0,
            layer: None,
            hooked: Vec::new(),
            press: None,
            hide_at: None,
        }
    }
}

struct WinState {
    menu: bool,
    peek: bool,
    sizing: bool,
    iconic: bool,
}

struct Ctl {
    panes: HashMap<(isize, usize), PaneState>,
    wins: HashMap<isize, WinState>,
}

impl Ctl {
    fn new() -> Self {
        Self { panes: HashMap::new(), wins: HashMap::new() }
    }

    fn win(&mut self, main: isize) -> &mut WinState {
        self.wins.entry(main).or_insert(WinState { menu: false, peek: false, sizing: false, iconic: false })
    }

    fn pane(&mut self, main: isize, pane: usize) -> &mut PaneState {
        self.panes.entry((main, pane)).or_insert_with(PaneState::new)
    }
}

thread_local! {
    static CTL: RefCell<Ctl> = RefCell::new(Ctl::new());
}

fn with_ctl<R>(f: impl FnOnce(&mut Ctl) -> R) -> R {
    CTL.with(|ctl| f(&mut ctl.borrow_mut()))
}

fn hwnd_bits(h: HWND) -> isize {
    h.0 as isize
}

fn current_main() -> HWND {
    app(|a| a.main)
}

fn pane_visible(pane: usize) -> bool {
    let hwnd = app(|a| a.panes.get(pane).and_then(|p| p.as_ref()).map(|p| p.hwnd).unwrap_or_default());
    let main = current_main();
    !hwnd.0.is_null() && unsafe { IsWindowVisible(hwnd).as_bool() && IsWindowVisible(main).as_bool() }
}

fn pane_folder(pane: usize) -> String {
    app(|a| a.s.paths.get(pane).cloned().unwrap_or_default())
}

fn show_stars() -> bool {
    app(|a| a.s.show_mark_stars)
}

unsafe fn shell_view(pane: usize) -> HWND {
    view_of::<IShellView>(pane).and_then(|sv| sv.GetWindow().ok()).unwrap_or_default()
}

unsafe fn items_hwnd(pane: usize) -> HWND {
    let view = shell_view(pane);
    if view.0.is_null() {
        HWND::default()
    } else {
        super::nav::list_under(view)
    }
}

unsafe fn mode_icon_dpi(pane: usize, list: HWND) -> (i32, i32, u32) {
    let (mut mode, mut icon) = (4i32, 16i32);
    if let Some(fv) = view_of::<IFolderView2>(pane) {
        let (mut m, mut z) = (FOLDERVIEWMODE(0), 0i32);
        if fv.GetViewModeAndIconSize(&mut m, &mut z).is_ok() {
            mode = m.0;
            if z > 0 {
                icon = z;
            }
        }
    }
    let dpi = if list.0.is_null() { app(|a| a.dpi) as u32 } else { GetDpiForWindow(list).max(96) };
    (mode, icon, dpi)
}

fn cover_flags(ctl: &Ctl, main: isize, pane: usize) -> Cover {
    let row = ctl.panes.get(&(main, pane));
    let win = ctl.wins.get(&main);
    Cover {
        menu: win.is_some_and(|w| w.menu),
        peek: win.is_some_and(|w| w.peek),
        sizing: win.is_some_and(|w| w.sizing),
        drag: row.is_some_and(|p| p.drag),
        rename: row.is_some_and(|p| p.rename),
    }
}

fn kill_timer(main: HWND, pane: usize) {
    unsafe { let _ = KillTimer(Some(main), TIMER_STAR + pane); }
}

fn arm_timer(main: HWND, pane: usize) {
    unsafe { let _ = SetTimer(Some(main), TIMER_STAR + pane, SETTLE_MS, None); }
}

/// Apply `effect` for one pane. `query` is filled when a UIA read should start.
struct PendingQuery {
    view: isize,
    mode: i32,
    icon: i32,
    dpi: u32,
    epoch: u64,
    names: Vec<String>,
    nav: bool,
}

unsafe fn apply_effect(main: HWND, pane: usize, effect: &Effect, nav: bool) -> Option<PendingQuery> {
    let bits = hwnd_bits(main);
    if effect.did_query {
        kill_timer(main, pane);
        let list = items_hwnd(pane);
        let view = shell_view(pane);
        let (mode, icon, dpi) = mode_icon_dpi(pane, list);
        let mut query = None;
        with_ctl(|ctl| {
            let state = ctl.pane(bits, pane);
            unsafe { ensure_layer(state, list, view, pane) };
            let names: Vec<String> = live_names(state);
            if names.is_empty() {
                state.layer.as_ref().map(|layer| layer.hide());
                return;
            }
            query = Some(PendingQuery {
                view: hwnd_bits(if !list.0.is_null() { list } else { view }),
                mode,
                icon,
                dpi,
                epoch: state.epoch,
                names,
                nav,
            });
        });
        return query;
    }
    if effect.did_hide {
        with_ctl(|ctl| {
            let state = ctl.pane(bits, pane);
            if !effect.did_settle {
                state.hide_at = None;
                state.scrolling = false;
            }
            state.layer.as_ref().map(|layer| layer.hide());
            let folder = state.folder.clone();
            if !show_stars() || live_ok(&folder).is_empty() {
                unhook(state);
            }
        });
    }
    if effect.did_settle {
        arm_timer(main, pane);
    } else if effect.did_hide {
        kill_timer(main, pane);
    }
    None
}

fn live_names(state: &PaneState) -> Vec<String> {
    live_ok(&state.folder).into_iter().map(|(_, name)| name).collect()
}

unsafe fn ensure_layer(state: &mut PaneState, list: HWND, view: HWND, pane: usize) {
    let host = hwnd_bits(list);
    if host == 0 {
        return;
    }
    let stale = state.host != host || state.layer.as_ref().is_none_or(|layer| !IsWindow(Some(layer.hwnd())).as_bool());
    if stale {
        if let Some(layer) = state.layer.take() {
            drop(layer);
        }
        state.host = host;
        state.layer = Layer::attach(list).ok();
        // False when CreateWindow could not make a layered child and the owned popup is in use.
        // Menus, peek, drag, and rename already hide that popup.
        let _popup = state.layer.as_ref().is_some_and(|layer| !layer.is_child());
    }
    hook_view(state, view, list, pane);
}

unsafe fn hook_view(state: &mut PaneState, view: HWND, list: HWND, pane: usize) {
    let mut found = Vec::new();
    if !view.0.is_null() {
        collect_shell(view, 0, &mut found);
    }
    if !list.0.is_null() {
        collect_shell(list, 0, &mut found);
    }
    state.hooked.retain(|bits| {
        let hwnd = HWND(*bits as *mut core::ffi::c_void);
        let alive = IsWindow(Some(hwnd)).as_bool();
        if !alive {
            return false;
        }
        let keep = found.iter().any(|h| hwnd_bits(*h) == *bits);
        if !keep {
            let _ = RemoveWindowSubclass(hwnd, Some(star_subproc), STAR_SUB);
        }
        keep
    });
    for hwnd in found {
        let bits = hwnd_bits(hwnd);
        if state.hooked.contains(&bits) {
            continue;
        }
        if SetWindowSubclass(hwnd, Some(star_subproc), STAR_SUB, pane).as_bool() {
            state.hooked.push(bits);
        }
    }
}

unsafe fn collect_shell(root: HWND, depth: u8, out: &mut Vec<HWND>) {
    if root.0.is_null() || depth > 4 {
        return;
    }
    let class = wnd_class(root);
    if matches!(class.as_str(), "SHELLDLL_DefView" | "DirectUIHWND" | "SysListView32") && !out.iter().any(|h| *h == root) {
        out.push(root);
    }
    let mut after = None;
    loop {
        let Ok(child) = FindWindowExW(Some(root), after, PCWSTR::null(), PCWSTR::null()) else { break };
        if child.0.is_null() {
            break;
        }
        collect_shell(child, depth + 1, out);
        after = Some(child);
    }
}

fn unhook(state: &mut PaneState) {
    for bits in state.hooked.drain(..) {
        let hwnd = HWND(bits as *mut core::ffi::c_void);
        unsafe {
            if IsWindow(Some(hwnd)).as_bool() {
                let _ = RemoveWindowSubclass(hwnd, Some(star_subproc), STAR_SUB);
            }
        }
    }
}

fn enqueue(main: HWND, pane: usize, pending: PendingQuery) {
    // The only path into the worker. `FindAllBuildCache` runs there, nowhere else.
    uia::enqueue(uia::Job {
        main: hwnd_bits(main),
        pane,
        view: pending.view,
        mode: pending.mode,
        icon: pending.icon,
        dpi: pending.dpi,
        epoch: pending.epoch,
        names: pending.names,
        nav: pending.nav,
    });
}

unsafe fn reconsider(main: HWND, pane: usize, nav: bool) {
    if main.0.is_null() || pane >= MAXP {
        return;
    }
    let show = show_stars();
    let shown = pane_visible(pane);
    let folder = pane_folder(pane);
    let marks = live_ok(&folder);
    let bits = hwnd_bits(main);
    let mut effect = Effect::default();
    with_ctl(|ctl| {
        let cover = cover_flags(ctl, bits, pane);
        let state = ctl.pane(bits, pane);
        state.folder = folder;
        state.shown = shown;
        let scrolling = state.scrolling;
        drive(show, marks.len(), shown, cover, scrolling, &mut effect);
        slog(&format!(
            "reconsider pane={pane} show={show} shown={shown} marks={} cover={cover:?} scrolling={scrolling} hide={} settle={} query={}",
            marks.len(),
            effect.did_hide,
            effect.did_settle,
            effect.did_query
        ));
    });
    if let Some(pending) = apply_effect(main, pane, &effect, nav) {
        enqueue(main, pane, pending);
    }
}

pub(crate) unsafe fn on_leaving(pane: usize) {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        let state = ctl.pane(bits, pane);
        state.epoch = state.epoch.wrapping_add(1);
        state.scrolling = false;
        state.retries = 0;
        state.hide_at = None;
        state.folder.clear();
        if let Some(layer) = state.layer.as_ref() {
            layer.hide();
        }
    });
    kill_timer(main, pane);
}

pub(crate) unsafe fn on_arrived(pane: usize) {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        let state = ctl.pane(bits, pane);
        state.scrolling = false;
        state.retries = 0;
        state.hide_at = None;
    });
    reconsider(main, pane, true);
}

pub(crate) unsafe fn on_view_mode(pane: usize) {
    on_arrived(pane);
}

pub(crate) unsafe fn on_marks_changed() {
    across_windows(|| sync_visible_force());
}

pub(crate) unsafe fn on_setting() {
    across_windows(|| sync_visible_force());
}

pub(crate) unsafe fn sync_visible() {
    let main = current_main();
    if main.0.is_null() {
        return;
    }
    let n = app(|a| a.s.panes());
    let bits = hwnd_bits(main);
    for pane in 0..MAXP {
        let now = pane < n && pane_visible(pane);
        let edge = with_ctl(|ctl| {
            let state = ctl.pane(bits, pane);
            let was = state.shown;
            state.shown = now;
            if !now && was {
                state.epoch = state.epoch.wrapping_add(1);
                state.scrolling = false;
                state.hide_at = None;
                state.layer.as_ref().map(|layer| layer.hide());
            }
            !was && now
        });
        if !now {
            kill_timer(main, pane);
        } else if edge {
            reconsider(main, pane, true);
        }
    }
}

unsafe fn sync_visible_force() {
    let main = current_main();
    if main.0.is_null() {
        return;
    }
    let n = app(|a| a.s.panes());
    for pane in 0..n {
        reconsider(main, pane, false);
    }
    sync_visible();
}

pub(crate) unsafe fn on_scroll(main: HWND, pane: usize) {
    if main.0.is_null() || pane >= MAXP {
        return;
    }
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        if cover_flags(ctl, bits, pane).blocks() {
            return;
        }
        let state = ctl.pane(bits, pane);
        state.epoch = state.epoch.wrapping_add(1);
        state.scrolling = true;
        state.hide_at = Some(Instant::now());
        state.layer.as_ref().map(|layer| layer.hide());
    });
    // The decision hides and arms the settle timer. It does not call UIA.
    let show = show_stars();
    let shown = pane_visible(pane);
    let marks = live_ok(&pane_folder(pane)).len();
    let mut effect = Effect::default();
    let blocked = with_ctl(|ctl| cover_flags(ctl, bits, pane).blocks());
    if blocked {
        return;
    }
    drive(show, marks, shown, Cover::default(), true, &mut effect);
    if let Some(pending) = apply_effect(main, pane, &effect, false) {
        enqueue(main, pane, pending);
    }
}

pub(crate) fn on_timer(main: HWND, id: usize) -> bool {
    if id < TIMER_STAR || id >= TIMER_STAR + MAXP {
        return false;
    }
    let pane = id - TIMER_STAR;
    unsafe {
        kill_timer(main, pane);
        let bits = hwnd_bits(main);
        let blocked = with_ctl(|ctl| {
            let blocked = cover_flags(ctl, bits, pane).blocks();
            if let Some(state) = ctl.panes.get_mut(&(bits, pane)) {
                state.scrolling = false;
            }
            blocked
        });
        if !blocked {
            reconsider(main, pane, false);
        }
    }
    true
}

pub(crate) unsafe fn on_enter_size() {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        ctl.win(bits).sizing = true;
        for pane in 0..MAXP {
            if let Some(state) = ctl.panes.get_mut(&(bits, pane)) {
                state.epoch = state.epoch.wrapping_add(1);
                state.hide_at = None;
                state.layer.as_ref().map(|layer| layer.hide());
            }
        }
    });
}

pub(crate) unsafe fn on_exit_size() {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| ctl.win(bits).sizing = false);
    sync_visible_force();
}

pub(crate) unsafe fn on_dpi() {
    sync_visible_force();
}

pub(crate) unsafe fn on_main_size(kind: u32) {
    let main = current_main();
    if main.0.is_null() {
        return;
    }
    let bits = hwnd_bits(main);
    let min = kind == SIZE_MINIMIZED;
    let was = with_ctl(|ctl| ctl.win(bits).iconic);
    if min && !was {
        with_ctl(|ctl| ctl.win(bits).iconic = true);
        on_enter_size();
    } else if !min && was {
        with_ctl(|ctl| {
            let win = ctl.win(bits);
            win.iconic = false;
            win.sizing = false;
        });
        sync_visible_force();
    }
}

pub(crate) fn note_menu(on: bool) {
    let main = current_main();
    if main.0.is_null() {
        return;
    }
    let bits = hwnd_bits(main);
    let changed = with_ctl(|ctl| {
        let win = ctl.win(bits);
        if win.menu == on {
            return false;
        }
        win.menu = on;
        true
    });
    if changed {
        unsafe { sync_visible_force() };
    }
}

pub(crate) fn note_peek(on: bool) {
    let main = current_main();
    if main.0.is_null() {
        return;
    }
    let bits = hwnd_bits(main);
    let changed = with_ctl(|ctl| {
        let win = ctl.win(bits);
        if win.peek == on {
            return false;
        }
        win.peek = on;
        true
    });
    if changed {
        unsafe { sync_visible_force() };
    }
}

pub(crate) unsafe fn note_all_hidden() {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        for pane in 0..MAXP {
            if let Some(state) = ctl.panes.get_mut(&(bits, pane)) {
                state.shown = false;
                state.epoch = state.epoch.wrapping_add(1);
                state.scrolling = false;
                state.hide_at = None;
                state.layer.as_ref().map(|layer| layer.hide());
            }
        }
    });
    for pane in 0..MAXP {
        kill_timer(main, pane);
    }
}

pub(crate) unsafe fn on_destroy_pane(pane: usize) {
    let main = current_main();
    let bits = hwnd_bits(main);
    with_ctl(|ctl| {
        if let Some(mut state) = ctl.panes.remove(&(bits, pane)) {
            unhook(&mut state);
            state.layer.take();
        }
    });
    kill_timer(main, pane);
}

pub(crate) fn shutdown() {
    uia::shutdown();
    with_ctl(|ctl| {
        for (_, mut state) in ctl.panes.drain() {
            unhook(&mut state);
            state.layer.take();
        }
        ctl.wins.clear();
    });
}

pub(crate) unsafe fn on_snapshot(main: HWND) {
    let bits = hwnd_bits(main);
    for snap in uia::take_ready(bits) {
        let fresh = with_ctl(|ctl| ctl.panes.get(&(bits, snap.pane)).is_some_and(|state| state.epoch == snap.epoch && !state.scrolling && state.shown));
        if !fresh {
            continue;
        }
        let blocked = with_ctl(|ctl| cover_flags(ctl, bits, snap.pane).blocks());
        if blocked || !show_stars() {
            with_ctl(|ctl| {
                ctl.panes.get(&(bits, snap.pane)).and_then(|state| state.layer.as_ref()).map(|layer| layer.hide());
            });
            continue;
        }
        let folder = with_ctl(|ctl| ctl.panes.get(&(bits, snap.pane)).map(|state| state.folder.clone()).unwrap_or_default());
        let marks = live_ok(&folder);
        if marks.is_empty() {
            with_ctl(|ctl| {
                if let Some(state) = ctl.panes.get_mut(&(bits, snap.pane)) {
                    state.layer.as_ref().map(|layer| layer.hide());
                    unhook(state);
                }
            });
            continue;
        }
        paint_snap(main, &snap, &marks);
    }
}

unsafe fn paint_snap(main: HWND, snap: &uia::Snap, marks: &[(String, String)]) {
    let displays: Vec<&str> = snap.hits.iter().map(|hit| hit.name.as_str()).collect();
    let mark_names: Vec<&str> = marks.iter().map(|(_, name)| name.as_str()).collect();
    let walk = needs_folder_walk(&displays, &mark_names);
    let placed = if walk { locate_items(snap.pane) } else { Vec::new() };
    let placed_ref: Vec<Placed> = placed.iter().map(|(d, p, x, y)| Placed { display: d, path: p, x: *x, y: *y }).collect();
    let mut owned: Vec<(String, Option<String>)> = Vec::new();
    for hit in &snap.hits {
        let path = if walk { path_for_rect(&hit.name, hit.item, &placed_ref).map(|p| p.to_string()) } else { None };
        owned.push((hit.name.clone(), path));
    }
    let rows: Vec<Row> = owned.iter().map(|(d, p)| Row { display: d, path: p.as_deref() }).collect();
    let mark_refs: Vec<MarkRef> = marks.iter().map(|(p, n)| MarkRef { path: p, name: n }).collect();
    let mask = assign_stars(&rows, &mark_refs);
    let large = large_badge(snap.mode, snap.icon);
    let mut icons = Vec::new();
    for (hit, on) in snap.hits.iter().zip(mask) {
        if !on {
            continue;
        }
        let icon = if icon_on_row(hit.icon, hit.item) {
            hit.icon
        } else {
            estimate_icon(hit.item, snap.mode, snap.icon, snap.dpi)
        };
        slog(&format!("star name={} icon={},{},{},{} item={},{},{},{} image={}", hit.name, icon.x, icon.y, icon.w, icon.h, hit.item.x, hit.item.y, hit.item.w, hit.item.h, hit.image));
        if !icon.is_degenerate() {
            icons.push((icon, large));
        }
    }
    let stars = place_stars(&icons, snap.viewport, snap.dpi);
    let bits = hwnd_bits(main);
    let retry = with_ctl(|ctl| {
        let Some(state) = ctl.panes.get_mut(&(bits, snap.pane)) else { return false };
        if state.epoch != snap.epoch {
            return false;
        }
        let list = unsafe { items_hwnd(snap.pane) };
        let view = unsafe { shell_view(snap.pane) };
        unsafe { ensure_layer(state, list, view, snap.pane) };
        slog(&format!(
            "paint pane={} hits={} stars={} viewport={},{},{},{} child={} note={}",
            snap.pane,
            snap.hits.len(),
            stars.len(),
            snap.viewport.x,
            snap.viewport.y,
            snap.viewport.w,
            snap.viewport.h,
            state.layer.as_ref().is_some_and(|layer| layer.is_child()),
            snap.note
        ));
        // The shell list often has no UIA tree on the first navigation callback.
        // A short bounded retry is not a standing poll: it stops once items appear.
        if snap.hits.is_empty() && !marks.is_empty() && state.retries < 15 {
            state.retries += 1;
            state.layer.as_ref().map(|layer| layer.hide());
            return true;
        }
        if stars.is_empty() {
            state.layer.as_ref().map(|layer| layer.hide());
            return false;
        }
        state.retries = 0;
        let hide_at = state.hide_at.take();
        if let Some(layer) = state.layer.as_mut() {
            layer.present(snap.viewport, &stars);
        }
        if let Some(t) = hide_at {
            let ms = t.elapsed().as_millis() as u64;
            LAST_REAPPEAR_MS.store(ms, Ordering::Relaxed);
            if let Ok(path) = std::env::var("MAGURO_STAR_TIMING") {
                let _ = std::fs::write(path, format!("{ms}\n"));
            }
        }
        false
    });
    if retry {
        arm_timer(main, snap.pane);
    }
}

unsafe fn locate_items(pane: usize) -> Vec<(String, String, i32, i32)> {
    let Some(fv) = view_of::<IFolderView2>(pane) else { return Vec::new() };
    let Ok(folder_view) = fv.cast::<IFolderView>() else { return Vec::new() };
    let view = shell_view(pane);
    let mut origin = POINT { x: 0, y: 0 };
    if !view.0.is_null() {
        let _ = ClientToScreen(view, &mut origin);
    }
    let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(0).clamp(0, 8_000);
    let mut out = Vec::new();
    for k in 0..n {
        let Ok(item) = fv.GetItem::<IShellItem>(k) else { continue };
        let display = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok().map(|p| pwstr(p)).unwrap_or_default();
        if display.is_empty() {
            continue;
        }
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok().map(|p| pwstr(p)).unwrap_or_default();
        let Ok(pidl) = fv.Item(k) else { continue };
        let pt = folder_view.GetItemPosition(pidl).unwrap_or_default();
        CoTaskMemFree(Some(pidl as *const _));
        out.push((display, path, origin.x + pt.x, origin.y + pt.y));
    }
    out
}

pub(crate) unsafe extern "system" fn star_subproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, pane: usize) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { star_sub_inner(h, msg, wp, lp, pane) }))
}

unsafe fn star_sub_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, pane: usize) -> LRESULT {
    let main = GetAncestor(h, GA_ROOT);
    if !main.0.is_null() && pane < MAXP {
        match msg {
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_VSCROLL | WM_HSCROLL => on_scroll(main, pane),
            WM_KEYDOWN if scroll_key(wp.0 as u16) && !rename_open(main, pane) => on_scroll(main, pane),
            WM_SIZE | WM_MOVE => {
                let sizing = with_ctl(|ctl| ctl.wins.get(&hwnd_bits(main)).is_some_and(|w| w.sizing));
                if !sizing {
                    on_scroll(main, pane);
                }
            }
            WM_LBUTTONDOWN => note_press(main, pane, lp, true),
            WM_MOUSEMOVE => note_move(main, pane, wp, lp),
            WM_LBUTTONUP => note_press(main, pane, lp, false),
            WM_PARENTNOTIFY => note_parent(main, pane, wp, lp),
            WM_ENTERMENULOOP => note_menu(true),
            WM_EXITMENULOOP => note_menu(false),
            _ => {}
        }
    }
    DefSubclassProc(h, msg, wp, lp)
}

fn scroll_key(vk: u16) -> bool {
    matches!(vk, v if v == VK_PRIOR.0 || v == VK_NEXT.0 || v == VK_HOME.0 || v == VK_END.0 || v == VK_LEFT.0 || v == VK_RIGHT.0 || v == VK_UP.0 || v == VK_DOWN.0)
}

fn rename_open(main: HWND, pane: usize) -> bool {
    with_ctl(|ctl| ctl.panes.get(&(hwnd_bits(main), pane)).is_some_and(|state| state.rename))
}

fn note_press(main: HWND, pane: usize, lp: LPARAM, down: bool) {
    let bits = hwnd_bits(main);
    if down {
        let x = (lp.0 & 0xffff) as i16 as i32;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
        with_ctl(|ctl| ctl.pane(bits, pane).press = Some((x, y)));
        return;
    }
    let was_drag = with_ctl(|ctl| {
        let state = ctl.pane(bits, pane);
        state.press = None;
        let drag = state.drag;
        state.drag = false;
        drag
    });
    if was_drag {
        unsafe { reconsider(main, pane, false) };
    }
}

fn note_move(main: HWND, pane: usize, wp: WPARAM, lp: LPARAM) {
    if wp.0 & MK_LBUTTON.0 as usize == 0 {
        return;
    }
    let x = (lp.0 & 0xffff) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG).max(4), GetSystemMetrics(SM_CYDRAG).max(4)) };
    let start = with_ctl(|ctl| {
        let state = ctl.pane(hwnd_bits(main), pane);
        if state.drag {
            return None;
        }
        state.press
    });
    let Some((sx, sy)) = start else { return };
    if (x - sx).abs() < cx && (y - sy).abs() < cy {
        return;
    }
    with_ctl(|ctl| {
        let state = ctl.pane(hwnd_bits(main), pane);
        state.drag = true;
        state.epoch = state.epoch.wrapping_add(1);
        state.hide_at = None;
        state.layer.as_ref().map(|layer| layer.hide());
    });
    kill_timer(main, pane);
}

fn note_parent(main: HWND, pane: usize, wp: WPARAM, lp: LPARAM) {
    let ev = (wp.0 & 0xffff) as u32;
    let child = HWND(lp.0 as *mut core::ffi::c_void);
    if ev == WM_CREATE {
        let class = wnd_class(child);
        if rename_edit_class(&class) {
            set_rename(main, pane, true);
        }
    } else if ev == WM_DESTROY {
        let open = unsafe { pane_shell_label_open(pane) };
        if !open {
            set_rename(main, pane, false);
        }
    }
}

fn set_rename(main: HWND, pane: usize, on: bool) {
    let changed = with_ctl(|ctl| {
        let state = ctl.pane(hwnd_bits(main), pane);
        if state.rename == on {
            return false;
        }
        state.rename = on;
        if on {
            state.epoch = state.epoch.wrapping_add(1);
            state.hide_at = None;
            state.layer.as_ref().map(|layer| layer.hide());
        }
        true
    });
    if changed && !on {
        unsafe { reconsider(main, pane, false) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::{FileIdentity, MarksLock, Store};
    use std::sync::{Arc, Mutex};

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_store() -> (TempDir, Store) {
        let dir = std::env::temp_dir().join(format!("maguro-star-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load_in(&dir, MarksLock::Shared(Arc::new(Mutex::new(()))));
        (TempDir(dir), store)
    }

    #[test]
    fn switched_off_or_empty_or_covered_does_not_request_uia() {
        let before = uia::QUERIES.load(Ordering::Relaxed);
        let cases = [
            (false, 3usize, true, Cover::default(), false),
            (true, 0, true, Cover::default(), false),
            (true, 2, false, Cover::default(), false),
            (true, 2, true, Cover { menu: true, ..Cover::default() }, false),
            (true, 2, true, Cover { peek: true, ..Cover::default() }, false),
            (true, 2, true, Cover { rename: true, ..Cover::default() }, false),
        ];
        for (show, marks, shown, cover, scrolling) in cases {
            let mut effect = Effect::default();
            drive(show, marks, shown, cover, scrolling, &mut effect);
            assert!(!effect.did_query, "{show} {marks} {shown} {cover:?}");
            assert!(effect.did_hide);
            assert!(!effect.did_settle);
        }
        assert_eq!(uia::QUERIES.load(Ordering::Relaxed), before);
    }

    #[test]
    fn scroll_hides_then_one_query_after_settle() {
        let mut effect = Effect::default();
        drive(true, 2, true, Cover::default(), true, &mut effect);
        assert!(effect.did_hide && effect.did_settle && !effect.did_query);
        let mut effect = Effect::default();
        drive(true, 2, true, Cover::default(), false, &mut effect);
        assert!(effect.did_query && !effect.did_hide && !effect.did_settle);
    }

    #[test]
    fn folder_mark_does_not_mark_the_children_and_an_empty_folder_skips_uia() {
        let before = uia::QUERIES.load(Ordering::Relaxed);
        let (_dir, mut store) = temp_store();
        store.add_with(r"C:\data\pics", FileIdentity { dir: true, ..FileIdentity::default() }, "2020-01-01T00:00:00Z").unwrap();
        store.add_with(r"C:\data\pics\a.txt", FileIdentity::default(), "2020-01-01T00:00:00Z").unwrap();
        let parent: Vec<_> = ok_in(&store, r"C:\data").into_iter().map(|(_, n)| n).collect();
        let child: Vec<_> = ok_in(&store, r"C:\data\pics").into_iter().map(|(_, n)| n).collect();
        assert_eq!(parent, vec!["pics".to_string()]);
        assert_eq!(child, vec!["a.txt".to_string()]);
        let mut effect = Effect::default();
        drive(true, ok_in(&store, r"C:\empty").len(), true, Cover::default(), false, &mut effect);
        assert!(!effect.did_query);
        assert_eq!(uia::QUERIES.load(Ordering::Relaxed), before);
    }
}
