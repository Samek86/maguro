//! Customize window in the shape of Finder's toolbar editor.
//! The frame, cards, and buttons are the shared Maguro chrome.
//! The top band is the bar being edited. The well below is the palette.
//! The window is resizable. The bar scrolls sideways and the palette scrolls down
//! when the icons do not fit. Hover tips name the icon under the pointer.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

use windows::Win32::UI::Controls::{TTF_ABSOLUTE, TTF_TRACK, TTM_GETBUBBLESIZE, TTM_POP, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;

use super::{ask_confirm, body_font, draw_line_icon, fill, in_rect, meta_font, paint_button, paint_card, paint_card_fill, paint_meta, paint_segment_group, paint_window, style_popup_chrome, visual_from_pointer, ButtonKind, Confirm, FocusRing, SegmentCell, BTN_H_FOOTER};
use crate::imports::*;
use crate::toolbar::{Action, Doc};
use crate::ui::tokens::px;

const HIT_RESET: i32 = 1;
const HIT_DONE: i32 = 2;
const HIT_AUTO: i32 = 3;
const HIT_MANUAL: i32 = 4;
const HIT_PREVIEW: i32 = 5;
const HIT_CHANGE: i32 = 6;
const BAR_BASE: i32 = 100;
const WELL_BASE: i32 = 200;
const ICON_BASE: i32 = 1000;
const TIMER_EDGE: usize = 1;
/// Client minimum, in DIP. `WM_GETMINMAXINFO` turns this into an outer size.
const EDIT_MIN_W: i32 = 420;
const EDIT_MIN_H: i32 = 360;
/// Posted after the editor is on screen. Checking a saved path here must not run inside `ShowWindow`.
const WM_TARGET_CHECK: u32 = WM_APP + 58;

static LIVE: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Well {
    Action(Action),
    Separator,
    /// Always on the palette. Each drop asks for a folder and can be repeated.
    OpenFolder,
    /// Always on the palette. Each drop asks for a file and can be repeated.
    OpenFile,
}

/// Commands that are not already a pane-header button or a right-hand icon.
fn catalog() -> &'static [Well] {
    &[
        Well::Separator,
        Well::Action(Action::NewFolder),
        Well::Action(Action::NewFile),
        Well::Action(Action::DeleteSelection),
        Well::Action(Action::Paste),
        Well::Action(Action::RenameInPlace),
        Well::Action(Action::BatchRename),
        Well::Action(Action::CopyPath),
        Well::Action(Action::InvertSelection),
        Well::Action(Action::ViewMode),
        Well::Action(Action::Properties),
        Well::Action(Action::Mark),
        Well::Action(Action::NewTab),
        Well::Action(Action::CloseTab),
        Well::Action(Action::DuplicateTab),
        Well::Action(Action::ShowHidden),
        Well::Action(Action::QuickLook),
        Well::Action(Action::PreviewPane),
        Well::Action(Action::SplitRight),
        Well::Action(Action::SplitBelow),
        Well::OpenFolder,
        Well::OpenFile,
        Well::Action(Action::PaneColor),
        Well::Action(Action::TabColor),
        Well::Action(Action::ClosePane),
        Well::Action(Action::Terminal),
        Well::Action(Action::TerminalAdmin),
    ]
}

/// Commands already on the bar stay off the palette. A separator can be added again.
/// Open folder and open file stay, so the bar can hold more than one target.
fn visible_well(doc: &Doc) -> Vec<Well> {
    catalog()
        .iter()
        .copied()
        .filter(|item| match item {
            Well::Separator | Well::OpenFolder | Well::OpenFile => true,
            Well::Action(action) => !doc.has_action(*action),
        })
        .collect()
}

/// One bar entry as the editor paints it. Built from the saved document, including targets.
struct EditorRow {
    label: String,
    tip: String,
    icon: Option<crate::ui::LineIcon>,
    path: Option<String>,
    missing: bool,
}

fn editor_rows(doc: &Doc) -> Vec<EditorRow> {
    (0..doc.item_count())
        .map(|i| EditorRow {
            label: doc.label_at(i),
            tip: doc.item_tip(i),
            icon: doc.glyph_at(i),
            path: doc.target_path(i).map(str::to_string),
            missing: doc.target_missing(i),
        })
        .collect()
}

