//! Tall grid of icons from `imageres.dll` and `shell32.dll`.
//!
//! The window opens before any image is extracted. A worker pulls only the rows
//! on screen (and a few around them) with `PrivateExtractIconsW`. The file browse
//! on the item dialog stays for an icon that is not in either library.
//! Choosing one writes the same `ItemIcon` path and index the dialog already stores.

use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicU64};
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;

use crate::i18n::tr;
use crate::imports::*;

use super::{
    body_font, bold_ui_font, buffered, paint_button, paint_section_header, paint_segment, paint_window, style_popup_chrome, visual_from_pointer, ButtonKind,
    FocusRing, BTN_GAP, BTN_H_FOOTER, FONT_PX, PAD_BODY_X, PAD_BODY_Y, SPACE_4, SPACE_8, px,
};

const ICON_READY: u32 = WM_APP + 80;
const PICK_W_DIP: i32 = 320;
const PICK_H_DIP: i32 = 520;
const ICON_DIP: i32 = 32;
const CELL_EXTRA_DIP: i32 = SPACE_8;
const HIT_SRC0: i32 = 1;
const HIT_SRC1: i32 = 2;
const HIT_OK: i32 = 3;
const HIT_CANCEL: i32 = 4;
const SOURCE_FILE: [&str; 2] = ["imageres.dll", "shell32.dll"];
const SOURCE_LABEL: [&str; 2] = ["imageres", "shell32"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickedIcon {
    pub(crate) path: String,
    pub(crate) index: i32,
}

struct Shared {
    gen: AtomicU64,
    start: AtomicI32,
    end: AtomicI32,
    side: AtomicI32,
    hwnd: AtomicIsize,
    path: Mutex<String>,
    icons: Mutex<Vec<Option<isize>>>,
    mu: Mutex<()>,
    wake: Condvar,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let mut slots = self.icons.lock().unwrap_or_else(|e| e.into_inner());
        free_slots(&mut slots);
    }
}

struct Pick {
    dpi: i32,
    source: usize,
    counts: [i32; 2],
    paths: [String; 2],
    remember: [i32; 2],
    sel: i32,
    scroll: i32,
    pin: bool,
    hot: i32,
    down: i32,
    shared: Arc<Shared>,
    out: Arc<Mutex<Option<PickedIcon>>>,
}

#[derive(Clone, Copy, Debug)]
struct PickLay {
    caption: RECT,
    sources: [RECT; 2],
    body: RECT,
    ok: RECT,
    cancel: RECT,
}

