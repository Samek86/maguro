//! Add or edit a favourite. Kind, name, colour, icon, and memo are shared.
//! Folder and file keep the path they have always stored. A password stays in the
//! edit and in Credential Manager; the favourites file only records that one exists.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Shell::{FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, IFileOpenDialog, SIGDN_FILESYSPATH};

use crate::app::{app, text_of};
use crate::i18n::tr;
use crate::imports::*;
use crate::session::{
    node_from_draft, node_ref, shell_places, stored_has_password, write_leaf, Draft, ItemIcon, ItemKind, RdpScreen, SecretPlan, KINDS, RDP_WINDOWS,
};
use crate::shell::{forget_hosts, stage_secret};

use super::{
    body_font, bold_ui_font, buffered, color_field_edit, field_edit_rect, paint_button, paint_check, paint_chip, paint_field, paint_label, paint_meta, paint_section_header,
    paint_segment, paint_wash, paint_window, style_popup_chrome, visual_from_pointer, wrap_text, ButtonKind, FocusRing, BTN_GAP, BTN_H_FOOTER, BTN_MIN_W, FIELD_H, FIELD_PAD_X,
    FONT_PX, PAD_BODY_X, PAD_BODY_Y, SPACE_4, SPACE_8, px,
};

const SAVE_MSG: u32 = WM_APP + 71;
const EM_SETMARGINS: u32 = 0x00D3;
const ID_NAME: i32 = 10;
const ID_TARGET: i32 = 11;
const ID_ARGS: i32 = 12;
const ID_WORK: i32 = 13;
const ID_USER: i32 = 14;
const ID_PASS: i32 = 15;
const ID_CRED: i32 = 16;
const ID_EXTRA: i32 = 17;
const ID_MEMO: i32 = 18;
const ID_ICON: i32 = 19;
const ID_INDEX: i32 = 20;
const ID_PLACE: i32 = 21;
const ID_SCREEN: i32 = 22;
const CB_RESETCONTENT: u32 = 0x014B;
const CB_ADDSTRING: u32 = 0x0143;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETCURSEL: u32 = 0x0147;
const CBN_SELCHANGE: u32 = 1;
const EN_CHANGE: u32 = 0x0300;

const HIT_SWATCH: i32 = 20;
const HIT_CLEAR_COLOR: i32 = 21;
const HIT_ADMIN: i32 = 24;
const HIT_ICON_AUTO: i32 = 25;
const HIT_ICON_CUSTOM: i32 = 26;
const HIT_CLEAR_PASS: i32 = 27;
const HIT_BROWSE_TARGET: i32 = 28;
const HIT_BROWSE_WORK: i32 = 29;
const HIT_BROWSE_ICON: i32 = 30;
const HIT_OK: i32 = 31;
const HIT_CANCEL: i32 = 32;

static EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Name,
    Place,
    Target,
    Args,
    Work,
    User,
    Pass,
    Screen,
    Cred,
    Extra,
    IconPath,
    Memo,
    Color,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Geom {
    pub(crate) title: RECT,
    pub(crate) kind_label: RECT,
    pub(crate) kinds: [RECT; 6],
    pub(crate) name: RECT,
    pub(crate) swatch: RECT,
    pub(crate) clear_color: RECT,
    pub(crate) labels: Vec<(Slot, RECT)>,
    pub(crate) slots: Vec<(Slot, RECT)>,
    pub(crate) browse: Vec<(Slot, RECT)>,
    pub(crate) admin: RECT,
    pub(crate) icon_auto: RECT,
    pub(crate) icon_custom: RECT,
    pub(crate) index: RECT,
    pub(crate) note: RECT,
    pub(crate) saved: RECT,
    pub(crate) clear_pass: RECT,
    pub(crate) ok: RECT,
    pub(crate) cancel: RECT,
    pub(crate) height: i32,
}

struct Dlg {
    dpi: i32,
    kind: ItemKind,
    editing: bool,
    edit_id: Vec<usize>,
    parent: Vec<usize>,
    color: Option<u32>,
    screen: RdpScreen,
    admin: bool,
    custom_icon: bool,
    show_clear: bool,
    clear_password: bool,
    hot: i32,
    down: i32,
    edits: [HWND; 11],
    place_combo: HWND,
    screen_combo: HWND,
}

