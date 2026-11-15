//! Important-marks list. Maguro-owned. No shell column and no registry entry.
//!
//! Import replaces the list with the chosen file. A blocked store shows why and where
//! the file is, and does not write it. The list refreshes when marks change and when
//! the window is shown. It does not poll the disk.

use std::cell::RefCell;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{GetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY, INVALID_FILE_ATTRIBUTES};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, HDITEMW, HDI_FORMAT, HDM_GETITEMW, HDM_SETITEMW, HDF_SORTDOWN, HDF_SORTUP, LIST_VIEW_ITEM_STATE_FLAGS,
    LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVIS_FOCUSED, LVIS_SELECTED, LVNI_SELECTED, LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_KEYDOWN,
    LVM_ENSUREVISIBLE, LVM_GETHEADER, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_SETBKCOLOR, LVM_SETCOLUMNW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, NMLISTVIEW,
    NMLVDISPINFOW, NMLVKEYDOWN, NM_DBLCLK, WC_LISTVIEWW,
};
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW};

use super::list::{
    blocked_message, cleanup_lost, export_text, file_name, filter_rows, format_added_local, import_replace, is_settings_file, plan_jump, rows_of, sort_rows, state_key, unmark, JumpPlan, PathKind, Row, SortCol,
};
use super::store::{marks_path, StoreError};
use crate::imports::*;
use crate::{px, BTN_GAP, BTN_H_FOOTER, BTN_MIN_W, BTN_PAD_X};

const ID_JUMP: i32 = 101;
const ID_UNMARK: i32 = 102;
const ID_CLEAN: i32 = 103;
const ID_EXPORT: i32 = 104;
const ID_IMPORT: i32 = 105;
const ID_SEARCH: i32 = 106;
const ID_LIST: i32 = 107;
const ID_BANNER: i32 = 108;
const ID_STATUS: i32 = 109;

const COPY_EXPORT: usize = 0x4D45_5850;
const COPY_IMPORT: usize = 0x4D49_4D50;
/// `wParam` is a column index: 0 name, 1 path, 2 state, 3 added.
pub(crate) const WM_SORT_COL: u32 = WM_APP + 121;
/// `wParam` 1 jumps the row in `lParam`. `wParam` 2 unmarks that row.
pub(crate) const WM_ROW_ACT: u32 = WM_APP + 122;

const BTN_IDS: [i32; 5] = [ID_JUMP, ID_UNMARK, ID_CLEAN, ID_EXPORT, ID_IMPORT];
const COLS: [SortCol; 4] = [SortCol::Name, SortCol::Path, SortCol::State, SortCol::Added];

struct Win {
    hwnd: HWND,
    sort: SortCol,
    desc: bool,
    rows: Vec<Row>,
    blocked: bool,
}

impl Win {
    fn blank() -> Self {
        Win { hwnd: HWND::default(), sort: SortCol::Name, desc: false, rows: Vec::new(), blocked: false }
    }
}

thread_local! {
    static WIN: RefCell<Win> = RefCell::new(Win::blank());
}

pub(crate) unsafe fn open(owner: HWND) {
    ensure_class();
    let existing = hwnd();
    if alive_hwnd(existing) {
        let _ = ShowWindow(existing, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(existing);
        refresh();
        return;
    }
    let dpi = if owner.0.is_null() { 96 } else { GetDpiForWindow(owner).max(96) as i32 };
    let (ww, hh) = (px(dpi, 920), px(dpi, 560));
    let (x, y) = place(owner, ww, hh);
    let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
    let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("MaguroMarksList"), w!("Maguro"), style, x, y, ww, hh, Some(owner), None, None, None) else { return };
    WIN.with(|w| {
        let mut u = w.borrow_mut();
        *u = Win::blank();
        u.hwnd = h;
    });
    crate::style_dialog_caption(h);
    let _ = SetWindowTextW(h, &HSTRING::from(crate::tr(crate::Key::ImportantList)));
    crate::note_side(h);
    build_children(h);
    apply_columns(h);
    refresh();
    layout(h);
    let _ = ShowWindow(h, SW_SHOW);
    let _ = SetForegroundWindow(h);
    focus_search();
}

pub(crate) unsafe fn close() {
    let h = hwnd();
    if alive_hwnd(h) {
        let _ = DestroyWindow(h);
    }
}

pub(crate) fn alive() -> bool {
    alive_hwnd(hwnd())
}