fn overlaps(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn contains_rect(outer: RECT, inner: RECT) -> bool {
    inner.left >= outer.left && inner.top >= outer.top && inner.right <= outer.right && inner.bottom <= outer.bottom
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Bar(usize),
    Well(Well),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drop {
    None,
    Remove(usize),
    Move { from: usize, to: usize },
    Insert(Well, usize),
}

/// Where a drag lands. `at` is a gap index on the bar, from 0 through `count`.
fn plan_drop(grab: Grab, on_bar: bool, at: usize) -> Drop {
    match grab {
        Grab::Bar(from) if !on_bar => Drop::Remove(from),
        Grab::Bar(from) if from == at || from + 1 == at => Drop::None,
        Grab::Bar(from) => Drop::Move { from, to: at },
        Grab::Well(_) if !on_bar => Drop::None,
        Grab::Well(item) => Drop::Insert(item, at),
    }
}

fn apply_drop(doc: &mut Doc, drop: Drop) {
    match drop {
        Drop::None => {}
        Drop::Remove(i) => {
            doc.remove_at(i);
        }
        Drop::Move { from, to } => {
            doc.move_item(from, to);
        }
        Drop::Insert(Well::Action(action), at) => doc.insert_action(at, action),
        Drop::Insert(Well::Separator, at) => doc.insert_separator(at),
        // A target needs a path from the picker. Cancel leaves the bar unchanged.
        Drop::Insert(Well::OpenFolder | Well::OpenFile, _) => {}
    }
}

fn scroll_max(content: i32, view: i32) -> i32 {
    (content - view).max(0)
}

fn clamp_scroll(offset: i32, max_off: i32) -> i32 {
    offset.clamp(0, max_off.max(0))
}

/// Pull `lo..hi` into the visible span. `offset` is the content pixel at the view's start.
fn reveal_scroll(offset: i32, view: i32, content: i32, lo: i32, hi: i32) -> i32 {
    let max_off = scroll_max(content, view);
    let mut s = clamp_scroll(offset, max_off);
    if view <= 0 {
        return s;
    }
    if lo < s {
        s = lo.max(0);
    }
    if hi > s.saturating_add(view) {
        s = hi.saturating_sub(view);
    }
    clamp_scroll(s, max_off)
}

/// Wheel up (positive notches) moves toward the start of the content.
fn wheel_scroll(offset: i32, max_off: i32, notches: i32, step: i32) -> i32 {
    clamp_scroll(offset.saturating_sub(notches.saturating_mul(step)), max_off)
}

/// Wheel right (positive notches) reveals the side that was clipped.
fn hwheel_scroll(offset: i32, max_off: i32, notches: i32, step: i32) -> i32 {
    clamp_scroll(offset.saturating_add(notches.saturating_mul(step)), max_off)
}

/// Signed step while the pointer sits in the edge zone of a span. Outside the slop, zero.
fn edge_delta(origin: i32, span: i32, pos: i32, zone: i32, step: i32) -> i32 {
    if span <= 0 || zone <= 0 || step <= 0 {
        return 0;
    }
    let end = origin + span;
    if pos < origin - zone || pos > end + zone {
        return 0;
    }
    if pos < origin + zone {
        -step
    } else if pos > end - zone {
        step
    } else {
        0
    }
}

/// Client size at which the bar, the change-target row, every palette card, the icon
/// settings, and the footer all sit fully inside the window and do not overlap.
///
/// Width is the footer row (restore, full-size icon segments, preview, done). Height is
/// the smallest size `edit_layout` accepts for the whole catalog, with a target selected
/// so the change row and the icon settings are both present.
fn min_client(dpi: i32) -> (i32, i32) {
    let w = footer_min_width(dpi).max(px(dpi, EDIT_MIN_W));
    let n = catalog().len();
    let mut lo = px(dpi, EDIT_MIN_H);
    let mut hi = lo + palette_height_for(n, w, dpi) + px(dpi, 160);
    if !client_fits(w, hi, dpi, n) {
        return (w, hi);
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if client_fits(w, mid, dpi, n) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    (w, lo)
}

fn footer_min_width(dpi: i32) -> i32 {
    let pad = px(dpi, 12);
    let gap = px(dpi, 6);
    let icons = px(dpi, 72) * 2 + px(dpi, 8) + px(dpi, BTN_H_FOOTER);
    pad * 2 + px(dpi, 128) + px(dpi, 88) + gap * 2 + icons + px(dpi, 4)
}

fn palette_height_for(n: usize, client_w: i32, dpi: i32) -> i32 {
    let inner = (client_w - px(dpi, 12) * 2).max(0);
    well_content_height(n, well_cols(inner, dpi), dpi)
}

/// The tallest editor: a selected target (change row + icon settings) and every palette card.
fn client_fits(w: i32, h: i32, dpi: i32, n: usize) -> bool {
    let lay = edit_layout(w, h, dpi, true, true);
    let client = RECT { left: 0, top: 0, right: w, bottom: h };
    let bands = [lay.bar, lay.hint, lay.change, lay.well, lay.icons, lay.reset, lay.done];
    if bands.iter().any(|r| !contains_rect(client, *r) || r.right <= r.left || r.bottom <= r.top) {
        return false;
    }
    for (i, a) in bands.iter().enumerate() {
        for b in bands.iter().skip(i + 1) {
            if overlaps(*a, *b) {
                return false;
            }
        }
    }
    let (auto, manual, preview) = icon_controls(lay.icons, dpi);
    if auto.right - auto.left < px(dpi, 72) || manual.right - manual.left < px(dpi, 72) || preview.right <= preview.left || !contains_rect(lay.icons, auto) || !contains_rect(lay.icons, manual) || !contains_rect(lay.icons, preview) {
        return false;
    }
    let (body, content) = well_body(lay.well, n, dpi);
    let view = (lay.well.bottom - lay.well.top).max(0);
    if content > view {
        return false;
    }
    well_slots(body, n, dpi, 0).iter().all(|cell| contains_rect(lay.well, *cell) && contains_rect(client, *cell) && !overlaps(*cell, lay.reset) && !overlaps(*cell, lay.done) && !overlaps(*cell, lay.icons))
}

fn open_client(saved: Option<(i32, i32)>, dpi: i32) -> (i32, i32) {
    let (min_w, min_h) = min_client(dpi);
    match saved {
        Some((w, h)) => (w.max(min_w), h.max(min_h)),
        None => (px(dpi, 560).max(min_w), px(dpi, 520).max(min_h)),
    }
}

fn edit_style() -> WINDOW_STYLE {
    WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_CLIPCHILDREN
}

struct EditLay {
    bar: RECT,
    hint: RECT,
    change: RECT,
    icons: RECT,
    well: RECT,
    reset: RECT,
    done: RECT,
}

fn edit_layout(w: i32, h: i32, dpi: i32, show_icons: bool, show_change: bool) -> EditLay {
    let pad = px(dpi, 12);
    let gap = px(dpi, 6);
    let bar_h = px(dpi, 48);
    let btn_h = px(dpi, BTN_H_FOOTER);
    let hint_h = if show_change { btn_h } else { px(dpi, 18) };
    let foot = h - pad - btn_h;
    let done_w = px(dpi, 88);
    let reset_w = px(dpi, 128);
    let hint_top = pad + bar_h + gap;
    let change_w = px(dpi, 148);
    let change = if show_change { RECT { left: (w - pad - change_w).max(pad), top: hint_top, right: w - pad, bottom: hint_top + hint_h } } else { RECT::default() };
    let hint_right = if show_change { (change.left - gap).max(pad) } else { w - pad };
    let hint = RECT { left: pad, top: hint_top, right: hint_right, bottom: hint_top + hint_h };
    let icons = if show_icons { RECT { left: pad + reset_w + gap, top: foot, right: (w - pad - done_w - gap).max(pad), bottom: foot + btn_h } } else { RECT::default() };
    let well_top = hint.bottom + gap;
    EditLay { bar: RECT { left: pad, top: pad, right: w - pad, bottom: pad + bar_h }, hint, change, icons, well: RECT { left: pad, top: well_top, right: w - pad, bottom: (foot - gap).max(well_top) }, reset: RECT { left: pad, top: foot, right: pad + reset_w, bottom: foot + btn_h }, done: RECT { left: (w - pad - done_w).max(pad), top: foot, right: w - pad, bottom: foot + btn_h } }
}

fn icon_slots(band: RECT, n: usize, dpi: i32) -> Vec<RECT> {
    if n == 0 || band.bottom <= band.top {
        return Vec::new();
    }
    let side = px(dpi, 22);
    let gap = px(dpi, 2);
    let cols = ((band.right - band.left + gap) / (side + gap).max(1)).max(1);
    (0..n)
        .map(|i| {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            let left = band.left + col * (side + gap);
            let top = band.top + row * (side + gap);
            RECT { left, top, right: left + side, bottom: top + side }
        })
        .collect()
}

fn bar_pad(dpi: i32) -> i32 {
    px(dpi, 8)
}

fn bar_metrics(dpi: i32) -> (i32, i32, i32) {
    (px(dpi, 30), px(dpi, 24), px(dpi, 2))
}

/// Width of the unscrolled row, including the padding on both ends.
fn bar_content_width(n: usize, dpi: i32) -> i32 {
    if n == 0 {
        return 0;
    }
    let (btn_w, _, gap) = bar_metrics(dpi);
    let pad = bar_pad(dpi);
    pad + n as i32 * btn_w + (n as i32 - 1) * gap + pad
}

/// Item span in content pixels. The view's left edge shows content pixel `scroll`.
fn bar_item_bounds(i: usize, dpi: i32) -> (i32, i32) {
    let (btn_w, _, gap) = bar_metrics(dpi);
    let left = bar_pad(dpi) + i as i32 * (btn_w + gap);
    (left, left + btn_w)
}

fn bar_slots(bar: RECT, n: usize, dpi: i32, scroll: i32) -> Vec<RECT> {
    let (btn_w, btn_h, gap) = bar_metrics(dpi);
    let y = bar.top + (bar.bottom - bar.top - btn_h).max(0) / 2;
    let mut x = bar.left + bar_pad(dpi) - scroll;
    (0..n)
        .map(|_| {
            let r = RECT { left: x, top: y, right: x + btn_w, bottom: y + btn_h };
            x += btn_w + gap;
            r
        })
        .collect()
}

fn reveal_bar(scroll: i32, bar: RECT, index: usize, n: usize, dpi: i32) -> i32 {
    if index >= n {
        return scroll;
    }
    let view = (bar.right - bar.left).max(0);
    let (lo, hi) = bar_item_bounds(index, dpi);
    reveal_scroll(scroll, view, bar_content_width(n, dpi), lo, hi)
}

fn insert_at(slots: &[RECT], x: i32) -> usize {
    for (i, r) in slots.iter().enumerate() {
        if x < (r.left + r.right) / 2 {
            return i;
        }
    }
    slots.len()
}

fn well_metrics(dpi: i32) -> (i32, i32, i32) {
    (px(dpi, 86), px(dpi, 64), px(dpi, 4))
}

fn well_cols(width: i32, dpi: i32) -> i32 {
    let (cw, _, gap) = well_metrics(dpi);
    ((width + gap) / (cw + gap).max(1)).max(1)
}

fn well_content_height(n: usize, cols: i32, dpi: i32) -> i32 {
    if n == 0 || cols <= 0 {
        return 0;
    }
    let (_, ch, gap) = well_metrics(dpi);
    let rows = (n as i32 + cols - 1) / cols;
    rows * ch + (rows - 1).max(0) * gap
}

/// Card area, and the content height. A tall palette keeps a gutter for the scrollbar.
fn well_body(well: RECT, n: usize, dpi: i32) -> (RECT, i32) {
    let width = (well.right - well.left).max(0);
    let height = well_content_height(n, well_cols(width, dpi), dpi);
    let view = (well.bottom - well.top).max(0);
    if height <= view {
        return (well, height);
    }
    let body = RECT { right: (well.right - px(dpi, 10)).max(well.left), ..well };
    let cols = well_cols((body.right - body.left).max(0), dpi);
    (body, well_content_height(n, cols, dpi))
}

fn well_slots(well: RECT, n: usize, dpi: i32, scroll: i32) -> Vec<RECT> {
    let (cw, ch, gap) = well_metrics(dpi);
    let cols = well_cols((well.right - well.left).max(0), dpi);
    (0..n)
        .map(|i| {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            let left = well.left + col * (cw + gap);
            let top = well.top + row * (ch + gap) - scroll;
            RECT { left, top, right: left + cw, bottom: top + ch }
        })
        .collect()
}

#[derive(Clone, Copy)]
struct Thumb {
    track: RECT,
    thumb: RECT,
}

fn hthumb(bar: RECT, content: i32, view: i32, offset: i32, dpi: i32) -> Option<Thumb> {
    let max_off = scroll_max(content, view);
    if max_off <= 0 || view <= 0 {
        return None;
    }
    let thick = px(dpi, 4).max(3);
    let inset = px(dpi, 4);
    let track = RECT { left: bar.left + inset, top: bar.bottom - thick - px(dpi, 3), right: bar.right - inset, bottom: bar.bottom - px(dpi, 3) };
    let span = (track.right - track.left).max(0);
    if span <= 0 {
        return None;
    }
    let len = crate::thumb_len(content, view, span, px(dpi, 16));
    let x = track.left + crate::thumb_offset(offset, max_off, (span - len).max(0));
    Some(Thumb { track, thumb: RECT { left: x, top: track.top, right: x + len, bottom: track.bottom } })
}

fn vthumb(well: RECT, body: RECT, content: i32, view: i32, offset: i32, dpi: i32) -> Option<Thumb> {
    let max_off = scroll_max(content, view);
    if max_off <= 0 || view <= 0 || body.right >= well.right {
        return None;
    }
    let inset = px(dpi, 3);
    let track = RECT { left: body.right + px(dpi, 2), top: well.top + inset, right: (well.right - px(dpi, 2)).max(body.right + px(dpi, 2) + 1), bottom: well.bottom - inset };
    let span = (track.bottom - track.top).max(0);
    if span <= 0 || track.right <= track.left {
        return None;
    }
    let len = crate::thumb_len(content, view, span, px(dpi, 16));
    let y = track.top + crate::thumb_offset(offset, max_off, (span - len).max(0));
    Some(Thumb { track, thumb: RECT { left: track.left, top: y, right: track.right, bottom: y + len } })
}

fn well_label(item: Well) -> String {
    match item {
        Well::Action(action) => crate::tr(crate::toolbar::label_key(action)).to_string(),
        Well::Separator => crate::tr(crate::Key::ToolbarSeparator).to_string(),
        Well::OpenFolder => crate::tr(crate::Key::ToolbarOpenFolder).to_string(),
        Well::OpenFile => crate::tr(crate::Key::ToolbarOpenFile).to_string(),
    }
}

fn well_icon(item: Well) -> Option<crate::ui::LineIcon> {
    match item {
        Well::Action(action) => Some(crate::toolbar::drawn_icon(action, &crate::toolbar::IconChoice::Auto)),
        Well::OpenFolder => Some(crate::ui::LineIcon::OpenFolder),
        Well::OpenFile => Some(crate::ui::LineIcon::PinFile),
        Well::Separator => None,
    }
}

#[derive(Clone, Copy)]
struct ThumbDrag {
    vertical: bool,
    grab: i32,
}

struct View {
    doc: Doc,
    /// Selected bar item. Its icon can be changed.
    sel: Option<usize>,
    hot: i32,
    down: i32,
    grab: Option<Grab>,
    drag: bool,
    origin: (i32, i32),
    insert: Option<usize>,
    /// Content pixels hidden to the left of the bar.
    bar_scroll: i32,
    /// Content pixels hidden above the palette.
    well_scroll: i32,
    last: (i32, i32),
    thumb: Option<ThumbDrag>,
    tip: Tip,
    /// Parallel to `doc` items. Filled after the window is shown, so opening does not stat.
    missing: Vec<bool>,
}

fn view(h: HWND) -> Option<*mut View> {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut View;
    if p.is_null() {
        None
    } else {
        Some(p)
    }
}

pub(crate) unsafe fn open(owner: HWND) {
    let live = LIVE.load(Ordering::Relaxed);
    if live != 0 {
        let existing = HWND(live as *mut _);
        if IsWindow(Some(existing)).as_bool() {
            let _ = SetForegroundWindow(existing);
            return;
        }
    }
    ensure_class();
    let dpi = dpi_of(owner);
    let (client_w, client_h) = open_client(remembered_size(), dpi);
    let (w, h) = outer_size(client_w, client_h, dpi);
    let (x, y) = center_on(owner, w, h);
    let Ok(hwnd) = CreateWindowExW(WS_EX_DLGMODALFRAME, w!("MaguroToolbarEdit"), &HSTRING::from(crate::tr(crate::Key::ToolbarEdit)), edit_style(), x, y, w, h, Some(owner), None, None, None) else {
        return;
    };
    let dlg = Box::new(View {
        doc: Doc::current(),
        sel: None,
        hot: 0,
        down: 0,
        grab: None,
        drag: false,
        origin: (0, 0),
        insert: None,
        bar_scroll: 0,
        well_scroll: 0,
        last: (0, 0),
        thumb: None,
        tip: tip_new(),
        missing: Vec::new(),
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(dlg) as isize);
    style_popup_chrome(hwnd);
    crate::note_side(hwnd);
    LIVE.store(hwnd.0 as isize, Ordering::Relaxed);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetFocus(Some(hwnd));
    let _ = SetForegroundWindow(hwnd);
    // Presence is resolved after the window is visible. Doing it inside ShowWindow's paint
    // stalls on a disconnected drive and the editor never appears.
    let _ = PostMessageW(Some(hwnd), WM_TARGET_CHECK, WPARAM(0), LPARAM(0));
}

fn dpi_of(owner: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(owner) as i32 };
    if dpi > 0 {
        dpi
    } else {
        96
    }
}

fn center_on(owner: HWND, w: i32, h: i32) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        if GetWindowRect(owner, &mut r).is_ok() && r.right > r.left {
            return ((r.left + r.right - w) / 2, (r.top + r.bottom - h) / 2);
        }
    }
    (80, 80)
}

unsafe fn outer_size(client_w: i32, client_h: i32, dpi: i32) -> (i32, i32) {
    sized_frame(client_w, client_h, dpi, edit_style())
}

unsafe fn sized_frame(client_w: i32, client_h: i32, dpi: i32, style: WINDOW_STYLE) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: client_w, bottom: client_h };
    if AdjustWindowRectExForDpi(&mut r, style, false, WS_EX_DLGMODALFRAME, dpi.max(96) as u32).is_ok() {
        ((r.right - r.left).max(client_w), (r.bottom - r.top).max(client_h))
    } else {
        (client_w + 16, client_h + 40)
    }
}