pub(crate) unsafe fn open_item_dialog(owner: HWND, parent: &[usize], edit: Option<&[usize]>) {
    let dpi = dpi_of(owner);
    let mut dlg = Dlg {
        dpi,
        kind: ItemKind::Folder,
        editing: false,
        edit_id: Vec::new(),
        parent: parent.to_vec(),
        color: None,
        screen: RdpScreen::Auto,
        admin: false,
        custom_icon: false,
        show_clear: false,
        clear_password: false,
        hot: -1,
        down: -1,
        edits: [HWND::default(); 11],
        place_combo: HWND::default(),
        screen_combo: HWND::default(),
    };
    let loaded = edit.and_then(|id| {
        app(|a| node_ref(&a.s.tree, id).filter(|n| !n.is_group() && n.server.is_none()).cloned()).map(|n| (id.to_vec(), n))
    });
    if edit.is_some() && loaded.is_none() {
        return;
    }
    if let Some((id, node)) = loaded.as_ref() {
        dlg.editing = true;
        dlg.edit_id = id.clone();
        dlg.kind = node.kind();
        dlg.color = node.item.as_ref().and_then(|i| i.color);
        dlg.screen = node.item.as_ref().map(|i| i.screen).unwrap_or(RdpScreen::Auto);
        dlg.admin = node.item.as_ref().is_some_and(|i| i.admin);
        dlg.custom_icon = node.item.as_ref().and_then(|i| i.icon.as_ref()).is_some();
        dlg.show_clear = node.item.as_ref().is_some_and(|i| i.has_password);
    }
    ensure_class();
    let client_w = sc(dpi, 520);
    let client_h = form_geom(dpi, client_w, dlg.kind, dlg.custom_icon, dlg.show_clear).height;
    let (w, h) = outer_size(client_w, client_h, dpi);
    let (x, y) = center_on(owner, w, h);
    let title = if dlg.editing { tr(crate::Key::EditFavourite) } else { tr(crate::Key::AddItem) };
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        w!("MaguroItem"),
        &HSTRING::from(title),
        WS_POPUP | WS_CLIPCHILDREN,
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
    let ptr = Box::into_raw(Box::new(dlg));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
    let font = body_font();
    let specs = [
        (ID_NAME, 0u32),
        (ID_TARGET, 0),
        (ID_ARGS, 0),
        (ID_WORK, 0),
        (ID_USER, 0),
        (ID_PASS, 0x20),
        (ID_CRED, 0x2000),
        (ID_EXTRA, 0x4 | 0x40 | 0x1000),
        (ID_MEMO, 0x4 | 0x40 | 0x1000),
        (ID_ICON, 0),
        (ID_INDEX, 0x2000),
    ];
    for (i, (id, extra)) in specs.iter().enumerate() {
        let style = 0x80u32 | 0x10000 | extra;
        let h = crate::child(hwnd, w!("EDIT"), w!(""), style, WINDOW_EX_STYLE(0), *id, font);
        (*ptr).edits[i] = h;
        subclass_edit(h);
    }
    let list = |id| {
        let c = crate::child(hwnd, w!("COMBOBOX"), w!(""), super::combo::LIST | 0x10000, WINDOW_EX_STYLE(0), id, font);
        super::combo::adopt(c, item_combo_face);
        c
    };
    (*ptr).place_combo = list(ID_PLACE);
    (*ptr).screen_combo = list(ID_SCREEN);
    fill_place_list((*ptr).place_combo);
    fill_screen_list((*ptr).screen_combo);
    if let Some((_, node)) = loaded.as_ref() {
        fill_edits(ptr, node);
    } else {
        _ = SetWindowTextW((*ptr).edits[6], w!("10"));
        _ = SetWindowTextW((*ptr).edits[10], w!("0"));
    }
    place_edits(hwnd);
    round_window(hwnd);
    let was_disabled = EnableWindow(owner, false).as_bool();
    _ = ShowWindow(hwnd, SW_SHOW);
    _ = SetForegroundWindow(hwnd);
    _ = SetFocus(Some((*ptr).edits[0]));
    let mut msg = MSG::default();
    loop {
        let r = GetMessageW(&mut msg, None, 0, 0);
        if !r.as_bool() {
            if msg.message == WM_QUIT {
                PostQuitMessage(msg.wParam.0 as i32);
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
}

unsafe fn fill_edits(ptr: *mut Dlg, node: &crate::FavNode) {
    set_text((*ptr).edits[0], &node.name);
    set_text((*ptr).edits[1], &node.path);
    let item = node.item.as_ref();
    set_text((*ptr).edits[2], item.map(|i| i.args.as_str()).unwrap_or(""));
    set_text((*ptr).edits[3], item.map(|i| i.workdir.as_str()).unwrap_or(""));
    set_text((*ptr).edits[4], item.map(|i| i.user.as_str()).unwrap_or(""));
    let secs = item.map(|i| i.cred_secs).unwrap_or(10);
    set_text((*ptr).edits[6], &secs.to_string());
    set_text((*ptr).edits[7], item.map(|i| i.rdp_extra.as_str()).unwrap_or(""));
    set_text((*ptr).edits[8], item.map(|i| i.memo.as_str()).unwrap_or(""));
    if let Some(icon) = item.and_then(|i| i.icon.as_ref()) {
        set_text((*ptr).edits[9], &icon.path);
        set_text((*ptr).edits[10], &icon.index.to_string());
    } else {
        set_text((*ptr).edits[10], "0");
    }
    sync_place((*ptr).place_combo, &text_of((*ptr).edits[1]));
    sync_screen((*ptr).screen_combo, (*ptr).screen);
}

fn set_text(h: HWND, s: &str) {
    unsafe { _ = SetWindowTextW(h, &HSTRING::from(s)); }
}

pub(crate) fn form_geom(dpi: i32, width: i32, kind: ItemKind, custom_icon: bool, show_clear: bool) -> Geom {
    let s = |v: i32| px(dpi, v);
    let pad = s(PAD_BODY_X);
    let gap = s(SPACE_8);
    let gap4 = s(SPACE_4);
    let lab = s(FONT_PX + SPACE_4);
    let edit_h = s(FIELD_H);
    let seg_h = s(BTN_H_FOOTER);
    let right = (width - pad).max(pad + 1);
    let inner = (right - pad).max(1);
    let mut y = pad;
    let mut labels = Vec::new();
    let mut slots = Vec::new();
    let mut browse = Vec::new();
    let title = RECT { left: pad, top: y, right, bottom: y + lab };
    y = title.bottom + gap;
    let kind_label = RECT { left: pad, top: y, right, bottom: y + lab };
    y = kind_label.bottom + gap4;
    // Six kind names. One row when each chip is wide enough, otherwise two rows of three.
    let min_seg = s(108);
    let six = (inner - gap * 5) / 6;
    let cols = if six >= min_seg { 6 } else { 3 };
    let slot_w = (inner - gap * (cols - 1)) / cols;
    let mut kinds = [RECT::default(); 6];
    for i in 0..6 {
        let col = (i as i32) % cols;
        let row = (i as i32) / cols;
        let left = pad + col * (slot_w + gap);
        let top = y + row * (seg_h + gap);
        kinds[i] = RECT { left, top, right: left + slot_w, bottom: top + seg_h };
    }
    y = kinds[5].bottom + gap;

    let name_lab = RECT { left: pad, top: y, right, bottom: y + lab };
    labels.push((Slot::Name, name_lab));
    y = name_lab.bottom + gap4;
    let name = RECT { left: pad, top: y, right, bottom: y + edit_h };
    slots.push((Slot::Name, name));
    y = name.bottom + gap;

    let color_lab = RECT { left: pad, top: y, right, bottom: y + lab };
    labels.push((Slot::Color, color_lab));
    y = color_lab.bottom + gap4;
    let chip = edit_h;
    let clear_w = s(BTN_MIN_W).max(s(88));
    let swatch = RECT { left: pad, top: y, right: (pad + chip).min(right), bottom: y + edit_h };
    let clear_left = swatch.right + gap;
    let clear_color = RECT { left: clear_left, top: y, right: (clear_left + clear_w).min(right), bottom: y + edit_h };
    y = swatch.bottom + gap;

    let browse_w = s(128).max(s(BTN_MIN_W));
    // One closure owns the row lists. 0 = field, 1 = field plus browse, 2 = button, 3 = tall field (`extra` is DIP).
    {
        let mut add = |slot: Slot, y: &mut i32, how: u8, extra: i32| {
            let label = RECT { left: pad, top: *y, right, bottom: *y + lab };
            labels.push((slot, label));
            *y = label.bottom + gap4;
            let h = if how == 3 { s(extra) } else { edit_h };
            if how == 1 {
                let b = RECT { left: right - browse_w, top: *y, right, bottom: *y + h };
                let f = RECT { left: pad, top: *y, right: b.left - gap, bottom: *y + h };
                slots.push((slot, f));
                browse.push((slot, b));
            } else {
                slots.push((slot, RECT { left: pad, top: *y, right, bottom: *y + h }));
            }
            *y += h + gap;
        };
        match kind {
            ItemKind::Folder | ItemKind::File => add(Slot::Target, &mut y, 1, 0),
            ItemKind::Special => {
                add(Slot::Place, &mut y, 2, 0);
                add(Slot::Target, &mut y, 0, 0);
            }
            ItemKind::Command => {
                add(Slot::Target, &mut y, 1, 0);
                add(Slot::Args, &mut y, 0, 0);
                add(Slot::Work, &mut y, 1, 0);
            }
            ItemKind::Url => add(Slot::Target, &mut y, 0, 0),
            ItemKind::Rdp => {
                add(Slot::Target, &mut y, 0, 0);
                add(Slot::User, &mut y, 0, 0);
                add(Slot::Pass, &mut y, 0, 0);
                add(Slot::Screen, &mut y, 2, 0);
                add(Slot::Cred, &mut y, 0, 0);
                add(Slot::Extra, &mut y, 3, 52);
            }
        }
    }
    let admin = if kind == ItemKind::Command {
        let r = RECT { left: pad, top: y, right, bottom: y + edit_h };
        y = r.bottom + gap;
        r
    } else {
        RECT::default()
    };
    let note = if kind == ItemKind::Rdp {
        let r = RECT { left: pad, top: y, right, bottom: y + s(64) };
        y = r.bottom + gap;
        r
    } else {
        RECT::default()
    };
    let (saved, clear_pass) = if kind == ItemKind::Rdp && show_clear {
        let saved = RECT { left: pad, top: y, right, bottom: y + lab };
        y = saved.bottom + gap4;
        let clear_pass = RECT { left: pad, top: y, right, bottom: y + edit_h };
        y = clear_pass.bottom + gap;
        (saved, clear_pass)
    } else {
        (RECT::default(), RECT::default())
    };

    let icon_lab = RECT { left: pad, top: y, right, bottom: y + lab };
    labels.push((Slot::IconPath, icon_lab));
    y = icon_lab.bottom + gap4;
    let half = (inner - gap) / 2;
    let icon_auto = RECT { left: pad, top: y, right: pad + half, bottom: y + seg_h };
    let icon_custom = RECT { left: icon_auto.right + gap, top: y, right, bottom: y + seg_h };
    y = icon_custom.bottom + gap;
    let index = if custom_icon {
        let b = RECT { left: right - browse_w, top: y, right, bottom: y + edit_h };
        let index = RECT { left: b.left - gap - s(56), top: y, right: b.left - gap, bottom: y + edit_h };
        let path = RECT { left: pad, top: y, right: index.left - gap, bottom: y + edit_h };
        browse.push((Slot::IconPath, b));
        slots.push((Slot::IconPath, path));
        y = b.bottom + gap;
        index
    } else {
        RECT::default()
    };

    let memo_lab = RECT { left: pad, top: y, right, bottom: y + lab };
    labels.push((Slot::Memo, memo_lab));
    y = memo_lab.bottom + gap4;
    let memo = RECT { left: pad, top: y, right, bottom: y + s(72) };
    slots.push((Slot::Memo, memo));
    y = memo.bottom + gap;

    let bh = s(BTN_H_FOOTER);
    let bw = (inner - s(BTN_GAP)) / 2;
    let ok = RECT { left: pad, top: y, right: pad + bw, bottom: y + bh };
    let cancel = RECT { left: ok.right + s(BTN_GAP), top: y, right, bottom: y + bh };
    y = cancel.bottom + s(PAD_BODY_Y);
    Geom {
        title,
        kind_label,
        kinds,
        name,
        swatch,
        clear_color,
        labels,
        slots,
        browse,
        admin,
        icon_auto,
        icon_custom,
        index,
        note,
        saved,
        clear_pass,
        ok,
        cancel,
        height: y,
    }
}

fn slot_rect(g: &Geom, slot: Slot) -> Option<RECT> {
    g.slots.iter().find(|(s, _)| *s == slot).map(|(_, r)| *r)
}

fn label_of(kind: ItemKind, slot: Slot) -> &'static str {
    match slot {
        Slot::Name => tr(crate::Key::DisplayName),
        Slot::Place => tr(crate::Key::ShellPlace),
        Slot::Target => match kind {
            ItemKind::Command => tr(crate::Key::ItemProgram),
            ItemKind::Url => tr(crate::Key::Address),
            ItemKind::Rdp => tr(crate::Key::ItemHost),
            ItemKind::Special => tr(crate::Key::ShellName),
            _ => tr(crate::Key::Path),
        },
        Slot::Args => tr(crate::Key::Arguments),
        Slot::Work => tr(crate::Key::WorkingDirectory),
        Slot::User => tr(crate::Key::UserName),
        Slot::Pass => tr(crate::Key::Password),
        Slot::Screen => tr(crate::Key::ItemScreen),
        Slot::Cred => tr(crate::Key::CredentialDeleteSeconds),
        Slot::Extra => tr(crate::Key::RdpExtra),
        Slot::IconPath => tr(crate::Key::ItemIcon),
        Slot::Memo => tr(crate::Key::ItemMemo),
        Slot::Color => tr(crate::Key::ItemColour),
    }
}

fn slot_edit(slot: Slot) -> Option<usize> {
    Some(match slot {
        Slot::Name => 0,
        Slot::Target => 1,
        Slot::Args => 2,
        Slot::Work => 3,
        Slot::User => 4,
        Slot::Pass => 5,
        Slot::Cred => 6,
        Slot::Extra => 7,
        Slot::Memo => 8,
        Slot::IconPath => 9,
        Slot::Place | Slot::Screen | Slot::Color => return None,
    })
}

fn sc(dpi: i32, dip: i32) -> i32 {
    px(dpi, dip)
}

unsafe fn place_edits(hwnd: HWND) {
    let Some(v) = view(hwnd) else { return };
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let g = form_geom((*v).dpi, (rc.right - rc.left).max(1), (*v).kind, (*v).custom_icon, (*v).show_clear);
    let m = sc((*v).dpi, FIELD_PAD_X) as isize;
    for h in &(*v).edits {
        if h.0.is_null() {
            continue;
        }
        _ = SendMessageW(*h, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(m | (m << 16))));
    }
    let show = |slot: Slot| {
        let Some(i) = slot_edit(slot) else { return };
        let h = (*v).edits[i];
        if let Some(r) = slot_rect(&g, slot) {
            crate::place_edit(h, field_edit_rect(r, (*v).dpi), true);
        } else {
            crate::place_edit(h, RECT::default(), false);
        }
    };
    show(Slot::Name);
    show(Slot::Target);
    show(Slot::Args);
    show(Slot::Work);
    show(Slot::User);
    show(Slot::Pass);
    show(Slot::Cred);
    show(Slot::Extra);
    show(Slot::Memo);
    show(Slot::IconPath);
    let index = (*v).edits[10];
    if (*v).custom_icon && g.index.right > g.index.left {
        crate::place_edit(index, field_edit_rect(g.index, (*v).dpi), true);
    } else {
        crate::place_edit(index, RECT::default(), false);
    }
    place_combo((*v).place_combo, slot_rect(&g, Slot::Place));
    place_combo((*v).screen_combo, slot_rect(&g, Slot::Screen));
}

unsafe fn place_combo(h: HWND, slot: Option<RECT>) {
    if h.0.is_null() {
        return;
    }
    let Some(r) = slot.filter(|r| r.right > r.left && r.bottom > r.top) else {
        let _ = ShowWindow(h, SW_HIDE);
        return;
    };
    let _ = SetWindowPos(h, None, r.left, r.top, (r.right - r.left).max(1), (r.bottom - r.top).max(1), SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
}

unsafe fn item_combo_face(_hwnd: HWND) -> super::combo::Face {
    let t = super::ui_tokens();
    super::combo::Face { fill: t.field_bg, edge: t.line, ink: t.text, ground: t.bg }
}

unsafe fn fill_place_list(h: HWND) {
    if h.0.is_null() {
        return;
    }
    let _ = SendMessageW(h, CB_RESETCONTENT, None, None);
    for place in shell_places() {
        let hs = HSTRING::from(tr(place.label));
        let _ = SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(hs.as_ptr() as isize)));
    }
}