pub(crate) fn captures(target: HWND) -> bool {
    let h = hwnd();
    if !alive_hwnd(h) {
        return false;
    }
    target == h || unsafe { IsChild(h, target).as_bool() }
}

pub(crate) unsafe fn refresh() {
    let h = hwnd();
    if !alive_hwnd(h) {
        return;
    }
    refill(h);
}

pub(crate) unsafe fn restyle() {
    let h = hwnd();
    if !alive_hwnd(h) {
        return;
    }
    crate::style_dialog_caption(h);
    let _ = SetWindowTextW(h, &HSTRING::from(crate::tr(crate::Key::ImportantList)));
    let font = crate::body_font();
    for id in BTN_IDS {
        let b = ctl(h, id);
        let _ = SetWindowTextW(b, &HSTRING::from(button_label(id)));
        if !font.is_invalid() {
            let _ = SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
    }
    let search = ctl(h, ID_SEARCH);
    if !font.is_invalid() {
        let _ = SendMessageW(search, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let list = ctl(h, ID_LIST);
        let _ = SendMessageW(list, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let header = header_of(list);
        if !header.0.is_null() {
            let _ = SendMessageW(header, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
    }
    cue(search);
    apply_columns(h);
    color_list(ctl(h, ID_LIST));
    refill(h);
    layout(h);
}

pub(crate) unsafe fn on_key(vk: u16, ctrl: bool, shift: bool, alt: bool, win: bool) -> bool {
    if !alive() {
        return false;
    }
    if vk == VK_ESCAPE.0 && !ctrl && !alt {
        close();
        return true;
    }
    if ctrl && !shift && !alt && !win && vk == b'F' as u16 {
        focus_search();
        return true;
    }
    let search = GetFocus() == ctl(hwnd(), ID_SEARCH);
    if search {
        if vk == VK_RETURN.0 && !ctrl && !alt {
            jump_selected();
            return true;
        }
        return false;
    }
    if vk == VK_DELETE.0 && !ctrl && !alt {
        unmark_selected();
        return true;
    }
    if vk == VK_RETURN.0 && !ctrl && !alt {
        jump_selected();
        return true;
    }
    crate::keys::lookup(vk, ctrl, shift, alt, win).is_some()
}

fn hwnd() -> HWND {
    WIN.with(|w| w.borrow().hwnd)
}

fn alive_hwnd(h: HWND) -> bool {
    !h.0.is_null() && unsafe { IsWindow(Some(h)).as_bool() }
}

fn ensure_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        let Ok(m) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = m.into();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: hinst,
            hCursor: arrow,
            hIcon: LoadIconW(Some(hinst), PCWSTR(crate::ICON_GROUP_LIGHT as _)).unwrap_or_default(),
            lpszClassName: w!("MaguroMarksList"),
            ..Default::default()
        });
    });
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { proc_inner(h, msg, wp, lp) }))
}

unsafe fn proc_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            crate::buffered(h, |dc, rc| crate::paint_window(dc, rc));
            return LRESULT(0);
        }
        WM_SIZE | WM_DPICHANGED => {
            if msg == WM_DPICHANGED {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            layout(h);
            return LRESULT(0);
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            let dpi = dpi_of(h);
            info.ptMinTrackSize.x = px(dpi, 640);
            info.ptMinTrackSize.y = px(dpi, 420);
            return LRESULT(0);
        }
        WM_DRAWITEM => {
            draw_btn(&*(lp.0 as *const DRAWITEMSTRUCT));
            return LRESULT(1);
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let dc = HDC(wp.0 as *mut _);
            let child = HWND(lp.0 as *mut _);
            let bg = if GetDlgCtrlID(child) == ID_SEARCH { crate::pal().card } else { crate::pal().bg };
            return crate::color_edit_on(dc, bg, crate::pal().text);
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let code = ((wp.0 >> 16) & 0xffff) as u32;
            if id == ID_SEARCH && code == 0x0300 {
                on_search(h);
                return LRESULT(0);
            }
            if code == 0 {
                on_button(h, id);
                return LRESULT(0);
            }
        }
        WM_NOTIFY => {
            on_notify(h, lp);
            return LRESULT(0);
        }
        WM_COPYDATA => {
            if on_copy(h, lp) {
                return LRESULT(1);
            }
        }
        WM_SORT_COL => {
            if let Some(col) = COLS.get(wp.0) {
                toggle_sort(h, *col);
            }
            return LRESULT(0);
        }
        WM_ROW_ACT => {
            let index = lp.0 as usize;
            if wp.0 == 1 {
                jump_index(index);
            } else if wp.0 == 2 {
                unmark_index(index);
            }
            return LRESULT(0);
        }
        WM_SHOWWINDOW if wp.0 != 0 => refresh(),
        WM_DESTROY => {
            crate::drop_side(h);
            WIN.with(|w| {
                if w.borrow().hwnd == h {
                    *w.borrow_mut() = Win::blank();
                }
            });
        }
        _ => {}
    }
    DefWindowProcW(h, msg, wp, lp)
}