fn ensure_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else {
            return;
        };
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: m.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MaguroToolbarEdit"),
            ..Default::default()
        });
    });
}

unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::catch_wnd(AssertUnwindSafe(|| unsafe { wnd_inner(h, msg, wp, lp) }))
}

unsafe fn wnd_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(h);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            on_move(h, point(lp));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            on_down(h, point(lp));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            on_up(h, point(lp));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            on_wheel(h, wp, lp, false);
            LRESULT(0)
        }
        WM_MOUSEHWHEEL => {
            on_wheel(h, wp, lp, true);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            on_leave(h);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_EDGE => {
            edge_tick(h);
            LRESULT(0)
        }
        WM_SIZE => {
            on_size(h, wp.0 as u32);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let dpi = GetDpiForWindow(h).max(96) as i32;
            let (cw, ch) = min_client(dpi);
            let (w, ht) = outer_size(cw, ch, dpi);
            let m = &mut *(lp.0 as *mut MINMAXINFO);
            m.ptMinTrackSize.x = w;
            m.ptMinTrackSize.y = ht;
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            end_capture(h);
            LRESULT(0)
        }
        WM_TARGET_CHECK => {
            refresh_missing(h);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wp.0 == VK_ESCAPE.0 as usize {
                close(h);
            } else if wp.0 == VK_RETURN.0 as usize {
                finish(h);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            close(h);
            LRESULT(0)
        }
        WM_DESTROY => {
            drop_view(h);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 as i16) as i32, ((lp.0 >> 16) as i16) as i32)
}

fn icon_controls(band: RECT, dpi: i32) -> (RECT, RECT, RECT) {
    let h = (band.bottom - band.top).max(0);
    let gap = px(dpi, 8);
    let width = (band.right - band.left).max(0);
    let preview_w = h.min(width);
    let seg = px(dpi, 72).min(((width - gap - preview_w) / 2).max(0));
    let auto = RECT { left: band.left, top: band.top, right: band.left + seg, bottom: band.top + h };
    let manual = RECT { left: auto.right, top: band.top, right: auto.right + seg, bottom: band.top + h };
    let left = (manual.right + gap).min(band.right);
    let preview = RECT { left, top: band.top, right: (left + preview_w).min(band.right), bottom: band.top + h };
    (auto, manual, preview)
}

fn hit_cells(clip: RECT, cells: &[RECT], x: i32, y: i32) -> Option<usize> {
    if !in_rect(clip, x, y) {
        return None;
    }
    cells.iter().enumerate().find(|(_, r)| in_rect(**r, x, y)).map(|(i, _)| i)
}

fn hit_id(g: &Geom, x: i32, y: i32) -> i32 {
    let lay = &g.lay;
    if in_rect(lay.reset, x, y) {
        return HIT_RESET;
    }
    if in_rect(lay.done, x, y) {
        return HIT_DONE;
    }
    if in_rect(lay.change, x, y) {
        return HIT_CHANGE;
    }
    if lay.icons.bottom > lay.icons.top {
        let (auto, manual, preview) = icon_controls(lay.icons, g.dpi);
        if in_rect(auto, x, y) {
            return HIT_AUTO;
        }
        if in_rect(manual, x, y) {
            return HIT_MANUAL;
        }
        if in_rect(preview, x, y) {
            return HIT_PREVIEW;
        }
    }
    if let Some(i) = hit_cells(g.bar_clip, &g.slots, x, y) {
        return BAR_BASE + i as i32;
    }
    if let Some(i) = hit_cells(g.well_body, &g.wells, x, y) {
        return WELL_BASE + i as i32;
    }
    0
}

struct Geom {
    lay: EditLay,
    slots: Vec<RECT>,
    wells: Vec<RECT>,
    bar: Option<Thumb>,
    well: Option<Thumb>,
    bar_clip: RECT,
    well_body: RECT,
    bar_scroll: i32,
    well_scroll: i32,
    bar_max: i32,
    well_max: i32,
    dpi: i32,
}

fn geom_of(doc: &Doc, sel: Option<usize>, bar_scroll: i32, well_scroll: i32, width: i32, height: i32, dpi: i32) -> Geom {
    let show_icons = chosen_icon_count(doc, sel) > 0;
    let show_change = sel.is_some_and(|i| doc.is_folder_target(i) || doc.is_file_target(i));
    let lay = edit_layout(width, height, dpi, show_icons, show_change);
    let n = doc.item_count();
    let content_w = bar_content_width(n, dpi);
    let view_w = (lay.bar.right - lay.bar.left).max(0);
    let bar_max = scroll_max(content_w, view_w);
    let bar_scroll = clamp_scroll(bar_scroll, bar_max);
    let slots = bar_slots(lay.bar, n, dpi, bar_scroll);
    let bar = hthumb(lay.bar, content_w, view_w, bar_scroll, dpi);
    let mut bar_clip = lay.bar;
    if let Some(bar) = bar {
        bar_clip.bottom = bar.track.top;
    }
    let shown = visible_well(doc).len();
    let (body, content_h) = well_body(lay.well, shown, dpi);
    let view_h = (lay.well.bottom - lay.well.top).max(0);
    let well_max = scroll_max(content_h, view_h);
    let well_scroll = clamp_scroll(well_scroll, well_max);
    let wells = well_slots(body, shown, dpi, well_scroll);
    let well = vthumb(lay.well, body, content_h, view_h, well_scroll, dpi);
    Geom { lay, slots, wells, bar, well, bar_clip, well_body: body, bar_scroll, well_scroll, bar_max, well_max, dpi }
}

