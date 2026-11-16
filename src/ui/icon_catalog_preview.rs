//! Icon-catalog grid. Built only with `--features icon-catalog-preview`, and
//! opened only when `MAGURO_ICON_CATALOG=1`. Icons only: no labels. Hover shows
//! the catalog key as a tooltip.

use std::cell::{Cell, RefCell};
use std::mem::size_of;

use windows::Win32::UI::Controls::{TTF_ABSOLUTE, TTF_TRACK, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;

use crate::imports::*;

const COLS: i32 = 11;
const ROWS: i32 = 9;

thread_local! {
    static HOT: Cell<i32> = const { Cell::new(-1) };
    static TIP: Cell<isize> = const { Cell::new(0) };
    static TIP_TEXT: RefCell<HSTRING> = RefCell::new(HSTRING::new());
    static LEAVE_ARMED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn run() -> Result<()> {
    unsafe { show() }
}

fn catalog_dark() -> bool {
    let path = crate::store::settings_path();
    let Ok(Some(text)) = crate::store::read_settings_file(&path) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    match v.get("theme").and_then(|t| t.as_str()) {
        Some("dark") => true,
        Some("light") => false,
        _ => crate::resolve_dark(0),
    }
}

fn grid(dpi: i32) -> (i32, i32, i32) {
    let s = |v: i32| v * dpi.max(96) / 96;
    let margin = s(16);
    let cell = s(36);
    (margin, cell, margin * 2 + cell * COLS)
}

fn client_size(dpi: i32) -> (i32, i32) {
    let (margin, cell, w) = grid(dpi);
    (w, margin * 2 + cell * ROWS)
}

fn hit(dpi: i32, x: i32, y: i32) -> Option<usize> {
    let (margin, cell, _) = grid(dpi);
    if x < margin || y < margin {
        return None;
    }
    let col = (x - margin) / cell;
    let row = (y - margin) / cell;
    if !(0..COLS).contains(&col) || !(0..ROWS).contains(&row) {
        return None;
    }
    let idx = (row * COLS + col) as usize;
    (idx < super::icon_catalog::icons().len()).then_some(idx)
}

unsafe fn show() -> Result<()> {
    _ = BufferedPaintInit();
    crate::prime_theme(catalog_dark());
    let Ok(module) = GetModuleHandleW(None) else { return Ok(()) };
    let hinst: HINSTANCE = module.into();
    let arrow = LoadCursorW(None, IDC_ARROW)?;
    let _ = RegisterClassW(&WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(proc),
        hInstance: hinst,
        hCursor: arrow,
        hbrBackground: HBRUSH::default(),
        lpszClassName: w!("MaguroIconCatalog"),
        ..Default::default()
    });
    let h = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("MaguroIconCatalog"),
        w!("Maguro"),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        None,
        None,
        Some(hinst),
        None,
    )?;
    place(h);
    crate::apply_window_icon(h);
    crate::style_dialog_caption(h);
    let tip = make_tip(h);
    TIP.with(|c| c.set(tip.0 as isize));
    let _ = ShowWindow(h, SW_SHOW);
    let _ = UpdateWindow(h);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    Ok(())
}

unsafe fn place(h: HWND) {
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let (cw, ch) = client_size(dpi);
    let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
    let (ww, hh) = if AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0), dpi as u32).is_ok() {
        ((r.right - r.left).max(cw), (r.bottom - r.top).max(ch))
    } else {
        (cw + 16, ch + 40)
    };
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let (x, y) = if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
        let x = mi.rcWork.left + ((mi.rcWork.right - mi.rcWork.left - ww) / 2).max(0);
        let y = mi.rcWork.top + ((mi.rcWork.bottom - mi.rcWork.top - hh) / 2).max(0);
        (x, y)
    } else {
        (CW_USEDEFAULT, CW_USEDEFAULT)
    };
    let _ = SetWindowPos(h, None, x, y, ww, hh, SWP_NOZORDER | SWP_NOACTIVATE);
}