unsafe fn build_children(h: HWND) {
    let font = crate::body_font();
    let tab = 0x0001_0000u32;
    for id in BTN_IDS {
        let _ = crate::child(h, w!("BUTTON"), PCWSTR(HSTRING::from(button_label(id)).as_ptr()), 0xB | tab, WINDOW_EX_STYLE(0), id, font);
    }
    let search = crate::child(h, w!("EDIT"), w!(""), 0x80 | tab, WS_EX_CLIENTEDGE, ID_SEARCH, font);
    cue(search);
    let list_style = WINDOW_STYLE((LVS_REPORT | LVS_SHOWSELALWAYS | LVS_OWNERDATA) as u32) | WS_CHILD | WS_VISIBLE | WS_TABSTOP;
    let list = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        WC_LISTVIEWW,
        w!(""),
        list_style,
        0,
        0,
        0,
        0,
        Some(h),
        Some(HMENU(ID_LIST as isize as _)),
        None,
        None,
    )
    .unwrap_or_default();
    if !font.is_invalid() {
        let _ = SendMessageW(list, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
    let ex = (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as usize;
    let _ = SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(ex)), Some(LPARAM(ex as isize)));
    let _ = SetWindowTheme(list, w!("Explorer"), PCWSTR::null());
    color_list(list);
    let _ = crate::child(h, w!("STATIC"), w!(""), 0x80 | 0x2000, WINDOW_EX_STYLE(0), ID_BANNER, font);
    let _ = crate::child(h, w!("STATIC"), w!(""), 0x80 | 0x4000, WINDOW_EX_STYLE(0), ID_STATUS, font);
    insert_columns(list, dpi_of(h));
}