enum Press {
    Thumb { vertical: bool, grab: i32 },
    Page { vertical: bool, next: i32 },
}

fn scroll_press(g: &Geom, x: i32, y: i32) -> Option<Press> {
    if let Some(bar) = g.bar {
        if in_rect(bar.thumb, x, y) {
            return Some(Press::Thumb { vertical: false, grab: x - bar.thumb.left });
        }
        if in_rect(bar.track, x, y) {
            let page = ((g.lay.bar.right - g.lay.bar.left) * 4 / 5).max(1);
            let len = (bar.thumb.right - bar.thumb.left).max(1);
            let next = crate::page_toward(g.bar_scroll, g.bar_max, bar.thumb.left, len, x, page);
            return Some(Press::Page { vertical: false, next });
        }
    }
    if let Some(well) = g.well {
        if in_rect(well.thumb, x, y) {
            return Some(Press::Thumb { vertical: true, grab: y - well.thumb.top });
        }
        if in_rect(well.track, x, y) {
            let page = ((g.lay.well.bottom - g.lay.well.top) * 4 / 5).max(1);
            let len = (well.thumb.bottom - well.thumb.top).max(1);
            let next = crate::page_toward(g.well_scroll, g.well_max, well.thumb.top, len, y, page);
            return Some(Press::Page { vertical: true, next });
        }
    }
    None
}

fn thumb_to_scroll(track_start: i32, track_len: i32, thumb_len: i32, grab: i32, mouse: i32, max_off: i32) -> i32 {
    let rel = mouse - track_start - grab;
    let travel = (track_len - thumb_len).max(0);
    crate::thumb_scroll(rel, travel, max_off)
}

fn bar_edge_step(bar: RECT, x: i32, y: i32, dpi: i32) -> i32 {
    let slack = px(dpi, 16);
    if y < bar.top - slack || y > bar.bottom + slack {
        return 0;
    }
    edge_delta(bar.left, (bar.right - bar.left).max(0), x, px(dpi, 28), px(dpi, 16))
}

unsafe fn refresh_geom(h: HWND) -> Geom {
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let Some(v) = view(h) else {
        return geom_of(&Doc::default_layout(), None, 0, 0, rc.right, rc.bottom, dpi);
    };
    let g = geom_of(&(*v).doc, (*v).sel, (*v).bar_scroll, (*v).well_scroll, rc.right, rc.bottom, dpi);
    (*v).bar_scroll = g.bar_scroll;
    (*v).well_scroll = g.well_scroll;
    g
}

unsafe fn on_down(h: HWND, pt: (i32, i32)) {
    let Some(v) = view(h) else { return };
    tip_hide(&mut (*v).tip, h);
    (*v).last = pt;
    let g = refresh_geom(h);
    if let Some(press) = scroll_press(&g, pt.0, pt.1) {
        let Some(v) = view(h) else { return };
        match press {
            Press::Thumb { vertical, grab } => {
                (*v).thumb = Some(ThumbDrag { vertical, grab });
                (*v).down = 0;
                (*v).grab = None;
                (*v).drag = false;
                let _ = SetCapture(h);
            }
            Press::Page { vertical, next } => {
                if vertical {
                    (*v).well_scroll = next;
                } else {
                    (*v).bar_scroll = next;
                }
                let _ = InvalidateRect(Some(h), None, false);
            }
        }
        return;
    }
    let Some(v) = view(h) else { return };
    let id = hit_id(&g, pt.0, pt.1);
    (*v).down = id;
    (*v).origin = pt;
    (*v).drag = false;
    (*v).grab = grab_of(&(*v).doc, id);
    let _ = SetCapture(h);
    let _ = InvalidateRect(Some(h), None, false);
}

fn grab_of(doc: &Doc, id: i32) -> Option<Grab> {
    if (BAR_BASE..WELL_BASE).contains(&id) {
        let i = (id - BAR_BASE) as usize;
        return (i < doc.item_count()).then_some(Grab::Bar(i));
    }
    if (WELL_BASE..ICON_BASE).contains(&id) {
        let i = (id - WELL_BASE) as usize;
        return visible_well(doc).get(i).copied().map(Grab::Well);
    }
    None
}

unsafe fn on_move(h: HWND, pt: (i32, i32)) {
    arm_leave(h);
    let Some(v) = view(h) else { return };
    (*v).last = pt;
    if (*v).thumb.is_some() {
        drag_thumb(h, pt);
        return;
    }
    let moved = (pt.0 - (*v).origin.0).abs() + (pt.1 - (*v).origin.1).abs() > 4;
    if (*v).down != 0 && moved {
        (*v).drag = true;
    }
    let g = refresh_geom(h);
    let Some(v) = view(h) else { return };
    let dragging = (*v).drag;
    if dragging && g.bar_max > 0 && bar_edge_step(g.lay.bar, pt.0, pt.1, g.dpi) != 0 {
        let _ = SetTimer(Some(h), TIMER_EDGE, 30, None);
    } else {
        let _ = KillTimer(Some(h), TIMER_EDGE);
    }
    let id = hit_id(&g, pt.0, pt.1);
    let insert = if dragging && in_rect(g.lay.bar, pt.0, pt.1) { Some(insert_at(&g.slots, pt.0)) } else { None };
    if (*v).hot != id || (*v).insert != insert {
        (*v).hot = id;
        (*v).insert = insert;
        let _ = InvalidateRect(Some(h), None, false);
    }
    if dragging {
        tip_hide(&mut (*v).tip, h);
    } else {
        sync_tip(h, id, &g);
    }
}

unsafe fn drag_thumb(h: HWND, pt: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let Some(drag) = (*v).thumb else { return };
    let g = refresh_geom(h);
    let Some(track) = (if drag.vertical { g.well } else { g.bar }) else {
        return;
    };
    let max_off = if drag.vertical { g.well_max } else { g.bar_max };
    let (start, span, len, mouse) = if drag.vertical { (track.track.top, (track.track.bottom - track.track.top).max(0), (track.thumb.bottom - track.thumb.top).max(1), pt.1) } else { (track.track.left, (track.track.right - track.track.left).max(0), (track.thumb.right - track.thumb.left).max(1), pt.0) };
    let next = thumb_to_scroll(start, span, len, drag.grab, mouse, max_off);
    let Some(v) = view(h) else { return };
    let changed = if drag.vertical {
        let same = (*v).well_scroll == next;
        (*v).well_scroll = next;
        !same
    } else {
        let same = (*v).bar_scroll == next;
        (*v).bar_scroll = next;
        !same
    };
    if changed {
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn on_up(h: HWND, pt: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let drag = (*v).drag;
    let grab = (*v).grab;
    let down = (*v).down;
    let thumb = (*v).thumb.take();
    (*v).down = 0;
    (*v).drag = false;
    (*v).grab = None;
    (*v).insert = None;
    let _ = ReleaseCapture();
    let _ = KillTimer(Some(h), TIMER_EDGE);
    if thumb.is_some() {
        let _ = InvalidateRect(Some(h), None, false);
        return;
    }
    let id = hit_now(h, pt);
    if drag {
        if let Some(grab) = grab {
            let g = refresh_geom(h);
            let on_bar = in_rect(g.lay.bar, pt.0, pt.1);
            let at = insert_at(&g.slots, pt.0);
            match plan_drop(grab, on_bar, at) {
                Drop::Insert(Well::OpenFolder, at) => place_open(h, at, true),
                Drop::Insert(Well::OpenFile, at) => place_open(h, at, false),
                landed => {
                    if let Some(v) = view(h) {
                        apply_drop(&mut (*v).doc, landed);
                        if (*v).sel.is_some_and(|i| i >= (*v).doc.item_count() || (*v).doc.is_separator(i)) {
                            (*v).sel = None;
                        }
                    }
                    post_target_check(h);
                }
            }
        }
    } else if down == id {
        match id {
            HIT_RESET => restore(h),
            HIT_DONE => {
                finish(h);
                return;
            }
            HIT_CHANGE => retarget(h),
            HIT_AUTO => {
                if let Some(v) = view(h) {
                    if let Some(sel) = (*v).sel {
                        (*v).doc.set_pack(sel, None);
                    }
                }
            }
            HIT_MANUAL => {
                if let Some(v) = view(h) {
                    if let Some(sel) = (*v).sel {
                        if !(*v).doc.manual_icon(sel) {
                            open_pack_picker(h, sel);
                        }
                    }
                }
            }
            HIT_PREVIEW => {
                if let Some(v) = view(h) {
                    if let Some(sel) = (*v).sel {
                        open_pack_picker(h, sel);
                    }
                }
            }
            n if (BAR_BASE..WELL_BASE).contains(&n) => {
                if let Some(v) = view(h) {
                    let i = (n - BAR_BASE) as usize;
                    if i < (*v).doc.item_count() && !(*v).doc.is_separator(i) {
                        (*v).sel = if (*v).sel == Some(i) { None } else { Some(i) };
                    } else {
                        (*v).sel = None;
                    }
                    reveal_sel(h);
                }
            }
            _ => {}
        }
    }
    let _ = InvalidateRect(Some(h), None, false);
    let g = refresh_geom(h);
    sync_tip(h, if drag { 0 } else { id }, &g);
}

unsafe fn place_open(h: HWND, at: usize, folder: bool) {
    let path = if folder { crate::pick_folder(h) } else { crate::pick_files(h).into_iter().next() };
    let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
        return;
    };
    let Some(v) = view(h) else { return };
    let before = (*v).doc.item_count();
    if folder {
        (*v).doc.insert_folder(at, &path);
    } else {
        (*v).doc.insert_file(at, &path);
    }
    if (*v).doc.item_count() == before {
        return;
    }
    (*v).sel = Some(at.min((*v).doc.item_count() - 1));
    reveal_sel(h);
    post_target_check(h);
}

unsafe fn retarget(h: HWND) {
    let Some(v) = view(h) else { return };
    let Some(i) = (*v).sel else { return };
    let folder = (*v).doc.is_folder_target(i);
    let file = (*v).doc.is_file_target(i);
    if !folder && !file {
        return;
    }
    let path = if folder { crate::pick_folder(h) } else { crate::pick_files(h).into_iter().next() };
    let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
        return;
    };
    let Some(v) = view(h) else { return };
    if (*v).doc.set_target_path(i, &path) {
        post_target_check(h);
    }
}