pub(crate) unsafe fn pick_system_icon(owner: HWND, current_path: &str, current_index: i32) -> Option<PickedIcon> {
    let paths = [system_dll(SOURCE_FILE[0]), system_dll(SOURCE_FILE[1])];
    let counts = [icon_count(&paths[0]), icon_count(&paths[1])];
    let source = system_icon_source(current_path).unwrap_or(0);
    let sel = match system_icon_source(current_path) {
        Some(i) if i == source && current_index >= 0 && current_index < counts[i] => current_index,
        _ => -1,
    };
    let shared = Arc::new(Shared {
        gen: AtomicU64::new(1),
        start: AtomicI32::new(0),
        end: AtomicI32::new(0),
        side: AtomicI32::new(32),
        hwnd: AtomicIsize::new(0),
        path: Mutex::new(paths[source].clone()),
        icons: Mutex::new(vec![None; counts[source].max(0) as usize]),
        mu: Mutex::new(()),
        wake: Condvar::new(),
    });
    let out = Arc::new(Mutex::new(None));
    let mut remember = [-1, -1];
    remember[source] = sel;
    ensure_class();
    let dpi0 = dpi_of(owner);
    let (w, h) = outer_size(px(dpi0, PICK_W_DIP), px(dpi0, PICK_H_DIP), dpi0);
    let (x, y) = center_on(owner, w, h);
    let style = WS_POPUP | WS_CLIPCHILDREN | WS_VSCROLL;
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        w!("MaguroIconPick"),
        &HSTRING::from(tr(crate::Key::ItemIcon)),
        style,
        x,
        y,
        w,
        h,
        Some(owner),
        None,
        None,
        None,
    ) else {
        return None;
    };
    let dpi = {
        let n = GetDpiForWindow(hwnd) as i32;
        if n > 0 { n } else { dpi0 }
    };
    shared.side.store(px(dpi, ICON_DIP).max(16), Ordering::Release);
    shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
    let pick = Pick {
        dpi,
        source,
        counts,
        paths,
        remember,
        sel,
        scroll: 0,
        pin: sel >= 0,
        hot: -1,
        down: -1,
        shared: Arc::clone(&shared),
        out: Arc::clone(&out),
    };
    let ptr = Box::into_raw(Box::new(pick));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
    style_popup_chrome(hwnd);
    sync_scroll(hwnd);
    let worker = Arc::clone(&shared);
    let _ = std::thread::Builder::new().name("maguro-iconpick".into()).spawn(move || loader(worker));
    let was_disabled = EnableWindow(owner, false).as_bool();
    _ = ShowWindow(hwnd, SW_SHOW);
    _ = SetForegroundWindow(hwnd);
    _ = SetFocus(Some(hwnd));
    let mut msg = MSG::default();
    loop {
        let r = GetMessageW(&mut msg, None, 0, 0);
        if !r.as_bool() {
            if msg.message == WM_QUIT {
                PostQuitMessage(msg.wParam.0 as i32);
            }
            if IsWindow(Some(hwnd)).as_bool() {
                _ = DestroyWindow(hwnd);
            }
            break;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if !IsWindow(Some(hwnd)).as_bool() {
            break;
        }
    }
    if !was_disabled {
        let _ = EnableWindow(owner, true);
    }
    _ = SetForegroundWindow(owner);
    let picked = out.lock().unwrap_or_else(|e| e.into_inner()).clone();
    picked
}

fn system_dll(file: &str) -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    format!(r"{root}\System32\{file}")
}

pub(crate) fn system_icon_source(path: &str) -> Option<usize> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    SOURCE_FILE.iter().position(|file| name.eq_ignore_ascii_case(file))
}

fn icon_count(path: &str) -> i32 {
    if path.is_empty() {
        return 0;
    }
    let n = unsafe { ExtractIconExW(&HSTRING::from(path), -1, None, None, 0) };
    if n == 0 || n > 20_000 { 0 } else { n as i32 }
}

fn pick_layout(dpi: i32, w: i32, h: i32) -> PickLay {
    let s = |v: i32| px(dpi, v);
    let pad = s(PAD_BODY_X);
    let gap = s(SPACE_8);
    let right = (w - pad).max(pad + 1);
    let mut y = pad;
    let caption = RECT { left: pad, top: y, right, bottom: y + s(FONT_PX + SPACE_4) };
    y = caption.bottom + gap;
    let seg_h = s(BTN_H_FOOTER);
    let inner = (right - pad).max(1);
    let seg_w = (inner - gap) / 2;
    let sources = [
        RECT { left: pad, top: y, right: pad + seg_w, bottom: y + seg_h },
        RECT { left: pad + seg_w + gap, top: y, right, bottom: y + seg_h },
    ];
    y = sources[0].bottom + gap;
    let btn_h = s(BTN_H_FOOTER);
    let footer_top = (h - s(PAD_BODY_Y) - btn_h).max(y + s(32));
    let body = RECT { left: pad, top: y, right, bottom: (footer_top - gap).max(y + 1) };
    let bw = (inner - s(BTN_GAP)) / 2;
    let ok = RECT { left: pad, top: footer_top, right: pad + bw, bottom: footer_top + btn_h };
    let cancel = RECT { left: ok.right + s(BTN_GAP), top: footer_top, right, bottom: footer_top + btn_h };
    PickLay { caption, sources, body, ok, cancel }
}