unsafe fn insert_columns(list: HWND, dpi: i32) {
    let widths = [px(dpi, 180), px(dpi, 360), px(dpi, 110), px(dpi, 170)];
    for (i, width) in widths.into_iter().enumerate() {
        let mut text = wide(&col_title(i));
        let col = LVCOLUMNW {
            mask: LVCF_FMT | LVCF_WIDTH | LVCF_TEXT,
            fmt: LVCFMT_LEFT,
            cx: width,
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        let _ = SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
    }
}

unsafe fn apply_columns(h: HWND) {
    let list = ctl(h, ID_LIST);
    for i in 0..COLS.len() {
        let mut text = wide(&col_title(i));
        let col = LVCOLUMNW { mask: LVCF_TEXT, pszText: PWSTR(text.as_mut_ptr()), ..Default::default() };
        let _ = SendMessageW(list, LVM_SETCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
    }
    let (col, desc) = WIN.with(|w| {
        let u = w.borrow();
        (u.sort, u.desc)
    });
    mark_sort(list, col, desc);
}

unsafe fn mark_sort(list: HWND, col: SortCol, desc: bool) {
    let header = header_of(list);
    if header.0.is_null() {
        return;
    }
    let active = COLS.iter().position(|c| *c == col).unwrap_or(0);
    for i in 0..COLS.len() {
        let mut item = HDITEMW { mask: HDI_FORMAT, ..Default::default() };
        if SendMessageW(header, HDM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize))).0 == 0 {
            continue;
        }
        let mut fmt = item.fmt.0 & !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
        if i == active {
            fmt |= if desc { HDF_SORTDOWN.0 } else { HDF_SORTUP.0 };
        }
        item.fmt.0 = fmt;
        let _ = SendMessageW(header, HDM_SETITEMW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
    }
}

unsafe fn color_list(list: HWND) {
    if list.0.is_null() {
        return;
    }
    let card = crate::cref(crate::pal().card).0 as isize;
    let ink = crate::cref(crate::pal().text).0 as isize;
    let _ = SendMessageW(list, LVM_SETBKCOLOR, Some(WPARAM(0)), Some(LPARAM(card)));
    let _ = SendMessageW(list, LVM_SETTEXTBKCOLOR, Some(WPARAM(0)), Some(LPARAM(card)));
    let _ = SendMessageW(list, LVM_SETTEXTCOLOR, Some(WPARAM(0)), Some(LPARAM(ink)));
}

unsafe fn layout(h: HWND) {
    let dpi = dpi_of(h);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let pad = px(dpi, 12);
    let gap = px(dpi, 8);
    let search_h = px(dpi, 28);
    let btn_h = px(dpi, BTN_H_FOOTER);
    let status_h = px(dpi, 22);
    let blocked = WIN.with(|w| w.borrow().blocked);
    let banner_h = if blocked { px(dpi, 72) } else { 0 };
    let width = (rc.right - rc.left).max(1);
    let height = (rc.bottom - rc.top).max(1);
    let mut y = pad;
    place_ctl(ctl(h, ID_SEARCH), pad, y, width - pad * 2, search_h);
    y += search_h + gap;
    let banner = ctl(h, ID_BANNER);
    if blocked {
        place_ctl(banner, pad, y, width - pad * 2, banner_h);
        let _ = ShowWindow(banner, SW_SHOW);
        y += banner_h + gap;
    } else {
        let _ = ShowWindow(banner, SW_HIDE);
    }
    let buttons_y = height - pad - status_h - gap - btn_h;
    let list_h = (buttons_y - gap - y).max(px(dpi, 80));
    place_ctl(ctl(h, ID_LIST), pad, y, width - pad * 2, list_h);
    size_path_column(h, width - pad * 2, dpi);
    place_buttons(h, pad, buttons_y, width - pad * 2, btn_h, dpi);
    place_ctl(ctl(h, ID_STATUS), pad, buttons_y + btn_h + gap, width - pad * 2, status_h);
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe fn place_buttons(h: HWND, x: i32, y: i32, width: i32, height: i32, dpi: i32) {
    let font = crate::body_font();
    let gap = px(dpi, BTN_GAP);
    let mut widths: Vec<i32> = BTN_IDS.iter().map(|id| (text_px(&button_label(*id), font) + px(dpi, BTN_PAD_X) * 2).max(px(dpi, BTN_MIN_W))).collect();
    let gaps = gap * (widths.len() as i32 - 1);
    let total: i32 = widths.iter().sum::<i32>() + gaps;
    if total > width && total > 0 {
        let scale = width as f32 / total as f32;
        for w in &mut widths {
            *w = (*w as f32 * scale) as i32;
        }
    }
    let mut left = x;
    for (id, w) in BTN_IDS.iter().zip(widths) {
        place_ctl(ctl(h, *id), left, y, w.max(1), height);
        left += w + gap;
    }
}

unsafe fn size_path_column(h: HWND, list_w: i32, dpi: i32) {
    let list = ctl(h, ID_LIST);
    let name = px(dpi, 180);
    let state = px(dpi, 110);
    let added = px(dpi, 170);
    let path_w = (list_w - name - state - added - px(dpi, 24)).max(px(dpi, 120));
    let widths = [name, path_w, state, added];
    for (i, w) in widths.into_iter().enumerate() {
        let _ = SendMessageW(list, LVM_SETCOLUMNWIDTH, Some(WPARAM(i)), Some(LPARAM(w as isize)));
    }
}

unsafe fn place_ctl(h: HWND, x: i32, y: i32, w: i32, height: i32) {
    if h.0.is_null() {
        return;
    }
    let _ = SetWindowPos(h, None, x, y, w, height, SWP_NOZORDER | SWP_NOACTIVATE);
}

fn on_search(h: HWND) {
    let text = unsafe { window_text(ctl(h, ID_SEARCH)) };
    unsafe { refill_query(h, &text) };
}

unsafe fn refill(h: HWND) {
    let query = window_text(ctl(h, ID_SEARCH));
    refill_query(h, &query);
}

unsafe fn refill_query(h: HWND, query: &str) {
    let keep = selected_paths(h);
    let (sort, desc) = WIN.with(|w| {
        let u = w.borrow();
        (u.sort, u.desc)
    });
    let (rows, message) = load_view(query, sort, desc);
    let blocked = message.is_some();
    let count = rows.len();
    WIN.with(|w| {
        let mut u = w.borrow_mut();
        u.rows = rows;
        u.blocked = blocked;
    });
    let list = ctl(h, ID_LIST);
    let _ = SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(0)));
    reselect(h, &keep);
    set_banner(h, message.as_deref().unwrap_or(""));
    if message.is_none() {
        set_status(h, &crate::tf(crate::Key::MarksReplaced, &[("n", &count.to_string())]));
    } else {
        set_status(h, "");
    }
    enable_actions(h, !blocked);
    let _ = InvalidateRect(Some(list), None, false);
}