unsafe fn fill_screen_list(h: HWND) {
    if h.0.is_null() {
        return;
    }
    let _ = SendMessageW(h, CB_RESETCONTENT, None, None);
    for label in screen_choices() {
        let hs = HSTRING::from(label);
        let _ = SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(hs.as_ptr() as isize)));
    }
}

fn screen_choices() -> Vec<String> {
    let mut out = vec![tr(crate::Key::Automatic).to_string(), tr(crate::Key::ScreenFull).to_string()];
    for (w, h) in RDP_WINDOWS {
        out.push(format!("{w} × {h}"));
    }
    out
}

unsafe fn sync_place(h: HWND, path: &str) {
    if h.0.is_null() {
        return;
    }
    let sel = shell_places().iter().position(|p| p.path.eq_ignore_ascii_case(path.trim()));
    let index = sel.map(|i| i as usize).unwrap_or(usize::MAX);
    let _ = SendMessageW(h, CB_SETCURSEL, Some(WPARAM(index)), None);
}

unsafe fn sync_screen(h: HWND, screen: RdpScreen) {
    if h.0.is_null() {
        return;
    }
    let sel = match screen {
        RdpScreen::Auto => 0,
        RdpScreen::Full => 1,
        RdpScreen::Window { w, h } => RDP_WINDOWS.iter().position(|(ww, hh)| *ww == w && *hh == h).map(|i| i + 2).unwrap_or(0),
    };
    let _ = SendMessageW(h, CB_SETCURSEL, Some(WPARAM(sel)), None);
}

