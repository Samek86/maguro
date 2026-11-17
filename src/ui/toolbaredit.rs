//! Customize window in the shape of Finder's toolbar editor.
//! The frame, cards, and buttons are the shared Maguro chrome.
//! The top band is the bar being edited. The well below is the palette.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;

use super::{
    ask_confirm, body_font, draw_line_icon, fill, in_rect, meta_font, paint_button, paint_card, paint_card_fill, paint_meta, paint_segment_group, paint_window, style_popup_chrome, visual_from_pointer, ButtonKind,
    Confirm, FocusRing, SegmentCell, BTN_H_FOOTER,
};
use crate::imports::*;
use crate::toolbar::{Action, Doc};
use crate::ui::tokens::px;

const CLASS: &str = "MaguroToolbarEdit";
const HIT_RESET: i32 = 1;
const HIT_DONE: i32 = 2;
const HIT_AUTO: i32 = 3;
const HIT_MANUAL: i32 = 4;
const HIT_PREVIEW: i32 = 5;
const BAR_BASE: i32 = 100;
const WELL_BASE: i32 = 200;
const ICON_BASE: i32 = 1000;

static LIVE: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Well {
    Action(Action),
    Separator,
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
        Well::Action(Action::Open),
        Well::Action(Action::PaneColor),
        Well::Action(Action::ClosePane),
        Well::Action(Action::Terminal),
    ]
}

/// Items already on the bar stay off the palette. A separator can be added again.
fn visible_well(doc: &Doc) -> Vec<Well> {
    catalog().iter().copied().filter(|item| match item {
        Well::Separator => true,
        Well::Action(action) => !doc.has_action(*action),
    }).collect()
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
    }
}

struct EditLay {
    bar: RECT,
    hint: RECT,
    icons: RECT,
    well: RECT,
    reset: RECT,
    done: RECT,
}

fn edit_layout(w: i32, h: i32, dpi: i32, show_icons: bool) -> EditLay {
    let pad = px(dpi, 12);
    let gap = px(dpi, 6);
    let bar_h = px(dpi, 48);
    let hint_h = px(dpi, 18);
    let btn_h = px(dpi, BTN_H_FOOTER);
    let foot = h - pad - btn_h;
    let done_w = px(dpi, 88);
    let reset_w = px(dpi, 128);
    let hint = RECT { left: pad, top: pad + bar_h + gap, right: w - pad, bottom: pad + bar_h + gap + hint_h };
    let icons = if show_icons {
        RECT { left: pad + reset_w + gap, top: foot, right: (w - pad - done_w - gap).max(pad), bottom: foot + btn_h }
    } else {
        RECT::default()
    };
    let well_top = hint.bottom + gap;
    EditLay {
        bar: RECT { left: pad, top: pad, right: w - pad, bottom: pad + bar_h },
        hint,
        icons,
        well: RECT { left: pad, top: well_top, right: w - pad, bottom: (foot - gap).max(well_top) },
        reset: RECT { left: pad, top: foot, right: pad + reset_w, bottom: foot + btn_h },
        done: RECT { left: (w - pad - done_w).max(pad), top: foot, right: w - pad, bottom: foot + btn_h },
    }
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



fn bar_slots(bar: RECT, n: usize, dpi: i32) -> Vec<RECT> {
    let btn_w = px(dpi, 30);
    let btn_h = px(dpi, 24);
    let gap = px(dpi, 2);
    let y = bar.top + (bar.bottom - bar.top - btn_h).max(0) / 2;
    let mut x = bar.left + px(dpi, 8);
    (0..n)
        .map(|_| {
            let r = RECT { left: x, top: y, right: x + btn_w, bottom: y + btn_h };
            x += btn_w + gap;
            r
        })
        .collect()
}

fn insert_at(slots: &[RECT], x: i32) -> usize {
    for (i, r) in slots.iter().enumerate() {
        if x < (r.left + r.right) / 2 {
            return i;
        }
    }
    slots.len()
}

fn well_slots(well: RECT, n: usize, dpi: i32) -> Vec<RECT> {
    // About six columns in a 560px window. The count follows the well width.
    let cw = px(dpi, 86);
    let ch = px(dpi, 64);
    let gap = px(dpi, 4);
    let cols = ((well.right - well.left + gap) / (cw + gap).max(1)).max(1);
    (0..n)
        .map(|i| {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;
            let left = well.left + col * (cw + gap);
            let top = well.top + row * (ch + gap);
            RECT { left, top, right: left + cw, bottom: top + ch }
        })
        .collect()
}

fn well_label(item: Well) -> String {
    match item {
        Well::Action(action) => crate::tr(crate::toolbar::label_key(action)).to_string(),
        Well::Separator => crate::tr(crate::Key::ToolbarSeparator).to_string(),
    }
}

fn well_icon(item: Well) -> Option<crate::ui::LineIcon> {
    match item {
        Well::Action(action) => Some(crate::toolbar::drawn_icon(action, &crate::toolbar::IconChoice::Auto)),
        Well::Separator => None,
    }
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
}

fn view(h: HWND) -> Option<*mut View> {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut View;
    if p.is_null() { None } else { Some(p) }
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
    let client_w = px(dpi, 560);
    let client_h = px(dpi, 520);
    let (w, h) = outer_size(client_w, client_h, dpi);
    let (x, y) = center_on(owner, w, h);
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        &HSTRING::from(CLASS),
        &HSTRING::from(crate::tr(crate::Key::ToolbarEdit)),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        x,
        y,
        w,
        h,
        Some(owner),
        None,
        None,
        None,
    ) else {
        return;
    };
    let dlg = Box::new(View { doc: Doc::current(), sel: None, hot: 0, down: 0, grab: None, drag: false, origin: (0, 0), insert: None });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(dlg) as isize);
    style_popup_chrome(hwnd);
    crate::note_side(hwnd);
    LIVE.store(hwnd.0 as isize, Ordering::Relaxed);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
}