fn load_view(query: &str, sort: SortCol, desc: bool) -> (Vec<Row>, Option<String>) {
    let Some((mut rows, message)) = crate::marks::with_store(|store| {
        let message = blocked_message(store);
        let rows = filter_rows(&rows_of(store), query);
        (rows, message)
    }) else {
        return (Vec::new(), None);
    };
    sort_rows(&mut rows, sort, desc);
    (rows, message)
}

unsafe fn reselect(h: HWND, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let list = ctl(h, ID_LIST);
    let indexes: Vec<usize> = WIN.with(|w| {
        w.borrow().rows.iter().enumerate().filter(|(_, row)| paths.iter().any(|p| p.eq_ignore_ascii_case(&row.path))).map(|(i, _)| i).collect()
    });
    for (n, index) in indexes.iter().enumerate() {
        select_index(list, *index, n == 0);
    }
    if let Some(first) = indexes.first() {
        let _ = SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(*first)), Some(LPARAM(0)));
    }
}

unsafe fn select_index(list: HWND, index: usize, focus: bool) {
    let flags = if focus { LVIS_SELECTED.0 | LVIS_FOCUSED.0 } else { LVIS_SELECTED.0 };
    let mut item = LVITEMW { state: LIST_VIEW_ITEM_STATE_FLAGS(flags), stateMask: LIST_VIEW_ITEM_STATE_FLAGS(flags), ..Default::default() };
    let _ = SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(index)), Some(LPARAM(&mut item as *mut _ as isize)));
}

unsafe fn on_notify(h: HWND, lp: LPARAM) {
    let hdr = &*(lp.0 as *const NMHDR);
    let list = ctl(h, ID_LIST);
    if hdr.hwndFrom != list && hdr.idFrom != ID_LIST as usize {
        return;
    }
    match hdr.code {
        code if code == LVN_GETDISPINFOW => fill_item(lp),
        code if code == LVN_COLUMNCLICK => {
            let view = &*(lp.0 as *const NMLISTVIEW);
            if let Some(col) = COLS.get(view.iSubItem as usize) {
                toggle_sort(h, *col);
            }
        }
        code if code == NM_DBLCLK => {
            let view = &*(lp.0 as *const NMLISTVIEW);
            if view.iItem >= 0 {
                jump_index(view.iItem as usize);
            }
        }
        code if code == LVN_KEYDOWN => {
            let key = &*(lp.0 as *const NMLVKEYDOWN);
            if key.wVKey == VK_RETURN.0 {
                jump_selected();
            }
        }
        _ => {}
    }
}

unsafe fn fill_item(lp: LPARAM) {
    let info = &mut *(lp.0 as *mut NMLVDISPINFOW);
    if info.item.mask.0 & LVIF_TEXT.0 == 0 || info.item.pszText.0.is_null() || info.item.cchTextMax <= 1 {
        return;
    }
    let index = info.item.iItem as usize;
    let sub = info.item.iSubItem;
    let text = WIN.with(|w| {
        let u = w.borrow();
        let Some(row) = u.rows.get(index) else { return String::new() };
        match sub {
            0 => row.name.clone(),
            1 => row.path.clone(),
            2 => crate::tr(state_key(&row.state)).to_string(),
            3 => format_added_local(&row.added),
            _ => String::new(),
        }
    });
    put_text(&info.item, &text);
}

fn put_text(item: &LVITEMW, text: &str) {
    let max = item.cchTextMax as usize;
    let mut encoded: Vec<u16> = text.encode_utf16().take(max.saturating_sub(1)).collect();
    encoded.push(0);
    unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), item.pszText.0, encoded.len()) };
}

fn toggle_sort(h: HWND, col: SortCol) {
    WIN.with(|w| {
        let mut u = w.borrow_mut();
        if u.sort == col {
            u.desc = !u.desc;
        } else {
            u.sort = col;
            u.desc = false;
        }
    });
    unsafe {
        let (sort, desc) = WIN.with(|w| {
            let u = w.borrow();
            (u.sort, u.desc)
        });
        mark_sort(ctl(h, ID_LIST), sort, desc);
        refill(h);
    }
}