unsafe fn make_tip(owner: HWND) -> HWND {
    let style = WS_POPUP | WINDOW_STYLE(0x01 | 0x02 | 0x10 | 0x20);
    let Ok(tip) = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, None, style, 0, 0, 0, 0, Some(owner), None, None, None) else {
        return HWND::default();
    };
    crate::set_tip_font(tip);
    let info = tool_info(owner, &HSTRING::new());
    let _ = SendMessageW(tip, TTM_ADDTOOLW, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize)));
    let _ = SendMessageW(tip, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(400)));
    tip
}

fn tool_info(owner: HWND, text: &HSTRING) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_TRACK | TTF_ABSOLUTE,
        hwnd: owner,
        uId: 1,
        lpszText: PWSTR(text.as_ptr() as *mut u16),
        ..Default::default()
    }
}

unsafe fn move_tip(owner: HWND, idx: Option<usize>, x: i32, y: i32) {
    let tip = HWND(TIP.with(|c| c.get()) as *mut _);
    if tip.0.is_null() {
        return;
    }
    let Some(idx) = idx else {
        let info = TIP_TEXT.with(|t| tool_info(owner, &t.borrow()));
        let _ = SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize)));
        return;
    };
    let key = super::icon_catalog::icons()[idx].key;
    TIP_TEXT.with(|t| {
        *t.borrow_mut() = HSTRING::from(key);
        let info = tool_info(owner, &t.borrow());
        let _ = SendMessageW(tip, TTM_UPDATETIPTEXTW, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize)));
        let _ = SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(LPARAM(&info as *const _ as isize)));
        let mut pt = POINT { x, y: y + 18 };
        let _ = ClientToScreen(owner, &mut pt);
        let packed = LPARAM((pt.x as u16 as usize as isize) | ((pt.y as u16 as usize as isize) << 16));
        let _ = SendMessageW(tip, TTM_TRACKPOSITION, Some(WPARAM(0)), Some(packed));
    });
}

unsafe fn arm_leave(h: HWND) {
    if LEAVE_ARMED.with(|c| c.get()) {
        return;
    }
    let mut ev = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: h,
        dwHoverTime: 0,
    };
    if TrackMouseEvent(&mut ev).is_ok() {
        LEAVE_ARMED.with(|c| c.set(true));
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            crate::paint_window(HDC(wp.0 as _), rc);
            LRESULT(1)
        }
        WM_PAINT => {
            paint(h);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let dpi = GetDpiForWindow(h).max(96) as i32;
            let idx = hit(dpi, x, y);
            let cur = HOT.with(|c| c.get());
            let next = idx.map(|i| i as i32).unwrap_or(-1);
            if cur != next {
                HOT.with(|c| c.set(next));
                move_tip(h, idx, x, y);
                let _ = InvalidateRect(Some(h), None, false);
            } else if idx.is_some() {
                move_tip(h, idx, x, y);
            }
            arm_leave(h);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            LEAVE_ARMED.with(|c| c.set(false));
            HOT.with(|c| c.set(-1));
            move_tip(h, None, 0, 0);
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            place(h);
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn paint(h: HWND) {
    let dpi = GetDpiForWindow(h).max(96) as i32;
    let (margin, cell, _) = grid(dpi);
    let hot = HOT.with(|c| c.get());
    let ink = crate::ui_tokens().text;
    let bg = crate::ui_tokens().bg;
    crate::buffered_solid(h, |dc, rc| {
        crate::paint_window(dc, rc);
        let entries = super::icon_catalog::icons();
        for (i, e) in entries.iter().enumerate() {
            let col = (i as i32) % COLS;
            let row = (i as i32) / COLS;
            let left = margin + col * cell;
            let top = margin + row * cell;
            let rc = RECT { left, top, right: left + cell, bottom: top + cell };
            if hot == i as i32 {
                crate::fill(dc, rc, crate::blend(bg, ink, 0.07));
            }
            crate::draw_line_icon(dc, rc, e.kind, ink, dpi);
        }
    });
}