fn grid_metrics(body_w: i32, dpi: i32) -> (i32, i32, i32) {
    let gap = px(dpi, SPACE_4).max(1);
    let cell = px(dpi, ICON_DIP + CELL_EXTRA_DIP).max(1);
    let inner = body_w.max(1);
    let cols = ((inner + gap) / (cell + gap)).max(1);
    (cols, cell, gap)
}

fn content_px(count: i32, cols: i32, stride: i32) -> i32 {
    if count <= 0 || cols <= 0 || stride <= 0 {
        return 0;
    }
    ((count + cols - 1) / cols) * stride
}

fn cell_origin(body: RECT, index: i32, cols: i32, cell: i32, gap: i32, scroll: i32) -> RECT {
    let cols = cols.max(1);
    let stride = cell + gap;
    let col = index.rem_euclid(cols);
    let row = index.div_euclid(cols);
    let left = body.left + col * stride;
    let top = body.top + row * stride - scroll;
    RECT { left, top, right: left + cell, bottom: top + cell }
}

fn cell_index_at(body: RECT, x: i32, y: i32, cols: i32, cell: i32, gap: i32, scroll: i32, count: i32) -> Option<i32> {
    if cols <= 0 || cell <= 0 || count <= 0 {
        return None;
    }
    if x < body.left || x >= body.right || y < body.top || y >= body.bottom {
        return None;
    }
    let stride = cell + gap.max(0);
    if stride <= 0 {
        return None;
    }
    let rel_x = x - body.left;
    let rel_y = y - body.top + scroll;
    if rel_x < 0 || rel_y < 0 {
        return None;
    }
    let col = rel_x / stride;
    let row = rel_y / stride;
    if col >= cols || rel_x % stride >= cell || rel_y % stride >= cell {
        return None;
    }
    let index = row * cols + col;
    (index >= 0 && index < count).then_some(index)
}

fn scroll_into(index: i32, cols: i32, stride: i32, view: i32, scroll: i32, content: i32) -> i32 {
    let view = view.max(1);
    let max_s = (content - view).max(0);
    if cols <= 0 || stride <= 0 || index < 0 {
        return scroll.clamp(0, max_s);
    }
    let top = (index / cols) * stride;
    let bottom = top + stride;
    let mut s = scroll;
    if top < s {
        s = top;
    }
    if bottom > s + view {
        s = bottom - view;
    }
    s.clamp(0, max_s)
}

fn icon_span(scroll: i32, view: i32, stride: i32, cols: i32, count: i32, pad_rows: i32) -> (i32, i32) {
    if count <= 0 || cols <= 0 || stride <= 0 {
        return (0, 0);
    }
    let view = view.max(1);
    let scroll = scroll.max(0);
    let pad = pad_rows.max(0);
    let first_row = (scroll / stride - pad).max(0);
    let last_row = (scroll + view - 1) / stride + pad;
    let start = (first_row * cols).min(count);
    let end = ((last_row + 1) * cols).clamp(start, count);
    (start, end)
}

fn view(hwnd: HWND) -> Option<*mut Pick> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Pick;
    if p.is_null() { None } else { Some(p) }
}

fn button_at(lay: &PickLay, x: i32, y: i32) -> i32 {
    let pairs = [(lay.sources[0], HIT_SRC0), (lay.sources[1], HIT_SRC1), (lay.ok, HIT_OK), (lay.cancel, HIT_CANCEL)];
    for (r, id) in pairs {
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return id;
        }
    }
    -1
}

fn grid_of(v: &Pick, rc: RECT) -> (PickLay, i32, i32, i32, i32) {
    let lay = pick_layout(v.dpi, (rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1));
    let (cols, cell, gap) = grid_metrics((lay.body.right - lay.body.left).max(1), v.dpi);
    (lay, cols, cell, gap, cell + gap)
}

unsafe fn client_grid(h: HWND) -> Option<(PickLay, i32, i32, i32, i32)> {
    let v = view(h)?;
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    Some(grid_of(&*v, rc))
}