fn on_button(h: HWND, id: i32) {
    match id {
        ID_JUMP => jump_selected(),
        ID_UNMARK => unmark_selected(),
        ID_CLEAN => clean_lost(h),
        ID_EXPORT => export_dialog(h),
        ID_IMPORT => import_dialog(h),
        _ => {}
    }
}

fn jump_selected() {
    let Some(index) = focused_or_first() else {
        unsafe { let _ = MessageBeep(MB_ICONWARNING); }
        return;
    };
    jump_index(index);
}

fn jump_index(index: usize) {
    let Some(row) = WIN.with(|w| w.borrow().rows.get(index).cloned()) else { return };
    let plan = plan_jump(&row.path, &row.state, kind_of);
    let h = hwnd();
    match plan {
        JumpPlan::Unavailable => {
            unsafe {
                set_status(h, crate::tr(crate::Key::ItemCannotBeOpened));
                let _ = MessageBeep(MB_ICONWARNING);
            }
        }
        JumpPlan::Open { folder, select } => {
            let pane = crate::app(|a| a.s.active);
            let opened = unsafe { crate::navigate(pane, &folder) };
            if !opened {
                unsafe {
                    set_status(h, crate::tr(crate::Key::ItemCannotBeOpened));
                    let _ = MessageBeep(MB_ICONWARNING);
                }
                return;
            }
            if let Some(name) = select {
                unsafe { crate::cliphot::select_soon(&name) };
            }
            let main = crate::app(|a| a.main);
            unsafe {
                if alive_hwnd(main) {
                    let _ = ShowWindow(main, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(main);
                }
                set_status(h, &folder);
            }
        }
    }
}

fn kind_of(path: &str) -> PathKind {
    let path = path.trim();
    if path.is_empty() || path.starts_with("\\\\") {
        return PathKind::Missing;
    }
    unsafe {
        let attr = GetFileAttributesW(&HSTRING::from(path));
        if attr == INVALID_FILE_ATTRIBUTES {
            PathKind::Missing
        } else if attr & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
            PathKind::Dir
        } else {
            PathKind::File
        }
    }
}

fn unmark_selected() {
    let paths = unsafe { selected_paths(hwnd()) };
    unmark_paths(paths);
}

fn unmark_index(index: usize) {
    let Some(path) = WIN.with(|w| w.borrow().rows.get(index).map(|r| r.path.clone())) else {
        unsafe { let _ = MessageBeep(MB_ICONWARNING); }
        return;
    };
    unmark_paths(vec![path]);
}

fn unmark_paths(paths: Vec<String>) {
    if paths.is_empty() {
        unsafe { let _ = MessageBeep(MB_ICONWARNING); }
        return;
    }
    let h = hwnd();
    let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
    if !edit(h, |store| unmark(store, &paths).map(|n| n > 0)) {
        return;
    }
    unsafe { set_status(h, &names.join(", ")) };
}

fn clean_lost(h: HWND) {
    let lost = crate::marks::with_store(|store| rows_of(store).iter().filter(|r| r.state == super::store::STATE_LOST).count()).unwrap_or(0);
    if lost == 0 {
        unsafe {
            set_status(h, crate::tr(crate::Key::CleanUpMissing));
            let _ = MessageBeep(MB_ICONWARNING);
        }
        return;
    }
    let ask = crate::Confirm::danger(crate::tr(crate::Key::ImportantList), crate::tr(crate::Key::CleanUpMissingConfirm)).affirm_label(crate::tr(crate::Key::CleanUpMissing));
    if !unsafe { crate::ask_confirm(h, ask) } {
        return;
    }
    let _ = edit(h, |store| cleanup_lost(store).map(|n| n > 0));
}

fn export_dialog(h: HWND) {
    let Some(path) = (unsafe { pick_file(h, true) }) else { return };
    export_to(h, Path::new(&path));
}

fn import_dialog(h: HWND) {
    let Some(path) = (unsafe { pick_file(h, false) }) else { return };
    let ask = crate::Confirm::danger(crate::tr(crate::Key::ImportantList), crate::tr(crate::Key::ReplaceMarksConfirm)).affirm_label(crate::tr(crate::Key::Import));
    if !unsafe { crate::ask_confirm(h, ask) } {
        return;
    }
    import_from(h, Path::new(&path));
}