fn dpi_of(owner: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(owner) as i32 };
    if dpi > 0 { dpi } else { 96 }
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
    let mut r = RECT { left: 0, top: 0, right: client_w, bottom: client_h };
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    if AdjustWindowRectExForDpi(&mut r, style, false, WS_EX_DLGMODALFRAME, dpi.max(96) as u32).is_ok() {
        ((r.right - r.left).max(client_w), (r.bottom - r.top).max(client_h))
    } else {
        (client_w + 16, client_h + 40)
    }
}

fn ensure_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else { return };
        let name: Vec<u16> = CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: m.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(name.as_ptr()),
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
    let seg = px(dpi, 72);
    let auto = RECT { left: band.left, top: band.top, right: band.left + seg, bottom: band.top + h };
    let manual = RECT { left: auto.right, top: band.top, right: auto.right + seg, bottom: band.top + h };
    let preview = RECT { left: manual.right + px(dpi, 8), top: band.top, right: manual.right + px(dpi, 8) + h, bottom: band.top + h };
    (auto, manual, preview)
}

fn hit_id(lay: &EditLay, slots: &[RECT], wells: &[RECT], x: i32, y: i32, dpi: i32) -> i32 {
    if in_rect(lay.reset, x, y) {
        return HIT_RESET;
    }
    if in_rect(lay.done, x, y) {
        return HIT_DONE;
    }
    if lay.icons.bottom > lay.icons.top {
        let (auto, manual, preview) = icon_controls(lay.icons, dpi);
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
    for (i, r) in slots.iter().enumerate() {
        if in_rect(*r, x, y) {
            return BAR_BASE + i as i32;
        }
    }
    for (i, r) in wells.iter().enumerate() {
        if in_rect(*r, x, y) {
            return WELL_BASE + i as i32;
        }
    }
    0
}

unsafe fn on_down(h: HWND, pt: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let id = hit_now(h, pt);
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
    let Some(v) = view(h) else { return };
    let id = hit_now(h, pt);
    let moved = (pt.0 - (*v).origin.0).abs() + (pt.1 - (*v).origin.1).abs() > 4;
    if (*v).down != 0 && moved {
        (*v).drag = true;
    }
    let insert = if (*v).drag {
        let (lay, slots, _, _) = slots_now(h);
        if in_rect(lay.bar, pt.0, pt.1) { Some(insert_at(&slots, pt.0)) } else { None }
    } else {
        None
    };
    if (*v).hot != id || (*v).insert != insert {
        (*v).hot = id;
        (*v).insert = insert;
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn on_up(h: HWND, pt: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let _ = ReleaseCapture();
    let id = hit_now(h, pt);
    let drag = (*v).drag;
    let grab = (*v).grab;
    let down = (*v).down;
    (*v).down = 0;
    (*v).drag = false;
    (*v).grab = None;
    (*v).insert = None;
    if drag {
        if let Some(grab) = grab {
            let (lay, slots, _, _) = slots_now(h);
            let on_bar = in_rect(lay.bar, pt.0, pt.1);
            let at = insert_at(&slots, pt.0);
            apply_drop(&mut (*v).doc, plan_drop(grab, on_bar, at));
        }
    } else if down == id {
        match id {
            HIT_RESET => restore(h),
            HIT_DONE => {
                finish(h);
                return;
            }
            HIT_AUTO => {
                if let Some(sel) = (*v).sel {
                    (*v).doc.set_pack(sel, None);
                }
            }
            HIT_MANUAL => {
                if let Some(sel) = (*v).sel {
                    if !(*v).doc.manual_icon(sel) {
                        open_pack_picker(h, sel);
                    }
                }
            }
            HIT_PREVIEW => {
                if let Some(sel) = (*v).sel {
                    open_pack_picker(h, sel);
                }
            }
            n if (BAR_BASE..WELL_BASE).contains(&n) => {
                let i = (n - BAR_BASE) as usize;
                if i < (*v).doc.item_count() && !(*v).doc.is_separator(i) {
                    (*v).sel = if (*v).sel == Some(i) { None } else { Some(i) };
                } else {
                    (*v).sel = None;
                }
            }
            _ => {}
        }
    }
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn restore(h: HWND) {
    let ask = Confirm::ok(crate::tr(crate::Key::ToolbarEdit), crate::tr(crate::Key::ToolbarRestoreAsk)).affirm_label(crate::tr(crate::Key::ToolbarRestore));
    if !ask_confirm(h, ask) {
        return;
    }
    if let Some(v) = view(h) {
        (*v).doc.reset_default();
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

unsafe fn slots_now(h: HWND) -> (EditLay, Vec<RECT>, Vec<RECT>, Vec<RECT>) {
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let (n, shown, icons_n) = view(h).map(|v| ((*v).doc.item_count(), visible_well(&(*v).doc).len(), chosen_icon_count(&(*v).doc, (*v).sel))).unwrap_or((0, 0, 0));
    let lay = edit_layout(rc.right, rc.bottom, dpi, icons_n > 0);
    let slots = bar_slots(lay.bar, n, dpi);
    let wells = well_slots(lay.well, shown, dpi);
    (lay, slots, wells, Vec::new())
}

unsafe fn hit_now(h: HWND, pt: (i32, i32)) -> i32 {
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let (lay, slots, wells, _) = slots_now(h);
    hit_id(&lay, &slots, &wells, pt.0, pt.1, dpi)
}

unsafe fn paint(h: HWND) {
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let (hot, down, doc, insert, sel) = match view(h) {
        Some(v) => ((*v).hot, (*v).down, (*v).doc.clone(), (*v).insert, (*v).sel),
        None => return,
    };
    crate::buffered(h, |dc, rc| {
        paint_window(dc, rc);
        let show_icons = chosen_icon_count(&doc, sel) > 0;
        let lay = edit_layout(rc.right, rc.bottom, dpi, show_icons);
        paint_card(dc, lay.bar, dpi);
        let slots = bar_slots(lay.bar, doc.item_count(), dpi);
        for (i, r) in slots.iter().enumerate() {
            if sel == Some(i) {
                fill(dc, *r, crate::blend(crate::ui_tokens().bg, crate::live_accent(), 0.16));
            } else if hot == BAR_BASE + i as i32 {
                fill(dc, *r, crate::ui_tokens().card);
            }
            if doc.is_separator(i) {
                let mid = (r.left + r.right) / 2;
                fill(dc, RECT { left: mid, top: r.top + 4, right: mid + 1, bottom: r.bottom - 4 }, crate::ui_tokens().line);
            } else if let Some(kind) = doc.glyph_at(i) {
                draw_line_icon(dc, *r, kind, crate::ui_tokens().text, dpi);
            }
        }
        if let Some(at) = insert {
            let x = slots.get(at).map(|r| r.left - px(dpi, 2)).unwrap_or_else(|| slots.last().map(|r| r.right + px(dpi, 1)).unwrap_or(lay.bar.left + px(dpi, 8)));
            let y = lay.bar.top + px(dpi, 10);
            fill(dc, RECT { left: x, top: y, right: x + px(dpi, 2), bottom: lay.bar.bottom - px(dpi, 10) }, crate::live_accent());
        }
        paint_meta(dc, lay.hint, crate::tr(crate::Key::ToolbarEditHint), meta_font());
        if show_icons {
            paint_icon_choice(dc, lay.icons, &doc, sel, hot, down, dpi);
        }
        let shown = visible_well(&doc);
        let wells = well_slots(lay.well, shown.len(), dpi);
        let body = body_font();
        let small = meta_font();
        for (i, item) in shown.iter().enumerate() {
            let Some(cell) = wells.get(i) else { continue };
            let id = WELL_BASE + i as i32;
            paint_card(dc, *cell, dpi);
            if hot == id {
                fill(dc, *cell, crate::ui_tokens().card);
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
        let ground = crate::ui_tokens().bg;
        paint_button(dc, lay.reset, ButtonKind::Secondary, visual_from_pointer(HIT_RESET, Some(hot).filter(|&h| h != 0), Some(down).filter(|&d| d != 0)), crate::tr(crate::Key::ToolbarRestore), body, dpi, ground, FocusRing::Outside);
        paint_button(dc, lay.done, ButtonKind::Primary, visual_from_pointer(HIT_DONE, Some(hot).filter(|&h| h != 0), Some(down).filter(|&d| d != 0)), crate::tr(crate::Key::ToolbarDone), body, dpi, ground, FocusRing::Outside);
        let _ = body;
    });
}

unsafe fn paint_icon_choice(dc: HDC, band: RECT, doc: &Doc, sel: Option<usize>, hot: i32, down: i32, dpi: i32) {
    let Some(i) = sel else { return };
    let (auto, manual, preview) = icon_controls(band, dpi);
    let manual_on = doc.manual_icon(i);
    let cells = [
        SegmentCell { rect: auto, label: crate::tr(crate::Key::ToolbarIconDefault), disabled: false },
        SegmentCell { rect: manual, label: crate::tr(crate::Key::ToolbarIconSet), disabled: false },
    ];
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
        draw_line_icon(dc, slot, kind, crate::ui_tokens().text, dpi);
    }
}

unsafe fn open_pack_picker(editor: HWND, index: usize) {
    ensure_pack_class();
    let dpi = GetDpiForWindow(editor).max(96) as i32;
    let client_w = px(dpi, 360);
    let client_h = px(dpi, 280);
    let (w, h) = outer_size(client_w, client_h, dpi);
    let (x, y) = center_on(editor, w, h);
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        w!("MaguroIconPack"),
        &HSTRING::from(crate::tr(crate::Key::IconCustom)),
        WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        x,
        y,
        w,
        h,
        Some(editor),
        None,
        None,
        None,
    ) else {
        return;
    };
    let pick = Box::new(Pack { editor, index });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(pick) as isize);
    style_popup_chrome(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOW);
}

struct Pack {
    editor: HWND,
    index: usize,
}

fn ensure_pack_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else { return };
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(pack_proc),
            hInstance: m.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MaguroIconPack"),
            ..Default::default()
        });
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
        WM_LBUTTONUP => {
            choose_pack(h, point(lp));
            LRESULT(0)
        }
        WM_CLOSE | WM_DESTROY => {
            let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pack;
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            if !p.is_null() {
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
    }

    #[test]
    fn the_well_hides_commands_already_on_the_bar_and_keeps_the_separator() {
        let doc = Doc::default_layout();
        let well = visible_well(&doc);
        assert!(well.contains(&Well::Separator));
        assert!(!well.contains(&Well::Action(Action::NewFolder)));
        assert!(well.contains(&Well::Action(Action::CopyPath)));
        assert!(well.contains(&Well::Action(Action::NewTab)));
    }

    #[test]
    fn the_bar_gap_is_the_midpoint_of_a_slot() {
        let slots = [RECT { left: 10, top: 0, right: 40, bottom: 10 }, RECT { left: 42, top: 0, right: 72, bottom: 10 }];
        assert_eq!(insert_at(&slots, 20), 0);
        assert_eq!(insert_at(&slots, 30), 1);
        assert_eq!(insert_at(&slots, 80), 2);
    }
}