unsafe fn sync_scroll(h: HWND) {
    let Some(v) = view(h) else { return };
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let (lay, cols, _cell, _gap, stride) = grid_of(&*v, rc);
    let view_h = (lay.body.bottom - lay.body.top).max(1);
    let count = (*v).counts[(*v).source];
    let content = content_px(count, cols, stride);
    let max_s = (content - view_h).max(0);
    if (*v).pin {
        (*v).scroll = if (*v).sel >= 0 { scroll_into((*v).sel, cols, stride, view_h, 0, content) } else { 0 };
        (*v).pin = false;
    }
    (*v).scroll = (*v).scroll.clamp(0, max_s);
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: (content - 1).max(0),
        nPage: view_h as u32,
        nPos: (*v).scroll,
        nTrackPos: 0,
    };
    let _ = windows::Win32::UI::Controls::SetScrollInfo(h, SB_VERT, &info, true);
    let (start, end) = icon_span((*v).scroll, view_h, stride, cols, count, 3);
    let shared = Arc::clone(&(*v).shared);
    shared.start.store(start, Ordering::Release);
    shared.end.store(end, Ordering::Release);
    trim_outside(&shared, start, end);
    wake(&shared);
}

unsafe fn use_source(h: HWND, which: usize) {
    let Some(v) = view(h) else { return };
    if which > 1 || (*v).source == which {
        return;
    }
    let prev = (*v).source;
    (*v).remember[prev] = (*v).sel;
    (*v).source = which;
    let count = (*v).counts[which];
    let sel = (*v).remember[which];
    (*v).sel = if sel >= 0 && sel < count { sel } else { -1 };
    (*v).scroll = 0;
    (*v).pin = true;
    let shared = Arc::clone(&(*v).shared);
    {
        let mut path = shared.path.lock().unwrap_or_else(|e| e.into_inner());
        *path = (*v).paths[which].clone();
    }
    shared.gen.fetch_add(1, Ordering::Release);
    resize_icons(&shared, count);
    sync_scroll(h);
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn accept(h: HWND) {
    let Some(v) = view(h) else { return };
    let sel = (*v).sel;
    let source = (*v).source;
    if sel < 0 || sel >= (*v).counts[source] {
        return;
    }
    let picked = PickedIcon { path: (*v).paths[source].clone(), index: sel };
    if let Ok(mut slot) = (*v).out.lock() {
        *slot = Some(picked);
    }
    _ = DestroyWindow(h);
}

unsafe fn on_scroll(h: HWND, wp: WPARAM) {
    let Some(v) = view(h) else { return };
    let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
    let _ = GetScrollInfo(h, SB_VERT, &mut si);
    let Some((_, _, _, _, stride)) = client_grid(h) else { return };
    let cur = (*v).scroll;
    let want = match SCROLLBAR_COMMAND((wp.0 & 0xFFFF) as i32) {
        SB_LINEUP => cur - stride,
        SB_LINEDOWN => cur + stride,
        SB_PAGEUP => cur - si.nPage as i32,
        SB_PAGEDOWN => cur + si.nPage as i32,
        SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos,
        SB_TOP => 0,
        SB_BOTTOM => i32::MAX,
        _ => cur,
    };
    (*v).scroll = want;
    (*v).pin = false;
    sync_scroll(h);
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe extern "system" fn pick_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            sync_scroll(h);
            LRESULT(0)
        }
        ICON_READY => {
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_CLOSE => {
            _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_DESTROY => {
            drop_view(h);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(h);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            point_move(h, lp_xy(lp));
            arm_leave(h);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(v) = view(h) {
                if (*v).hot != -1 {
                    (*v).hot = -1;
                    let _ = InvalidateRect(Some(h), None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            _ = SetFocus(Some(h));
            pointer_down(h, lp_xy(lp));
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            if let Some(i) = cell_at(h, lp_xy(lp)) {
                if let Some(v) = view(h) {
                    (*v).sel = i;
                }
                accept(h);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            pointer_up(h, lp_xy(lp));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) as i16 as i32) / 120;
            if delta != 0 {
                if let Some((_, _, _, _, stride)) = client_grid(h) {
                    if let Some(v) = view(h) {
                        (*v).scroll -= delta * stride * 3;
                        (*v).pin = false;
                    }
                    sync_scroll(h);
                    let _ = InvalidateRect(Some(h), None, false);
                }
            }
            LRESULT(0)
        }
        WM_VSCROLL => {
            on_scroll(h, wp);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
            _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize => {
            accept(h);
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let hit = DefWindowProcW(h, msg, wp, lp);
            if hit.0 == HTCLIENT as isize {
                let mut pt = POINT { x: (lp.0 & 0xffff) as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as i16 as i32 };
                if ScreenToClient(h, &mut pt).as_bool() {
                    if let Some((lay, _, _, _, _)) = client_grid(h) {
                        if pt.y >= lay.caption.top && pt.y < lay.caption.bottom && pt.x >= lay.caption.left && pt.x < lay.caption.right {
                            return LRESULT(HTCAPTION as isize);
                        }
                    }
                }
            }
            hit
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn cell_at(h: HWND, xy: (i32, i32)) -> Option<i32> {
    let v = view(h)?;
    let (lay, cols, cell, gap, _) = client_grid(h)?;
    let count = (*v).counts[(*v).source];
    cell_index_at(lay.body, xy.0, xy.1, cols, cell, gap, (*v).scroll, count)
}

unsafe fn pointer_down(h: HWND, xy: (i32, i32)) {
    let Some(v) = view(h) else { return };
    if let Some(i) = cell_at(h, xy) {
        (*v).sel = i;
        (*v).down = -1;
        let _ = InvalidateRect(Some(h), None, false);
        return;
    }
    let Some((lay, _, _, _, _)) = client_grid(h) else { return };
    let id = button_at(&lay, xy.0, xy.1);
    (*v).down = id;
    (*v).hot = id;
    if id >= 0 {
        _ = SetCapture(h);
    }
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn pointer_up(h: HWND, xy: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let Some((lay, _, _, _, _)) = client_grid(h) else { return };
    let id = button_at(&lay, xy.0, xy.1);
    let fired = (*v).down == id && id >= 0;
    (*v).down = -1;
    (*v).hot = id;
    let _ = ReleaseCapture();
    let _ = InvalidateRect(Some(h), None, false);
    if !fired {
        return;
    }
    match id {
        HIT_SRC0 => use_source(h, 0),
        HIT_SRC1 => use_source(h, 1),
        HIT_OK => accept(h),
        HIT_CANCEL => _ = DestroyWindow(h),
        _ => {}
    }
}

unsafe fn point_move(h: HWND, xy: (i32, i32)) {
    let Some(v) = view(h) else { return };
    let Some((lay, _, _, _, _)) = client_grid(h) else { return };
    let id = button_at(&lay, xy.0, xy.1);
    if (*v).hot != id {
        (*v).hot = id;
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn paint(h: HWND) {
    let Some(v) = view(h) else { return };
    let dpi = (*v).dpi;
    let source = (*v).source;
    let sel = (*v).sel;
    let scroll = (*v).scroll;
    let hot = (*v).hot;
    let down = (*v).down;
    let count = (*v).counts[source];
    let shared = Arc::clone(&(*v).shared);
    buffered(h, |dc, rc| {
        let (lay, cols, cell, gap, _stride) = grid_of(unsafe { &*v }, rc);
        paint_window(dc, rc);
        let body = body_font();
        let bold = bold_ui_font();
        let ground = super::ui_tokens().bg;
        let hot_id = (hot >= 0).then_some(hot);
        let down_id = (down >= 0).then_some(down);
        paint_section_header(dc, lay.caption, tr(crate::Key::ItemIcon), bold);
        for i in 0..2 {
            let id = if i == 0 { HIT_SRC0 } else { HIT_SRC1 };
            paint_segment(dc, lay.sources[i], SOURCE_LABEL[i], source == i, visual_from_pointer(id, hot_id, down_id), dpi, ground);
        }
        let saved = SaveDC(dc);
        if lay.body.bottom > lay.body.top && lay.body.right > lay.body.left {
            let _ = IntersectClipRect(dc, lay.body.left, lay.body.top, lay.body.right, lay.body.bottom);
        }
        let side = px(dpi, ICON_DIP).min(cell).max(1);
        let t = super::ui_tokens();
        if sel >= 0 && sel < count {
            let r = cell_origin(lay.body, sel, cols, cell, gap, scroll);
            if r.bottom > lay.body.top && r.top < lay.body.bottom {
                crate::fill(dc, r, t.sel_mist());
                crate::frame(dc, r, px(dpi, 1).max(1), crate::live_accent());
            }
        }
        let draw = {
            let slots = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
            let mut draw = Vec::new();
            let view_h = (lay.body.bottom - lay.body.top).max(1);
            let (from, to) = icon_span(scroll, view_h, cell + gap, cols, count, 1);
            for index in from..to {
                let Some(bits) = slots.get(index as usize).and_then(|s| *s) else { continue };
                if bits == 0 {
                    continue;
                }
                let r = cell_origin(lay.body, index, cols, cell, gap, scroll);
                if r.bottom <= lay.body.top || r.top >= lay.body.bottom {
                    continue;
                }
                draw.push((r, bits));
            }
            draw
        };
        for (r, bits) in draw {
            let icon = HICON(bits as *mut _);
            let x = r.left + (cell - side) / 2;
            let y = r.top + (cell - side) / 2;
            let _ = DrawIconEx(dc, x, y, icon, side, side, 0, None, DI_NORMAL);
        }
        if saved != 0 {
            let _ = RestoreDC(dc, saved);
        }
        paint_button(dc, lay.ok, ButtonKind::Primary, visual_from_pointer(HIT_OK, hot_id, down_id), tr(crate::Key::Ok), bold, dpi, ground, FocusRing::Outside);
        paint_button(
            dc,
            lay.cancel,
            ButtonKind::Secondary,
            visual_from_pointer(HIT_CANCEL, hot_id, down_id),
            tr(crate::Key::Cancel),
            body,
            dpi,
            ground,
            FocusRing::Outside,
        );
    });
}

fn wake(shared: &Shared) {
    let _guard = shared.mu.lock().unwrap_or_else(|e| e.into_inner());
    shared.wake.notify_one();
}

fn park(shared: &Shared, gen: u64, start: i32, end: i32) -> bool {
    let guard = shared.mu.lock().unwrap_or_else(|e| e.into_inner());
    if shared.gen.load(Ordering::Acquire) == 0 {
        return false;
    }
    if shared.gen.load(Ordering::Acquire) != gen || shared.start.load(Ordering::Acquire) != start || shared.end.load(Ordering::Acquire) != end {
        return true;
    }
    let _ = shared.wake.wait_timeout(guard, Duration::from_millis(400));
    shared.gen.load(Ordering::Acquire) != 0
}

fn resize_icons(shared: &Shared, count: i32) {
    let mut slots = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
    free_slots(&mut slots);
    slots.clear();
    slots.resize(count.max(0) as usize, None);
}

fn trim_outside(shared: &Shared, start: i32, end: i32) {
    let mut slots = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
    for (i, slot) in slots.iter_mut().enumerate() {
        let i = i as i32;
        if i < start || i >= end {
            if let Some(bits) = slot.take() {
                destroy_bits(bits);
            }
        }
    }
}

fn free_slots(slots: &mut [Option<isize>]) {
    for slot in slots.iter_mut() {
        if let Some(bits) = slot.take() {
            destroy_bits(bits);
        }
    }
}

fn destroy_bits(bits: isize) {
    if bits != 0 {
        unsafe { let _ = DestroyIcon(HICON(bits as *mut _)); }
    }
}

fn discard(icons: Vec<HICON>) {
    for icon in icons {
        if !icon.is_invalid() {
            unsafe { let _ = DestroyIcon(icon); }
        }
    }
}

fn first_missing(slots: &[Option<isize>], start: i32, end: i32) -> Option<i32> {
    let mut i = start.max(0);
    let end = end.max(i);
    while i < end {
        match slots.get(i as usize) {
            Some(None) => return Some(i),
            Some(_) => i += 1,
            None => return None,
        }
    }
    None
}

fn store_chunk(shared: &Shared, gen: u64, start: i32, icons: Vec<HICON>) -> bool {
    let mut slots = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
    if shared.gen.load(Ordering::Acquire) != gen {
        drop(slots);
        discard(icons);
        return false;
    }
    let req_s = shared.start.load(Ordering::Acquire);
    let req_e = shared.end.load(Ordering::Acquire);
    let mut wrote = false;
    for (k, icon) in icons.into_iter().enumerate() {
        let idx = start as usize + k;
        let inside = (idx as i32) >= req_s && (idx as i32) < req_e;
        if idx >= slots.len() || !inside {
            if !icon.is_invalid() {
                unsafe { let _ = DestroyIcon(icon); }
            }
            continue;
        }
        if slots[idx].is_some() {
            if !icon.is_invalid() {
                unsafe { let _ = DestroyIcon(icon); }
            }
            continue;
        }
        slots[idx] = Some(if icon.is_invalid() { 0 } else { icon.0 as isize });
        wrote = true;
    }
    wrote
}

fn wide_fixed(path: &str) -> [u16; 260] {
    let mut buf = [0u16; 260];
    for (i, unit) in path.encode_utf16().take(259).enumerate() {
        buf[i] = unit;
    }
    buf
}

fn extract_range(path: &str, start: i32, n: i32, side: i32) -> Vec<HICON> {
    let n = n.max(0) as usize;
    let mut icons = vec![HICON::default(); n];
    if icons.is_empty() {
        return icons;
    }
    let name = wide_fixed(path);
    let _got = unsafe { PrivateExtractIconsW(&name, start, side.max(16), side.max(16), Some(&mut icons), None, 0) };
    icons
}

fn loader(shared: Arc<Shared>) {
    loop {
        let gen = shared.gen.load(Ordering::Acquire);
        if gen == 0 {
            return;
        }
        let start = shared.start.load(Ordering::Acquire);
        let end = shared.end.load(Ordering::Acquire);
        let side = shared.side.load(Ordering::Acquire).max(16);
        let hwnd_bits = shared.hwnd.load(Ordering::Acquire);
        let path = shared.path.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if path.is_empty() || start >= end || hwnd_bits == 0 {
            if !park(&shared, gen, start, end) {
                return;
            }
            continue;
        }
        let missing = {
            let slots = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
            first_missing(&slots, start, end)
        };
        let Some(at) = missing else {
            if !park(&shared, gen, start, end) {
                return;
            }
            continue;
        };
        if shared.gen.load(Ordering::Acquire) != gen {
            continue;
        }
        let n = 12.min(end - at);
        if n <= 0 {
            if !park(&shared, gen, start, end) {
                return;
            }
            continue;
        }
        let icons = extract_range(&path, at, n, side);
        if shared.gen.load(Ordering::Acquire) != gen {
            discard(icons);
            continue;
        }
        if store_chunk(&shared, gen, at, icons) && hwnd_bits != 0 {
            let hwnd = HWND(hwnd_bits as *mut _);
            let _ = unsafe { PostMessageW(Some(hwnd), ICON_READY, WPARAM(0), LPARAM(0)) };
        }
    }
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
    let style = WS_POPUP | WS_CLIPCHILDREN | WS_VSCROLL;
    if AdjustWindowRectExForDpi(&mut r, style, false, WS_EX_DLGMODALFRAME, dpi.max(96) as u32).is_ok() {
        ((r.right - r.left).max(client_w), (r.bottom - r.top).max(client_h))
    } else {
        (client_w, client_h)
    }
}

fn ensure_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else { return };
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(pick_proc),
            hInstance: m.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MaguroIconPick"),
            ..Default::default()
        });
    });
}

unsafe fn drop_view(h: HWND) {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Pick;
    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
    if p.is_null() {
        return;
    }
    let pick = Box::from_raw(p);
    pick.shared.gen.store(0, Ordering::Release);
    wake(&pick.shared);
}

unsafe fn arm_leave(h: HWND) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: h,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut t);
}

fn lp_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_window_is_tall_and_the_grid_fits() {
        assert!((280..=360).contains(&PICK_W_DIP));
        assert!((480..=560).contains(&PICK_H_DIP));
        for dpi in [96, 144, 192] {
            let w = px(dpi, PICK_W_DIP);
            let h = px(dpi, PICK_H_DIP);
            let lay = pick_layout(dpi, w, h);
            let bw = lay.body.right - lay.body.left;
            let bh = lay.body.bottom - lay.body.top;
            assert!(bh > bw, "body {bw}x{bh} at {dpi}");
            assert!(lay.body.bottom <= lay.ok.top, "{dpi}");
            assert!(lay.ok.right < lay.cancel.left, "{dpi}");
            assert!(lay.cancel.right <= w, "{dpi}");
            assert!(lay.sources[0].right <= lay.sources[1].left, "{dpi}");
            assert_eq!(lay.ok.bottom - lay.ok.top, px(dpi, BTN_H_FOOTER), "{dpi}");
            assert_eq!(lay.sources[0].bottom - lay.sources[0].top, px(dpi, BTN_H_FOOTER), "{dpi}");
            let (cols, cell, gap) = grid_metrics(bw, dpi);
            assert!(cols >= 4, "{dpi} cols {cols}");
            assert_eq!(cell, px(dpi, ICON_DIP + CELL_EXTRA_DIP));
            let last = cell_origin(lay.body, cols - 1, cols, cell, gap, 0);
            assert!(last.right <= lay.body.right, "{dpi} {:?}", last);
            assert!(last.left >= lay.body.left, "{dpi}");
            let used = cols * cell + (cols - 1) * gap;
            assert!(used <= bw, "{dpi} used {used} body {bw}");
        }
    }

    #[test]
    fn scroll_selection_and_hits_follow_the_grid() {
        let body = RECT { left: 0, top: 0, right: 200, bottom: 200 };
        assert_eq!(cell_index_at(body, 10, 10, 4, 40, 4, 0, 20), Some(0));
        assert_eq!(cell_index_at(body, 41, 10, 4, 40, 4, 0, 20), None, "gap");
        assert_eq!(cell_index_at(body, 44, 10, 4, 40, 4, 0, 20), Some(1));
        assert_eq!(cell_index_at(body, 10, 41, 4, 40, 4, 0, 20), None, "row gap");
        assert_eq!(cell_index_at(body, 10, 44, 4, 40, 4, 0, 20), Some(4));
        assert_eq!(cell_index_at(body, 10, 10, 4, 40, 4, 0, 0), None);
        let content = content_px(20, 4, 44);
        assert_eq!(scroll_into(10, 4, 44, 100, 0, content), 32);
        assert_eq!(scroll_into(0, 4, 44, 100, 0, content), 0);
        assert_eq!(scroll_into(1, 4, 44, 100, 0, content), 0);
        let (start, end) = icon_span(0, 100, 44, 4, 10, 3);
        assert_eq!(start, 0);
        assert!(end <= 10 && end >= start);
        let (wide_start, wide_end) = icon_span(0, 100, 44, 4, 100, 2);
        assert_eq!(wide_start, 0);
        assert!(wide_end > 4 && wide_end <= 100);
    }

    #[test]
    fn system_dll_names_map_to_the_two_sources() {
        assert_eq!(system_icon_source(r"C:\Windows\System32\imageres.dll"), Some(0));
        assert_eq!(system_icon_source(r"C:\Windows\System32\SHELL32.DLL"), Some(1));
        assert_eq!(system_icon_source(r"D:\icons\app.ico"), None);
        assert_eq!(system_icon_source(""), None);
        let path = system_dll("imageres.dll");
        assert!(path.to_ascii_lowercase().ends_with(r"\system32\imageres.dll"), "{path}");
    }
}