unsafe fn reveal_sel(h: HWND) {
    let g = refresh_geom(h);
    let Some(v) = view(h) else { return };
    if let Some(i) = (*v).sel {
        (*v).bar_scroll = reveal_bar((*v).bar_scroll, g.lay.bar, i, (*v).doc.item_count(), g.dpi);
    }
}

unsafe fn restore(h: HWND) {
    let ask = Confirm::ok(crate::tr(crate::Key::ToolbarEdit), crate::tr(crate::Key::ToolbarRestoreAsk)).affirm_label(crate::tr(crate::Key::ToolbarRestore));
    if !ask_confirm(h, ask) {
        return;
    }
    if let Some(v) = view(h) {
        (*v).doc.reset_default();
        (*v).sel = None;
        (*v).bar_scroll = 0;
        (*v).well_scroll = 0;
        post_target_check(h);
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn finish(h: HWND) {
    let Some(v) = view(h) else { return };
    let doc = (*v).doc.clone();
    if doc.install() {
        crate::relayout();
        close(h);
    }
}

unsafe fn close(h: HWND) {
    let _ = DestroyWindow(h);
}

unsafe fn drop_view(h: HWND) {
    let _ = KillTimer(Some(h), TIMER_EDGE);
    if let Some(v) = view(h) {
        tip_hide(&mut (*v).tip, h);
    }
    crate::drop_side(h);
    if LIVE.load(Ordering::Relaxed) == h.0 as isize {
        LIVE.store(0, Ordering::Relaxed);
    }
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut View;
    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}

fn chosen_icon_count(doc: &Doc, sel: Option<usize>) -> usize {
    match sel {
        Some(i) if i < doc.item_count() && !doc.is_separator(i) => 1 + crate::ui::icon_catalog::icons().len(),
        _ => 0,
    }
}

unsafe fn hit_now(h: HWND, pt: (i32, i32)) -> i32 {
    let g = refresh_geom(h);
    hit_id(&g, pt.0, pt.1)
}

unsafe fn paint(h: HWND) {
    if view(h).is_none() {
        // Validate the update region. Returning from WM_PAINT without BeginPaint paints forever.
        crate::buffered(h, |dc, rc| paint_window(dc, rc));
        return;
    }
    let g = refresh_geom(h);
    let dpi = g.dpi;
    let (hot, down, doc, insert, sel, missing) = match view(h) {
        Some(v) => ((*v).hot, (*v).down, (*v).doc.clone(), (*v).insert, (*v).sel, (*v).missing.clone()),
        None => return,
    };
    crate::buffered(h, |dc, rc| {
        let _ = rc;
        paint_window(dc, rc);
        let lay = &g.lay;
        paint_card(dc, lay.bar, dpi);
        let saved = SaveDC(dc);
        if saved != 0 {
            let _ = IntersectClipRect(dc, g.bar_clip.left, g.bar_clip.top, g.bar_clip.right, g.bar_clip.bottom);
        }
        for (i, r) in g.slots.iter().enumerate() {
            if sel == Some(i) {
                fill(dc, *r, crate::blend(crate::ui_tokens().bg, crate::live_accent(), 0.16));
            } else if hot == BAR_BASE + i as i32 {
                fill(dc, *r, crate::ui_tokens().card);
            }
            if doc.is_separator(i) {
                let mid = (r.left + r.right) / 2;
                fill(dc, RECT { left: mid, top: r.top + 4, right: mid + 1, bottom: r.bottom - 4 }, crate::ui_tokens().line);
            } else if let Some(kind) = doc.glyph_at(i) {
                let fg = if missing.get(i).copied().unwrap_or(false) { crate::blend(crate::ui_tokens().bg, crate::ui_tokens().sub, 0.45) } else { crate::ui_tokens().text };
                draw_line_icon(dc, *r, kind, fg, dpi);
            }
        }
        if let Some(at) = insert {
            let x = g.slots.get(at).map(|r| r.left - px(dpi, 2)).unwrap_or_else(|| g.slots.last().map(|r| r.right + px(dpi, 1)).unwrap_or(lay.bar.left + px(dpi, 8)));
            if x >= lay.bar.left && x < lay.bar.right {
                let y = lay.bar.top + px(dpi, 10);
                let bottom = g.bar_clip.bottom - px(dpi, 6);
                fill(dc, RECT { left: x, top: y, right: x + px(dpi, 2), bottom }, crate::live_accent());
            }
        }
        if saved != 0 {
            let _ = RestoreDC(dc, saved);
        }
        if let Some(bar) = g.bar {
            unsafe { paint_scroll(dc, &bar) };
        }
        paint_meta(dc, lay.hint, crate::tr(crate::Key::ToolbarEditHint), meta_font());
        if lay.change.right > lay.change.left {
            let ground = crate::ui_tokens().bg;
            let body = body_font();
            paint_button(dc, lay.change, ButtonKind::Secondary, visual_from_pointer(HIT_CHANGE, Some(hot).filter(|&h| h != 0), Some(down).filter(|&d| d != 0)), crate::tr(crate::Key::ToolbarChangeTarget), body, dpi, ground, FocusRing::Outside);
        }
        if lay.icons.bottom > lay.icons.top {
            paint_icon_choice(dc, lay.icons, &doc, sel, hot, down, dpi, sel.and_then(|i| missing.get(i).copied()).unwrap_or(false));
        }
        let shown = visible_well(&doc);
        let saved = SaveDC(dc);
        if saved != 0 {
            let _ = IntersectClipRect(dc, g.well_body.left, g.well_body.top, g.well_body.right, g.well_body.bottom);
        }
        let small = meta_font();
        for (i, item) in shown.iter().enumerate() {
            let Some(cell) = g.wells.get(i).copied() else { continue };
            if overlaps(cell, lay.reset) || overlaps(cell, lay.done) || (lay.icons.bottom > lay.icons.top && overlaps(cell, lay.icons)) {
                continue;
            }
            let id = WELL_BASE + i as i32;
            paint_card(dc, cell, dpi);
            if hot == id {
                fill(dc, cell, crate::ui_tokens().card);
            }
            let icon = RECT { left: cell.left + (cell.right - cell.left - px(dpi, 30)) / 2, top: cell.top + px(dpi, 2), right: 0, bottom: 0 };
            let icon = RECT { right: icon.left + px(dpi, 30), bottom: icon.top + px(dpi, 24), ..icon };
            if let Some(kind) = well_icon(*item) {
                draw_line_icon(dc, icon, kind, crate::ui_tokens().text, dpi);
            } else {
                let mid = (icon.left + icon.right) / 2;
                fill(dc, RECT { left: mid, top: icon.top + 4, right: mid + 1, bottom: icon.bottom - 4 }, crate::ui_tokens().line);
            }
            let label = RECT { left: cell.left, top: icon.bottom, right: cell.right, bottom: cell.bottom };
            crate::text(dc, label, &well_label(*item), small, crate::ui_tokens().sub, DT_CENTER | DT_WORDBREAK | DT_NOPREFIX);
        }
        if saved != 0 {
            let _ = RestoreDC(dc, saved);
        }
        if let Some(well) = g.well {
            unsafe { paint_scroll(dc, &well) };
        }
        let ground = crate::ui_tokens().bg;
        let body = body_font();
        paint_button(dc, lay.reset, ButtonKind::Secondary, visual_from_pointer(HIT_RESET, Some(hot).filter(|&h| h != 0), Some(down).filter(|&d| d != 0)), crate::tr(crate::Key::ToolbarRestore), body, dpi, ground, FocusRing::Outside);
        paint_button(dc, lay.done, ButtonKind::Primary, visual_from_pointer(HIT_DONE, Some(hot).filter(|&h| h != 0), Some(down).filter(|&d| d != 0)), crate::tr(crate::Key::ToolbarDone), body, dpi, ground, FocusRing::Outside);
    });
}

unsafe fn refresh_missing(h: HWND) {
    let Some(v) = view(h) else { return };
    let rows = editor_rows(&(*v).doc);
    (*v).missing = rows.iter().map(|row| row.missing).collect();
    let _ = InvalidateRect(Some(h), None, false);
}

fn post_target_check(h: HWND) {
    if let Some(v) = view(h) {
        unsafe { (*v).missing.clear() };
    }
    unsafe {
        let _ = PostMessageW(Some(h), WM_TARGET_CHECK, WPARAM(0), LPARAM(0));
    }
}

unsafe fn paint_scroll(dc: HDC, thumb: &Thumb) {
    let t = crate::ui_tokens();
    fill(dc, thumb.track, crate::blend(t.bg, t.line, 0.7));
    fill(dc, thumb.thumb, t.sub);
}

fn size_slot() -> std::sync::MutexGuard<'static, Option<(i32, i32)>> {
    static SIZE: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    SIZE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner())
}