fn export_to(h: HWND, path: &Path) {
    if refuse_protected(h, path) {
        return;
    }
    let text = crate::marks::with_store(export_text).unwrap_or_default();
    let n = crate::marks::with_store(|store| store.len()).unwrap_or(0);
    if std::fs::write(path, text.as_bytes()).is_err() {
        unsafe { set_status(h, crate::tr(crate::Key::MarksSaveFailed)) };
        return;
    }
    unsafe { set_status(h, &crate::tf(crate::Key::MarksExported, &[("n", &n.to_string())])) };
}

fn import_from(h: HWND, path: &Path) {
    if refuse_protected(h, path) {
        return;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            unsafe { set_status(h, crate::tr(crate::Key::MarksImportBad)) };
            return;
        }
    };
    if !edit(h, |store| import_replace(store, &bytes).map(|report| report.kept > 0 || store.len() == report.kept)) {
        return;
    }
    unsafe { refill(h) };
}

fn refuse_protected(h: HWND, path: &Path) -> bool {
    if is_settings_file(path) || is_live_marks(path) {
        unsafe { set_status(h, crate::tr(crate::Key::MarksSaveFailed)) };
        return true;
    }
    false
}

fn is_live_marks(path: &Path) -> bool {
    let Some(live) = crate::marks::with_store(|store| marks_path(store.folder())) else { return false };
    norm(path).eq_ignore_ascii_case(&norm(&live))
}

fn norm(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_string()
}

/// Run `f` and save when it changes the store. Returns false when the store refused or the save failed.
fn edit(h: HWND, f: impl FnOnce(&mut super::store::Store) -> std::result::Result<bool, StoreError>) -> bool {
    let mut failed: Option<StoreError> = None;
    crate::marks::commit_edit(|store| match f(store) {
        Ok(changed) => changed,
        Err(err) => {
            failed = Some(err);
            false
        }
    });
    if let Some(err) = failed {
        let text = err_text(err);
        unsafe { refill(h) };
        unsafe { set_status(h, &text) };
        return false;
    }
    let dirty = crate::marks::with_store(|store| store.is_dirty()).unwrap_or(false);
    if dirty {
        unsafe { set_status(h, crate::tr(crate::Key::MarksSaveFailed)) };
        return false;
    }
    unsafe { refill(h) };
    true
}

fn err_text(err: StoreError) -> String {
    match err {
        StoreError::Blocked => crate::marks::with_store(|store| blocked_message(store).unwrap_or_else(|| crate::tr(crate::Key::MarksBlockedBad).to_string()))
            .unwrap_or_else(|| crate::tr(crate::Key::MarksBlockedBad).to_string()),
        StoreError::Io(text) if text == "not a marks file" || text == "newer version" => crate::tr(crate::Key::MarksImportBad).to_string(),
        StoreError::Full => crate::tr(crate::Key::ImportantMarksFull).to_string(),
        _ => crate::tr(crate::Key::MarksSaveFailed).to_string(),
    }
}

unsafe fn on_copy(h: HWND, lp: LPARAM) -> bool {
    let cd = &*(lp.0 as *const COPYDATASTRUCT);
    if cd.lpData.is_null() || cd.cbData < 2 {
        return false;
    }
    let words = cd.cbData as usize / 2;
    let text = String::from_utf16_lossy(std::slice::from_raw_parts(cd.lpData as *const u16, words));
    let path = text.trim_end_matches('\0').trim().to_string();
    if path.is_empty() {
        return false;
    }
    match cd.dwData {
        COPY_EXPORT => export_to(h, Path::new(&path)),
        COPY_IMPORT => import_from(h, Path::new(&path)),
        _ => return false,
    }
    true
}

unsafe fn pick_file(owner: HWND, save: bool) -> Option<String> {
    let mut buf = [0u16; 520];
    if save {
        for (slot, ch) in buf.iter_mut().zip("marks-export.json".encode_utf16()) {
            *slot = ch;
        }
    }
    let filter = file_filter();
    let def = w!("json");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: if save { OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST } else { OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST },
        lpstrDefExt: if save { def } else { PCWSTR::null() },
        ..Default::default()
    };
    let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
    ok.as_bool().then(|| {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    })
}

fn file_filter() -> Vec<u16> {
    let mut s = String::new();
    for (label, spec) in [(crate::tr(crate::Key::Json), "*.json"), (crate::tr(crate::Key::All), "*.*")] {
        s.push_str(label);
        s.push('\0');
        s.push_str(spec);
        s.push('\0');
    }
    s.push('\0');
    s.encode_utf16().collect()
}