unsafe fn chosen_place(h: HWND) -> Option<&'static str> {
    if h.0.is_null() {
        return None;
    }
    let sel = SendMessageW(h, CB_GETCURSEL, None, None).0;
    if sel < 0 {
        return None;
    }
    shell_places().get(sel as usize).map(|p| p.path)
}

unsafe fn chosen_screen(h: HWND) -> Option<RdpScreen> {
    if h.0.is_null() {
        return None;
    }
    let sel = SendMessageW(h, CB_GETCURSEL, None, None).0;
    if sel < 0 {
        return None;
    }
    Some(match sel {
        0 => RdpScreen::Auto,
        1 => RdpScreen::Full,
        n => {
            let (w, h) = RDP_WINDOWS.get((n as usize) - 2)?;
            RdpScreen::Window { w: *w, h: *h }
        }
    })
}

unsafe fn fit_window(h: HWND) {
    let Some(v) = view(h) else { return };
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let width = (rc.right - rc.left).max(sc((*v).dpi, 520));
    let want = form_geom((*v).dpi, width, (*v).kind, (*v).custom_icon, (*v).show_clear).height;
    if (rc.bottom - rc.top - want).abs() < 2 {
        place_edits(h);
        return;
    }
    let (w, height) = outer_size(width, want, (*v).dpi);
    let mut wr = RECT::default();
    let _ = GetWindowRect(h, &mut wr);
    _ = SetWindowPos(h, None, wr.left, wr.top, w, height, SWP_NOZORDER | SWP_NOACTIVATE);
    place_edits(h);
}