fn remembered_size() -> Option<(i32, i32)> {
    *size_slot()
}

fn remember_size(w: i32, h: i32) {
    if w > 0 && h > 0 {
        *size_slot() = Some((w, h));
    }
}

unsafe fn on_size(h: HWND, kind: u32) {
    if kind == SIZE_MINIMIZED {
        return;
    }
    if kind == SIZE_RESTORED {
        if view(h).is_some() {
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            remember_size(rc.right, rc.bottom);
        }
    }
    if view(h).is_some() {
        reveal_sel(h);
    }
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn on_wheel(h: HWND, wp: WPARAM, lp: LPARAM, horizontal: bool) {
    let notches = ((wp.0 >> 16) as i16 as i32) / 120;
    if notches == 0 {
        return;
    }
    let pt = wheel_client(h, lp);
    let g = refresh_geom(h);
    let over_bar = in_rect(g.lay.bar, pt.0, pt.1);
    let over_well = in_rect(g.lay.well, pt.0, pt.1);
    let Some(v) = view(h) else { return };
    (*v).last = pt;
    if horizontal || shift_down() || over_bar {
        let step = px(g.dpi, 48);
        let next = if horizontal { hwheel_scroll((*v).bar_scroll, g.bar_max, notches, step) } else { wheel_scroll((*v).bar_scroll, g.bar_max, notches, step) };
        if next != (*v).bar_scroll {
            (*v).bar_scroll = next;
            let _ = InvalidateRect(Some(h), None, false);
        }
    } else if over_well {
        let (_, ch, gap) = well_metrics(g.dpi);
        let next = wheel_scroll((*v).well_scroll, g.well_max, notches, ch + gap);
        if next != (*v).well_scroll {
            (*v).well_scroll = next;
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
    let g = refresh_geom(h);
    let id = hit_id(&g, pt.0, pt.1);
    if let Some(v) = view(h) {
        if (*v).hot != id && !(*v).drag {
            (*v).hot = id;
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
    sync_tip(h, id, &g);
}

fn wheel_client(h: HWND, lp: LPARAM) -> (i32, i32) {
    let mut pt = POINT { x: (lp.0 as i16) as i32, y: ((lp.0 >> 16) as i16) as i32 };
    unsafe {
        let _ = ScreenToClient(h, &mut pt);
    }
    (pt.x, pt.y)
}

fn shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

unsafe fn on_leave(h: HWND) {
    if let Some(v) = view(h) {
        (*v).hot = 0;
        tip_hide(&mut (*v).tip, h);
    }
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn end_capture(h: HWND) {
    let _ = KillTimer(Some(h), TIMER_EDGE);
    if let Some(v) = view(h) {
        if (*v).thumb.is_some() || (*v).drag {
            (*v).thumb = None;
            (*v).drag = false;
            (*v).grab = None;
            (*v).down = 0;
            (*v).insert = None;
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

unsafe fn edge_tick(h: HWND) {
    let Some(v) = view(h) else { return };
    if !(*v).drag || (*v).thumb.is_some() {
        let _ = KillTimer(Some(h), TIMER_EDGE);
        return;
    }
    let pt = (*v).last;
    let g = refresh_geom(h);
    let delta = if g.bar_max > 0 { bar_edge_step(g.lay.bar, pt.0, pt.1, g.dpi) } else { 0 };
    if delta == 0 {
        let _ = KillTimer(Some(h), TIMER_EDGE);
        return;
    }
    let Some(v) = view(h) else { return };
    let next = clamp_scroll((*v).bar_scroll.saturating_add(delta), g.bar_max);
    if next == (*v).bar_scroll {
        return;
    }
    (*v).bar_scroll = next;
    let g = refresh_geom(h);
    let Some(v) = view(h) else { return };
    (*v).insert = if in_rect(g.lay.bar, pt.0, pt.1) { Some(insert_at(&g.slots, pt.0)) } else { None };
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn arm_leave(h: HWND) {
    let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
    let _ = TrackMouseEvent(&mut t);
}

fn intersect_rect(a: RECT, b: RECT) -> Option<RECT> {
    let r = RECT { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

fn tip_anchor(doc: &Doc, g: &Geom, id: i32) -> Option<(String, RECT)> {
    if (BAR_BASE..WELL_BASE).contains(&id) {
        let i = (id - BAR_BASE) as usize;
        let slot = *g.slots.get(i)?;
        let anchor = intersect_rect(slot, g.bar_clip)?;
        let text = doc.item_tip(i);
        return (!text.is_empty()).then_some((text, anchor));
    }
    if (WELL_BASE..ICON_BASE).contains(&id) {
        let i = (id - WELL_BASE) as usize;
        let item = visible_well(doc).get(i).copied()?;
        let slot = *g.wells.get(i)?;
        let anchor = intersect_rect(slot, g.well_body)?;
        let text = well_label(item);
        return (!text.is_empty()).then_some((text, anchor));
    }
    None
}

unsafe fn sync_tip(h: HWND, id: i32, g: &Geom) {
    let text = view(h).and_then(|v| tip_anchor(&(*v).doc, g, id));
    let Some(v) = view(h) else { return };
    match text {
        Some((text, anchor)) => tip_show(&mut (*v).tip, h, id, &text, anchor),
        None => tip_hide(&mut (*v).tip, h),
    }
}

struct Tip {
    hwnd: isize,
    added: bool,
    id: i32,
    text: String,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn tip_new() -> Tip {
    Tip { hwnd: 0, added: false, id: -1, text: String::new(), left: 0, top: 0, right: 0, bottom: 0 }
}

thread_local! {
    static TIP_BUSY: Cell<bool> = const { Cell::new(false) };
}

struct TipGuard;
impl std::ops::Drop for TipGuard {
    fn drop(&mut self) {
        TIP_BUSY.with(|c| c.set(false));
    }
}

fn track_info(owner: HWND, label: &HSTRING) -> TTTOOLINFOW {
    TTTOOLINFOW { cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32, uFlags: TTF_TRACK | TTF_ABSOLUTE, hwnd: owner, uId: 1, lpszText: PWSTR(label.as_ptr() as *mut u16), ..Default::default() }
}

fn pack_xy(x: i32, y: i32) -> LPARAM {
    LPARAM((x as u16 as usize as isize) | ((y as u16 as usize as isize) << 16))
}

fn tip_same(tip: &Tip, id: i32, text: &str, r: RECT) -> bool {
    tip.id == id && tip.text == text && tip.left == r.left && tip.top == r.top && tip.right == r.right && tip.bottom == r.bottom
}

unsafe fn tip_window(tip: &mut Tip, owner: HWND) -> HWND {
    if tip.hwnd != 0 {
        let hwnd = HWND(tip.hwnd as *mut std::ffi::c_void);
        if IsWindow(Some(hwnd)).as_bool() {
            return hwnd;
        }
    }
    let style = WS_POPUP | WINDOW_STYLE(0x01 | 0x02 | 0x10 | 0x20);
    let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, None, style, 0, 0, 0, 0, Some(owner), None, None, None) else {
        return HWND::default();
    };
    crate::set_tip_font(hwnd);
    tip.hwnd = hwnd.0 as isize;
    tip.added = false;
    hwnd
}

unsafe fn tip_show(tip: &mut Tip, owner: HWND, id: i32, text: &str, client: RECT) {
    if text.is_empty() {
        tip_hide(tip, owner);
        return;
    }
    if tip_same(tip, id, text, client) && tip.hwnd != 0 {
        return;
    }
    if TIP_BUSY.with(|c| c.replace(true)) {
        return;
    }
    let _guard = TipGuard;
    let hwnd = tip_window(tip, owner);
    if hwnd.0.is_null() {
        return;
    }
    let label = HSTRING::from(text);
    let info = track_info(owner, &label);
    let lp = LPARAM(&info as *const _ as isize);
    if !tip.added {
        let _ = SendMessageW(hwnd, TTM_ADDTOOLW, None, Some(lp));
        tip.added = true;
    }
    let _ = SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(640)));
    let _ = SendMessageW(hwnd, TTM_UPDATETIPTEXTW, None, Some(lp));
    let size = SendMessageW(hwnd, TTM_GETBUBBLESIZE, None, Some(lp));
    let (tw, th) = if size.0 != 0 {
        ((size.0 & 0xffff) as i32, ((size.0 >> 16) & 0xffff) as i32)
    } else {
        let lines = text.split('\n').count().max(1) as i32;
        let chars = text.split('\n').map(|s| s.chars().count()).max().unwrap_or(8) as i32;
        (chars * 8 + 16, lines * 18 + 8)
    };
    let screen = crate::tip_screen_rect(owner, client);
    let work = crate::tip_work_area(screen);
    let (x, y) = crate::tip_place(screen, tw, th, crate::TipSide::Below, work);
    let _ = SendMessageW(hwnd, TTM_TRACKPOSITION, None, Some(pack_xy(x, y)));
    let _ = SendMessageW(hwnd, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(lp));
    tip.id = id;
    tip.text = text.to_string();
    tip.left = client.left;
    tip.top = client.top;
    tip.right = client.right;
    tip.bottom = client.bottom;
}

unsafe fn tip_hide(tip: &mut Tip, owner: HWND) {
    if tip.hwnd == 0 || tip.id < 0 {
        tip.id = -1;
        tip.text.clear();
        return;
    }
    if TIP_BUSY.with(|c| c.replace(true)) {
        return;
    }
    let _guard = TipGuard;
    let hwnd = HWND(tip.hwnd as *mut std::ffi::c_void);
    tip.id = -1;
    tip.text.clear();
    if hwnd.0.is_null() || !IsWindow(Some(hwnd)).as_bool() {
        tip.hwnd = 0;
        tip.added = false;
        return;
    }
    let label = HSTRING::new();
    let info = track_info(owner, &label);
    let lp = LPARAM(&info as *const _ as isize);
    let _ = SendMessageW(hwnd, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(lp));
    let _ = SendMessageW(hwnd, TTM_POP, None, None);
}

fn catalog_name(def: &crate::ui::icon_catalog::IconDef) -> String {
    if def.name_ko.is_empty() {
        def.key.to_string()
    } else {
        def.name_ko.to_string()
    }
}

fn pack_band(rc: RECT, dpi: i32) -> RECT {
    RECT { left: px(dpi, 8), top: px(dpi, 8), right: rc.right - px(dpi, 8), bottom: rc.bottom - px(dpi, 8) }
}

unsafe fn pack_move(h: HWND, pt: (i32, i32)) {
    arm_leave(h);
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pack;
    if p.is_null() {
        return;
    }
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let icons = crate::ui::icon_catalog::icons();
    let cells = icon_slots(pack_band(rc, dpi), icons.len(), dpi);
    let hot = cells.iter().position(|r| in_rect(*r, pt.0, pt.1)).filter(|i| *i < icons.len());
    match hot {
        Some(i) => {
            let name = catalog_name(&icons[i]);
            if let Some(r) = cells.get(i).copied() {
                tip_show(&mut (*p).tip, h, i as i32, &name, r);
            }
        }
        None => tip_hide(&mut (*p).tip, h),
    }
}

unsafe fn pack_leave(h: HWND) {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pack;
    if p.is_null() {
        return;
    }
    tip_hide(&mut (*p).tip, h);
}

unsafe fn paint_icon_choice(dc: HDC, band: RECT, doc: &Doc, sel: Option<usize>, hot: i32, down: i32, dpi: i32, missing: bool) {
    let Some(i) = sel else { return };
    let (auto, manual, preview) = icon_controls(band, dpi);
    let manual_on = doc.manual_icon(i);
    let cells = [SegmentCell { rect: auto, label: crate::tr(crate::Key::ToolbarIconDefault), disabled: false }, SegmentCell { rect: manual, label: crate::tr(crate::Key::ToolbarIconSet), disabled: false }];
    let hot_i = match hot {
        HIT_AUTO => Some(0usize),
        HIT_MANUAL => Some(1),
        _ => None,
    };
    let down_i = match down {
        HIT_AUTO => Some(0usize),
        HIT_MANUAL => Some(1),
        _ => None,
    };
    paint_segment_group(dc, &cells, Some(if manual_on { 1 } else { 0 }), hot_i, down_i, None, dpi, crate::ui_tokens().bg, FocusRing::Outside);
    paint_card_fill(dc, preview, dpi, crate::ui_tokens().card);
    if let Some(kind) = doc.glyph_at(i) {
        let inset = px(dpi, 4);
        let slot = RECT { left: preview.left + inset, top: preview.top + inset, right: preview.right - inset, bottom: preview.bottom - inset };
        let fg = if missing { crate::blend(crate::ui_tokens().bg, crate::ui_tokens().sub, 0.45) } else { crate::ui_tokens().text };
        draw_line_icon(dc, slot, kind, fg, dpi);
    }
}

unsafe fn open_pack_picker(editor: HWND, index: usize) {
    ensure_pack_class();
    let dpi = GetDpiForWindow(editor).max(96) as i32;
    let client_w = px(dpi, 360);
    let client_h = px(dpi, 280);
    let pack_style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    let (w, h) = sized_frame(client_w, client_h, dpi, pack_style);
    let (x, y) = center_on(editor, w, h);
    let Ok(hwnd) = CreateWindowExW(WS_EX_DLGMODALFRAME, w!("MaguroIconPack"), &HSTRING::from(crate::tr(crate::Key::IconCustom)), pack_style, x, y, w, h, Some(editor), None, None, None) else {
        return;
    };
    if let Some(v) = view(editor) {
        tip_hide(&mut (*v).tip, editor);
    }
    let pick = Box::new(Pack { editor, index, tip: tip_new() });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(pick) as isize);
    style_popup_chrome(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
}

struct Pack {
    editor: HWND,
    index: usize,
    tip: Tip,
}

fn ensure_pack_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else {
            return;
        };
        let _ = RegisterClassW(&WNDCLASSW { style: CS_HREDRAW | CS_VREDRAW, lpfnWndProc: Some(pack_proc), hInstance: m.into(), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), lpszClassName: w!("MaguroIconPack"), ..Default::default() });
    });
}

unsafe extern "system" fn pack_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::catch_wnd(AssertUnwindSafe(|| unsafe { pack_inner(h, msg, wp, lp) }))
}

unsafe fn pack_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_pack(h);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            pack_move(h, point(lp));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            pack_leave(h);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            choose_pack(h, point(lp));
            LRESULT(0)
        }
        WM_CLOSE | WM_DESTROY => {
            let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pack;
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            if !p.is_null() {
                tip_hide(&mut (*p).tip, h);
                drop(Box::from_raw(p));
            }
            if msg == WM_CLOSE {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn paint_pack(h: HWND) {
    let dpi = GetDpiForWindow(h).max(96) as i32;
    crate::buffered(h, |dc, rc| {
        paint_window(dc, rc);
        let icons = crate::ui::icon_catalog::icons();
        let cells = icon_slots(RECT { left: px(dpi, 8), top: px(dpi, 8), right: rc.right - px(dpi, 8), bottom: rc.bottom - px(dpi, 8) }, icons.len(), dpi);
        for (i, def) in icons.iter().enumerate() {
            if let Some(r) = cells.get(i) {
                draw_line_icon(dc, *r, def.kind, crate::ui_tokens().text, dpi);
            }
        }
    });
}

unsafe fn choose_pack(h: HWND, pt: (i32, i32)) {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pack;
    if p.is_null() {
        return;
    }
    let editor = (*p).editor;
    let index = (*p).index;
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let icons = crate::ui::icon_catalog::icons();
    let cells = icon_slots(RECT { left: px(dpi, 8), top: px(dpi, 8), right: rc.right - px(dpi, 8), bottom: rc.bottom - px(dpi, 8) }, icons.len(), dpi);
    for (i, r) in cells.iter().enumerate() {
        if in_rect(*r, pt.0, pt.1) {
            if let Some(def) = icons.get(i) {
                if let Some(v) = view(editor) {
                    (*v).doc.set_pack(index, Some(def.key));
                    let _ = InvalidateRect(Some(editor), None, false);
                }
            }
            let _ = DestroyWindow(h);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_on_the_bar_reorders_and_a_drag_off_removes() {
        assert_eq!(plan_drop(Grab::Bar(1), true, 1), Drop::None);
        assert_eq!(plan_drop(Grab::Bar(1), true, 2), Drop::None);
        assert_eq!(plan_drop(Grab::Bar(0), true, 3), Drop::Move { from: 0, to: 3 });
        assert_eq!(plan_drop(Grab::Bar(2), false, 0), Drop::Remove(2));
        assert_eq!(plan_drop(Grab::Well(Well::Separator), false, 1), Drop::None);
        assert_eq!(plan_drop(Grab::Well(Well::Action(Action::Paste)), true, 0), Drop::Insert(Well::Action(Action::Paste), 0));
        assert_eq!(plan_drop(Grab::Well(Well::OpenFolder), true, 2), Drop::Insert(Well::OpenFolder, 2));
        assert_eq!(plan_drop(Grab::Well(Well::OpenFile), false, 0), Drop::None);
    }

    #[test]
    fn the_well_hides_commands_already_on_the_bar_and_keeps_open_targets() {
        let mut doc = Doc::default_layout();
        doc.insert_folder(0, r"D:\A");
        doc.insert_folder(1, r"D:\B");
        doc.insert_file(2, r"D:\B\a.txt");
        let well = visible_well(&doc);
        assert!(well.contains(&Well::Separator));
        assert!(!well.contains(&Well::Action(Action::NewFolder)));
        assert!(well.contains(&Well::Action(Action::CopyPath)));
        assert!(well.contains(&Well::Action(Action::NewTab)));
        assert_eq!(well.iter().filter(|w| matches!(w, Well::OpenFolder)).count(), 1);
        assert_eq!(well.iter().filter(|w| matches!(w, Well::OpenFile)).count(), 1);
        assert!(!well.iter().any(|w| matches!(w, Well::Action(Action::Open))));
        let n = doc.item_count();
        apply_drop(&mut doc, Drop::Insert(Well::OpenFolder, 0));
        apply_drop(&mut doc, Drop::Insert(Well::OpenFile, 0));
        assert_eq!(doc.item_count(), n, "a target without a path is not inserted");
    }

    #[test]
    fn the_bar_gap_is_the_midpoint_of_a_slot() {
        let slots = [RECT { left: 10, top: 0, right: 40, bottom: 10 }, RECT { left: 42, top: 0, right: 72, bottom: 10 }];
        assert_eq!(insert_at(&slots, 20), 0);
        assert_eq!(insert_at(&slots, 30), 1);
        assert_eq!(insert_at(&slots, 80), 2);
    }

    #[test]
    fn bar_scroll_moves_slots_and_hits_only_the_visible_part() {
        let bar = RECT { left: 0, top: 0, right: 120, bottom: 48 };
        let still = bar_slots(bar, 6, 96, 0);
        let moved = bar_slots(bar, 6, 96, 40);
        assert_eq!(moved[0].left, still[0].left - 40);
        let at = insert_at(&moved, 20);
        assert!(at == moved.len() || (moved[at].left + moved[at].right) / 2 > 20);
        if at > 0 {
            assert!((moved[at - 1].left + moved[at - 1].right) / 2 <= 20);
        }
        let doc = Doc::default_layout();
        let g = geom_of(&doc, None, 10_000, 0, 220, 520, 96);
        assert!(g.bar_max > 0);
        assert_eq!(g.bar_scroll, g.bar_max);
        assert!(g.bar.is_some());
        let wide = geom_of(&doc, None, 40, 0, 900, 520, 96);
        assert_eq!(wide.bar_max, 0);
        assert!(wide.bar.is_none());
        assert_eq!(wide.bar_scroll, 0);
        let hidden = g.slots.iter().find(|r| r.right <= g.lay.bar.left);
        if let Some(r) = hidden {
            assert_eq!(hit_id(&g, (r.left + r.right) / 2, (r.top + r.bottom) / 2), 0);
        }
        let visible = g.slots.iter().enumerate().find(|(_, r)| r.left >= g.bar_clip.left && r.right <= g.bar_clip.right && r.bottom <= g.bar_clip.bottom);
        let (i, r) = visible.expect("a slot stays fully inside the scrolled bar");
        assert_eq!(hit_id(&g, (r.left + r.right) / 2, (r.top + r.bottom) / 2), BAR_BASE + i as i32);
        assert_eq!(reveal_scroll(0, 100, 300, 180, 210), 110);
        assert_eq!(reveal_scroll(200, 100, 300, 10, 40), 10);
        assert_eq!(reveal_scroll(20, 100, 300, 30, 60), 20);
        assert_eq!(wheel_scroll(40, 100, 1, 16), 24);
        assert_eq!(hwheel_scroll(90, 100, 2, 16), 100);
        assert_eq!(edge_delta(0, 200, 10, 28, 16), -16);
        assert_eq!(edge_delta(0, 200, 190, 28, 16), 16);
        assert_eq!(edge_delta(0, 200, 100, 28, 16), 0);
    }

    #[test]
    fn the_palette_scrolls_when_the_cards_are_taller_than_the_well() {
        let doc = Doc::default_layout();
        let n = visible_well(&doc).len();
        let well = RECT { left: 0, top: 0, right: 400, bottom: 100 };
        let (body, content) = well_body(well, n, 96);
        assert!(content > 100);
        assert!(body.right < well.right);
        let slots = well_slots(body, n, 96, 0);
        let scrolled = well_slots(body, n, 96, 24);
        assert_eq!(scrolled[0].top, slots[0].top - 24);
        let short = geom_of(&doc, None, 0, 0, 560, 360, 96);
        assert!(short.well_max > 0, "a short editor scrolls the palette");
        assert!(short.well.is_some());
    }

    #[test]
    fn an_open_target_tip_names_the_path_and_the_palette_names_the_action() {
        let mut doc = Doc::default_layout();
        doc.insert_folder(0, r"D:\Work");
        let g = geom_of(&doc, Some(0), 0, 0, 700, 560, 96);
        let (text, _) = tip_anchor(&doc, &g, BAR_BASE).expect("bar tip");
        assert_eq!(text, "Work\r\nD:\\Work");
        let folder = visible_well(&doc).iter().position(|w| matches!(w, Well::OpenFolder)).unwrap();
        let (text, _) = tip_anchor(&doc, &g, WELL_BASE + folder as i32).expect("palette tip");
        assert_eq!(text, crate::tr(crate::Key::ToolbarOpenFolder));
        assert!(g.lay.change.right > g.lay.change.left, "a selected target offers Change target");
    }

    #[test]
    fn the_editor_minimum_fits_every_palette_card_and_the_footer() {
        assert!(edit_style().0 & WS_THICKFRAME.0 != 0);
        assert!(edit_style().0 & WS_MAXIMIZEBOX.0 != 0);
        let (w, h) = min_client(96);
        assert_eq!((w, h), (436, 618), "the minimum rose above 420x360");
        assert!(!client_fits(w, h - 1, 96, catalog().len()), "one pixel shorter clips a palette card");
        assert_eq!(min_client(144), (w * 3 / 2, h * 3 / 2), "the minimum scales with DPI");
        let (outer_w, outer_h) = unsafe { outer_size(w, h, 96) };
        assert!(outer_w >= w && outer_h >= h);
        assert_eq!(open_client(None, 96), (560.max(w), 520.max(h)));
        assert_eq!(open_client(Some((100, 100)), 96), (w, h));
        assert_eq!(open_client(Some((w + 80, h + 40)), 96), (w + 80, h + 40));

        let doc = targets_only();
        assert_no_overlap(&doc, w, h, 96);
        assert_no_overlap(&Doc::default_layout(), w, h, 96);
        let old = geom_of(&Doc::default_layout(), None, 0, 0, 420, 360, 96);
        let folder = visible_well(&Doc::default_layout()).iter().position(|item| matches!(item, Well::OpenFolder)).unwrap();
        assert!(overlaps(old.wells[folder], old.lay.reset) || !contains_rect(old.lay.well, old.wells[folder]), "420x360 still covers the open-folder card");
    }

    #[test]
    fn editor_rows_keep_a_folder_target_and_a_missing_file() {
        let dir = std::env::temp_dir().join(format!("maguro-edit-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let folder = dir.join("box");
        std::fs::create_dir_all(&folder).unwrap();
        let folder_s = folder.to_str().unwrap().to_string();
        let missing = dir.join("gone.txt");
        let missing_s = missing.to_str().unwrap().to_string();
        let json = format!(
            "{{\"version\":1,\"items\":[{{\"type\":\"folder\",\"path\":{},\"icon\":{{\"mode\":\"auto\"}}}},{{\"type\":\"file\",\"path\":{},\"icon\":{{\"mode\":\"auto\"}}}}]}}",
            serde_json::to_string(&folder_s).unwrap(),
            serde_json::to_string(&missing_s).unwrap()
        );
        let doc = crate::toolbar::parse(json.as_bytes()).expect("config");
        let rows = editor_rows(&doc);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].icon, Some(crate::ui::LineIcon::OpenFolder));
        assert_eq!(rows[0].path.as_deref(), Some(folder_s.as_str()));
        assert!(!rows[0].missing, "the folder is on disk");
        assert!(rows[0].tip.contains(&folder_s));
        assert_eq!(rows[1].icon, Some(crate::ui::LineIcon::PinFile));
        assert_eq!(rows[1].label, "gone.txt");
        assert_eq!(rows[1].path.as_deref(), Some(missing_s.as_str()));
        assert!(rows[1].missing, "the file target is not on disk");
        assert!(rows[1].tip.contains(&missing_s));
        let shown = visible_well(&doc);
        assert!(shown.iter().any(|item| matches!(item, Well::OpenFolder)));
        assert!(shown.iter().any(|item| matches!(item, Well::OpenFile)));
        let (w, h) = min_client(96);
        assert_no_overlap(&doc, w, h, 96);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn targets_only() -> Doc {
        let mut doc = crate::toolbar::parse(br#"{"version":1,"items":[]}"#).unwrap();
        doc.insert_folder(0, r"D:\Work");
        doc.insert_file(1, r"D:\missing\a.txt");
        doc
    }

    fn assert_no_overlap(doc: &Doc, w: i32, h: i32, dpi: i32) {
        let sel = (0..doc.item_count()).find(|&i| doc.is_folder_target(i) || doc.is_file_target(i));
        let g = geom_of(doc, sel, 0, 0, w, h, dpi);
        let lay = &g.lay;
        let client = RECT { left: 0, top: 0, right: w, bottom: h };
        let mut bands = vec![("bar", lay.bar), ("hint", lay.hint), ("well", lay.well), ("reset", lay.reset), ("done", lay.done)];
        if sel.is_some() {
            bands.push(("change", lay.change));
            bands.push(("icons", lay.icons));
        }
        for (name, r) in &bands {
            assert!(r.right > r.left && r.bottom > r.top, "{name} is empty at {w}x{h}");
            assert!(contains_rect(client, *r), "{name} leaves the client {r:?}");
        }
        for (i, (an, a)) in bands.iter().enumerate() {
            for (bn, b) in bands.iter().skip(i + 1) {
                assert!(!overlaps(*a, *b), "{an} overlaps {bn}");
            }
        }
        assert_eq!(g.well_max, 0, "the palette scrolls at the minimum");
        let shown = visible_well(doc);
        assert_eq!(g.wells.len(), shown.len());
        for (i, cell) in g.wells.iter().enumerate() {
            assert!(contains_rect(lay.well, *cell), "palette card {i} is clipped by the well");
            assert!(contains_rect(client, *cell), "palette card {i} leaves the client");
            assert!(!overlaps(*cell, lay.reset) && !overlaps(*cell, lay.done), "palette card {i} meets the footer");
            if sel.is_some() {
                assert!(!overlaps(*cell, lay.icons), "palette card {i} meets the icon settings");
            }
        }
        if sel.is_some() {
            let (auto, manual, preview) = icon_controls(lay.icons, dpi);
            assert!(contains_rect(lay.icons, auto) && contains_rect(lay.icons, manual) && contains_rect(lay.icons, preview));
            assert!(auto.right - auto.left >= px(dpi, 72) && manual.right - manual.left >= px(dpi, 72));
        }
        assert!(shown.iter().any(|item| matches!(item, Well::OpenFolder)));
        assert!(shown.iter().any(|item| matches!(item, Well::OpenFile)));
    }
}