unsafe fn selected_paths(h: HWND) -> Vec<String> {
    let list = ctl(h, ID_LIST);
    let mut out = Vec::new();
    let mut i = -1isize;
    loop {
        i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(i as usize)), Some(LPARAM(LVNI_SELECTED as isize))).0 as isize;
        if i < 0 {
            break;
        }
        if let Some(path) = WIN.with(|w| w.borrow().rows.get(i as usize).map(|r| r.path.clone())) {
            out.push(path);
        }
    }
    out
}

fn focused_or_first() -> Option<usize> {
    let paths = unsafe { selected_paths(hwnd()) };
    if paths.is_empty() {
        return None;
    }
    WIN.with(|w| w.borrow().rows.iter().position(|r| r.path.eq_ignore_ascii_case(&paths[0])))
}

unsafe fn enable_actions(h: HWND, on: bool) {
    for id in [ID_UNMARK, ID_CLEAN, ID_IMPORT] {
        let _ = EnableWindow(ctl(h, id), on);
    }
}

unsafe fn set_banner(h: HWND, text: &str) {
    let _ = SetWindowTextW(ctl(h, ID_BANNER), &HSTRING::from(text));
}

unsafe fn set_status(h: HWND, text: &str) {
    if !alive_hwnd(h) {
        return;
    }
    let _ = SetWindowTextW(ctl(h, ID_STATUS), &HSTRING::from(text));
}

unsafe fn focus_search() {
    let edit = ctl(hwnd(), ID_SEARCH);
    let _ = SetFocus(Some(edit));
    let _ = SendMessageW(edit, crate::EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
}

unsafe fn cue(edit: HWND) {
    let text = HSTRING::from(crate::tr(crate::Key::Search));
    let _ = SendMessageW(edit, 0x1501, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
}

unsafe fn draw_btn(d: &DRAWITEMSTRUCT) {
    let dpi = GetDpiForWindow(d.hwndItem).max(96) as i32;
    let kind = if d.CtlID == ID_JUMP as u32 { crate::ButtonKind::Primary } else { crate::ButtonKind::Secondary };
    let visual = crate::visual_from_item(d.itemState.0, crate::app(|a| a.hover) == d.hwndItem);
    crate::paint_button(d.hDC, d.rcItem, kind, visual, &crate::text_of(d.hwndItem), crate::body_font(), dpi, crate::ui_tokens().bg, crate::FocusRing::Inside);
}

fn button_label(id: i32) -> String {
    let key = match id {
        ID_JUMP => crate::Key::OpenLocation,
        ID_UNMARK => crate::Key::RemoveImportantMark,
        ID_CLEAN => crate::Key::CleanUpMissing,
        ID_EXPORT => crate::Key::Export,
        ID_IMPORT => crate::Key::Import,
        _ => crate::Key::ImportantList,
    };
    crate::tr(key).to_string()
}

fn col_title(i: usize) -> String {
    let key = match i {
        0 => crate::Key::Name,
        1 => crate::Key::Path,
        2 => crate::Key::MarksState,
        _ => crate::Key::ChangeAdded,
    };
    crate::tr(key).to_string()
}

unsafe fn window_text(h: HWND) -> String {
    let n = GetWindowTextLengthW(h).max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(h, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..got])
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn text_px(text: &str, font: HFONT) -> i32 {
    let hdc = GetDC(None);
    if hdc.0.is_null() {
        return 80;
    }
    let old = SelectObject(hdc, font.into());
    let encoded = wide(text);
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &encoded[..encoded.len().saturating_sub(1)], &mut sz);
    if !old.0.is_null() {
        SelectObject(hdc, old);
    }
    let _ = ReleaseDC(None, hdc);
    sz.cx
}

fn header_of(list: HWND) -> HWND {
    unsafe { HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as *mut _) }
}

fn ctl(h: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(h), id).unwrap_or_default() }
}

fn dpi_of(h: HWND) -> i32 {
    unsafe { GetDpiForWindow(h).max(96) as i32 }
}

unsafe fn place(owner: HWND, ww: i32, hh: i32) -> (i32, i32) {
    if owner.0.is_null() {
        return (80, 80);
    }
    let mut orc = RECT::default();
    let _ = GetWindowRect(owner, &mut orc);
    (orc.left + (orc.right - orc.left - ww) / 2, orc.top + (orc.bottom - orc.top - hh) / 2)
}