fn view(hwnd: HWND) -> Option<*mut Dlg> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Dlg;
    if p.is_null() { None } else { Some(p) }
}

unsafe extern "system" fn item_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            place_edits(h);
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
        SAVE_MSG => {
            commit(h);
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            if view(h).is_none() {
                return DefWindowProcW(h, msg, wp, lp);
            }
            color_field_edit(HDC(wp.0 as *mut _))
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let code = ((wp.0 >> 16) & 0xffff) as u32;
            if code == CBN_SELCHANGE {
                apply_combo(h, id);
            } else if code == EN_CHANGE && id == ID_TARGET {
                if let Some(v) = view(h) {
                    if (*v).kind == ItemKind::Special && GetFocus() != (*v).place_combo {
                        sync_place((*v).place_combo, &text_of((*v).edits[1]));
                    }
                }
            }
            if code == 0x0100 || code == 0x0200 || code == EN_CHANGE || code == CBN_SELCHANGE {
                let _ = InvalidateRect(Some(h), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(h);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            hit(h, lp, false, false);
            arm_leave(h);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(v) = view(h) {
                (*v).hot = -1;
                let _ = InvalidateRect(Some(h), None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            hit(h, lp, true, false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            hit(h, lp, false, true);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
            _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize => {
            commit(h);
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let hit = DefWindowProcW(h, msg, wp, lp);
            if hit.0 == HTCLIENT as isize {
                let (_x, y) = lp_xy(lp);
                if y < sc(dpi_of(h), PAD_BODY_X + FONT_PX + SPACE_4) {
                    return LRESULT(HTCAPTION as isize);
                }
            }
            hit
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn paint(h: HWND) {
    let Some(v) = view(h) else { return };
    let kind = (*v).kind;
    let dpi = (*v).dpi;
    let custom = (*v).custom_icon;
    let show_clear = (*v).show_clear;
    let hot = (*v).hot;
    let down = (*v).down;
    let color = (*v).color;
    let admin = (*v).admin;
    let clear_pass = (*v).clear_password;
    let edits = (*v).edits;
    buffered(h, |dc, rc| {
        let g = form_geom(dpi, (rc.right - rc.left).max(1), kind, custom, show_clear);
        paint_window(dc, rc);
        let body = body_font();
        let bold = bold_ui_font();
        let ground = super::ui_tokens().bg;
        let t = super::ui_tokens();
        let focus = GetFocus();
        let hot_id = (hot >= 0).then_some(hot);
        let down_id = (down >= 0).then_some(down);
        let editing = view(h).is_some_and(|p| (*p).editing);
        let title = if editing { tr(crate::Key::EditFavourite) } else { tr(crate::Key::AddItem) };
        paint_section_header(dc, g.title, title, bold);
        paint_meta(dc, g.kind_label, tr(crate::Key::ItemKind), body);
        for (i, kind_i) in KINDS.iter().enumerate() {
            let vis = visual_from_pointer(i as i32, hot_id, down_id);
            paint_segment(dc, g.kinds[i], tr(kind_i.label()), kind == *kind_i, vis, dpi, ground);
        }
        for (slot, r) in &g.labels {
            paint_meta(dc, *r, label_of(kind, *slot), body);
        }
        paint_chip(dc, g.swatch, dpi, ground);
        if let Some(color) = color {
            paint_wash(dc, inset(g.swatch, sc(dpi, 3)), dpi, color, t.card);
        }
        paint_button(dc, g.clear_color, ButtonKind::Secondary, visual_from_pointer(HIT_CLEAR_COLOR, hot_id, down_id), tr(crate::Key::ClearBasket), body, dpi, ground, FocusRing::Outside);
        for (slot, r) in &g.slots {
            if matches!(slot, Slot::Place | Slot::Screen) {
                continue;
            }
            let focused = slot_edit(*slot).is_some_and(|i| focus == edits[i]);
            paint_field(dc, *r, dpi, focused);
        }
        if g.index.right > g.index.left && g.index.bottom > g.index.top {
            paint_field(dc, g.index, dpi, focus == edits[10]);
        }
        for (_, r) in &g.browse {
            paint_button(dc, *r, ButtonKind::Secondary, visual_from_pointer(browse_hit(*r, &g), hot_id, down_id), tr(crate::Key::Browse), body, dpi, ground, FocusRing::Outside);
        }
        if g.admin.bottom > g.admin.top {
            let tick_w = sc(dpi, 16);
            let tick = RECT { left: g.admin.left, top: g.admin.top + sc(dpi, 4), right: g.admin.left + tick_w, bottom: g.admin.top + tick_w + sc(dpi, 4) };
            paint_check(dc, tick, admin, hot == HIT_ADMIN, dpi, ground);
            paint_label(dc, RECT { left: tick.right + sc(dpi, SPACE_8), ..g.admin }, tr(crate::Key::RunAsAdministrator), body);
        }
        if g.note.bottom > g.note.top {
            wrap_text(dc, g.note, tr(crate::Key::PasswordInCredentialManager), body, t.sub, false);
        }
        if g.saved.bottom > g.saved.top {
            paint_meta(dc, g.saved, tr(crate::Key::SavedPassword), body);
        }
        if g.clear_pass.bottom > g.clear_pass.top {
            let tick_w = sc(dpi, 16);
            let tick = RECT { left: g.clear_pass.left, top: g.clear_pass.top + sc(dpi, 4), right: g.clear_pass.left + tick_w, bottom: g.clear_pass.top + tick_w + sc(dpi, 4) };
            paint_check(dc, tick, clear_pass, hot == HIT_CLEAR_PASS, dpi, ground);
            paint_label(dc, RECT { left: tick.right + sc(dpi, SPACE_8), ..g.clear_pass }, tr(crate::Key::ClearSavedPassword), body);
        }
        paint_segment(dc, g.icon_auto, tr(crate::Key::Automatic), !custom, visual_from_pointer(HIT_ICON_AUTO, hot_id, down_id), dpi, ground);
        paint_segment(dc, g.icon_custom, tr(crate::Key::IconCustom), custom, visual_from_pointer(HIT_ICON_CUSTOM, hot_id, down_id), dpi, ground);
        let ok = if editing { tr(crate::Key::Save) } else { tr(crate::Key::ItemAdd) };
        paint_button(dc, g.ok, ButtonKind::Primary, visual_from_pointer(HIT_OK, hot_id, down_id), ok, bold, dpi, ground, FocusRing::Outside);
        paint_button(dc, g.cancel, ButtonKind::Secondary, visual_from_pointer(HIT_CANCEL, hot_id, down_id), tr(crate::Key::Cancel), body, dpi, ground, FocusRing::Outside);
    });
}

fn browse_hit(r: RECT, g: &Geom) -> i32 {
    g.browse.iter().find(|(_, b)| *b == r).map(|(slot, _)| match slot {
        Slot::Work => HIT_BROWSE_WORK,
        Slot::IconPath => HIT_BROWSE_ICON,
        _ => HIT_BROWSE_TARGET,
    }).unwrap_or(HIT_BROWSE_TARGET)
}

fn inset(r: RECT, d: i32) -> RECT {
    RECT { left: r.left + d, top: r.top + d, right: (r.right - d).max(r.left + d), bottom: (r.bottom - d).max(r.top + d) }
}

unsafe fn hit(h: HWND, lp: LPARAM, down: bool, up: bool) {
    let Some(v) = view(h) else { return };
    let (x, y) = lp_xy(lp);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let g = form_geom((*v).dpi, (rc.right - rc.left).max(1), (*v).kind, (*v).custom_icon, (*v).show_clear);
    let mut id = -1i32;
    for (i, r) in g.kinds.iter().enumerate() {
        if inside(*r, x, y) {
            id = i as i32;
        }
    }
    let pairs = [
        (g.swatch, HIT_SWATCH),
        (g.clear_color, HIT_CLEAR_COLOR),
        (g.admin, HIT_ADMIN),
        (g.icon_auto, HIT_ICON_AUTO),
        (g.icon_custom, HIT_ICON_CUSTOM),
        (g.clear_pass, HIT_CLEAR_PASS),
        (g.ok, HIT_OK),
        (g.cancel, HIT_CANCEL),
    ];
    for (r, hit) in pairs {
        if r.bottom > r.top && inside(r, x, y) {
            id = hit;
        }
    }
    for (slot, r) in &g.browse {
        if inside(*r, x, y) {
            id = match slot {
                Slot::Work => HIT_BROWSE_WORK,
                Slot::IconPath => HIT_BROWSE_ICON,
                _ => HIT_BROWSE_TARGET,
            };
        }
    }
    if down {
        (*v).down = id;
        (*v).hot = id;
        if id >= 0 {
            _ = SetCapture(h);
        }
        let _ = InvalidateRect(Some(h), None, false);
        return;
    }
    if up {
        let fired = (*v).down == id && id >= 0;
        (*v).down = -1;
        (*v).hot = id;
        let _ = ReleaseCapture();
        let _ = InvalidateRect(Some(h), None, false);
        if fired {
            activate(h, id);
        }
        return;
    }
    if (*v).hot != id {
        (*v).hot = id;
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn activate(h: HWND, id: i32) {
    let Some(v) = view(h) else { return };
    if (0..6).contains(&id) {
        (*v).kind = KINDS[id as usize];
        if (*v).kind != ItemKind::Rdp {
            (*v).clear_password = false;
        }
        fit_window(h);
        sync_place((*v).place_combo, &text_of((*v).edits[1]));
        sync_screen((*v).screen_combo, (*v).screen);
        let _ = InvalidateRect(Some(h), None, true);
        return;
    }
    match id {
        HIT_SWATCH => {
            if let Some(c) = pick_color(h, (*v).color) {
                (*v).color = Some(c);
                let _ = InvalidateRect(Some(h), None, false);
            }
        }
        HIT_CLEAR_COLOR => {
            (*v).color = None;
            let _ = InvalidateRect(Some(h), None, false);
        }
        HIT_ADMIN => {
            (*v).admin = !(*v).admin;
            let _ = InvalidateRect(Some(h), None, false);
        }
        HIT_ICON_AUTO => {
            (*v).custom_icon = false;
            fit_window(h);
            let _ = InvalidateRect(Some(h), None, true);
        }
        HIT_ICON_CUSTOM => {
            (*v).custom_icon = true;
            fit_window(h);
            let _ = InvalidateRect(Some(h), None, true);
        }
        HIT_CLEAR_PASS => {
            (*v).clear_password = !(*v).clear_password;
            let _ = InvalidateRect(Some(h), None, false);
        }
        HIT_BROWSE_TARGET => browse_target(h),
        HIT_BROWSE_WORK => {
            if let Some(path) = crate::pick_folder(h) {
                set_text((*view(h).unwrap()).edits[3], &path);
            }
        }
        HIT_BROWSE_ICON => {
            if let Some(path) = pick_icon(h) {
                set_text((*view(h).unwrap()).edits[9], &path);
            }
        }
        HIT_OK => commit(h),
        HIT_CANCEL => _ = DestroyWindow(h),
        _ => {}
    }
}

unsafe fn browse_target(h: HWND) {
    let Some(v) = view(h) else { return };
    let path = match (*v).kind {
        ItemKind::Folder | ItemKind::Special => crate::pick_folder(h),
        ItemKind::File | ItemKind::Command => pick_any(h),
        _ => None,
    };
    if let Some(path) = path {
        set_text((*v).edits[1], &path);
        if (*v).kind == ItemKind::Special {
            sync_place((*v).place_combo, &path);
        }
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn apply_combo(h: HWND, id: i32) {
    let Some(v) = view(h) else { return };
    if id == ID_PLACE {
        if let Some(path) = chosen_place((*v).place_combo) {
            set_text((*v).edits[1], path);
        }
    } else if id == ID_SCREEN {
        if let Some(screen) = chosen_screen((*v).screen_combo) {
            (*v).screen = screen;
        }
    }
}

unsafe fn pick_color(owner: HWND, current: Option<u32>) -> Option<u32> {
    let mut custom = [COLORREF(0xFFFFFF); 16];
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: crate::cref(current.unwrap_or(0x3D7EE0)),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    ChooseColorW(&mut cc).as_bool().then(|| crate::cref(cc.rgbResult.0).0)
}

unsafe fn pick_any(owner: HWND) -> Option<String> {
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let opts = dlg.GetOptions().unwrap_or_default();
    dlg.SetOptions(opts | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
    dlg.Show(Some(owner)).ok()?;
    let name = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    Some(crate::pwstr(name))
}

unsafe fn pick_icon(owner: HWND) -> Option<String> {
    let path = pick_any(owner)?;
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "ico" | "exe" | "dll").then_some(path)
}

unsafe fn commit(h: HWND) {
    let Some(v) = view(h) else { return };
    let mut draft = read_draft(v);
    if let Err(fault) = crate::session::validate(&draft) {
        tell(h, tr(fault.key()));
        wipe_draft(&mut draft);
        return;
    }
    let old = if (*v).editing {
        app(|a| node_ref(&a.s.tree, &(*v).edit_id).cloned())
    } else {
        None
    };
    let old_kind = old.as_ref().map(|n| n.kind());
    let old_host = old.as_ref().filter(|n| n.kind() == ItemKind::Rdp).map(|n| n.path.clone()).unwrap_or_default();
    let old_has = old.as_ref().and_then(|n| n.item.as_ref()).is_some_and(|i| i.has_password);
    let has = stored_has_password(old_has, &draft);
    let mut plan: SecretPlan = crate::session::secret_plan(old_kind, &old_host, old_has, &draft);
    if !stage_secret(&mut plan) {
        tell(h, tr(crate::Key::CouldNotSavePassword));
        wipe_draft(&mut draft);
        return;
    }
    let node = node_from_draft(&draft, has);
    wipe_draft(&mut draft);
    let parent = (*v).parent.clone();
    let editing = (*v).editing;
    let id = (*v).edit_id.clone();
    let saved = app(|a| {
        a.s.ensure_fav_tree();
        let ok = if editing {
            write_leaf(&mut a.s.tree, &id, node)
        } else {
            crate::session::append_node(&mut a.s.tree, &parent, node)
        };
        if ok {
            a.s.sync_favs();
        }
        ok
    });
    if !saved {
        _ = MessageBeep(MB_ICONWARNING);
        tell(h, tr(crate::Key::AlreadyExists));
        return;
    }
    forget_hosts(&plan.forget);
    crate::commit_favs();
    _ = DestroyWindow(h);
}

fn read_draft(v: *mut Dlg) -> Draft {
    unsafe {
        if (*v).kind == ItemKind::Rdp {
            if let Some(screen) = chosen_screen((*v).screen_combo) {
                (*v).screen = screen;
            }
        }
        let mut d = Draft::blank((*v).kind);
        d.name = text_of((*v).edits[0]);
        d.target = text_of((*v).edits[1]);
        d.args = text_of((*v).edits[2]);
        d.workdir = text_of((*v).edits[3]);
        d.user = text_of((*v).edits[4]);
        d.password = text_of((*v).edits[5]);
        let cred = text_of((*v).edits[6]);
        d.cred_secs = cred.trim().parse().unwrap_or(10);
        d.rdp_extra = text_of((*v).edits[7]);
        d.memo = text_of((*v).edits[8]);
        d.admin = (*v).admin;
        d.clear_password = (*v).clear_password;
        d.color = (*v).color;
        d.screen = (*v).screen;
        if (*v).custom_icon {
            let path = text_of((*v).edits[9]);
            let index = text_of((*v).edits[10]).trim().parse().unwrap_or(0);
            d.icon = Some(ItemIcon { path, index });
        }
        d
    }
}

fn wipe_draft(d: &mut Draft) {
    let mut bytes = std::mem::take(&mut d.password).into_bytes();
    bytes.fill(0);
}

unsafe fn tell(h: HWND, text: &str) {
    _ = MessageBoxW(Some(h), &HSTRING::from(text), w!("Maguro"), MB_ICONWARNING);
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
    let style = WS_POPUP | WS_CLIPCHILDREN;
    if AdjustWindowRectExForDpi(&mut r, style, false, WS_EX_DLGMODALFRAME, dpi.max(96) as u32).is_ok() {
        ((r.right - r.left).max(client_w), (r.bottom - r.top).max(client_h))
    } else {
        (client_w, client_h)
    }
}

fn round_window(hwnd: HWND) {
    unsafe { style_popup_chrome(hwnd) }
}

unsafe fn drop_view(h: HWND) {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Dlg;
    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
    if p.is_null() {
        return;
    }
    let dlg = Box::from_raw(p);
    if !dlg.edits[5].0.is_null() {
        _ = SetWindowTextW(dlg.edits[5], w!(""));
    }
}

fn ensure_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(m) = GetModuleHandleW(None) else { return };
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(item_proc),
            hInstance: m.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MaguroItem"),
            ..Default::default()
        });
    });
}

unsafe fn subclass_edit(h: HWND) {
    if h.0.is_null() {
        return;
    }
    let prev = SetWindowLongPtrW(h, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
    if prev != 0 && EDIT_PROC.load(Ordering::Relaxed) == 0 {
        EDIT_PROC.store(prev, Ordering::Relaxed);
    }
}

unsafe extern "system" fn edit_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_KEYDOWN && (wp.0 == VK_RETURN.0 as usize || wp.0 == VK_ESCAPE.0 as usize) {
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        let multiline = style & 0x4 != 0;
        if let Ok(parent) = GetParent(h) {
            if wp.0 == VK_ESCAPE.0 as usize {
                let _ = PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            if !multiline {
                let _ = PostMessageW(Some(parent), SAVE_MSG, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
    }
    let prev = EDIT_PROC.load(Ordering::Relaxed);
    if prev == 0 {
        DefWindowProcW(h, msg, wp, lp)
    } else {
        let f: WNDPROC = std::mem::transmute(prev);
        CallWindowProcW(f, h, msg, wp, lp)
    }
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn lp_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(g: &Geom) -> Vec<RECT> {
        let mut v = Vec::new();
        v.push(g.title);
        v.push(g.kind_label);
        v.extend(g.kinds);
        v.push(g.swatch);
        v.push(g.clear_color);
        v.extend(g.labels.iter().map(|(_, r)| *r));
        v.extend(g.slots.iter().map(|(_, r)| *r));
        v.extend(g.browse.iter().map(|(_, r)| *r));
        for r in [g.admin, g.icon_auto, g.icon_custom, g.index, g.note, g.saved, g.clear_pass, g.ok, g.cancel] {
            if r.bottom > r.top && r.right > r.left {
                v.push(r);
            }
        }
        v
    }

    fn overlap(a: RECT, b: RECT) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    fn has(g: &Geom, slot: Slot) -> bool {
        g.slots.iter().any(|(s, _)| *s == slot)
    }

    #[test]
    fn every_kind_fits_at_96_144_and_192() {
        for dpi in [96, 144, 192] {
            let width = 520 * dpi / 96;
            for kind in KINDS {
                for custom in [false, true] {
                    for clear in [false, true] {
                        let g = form_geom(dpi, width, kind, custom, clear);
                        assert!(g.height > g.cancel.bottom - 1, "{kind:?} {dpi}");
                        assert!(g.ok.right < g.cancel.left || g.ok.right == g.cancel.left, "{:?}", g.ok);
                        assert!(g.cancel.right <= width, "cancel {} > {width}", g.cancel.right);
                        assert!(g.ok.left >= 16 * dpi / 96);
                        assert!(g.ok.right - g.ok.left >= 72 * dpi / 96, "ok button");
                        assert!(g.cancel.right - g.cancel.left >= 72 * dpi / 96, "cancel");
                        assert_eq!(g.ok.bottom - g.ok.top, 28 * dpi / 96);
                        assert_eq!(g.cancel.left - g.ok.right, 8 * dpi / 96);
                        assert_eq!(g.kinds[0].bottom - g.kinds[0].top, 28 * dpi / 96);
                        let seg_w = g.kinds[2].right - g.kinds[2].left;
                        assert!(seg_w >= 140 * dpi / 96, "segment {seg_w} at {dpi}");
                        assert!(g.kinds[2].right <= width - 16 * dpi / 96);
                        assert_eq!(g.kinds[0].top, g.kinds[2].top);
                        assert!(g.kinds[3].top >= g.kinds[0].bottom);
                        assert_eq!(g.icon_auto.top, g.icon_custom.top);
                        assert!(g.icon_auto.right <= g.icon_custom.left);
                        assert!(g.name.right >= width - 16 * dpi / 96);
                        assert!(g.swatch.top >= g.name.bottom);
                        assert!(g.clear_color.left >= g.swatch.right);
                        assert!(g.clear_color.right - g.clear_color.left >= 72 * dpi / 96);
                        if custom {
                            assert!(has(&g, Slot::IconPath));
                            let path = g.slots.iter().find(|(s, _)| *s == Slot::IconPath).unwrap().1;
                            assert!(path.top >= g.icon_custom.bottom);
                            assert!(path.right <= g.index.left);
                            assert!(g.index.right < g.browse.iter().find(|(s, _)| *s == Slot::IconPath).unwrap().1.left || g.index.right == g.browse.iter().find(|(s, _)| *s == Slot::IconPath).unwrap().1.left);
                        }
                        let all = boxes(&g);
                        for r in &all {
                            assert!(r.left >= 0 && r.top >= 0, "{r:?}");
                            assert!(r.right <= width && r.bottom <= g.height, "{r:?} h {}", g.height);
                        }
                        for i in 0..all.len() {
                            for j in (i + 1)..all.len() {
                                assert!(!overlap(all[i], all[j]), "{kind:?} dpi {dpi} custom {custom} {:?} vs {:?}", all[i], all[j]);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rdp_shows_host_password_and_screen_folder_does_not() {
        let rdp = form_geom(96, 520, ItemKind::Rdp, false, true);
        assert!(has(&rdp, Slot::Target));
        assert!(has(&rdp, Slot::User));
        assert!(has(&rdp, Slot::Pass));
        assert!(has(&rdp, Slot::Screen));
        assert!(has(&rdp, Slot::Cred));
        assert!(has(&rdp, Slot::Extra));
        assert!(rdp.note.bottom > rdp.note.top);
        assert!(rdp.clear_pass.bottom > rdp.clear_pass.top);
        assert!(rdp.admin.bottom == 0);
        let folder = form_geom(96, 520, ItemKind::Folder, false, false);
        assert!(has(&folder, Slot::Target));
        assert!(folder.browse.iter().any(|(s, _)| *s == Slot::Target));
        assert!(!has(&folder, Slot::Pass));
        assert!(!has(&folder, Slot::Screen));
        assert!(folder.note.bottom == 0);
        let url = form_geom(96, 520, ItemKind::Url, true, false);
        assert!(url.browse.iter().all(|(s, _)| *s != Slot::Target));
        assert!(has(&url, Slot::IconPath));
        let cmd = form_geom(96, 520, ItemKind::Command, false, false);
        assert!(has(&cmd, Slot::Args));
        assert!(has(&cmd, Slot::Work));
        assert!(cmd.admin.bottom > cmd.admin.top);
        assert!(cmd.height < rdp.height);
    }

    #[test]
    fn field_face_follows_light_and_dark_tokens() {
        let light = crate::tokens_for(false);
        let dark = crate::tokens_for(true);
        assert_ne!(light.bg, dark.bg);
        assert_ne!(light.field_bg, dark.field_bg);
        assert_ne!(light.text, dark.text);
        assert_ne!(light.line, dark.line);
        assert_eq!(light.field_bg, crate::FIELD_LIGHT);
        assert_eq!(dark.field_bg, crate::FIELD_DARK);
        for dpi in [96, 144, 192] {
            let light_g = form_geom(dpi, 520 * dpi / 96, ItemKind::Folder, false, false);
            let dark_g = form_geom(dpi, 520 * dpi / 96, ItemKind::Folder, false, false);
            assert_eq!(light_g.height, dark_g.height);
            assert_eq!(light_g.kinds, dark_g.kinds);
        }
    }
}
