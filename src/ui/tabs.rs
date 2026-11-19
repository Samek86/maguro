use crate::imports::*;
use super::*;
use crate::app::*;
use crate::pane::*;
use crate::session::*;
use crate::i18n::*;


/// Widest a tab grows, at 96 DPI. A longer title is drawn with `DT_END_ELLIPSIS`.
const TAB_MAX_W: i32 = 135;
/// Tracking tooltip for the paths under one tab. The lock glyph uses id 7.
const TAB_TIP_ID: usize = 8;
const ICON_LIST: &str = "\u{E8FD}";

// Segoe Fluent Icons / MDL2 Assets code points
pub(crate) const ICON_SEARCH: &str = "\u{E721}";
pub(crate) const ICON_FILTER: &str = "\u{E71C}";
pub(crate) const ICON_LOCK: &str = "\u{E72E}";
pub(crate) const ICON_CLOSE: &str = "\u{E711}";
pub(crate) const ICON_PLUS: &str = "\u{E710}";
pub(crate) const ICON_CHEVRON_DOWN: &str = "\u{E70D}";
/// Tab stripe colours, 0xRRGGBB. Chosen so a thin bar and a light tint read on both themes.
pub(crate) const TAB_COLORS: [(u32, crate::Key); 8] = [
    (0xE15A4A, crate::Key::ColorRed),
    (0xE8943A, crate::Key::ColorOrange),
    (0xC49212, crate::Key::ColorYellow),
    (0x3EAE6E, crate::Key::ColorGreen),
    (0x2A9D9A, crate::Key::ColorTeal),
    (0x3D7EE0, crate::Key::ColorBlue),
    (0x8A5CD0, crate::Key::ColorPurple),
    (0xD45A8C, crate::Key::ColorPink),
];
pub(crate) const CLOSED_KEEP: usize = 10;
pub(crate) const ID_TAB_EDIT: i32 = 208;

pub(crate) fn glyph(id: i32) -> &'static str {
    match id {
        ID_BACK => "\u{E72B}",
        ID_FWD => "\u{E72A}",
        ID_UP => "\u{E74A}",
        ID_OPEN_IN => "\u{E8AB}",
        ID_PANE_ADD => ICON_PLUS,
        ID_TB_LAYOUT => "\u{E8A9}",
        // Tab-row save/load are drawn in sicon.rs. These points remain for glyph().
        ID_TB_SAVE => "\u{E74E}",
        ID_TB_LOAD => "\u{E838}",
        ID_TB_SET => "\u{E713}",
        ID_TB_LIST => ICON_LIST,
        _ => "\u{E890}",
    }
}

// ---------- tabs ----------
// The strip is its own child so splitter hit-testing stays below it. Every hidden tab keeps its
// grid on the session. Only the most recently hidden tab also keeps its Explorer windows.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TabHit {
    Tab(usize),
    Close(usize),
    /// The close slot of a locked tab. It does not close.
    Lock(usize),
    Plus,
    /// The ▾ that lists every tab when they no longer fit.
    Overflow,
    Empty,
}

pub(crate) struct TabLay {
    pub(crate) body: Vec<RECT>,
    pub(crate) close: Vec<Option<RECT>>,
    pub(crate) lock: Vec<bool>,
    pub(crate) plus: RECT,
    /// Set when the tabs do not fit at `min_w`. Sits at the right of the tab area, left of the icon group.
    pub(crate) overflow: Option<RECT>,
    /// First tab that has a rect. Clamped so the window never runs past the last tab.
    pub(crate) first: usize,
    pub(crate) count: usize,
}

impl TabLay {
    /// Tab under a drag, among the ones on screen. Past the last visible tab, that tab.
    pub(crate) fn drag_target(&self, x: i32) -> usize {
        tab_drag_index(&self.body, x)
    }

    /// Where a tab dragged in from another window is inserted.
    pub(crate) fn insert_at(&self, x: i32) -> usize {
        self.body
            .iter()
            .enumerate()
            .find(|(_, r)| r.right > r.left && x < (r.left + r.right) / 2)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.body.iter().enumerate().rev().find(|(_, r)| r.right > r.left).map(|(i, _)| i + 1).unwrap_or(self.body.len()))
    }

    /// Same as [`insert_at`], using the vertical midpoint. Past the last row, after that row.
    pub(crate) fn insert_at_y(&self, y: i32) -> usize {
        self.body
            .iter()
            .enumerate()
            .find(|(_, r)| r.bottom > r.top && y < (r.top + r.bottom) / 2)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.body.iter().enumerate().rev().find(|(_, r)| r.bottom > r.top).map(|(i, _)| i + 1).unwrap_or(self.body.len()))
    }
}

/// Index a vertical drag lands on. The first body whose midpoint is below `y`, else the last one with a height.
pub(crate) fn vtab_drag_index(body: &[RECT], y: i32) -> usize {
    body.iter()
        .enumerate()
        .find(|(_, r)| r.bottom > r.top && y < (r.top + r.bottom) / 2)
        .map(|(i, _)| i)
        .unwrap_or_else(|| body.iter().enumerate().rev().find(|(_, r)| r.bottom > r.top).map(|(i, _)| i).unwrap_or(0))
}

/// Index a horizontal drag lands on. The first body whose midpoint is to the right of `x`, else the last one with a width.
pub(crate) fn tab_drag_index(body: &[RECT], x: i32) -> usize {
    body.iter()
        .enumerate()
        .find(|(_, r)| r.right > r.left && x < (r.left + r.right) / 2)
        .map(|(i, _)| i)
        .unwrap_or_else(|| body.iter().enumerate().rev().find(|(_, r)| r.right > r.left).map(|(i, _)| i).unwrap_or(0))
}

/// X uses the accent while the pointer is on it. A lock stays the quiet colour, hot or not.
pub(crate) fn close_ink(locked: bool, hot: bool, accent: u32, sub: u32) -> u32 {
    if locked || !hot { sub } else { accent }
}

pub(crate) fn close_glyph(locked: bool) -> &'static str {
    if locked { ICON_LOCK } else { ICON_CLOSE }
}

/// Middle-click closes an unlocked tab. A lock, and the body of a locked tab, do not.
pub(crate) fn middle_closes(lay: &TabLay, hit: TabHit) -> bool {
    match hit {
        TabHit::Close(i) | TabHit::Tab(i) => !lay.lock.get(i).copied().unwrap_or(false),
        _ => false,
    }
}

/// Right-hand group on the tab row. Drive letters are a prefix of this, left of Layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IconLay {
    /// Width the tabs may use. A gap sits between this and the group.
    pub(crate) tab_w: i32,
    pub(crate) icons: [(i32, RECT); ICON_N],
    /// Drive buttons that fit, left to right. A prefix of the A→Z list; the rest are hidden.
    pub(crate) drives: [RECT; 26],
    pub(crate) drive_n: usize,
    /// Tab-list button, immediately left of the drive letters or the icon group.
    /// Same size and vertical span as the glyph toolbar buttons (gear, star, save, load).
    pub(crate) list: RECT,
}

/// pad, gap, plus, max tab, min tab, overflow chevron. Shared with the drive-button reserve.
/// The chevron width matches the +, so both buttons are the same square.
fn tab_px(dpi: i32) -> (i32, i32, i32, i32, i32, i32) {
    let s = |v: i32| v * dpi / 96;
    let plus = s(28);
    (s(6), s(2), plus, s(TAB_MAX_W), s(44), plus)
}

/// Width `tab_lay` needs for one minimum tab, the +, and the ▾. Drive buttons stop before this.
fn tab_reserve(dpi: i32) -> i32 {
    let (pad, gap, plus_w, _, min_w, over_w) = tab_px(dpi);
    pad + min_w + gap + plus_w + gap + over_w + pad
}

/// How many drive buttons the tab row reserves. Off reserves none, so the tabs take that width.
pub(crate) fn drive_slots(show: bool, n: usize) -> usize {
    if show { n } else { 0 }
}

/// Glyph button on the main-window tool row, width in DIP.
/// Save, load, the star, the gear, drive letters, and the tab list share it.
/// The layout button is wider because it also shows the grid. Height is [`BTN_H_TOOL`].
pub(crate) const TOOL_ICON_W: i32 = 30;
/// Space between neighbouring icon buttons on the tab row, in DIP.
pub(crate) const TOOL_ICON_GAP: i32 = 2;

pub(crate) fn tool_icon_px(dpi: i32) -> (i32, i32) {
    let s = |v: i32| v * dpi / 96;
    (s(TOOL_ICON_W), s(BTN_H_TOOL))
}

/// Tab-row icon buttons and the line master [`draw_button`] paints on each: same size, ink,
/// hover, and press. The layout button is not one of them; it also shows the grid as text.
pub(crate) fn tool_line_icon(id: i32) -> Option<LineIcon> {
    Some(match id {
        ID_TB_LIST => LineIcon::List,
        ID_TB_SAVE => LineIcon::Save,
        ID_TB_LOAD => LineIcon::Load,
        ID_TB_FAV => LineIcon::Star,
        ID_TB_BASKET => LineIcon::Basket,
        ID_TB_WATCH => LineIcon::Watch,
        ID_TB_SET => LineIcon::Gear,
        _ => return None,
    })
}

/// Layout, save, load, star, basket, watch, gear.
pub(crate) const ICON_N: usize = 7;
/// Index of the gear in [`IconLay::icons`]. It is the last one.
pub(crate) const GEAR: usize = ICON_N - 1;

fn icon_widths(dpi: i32) -> [i32; ICON_N] {
    let s = |v: i32| v * dpi / 96;
    let icon = tool_icon_px(dpi).0;
    [s(70), icon, icon, icon, icon, icon, icon]
}

/// The gear stays when the toolbar is hidden. It is the mouse path to settings.
fn icon_on(bar: bool) -> [bool; ICON_N] {
    let mut on = [bar; ICON_N];
    on[GEAR] = true;
    on
}

/// Narrowest tab row that still keeps one minimum tab clear of the icon group.
/// Drive letters are not part of this: they hide before the group does.
pub(crate) fn icon_group_min(dpi: i32, bar: bool) -> i32 {
    let s = |v: i32| v * dpi / 96;
    let (pad, gap) = (s(8), s(TOOL_ICON_GAP));
    let on = icon_on(bar);
    let n_on = on.iter().filter(|v| **v).count() as i32;
    let icons_w = icon_widths(dpi).iter().zip(&on).filter(|(_, v)| **v).map(|(w, _)| *w).sum::<i32>() + gap * (n_on - 1).max(0);
    let list_w = tool_icon_px(dpi).0;
    tab_reserve(dpi) + gap + list_w + gap + icons_w + pad
}

/// Layout, save, load, the favourites star, the gear, and `drives` letter buttons, right-aligned on the tab row.
/// Letters sit to the left of Layout. The last ones drop when the row cannot also keep `tab_reserve`.
/// `drives == 0` (see `drive_slots`) hides every letter and gives that width to the tabs.
/// `bar == false` keeps the gear and the tab-list button. The other icons and the letters are not placed, and the tabs use that width.
/// The star, the file basket, and folder watch sit in that order immediately left of the gear.
pub(crate) fn icon_slots(row_w: i32, top: i32, dpi: i32, drives: usize, bar: bool) -> IconLay {
    let s = |v: i32| v * dpi / 96;
    let (icon_w, bh) = tool_icon_px(dpi);
    let (pad, gap) = (s(8), s(TOOL_ICON_GAP));
    let specs = [ID_TB_LAYOUT, ID_TB_SAVE, ID_TB_LOAD, ID_TB_FAV, ID_TB_BASKET, ID_TB_WATCH, ID_TB_SET];
    let widths = icon_widths(dpi);
    // The gear is the mouse path to settings, so a hidden toolbar still reserves its slot.
    let on = icon_on(bar);
    let n_on = on.iter().filter(|v| **v).count() as i32;
    let icons_w = widths.iter().zip(&on).filter(|(_, v)| **v).map(|(w, _)| *w).sum::<i32>() + gap * (n_on - 1).max(0);
    let icons_left = (row_w - pad - icons_w).max(0);
    let dw = icon_w;
    let stride = (dw + gap).max(1);
    // Same width and height as the gear, on the same top the caller used for that row.
    let list_w = icon_w;
    let list_h = bh;
    // The first drive must start far enough right that one minimum tab and the list button still fit.
    let min_left = tab_reserve(dpi) + gap + list_w + gap;
    let room = icons_left - min_left;
    let fit = if !bar || room < stride { 0 } else { (room / stride) as usize };
    let drive_n = drives.min(26).min(fit);
    let mut drive_rects = [RECT::default(); 26];
    let mut x = icons_left - drive_n as i32 * stride;
    let anchor = x;
    for i in 0..drive_n {
        drive_rects[i] = RECT { left: x, top, right: x + dw, bottom: top + bh };
        x += stride;
    }
    let mut icons = [(0, RECT::default()); ICON_N];
    for i in 0..specs.len() {
        if !on[i] {
            icons[i] = (specs[i], RECT { left: x, top, right: x, bottom: top });
            continue;
        }
        let w = widths[i];
        icons[i] = (specs[i], RECT { left: x, top, right: x + w, bottom: top + bh });
        x += w + gap;
    }
    let list_right = (anchor - gap).max(0);
    let list_left = (list_right - list_w).max(0);
    let list = if list_right > list_left {
        RECT { left: list_left, top, right: list_right, bottom: top + list_h }
    } else {
        RECT { left: 0, top, right: 0, bottom: top }
    };
    let tab_w = if list.right > list.left { (list.left - gap).max(1) } else { (anchor - gap).max(1) };
    IconLay { tab_w, icons, drives: drive_rects, drive_n, list }
}

/// `scroll` moved so `index` is inside a window of `shown` tabs. Pure.
pub(crate) fn scroll_to_show(scroll: usize, index: usize, shown: usize, n: usize) -> usize {
    let n = n.max(1);
    let shown = shown.clamp(1, n);
    let index = index.min(n - 1);
    let max_first = n - shown;
    let scroll = scroll.min(max_first);
    if index < scroll {
        index
    } else if index >= scroll + shown {
        index + 1 - shown
    } else {
        scroll
    }
}

/// Width the tabs may use. The icon group and the drive buttons that fit keep the rest while the bar is on.
/// Hidden drive buttons (`drives == 0`) reserve nothing, so the tabs take that width.
/// A hidden bar still reserves the gear. The other icons are not on screen, and the tabs use that width.
pub(crate) fn tab_area_width(client_w: i32, dpi: i32, bar: bool, drives: usize) -> i32 {
    icon_slots(client_w.max(1), 0, dpi, drives, bar).tab_w
}

pub(crate) fn tab_lay(width: i32, n: usize, locked: &[bool], dpi: i32, scroll: usize) -> TabLay {
    let s = |v: i32| v * dpi / 96;
    let n = n.max(1);
    let width = width.max(1);
    let (pad, gap, plus_w, max_w, min_w, over_w) = tab_px(dpi);
    let y = s(4);
    let h = s(26);
    let used = |w: i32| n as i32 * w + gap * (n as i32 - 1) + plus_w + pad * 2;
    let slot = |r: RECT| {
        let cw = s(16);
        RECT {
            left: r.right - s(4) - cw,
            top: r.top + (h - cw) / 2,
            right: r.right - s(4),
            bottom: r.top + (h - cw) / 2 + cw,
        }
    };
    let mut body = vec![RECT::default(); n];
    let mut close = vec![None; n];
    let lock: Vec<bool> = (0..n).map(|i| locked.get(i).copied().unwrap_or(false)).collect();
    if used(min_w) <= width {
        let mut w = max_w;
        if used(w) > width {
            w = ((width - pad * 2 - plus_w - gap * (n as i32 - 1)) / n as i32).max(min_w);
        }
        let mut x = pad;
        for i in 0..n {
            let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
            close[i] = Some(slot(r));
            body[i] = r;
            x += w + gap;
        }
        return TabLay {
            body,
            close,
            lock,
            plus: RECT { left: x, top: y, right: x + plus_w, bottom: y + h },
            overflow: None,
            first: 0,
            count: n,
        };
    }
    // Chevron at the right. The + stays pinned just to its left, still plus_w wide.
    let over_right = (width - pad).max(over_w.min(width));
    let over_left = (over_right - over_w).max(0);
    let over = RECT { left: over_left, top: y, right: over_right, bottom: y + h };
    let plus_right = (over.left - gap).max(plus_w.min(width));
    let plus_left = (plus_right - plus_w).max(0);
    let plus = RECT { left: plus_left, top: y, right: plus_right, bottom: y + h };
    let avail = (plus.left - gap - pad).max(0);
    let stride = (min_w + gap).max(1);
    let mut count = ((avail + gap) / stride).max(1) as usize;
    count = count.min(n);
    let max_first = n - count;
    let first = scroll.min(max_first);
    let mut x = pad;
    for i in first..first + count {
        let r = RECT { left: x, top: y, right: x + min_w, bottom: y + h };
        close[i] = Some(slot(r));
        body[i] = r;
        x += min_w + gap;
    }
    TabLay { body, close, lock, plus, overflow: Some(over), first, count }
}

pub(crate) fn in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Clear space inside the vertical strip, DIP, on every side.
/// Top and bottom have no stroke, so this is the inset. Left and right add [`VTAB_HAIRLINE_PX`].
/// The row gap stays the top-row tab gap (2 DIP). That is the rhythm inside the strip, not a plate splitter.
pub(crate) const VTAB_PAD_DIP: i32 = 4;

/// Hairline `paint_tabs` draws on the strip's left and right, in device pixels. It does not scale.
pub(crate) const VTAB_HAIRLINE_PX: i32 = 1;

/// Grip thickness, DIP. It covers the plate gap after the strip and the strip's own right edge.
/// The close box stays in the tab, clear of this band.
pub(crate) const VTAB_EDGE_DIP: i32 = 6;

/// Pad inside the vertical strip, scaled. Top and bottom use this alone.
pub(crate) fn vtab_pad_px(dpi: i32) -> i32 {
    VTAB_PAD_DIP * dpi.max(1) / 96
}

/// Left and right content inset: the hairline, then [`vtab_pad_px`].
pub(crate) fn vtab_inset_x(dpi: i32) -> i32 {
    VTAB_HAIRLINE_PX + vtab_pad_px(dpi)
}

/// Default strip width at `dpi`. A width the user dragged is [`vtab_pref_px`].
pub(crate) fn vtab_width(dpi: i32) -> i32 {
    VTAB_W * dpi.max(1) / 96
}

/// Largest DIP for a client `client_w` px wide: the smaller of 600 and half that client.
/// Both limits are in DIP. A non-positive client keeps the 600 cap.
pub(crate) fn vtab_max_dip(client_w: i32, dpi: i32) -> i32 {
    let dpi = i64::from(dpi.max(1));
    let half = if client_w <= 0 { i64::from(VTAB_MAX_DIP) } else { i64::from(client_w) * 96 / (dpi * 2) };
    half.clamp(0, i64::from(VTAB_MAX_DIP)) as i32
}

/// Pixel width from the stored DIP, capped by 600 and by half the client. Not yet cut for the panes.
pub(crate) fn vtab_pref_px(stored_dip: i32, dpi: i32, client_w: i32) -> i32 {
    let dpi = dpi.max(1);
    let stored = vtab_stored_dip(Some(i64::from(stored_dip)));
    let max = vtab_max_dip(client_w, dpi);
    let dip = if max < VTAB_MIN_DIP { max.max(0) } else { stored.min(max) };
    (i64::from(dip) * i64::from(dpi) / 96) as i32
}

/// `pref` limited to the pixels the panes can spare.
pub(crate) fn vtab_fit_px(pref: i32, room_px: i32) -> i32 {
    pref.max(0).min(room_px.max(0))
}

/// On-screen strip width. `0` when vertical tabs are off.
/// `room_px` is what is left after the side panel, the right pad, and the pane minimums.
/// A tight window may return less than 100 DIP.
pub(crate) fn vtab_display_px(vertical: bool, stored_dip: i32, dpi: i32, client_w: i32, room_px: i32) -> i32 {
    if !vertical {
        return 0;
    }
    vtab_fit_px(vtab_pref_px(stored_dip, dpi, client_w), room_px)
}

/// Width while the vertical-tab edge is dragged.
/// `shown` is the DIP the windows are using. `pending` is the latest clamp.
/// The session keeps its old value until [`vtab_drag_finish`].
///
/// Esc, the button going up, and a lost capture all keep `pending`.
/// The side-panel splitter keeps the value written during the move; this drag does too,
/// and it does not put the old width back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VtabLive {
    pub shown: i32,
    pub pending: i32,
    pub placed_px: i32,
}

impl VtabLive {
    /// `placed_px` is the strip width already on screen.
    pub(crate) fn begin(stored_dip: i32, placed_px: i32) -> Self {
        Self { shown: stored_dip, pending: stored_dip, placed_px }
    }

    /// One pointer move. `dip` and `px` are already clamped.
    /// The bool is whether the strip and the panes should move now.
    ///
    /// A burst of moves is folded to the newest point before this runs.
    /// Skipping because another move is queued would freeze the strip: during a drag the
    /// next move is usually waiting before this one has finished laying out.
    pub(crate) fn moved(self, dip: i32, px: i32) -> (Self, bool) {
        let place = vtab_should_place(self.placed_px, px);
        let next = Self {
            shown: if place { dip } else { self.shown },
            pending: dip,
            placed_px: if place { px } else { self.placed_px },
        };
        (next, place)
    }
}

/// Move the windows when the on-screen pixel width changed.
/// The same pixel (a DIP step the screen cannot show) does not layout again.
pub(crate) fn vtab_should_place(placed_px: i32, next_px: i32) -> bool {
    next_px != placed_px
}

/// Button up, Esc, and a lost capture all keep the latest clamped width.
/// `None` means the gesture is not running, so the stored width stays.
/// Nothing is restored: the side-panel splitter has no cancel either.
pub(crate) fn vtab_drag_finish(stored_dip: i32, pending: Option<i32>) -> i32 {
    pending.unwrap_or(stored_dip)
}

/// DIP to store while the right edge is at main-window `edge_x`.
/// The cursor stops at `room_px`. The value stays in 100..=min(600, half the client).
/// A window whose half is under 100 DIP still stores 100; [`vtab_display_px`] draws it narrower.
pub(crate) fn vtab_drag_dip(edge_x: i32, left: i32, dpi: i32, client_w: i32, room_px: i32) -> i32 {
    let dpi = dpi.max(1);
    let px = (edge_x - left).clamp(0, room_px.max(0));
    let dip = (i64::from(px) * 96 / i64::from(dpi)) as i32;
    let max = vtab_max_dip(client_w, dpi);
    if max < VTAB_MIN_DIP {
        return VTAB_MIN_DIP;
    }
    dip.clamp(VTAB_MIN_DIP, max)
}

/// Grip thickness in px. At least one pixel, so a hit test still has an edge.
pub(crate) fn vtab_edge_px(dpi: i32) -> i32 {
    (VTAB_EDGE_DIP * dpi.max(1) / 96).max(1)
}

/// Grip width from the outer pad. The pad is 8 DIP and the grip is 6 DIP, so this is `pad * 6 / 8`.
pub(crate) fn vtab_edge_from_pad(pad: i32) -> i32 {
    if pad <= 0 {
        return 0;
    }
    (pad * VTAB_EDGE_DIP / 8).max(1)
}

/// How much of `gap` actually sits between the strip and the grid.
/// A clipped strip does not push the grip across the window.
pub(crate) fn vtab_gap_after(strip_right: i32, grid_left: i32, gap: i32) -> i32 {
    (grid_left - strip_right).max(0).min(gap.max(0))
}

/// Full grip, in the same coordinates as `strip`. It ends at the far side of `gap`
/// and is `edge` wide, but it never starts before the strip.
pub(crate) fn vtab_grip_span(strip: RECT, edge: i32, gap: i32) -> (i32, i32) {
    let right = strip.right.saturating_add(gap.max(0));
    let left = right.saturating_sub(edge.max(0)).max(strip.left);
    (left, right)
}

/// True when `(x, y)` is on the part of the grip the strip window owns.
/// The plate gap after `strip.right` belongs to the main window. `gap` still shrinks
/// the inside portion so the two together stay `edge` wide.
/// Off, a non-positive `edge`, or an empty strip is never a hit, so the top row has no grip.
pub(crate) fn vtab_edge_hit(vertical: bool, strip: RECT, edge: i32, gap: i32, x: i32, y: i32) -> bool {
    if !vertical || edge <= 0 || strip.right <= strip.left || strip.bottom <= strip.top {
        return false;
    }
    if y < strip.top || y >= strip.bottom {
        return false;
    }
    let (mut left, mut right) = vtab_grip_span(strip, edge, gap);
    right = right.min(strip.right);
    left = left.max(strip.left);
    left < right && x >= left && x < right
}

/// Height the vertical strip can use: below the toolbar row, above the status line.
pub(crate) fn vtab_body_h(client_h: i32, dpi: i32) -> i32 {
    let s = |v: i32| v * dpi.max(1) / 96;
    let top = s(32);
    let bot = (client_h - s(24)).max(top);
    (bot - top).max(0)
}

/// `vertical == false` is [`tab_lay`] of `width`. `height` is ignored, so the top row does not move.
pub(crate) fn window_tab_lay(vertical: bool, width: i32, height: i32, n: usize, locked: &[bool], dpi: i32, scroll: usize) -> TabLay {
    if vertical {
        vtab_lay(width, height, n, locked, dpi, scroll)
    } else {
        tab_lay(width, n, locked, dpi, scroll)
    }
}

/// True when `text_px` does not fit in `room`, so the title is drawn with an ellipsis.
pub(crate) fn title_clipped(text_px: i32, room: i32) -> bool {
    room < text_px
}

/// Which side marks a selected tab.
/// A horizontal strip uses the top bar. A vertical strip uses the left bar.
/// Colour and accent share that one edge. The other side stays plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabEdge {
    Top,
    Left,
}

/// `vertical` is the window-tab strip. Pane tabs are a horizontal strip, so they pass `false`.
pub(crate) fn selected_tab_edge(vertical: bool) -> TabEdge {
    if vertical { TabEdge::Left } else { TabEdge::Top }
}

/// How one window tab is painted.
/// A selected tab has one edge: the top bar when the strip is horizontal, the left bar when it is vertical.
/// That edge is the tab colour, or the accent when the tab has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabFace {
    pub(crate) bg: u32,
    /// Left bar of a selected vertical tab. The tab colour, or the accent when uncoloured.
    /// `None` on a horizontal tab and on every unselected tab.
    pub(crate) left_bar: Option<u32>,
    /// 2 DIP top edge of a selected horizontal tab. The tab colour, or the accent when uncoloured.
    /// `None` on a vertical tab and on every unselected tab.
    pub(crate) top_bar: Option<u32>,
    pub(crate) title: u32,
}

/// Face of one main-window tab. `ground` is the light or dark palette, not the live theme.
/// Hover on a selected tab keeps the selected face.
pub(crate) fn tab_face(selected: bool, hover: bool, color: Option<u32>, vertical: bool, ground: &Palette, accent: u32) -> TabFace {
    let bg = match (selected, hover, color) {
        (true, _, Some(c)) => blend(ground.card, c, 0.18),
        (true, _, None) => ground.card,
        (_, true, Some(c)) => blend(blend(ground.card, ground.bg, 0.45), c, 0.16),
        (_, true, None) => blend(ground.card, ground.bg, 0.45),
        (_, _, Some(c)) => blend(ground.bg, c, 0.18),
        _ => ground.bg,
    };
    let ink = color.unwrap_or(accent);
    let (top_bar, left_bar) = if selected {
        match selected_tab_edge(vertical) {
            TabEdge::Top => (Some(ink), None),
            TabEdge::Left => (None, Some(ink)),
        }
    } else {
        (None, None)
    };
    let title = if selected { ground.text } else { ground.sub };
    TabFace { bg, left_bar, top_bar, title }
}

/// Left bar, 4 DIP, flush with the tab. A selected vertical tab fills it, in the tab colour or the accent.
pub(crate) fn tab_color_stripe_rect(body: RECT, dpi: i32) -> RECT {
    let w = 4 * dpi.max(1) / 96;
    RECT { left: body.left, top: body.top, right: body.left + w, bottom: body.bottom }
}

/// Extra left inset for a coloured vertical tab's title, in pixels.
/// Horizontal tabs have no left bar, so their titles stay put.
/// An unselected vertical tab keeps the inset too, so the title does not jump when it is selected.
pub(crate) fn tab_color_label_nudge(colored: bool, vertical: bool, dpi: i32) -> i32 {
    if colored && vertical { 5 * dpi.max(1) / 96 } else { 0 }
}

/// Width of the title inside one tab. The insets match the rectangle [`paint_tabs`] draws into.
pub(crate) fn tab_label_room(body: RECT, close: Option<RECT>, dpi: i32, colored: bool, vertical: bool) -> i32 {
    let s = |v: i32| v * dpi.max(1) / 96;
    let left = body.left + s(8) + tab_color_label_nudge(colored, vertical, dpi);
    let right = close.map(|c| c.left).unwrap_or(body.right - s(8));
    (right - left).max(0)
}

/// Hover text. A clipped title is the first line; the paths stay underneath, as on the top row.
pub(crate) fn tab_hover_text(name: &str, paths: &str, clipped: bool) -> String {
    if !clipped || name.is_empty() {
        return paths.to_string();
    }
    if paths.is_empty() {
        name.to_string()
    } else {
        format!("{name}\n{paths}")
    }
}

/// Vertical window tabs. Each row is as tall as a tab on the top row.
/// Top and bottom inset by [`vtab_pad_px`]. Left and right inset by [`vtab_inset_x`]
/// so the clear space inside the hairline matches that pad.
/// The row gap is the top-row tab gap (2 DIP), not the 4 DIP plate gap: a wider row gap
/// only shortens the list, and the horizontal row uses the same 2 DIP.
/// Tabs that fit stack from the top, with + under the last one. Otherwise + and the ▾ pin to the bottom
/// and `scroll` chooses which rows are on screen. A tiny strip keeps every rect non-inverted.
pub(crate) fn vtab_lay(width: i32, height: i32, n: usize, locked: &[bool], dpi: i32, scroll: usize) -> TabLay {
    let s = |v: i32| v * dpi.max(1) / 96;
    let n = n.max(1);
    let width = width.max(1);
    let height = height.max(0);
    let (_, gap, _, _, _, _) = tab_px(dpi);
    let pad_y = vtab_pad_px(dpi);
    let pad_x = vtab_inset_x(dpi);
    let h = s(26);
    let x = pad_x.min(width);
    let tab_w = (width - pad_x * 2).max(0);
    let row = |top: i32, bottom: i32| -> RECT {
        if bottom <= top || tab_w <= 0 {
            RECT { left: x, top, right: x, bottom: top }
        } else {
            RECT { left: x, top, right: x + tab_w, bottom }
        }
    };
    let slot = |r: RECT| {
        let cw = s(16);
        RECT {
            left: r.right - s(4) - cw,
            top: r.top + (h - cw) / 2,
            right: r.right - s(4),
            bottom: r.top + (h - cw) / 2 + cw,
        }
    };
    let mut body = vec![RECT::default(); n];
    let mut close = vec![None; n];
    let lock: Vec<bool> = (0..n).map(|i| locked.get(i).copied().unwrap_or(false)).collect();
    let stack = n as i32 * h + gap * (n as i32 - 1);
    let used = pad_y + stack + gap + h + pad_y;
    if used <= height && tab_w > 0 {
        let mut y = pad_y;
        for i in 0..n {
            let r = row(y, y + h);
            if r.bottom > r.top {
                close[i] = Some(slot(r));
                body[i] = r;
            }
            y += h + gap;
        }
        return TabLay {
            body,
            close,
            lock,
            plus: row(y, y + h),
            overflow: None,
            first: 0,
            count: n,
        };
    }
    let over_bottom = (height - pad_y).max(0);
    let over_top = (over_bottom - h).max(0);
    let over = row(over_top, over_bottom);
    let plus_bottom = (over.top - gap).max(0);
    let plus_top = (plus_bottom - h).max(0);
    let plus = row(plus_top, plus_bottom);
    let limit = if plus.bottom > plus.top { plus.top } else { height };
    let avail = (limit - gap - pad_y).max(0);
    let stride = (h + gap).max(1);
    let count = if h <= 0 { 0 } else { ((avail + gap) / stride) as usize }.min(n);
    let first = if count == 0 { 0 } else { scroll.min(n - count) };
    let mut y = pad_y;
    for i in first..first + count {
        let bottom = y + h;
        let stop = if plus.bottom > plus.top { (plus.top - gap).max(y) } else { bottom };
        let r = row(y, bottom.min(stop));
        if r.bottom > r.top {
            close[i] = Some(slot(r));
            body[i] = r;
        }
        y += h + gap;
    }
    TabLay { body, close, lock, plus, overflow: Some(over), first, count }
}

pub(crate) fn tab_hit(lay: &TabLay, x: i32, y: i32) -> TabHit {
    if in_rect(lay.plus, x, y) {
        return TabHit::Plus;
    }
    if lay.overflow.is_some_and(|r| in_rect(r, x, y)) {
        return TabHit::Overflow;
    }
    for (i, r) in lay.body.iter().enumerate() {
        if r.right <= r.left {
            continue;
        }
        if in_rect(*r, x, y) {
            if lay.close[i].is_some_and(|c| in_rect(c, x, y)) {
                return if lay.lock.get(i).copied().unwrap_or(false) { TabHit::Lock(i) } else { TabHit::Close(i) };
            }
            return TabHit::Tab(i);
        }
    }
    TabHit::Empty
}

pub(crate) fn put_filters(f: &[String]) {
    if let Ok(mut g) = FILTERS.lock() {
        for i in 0..MAXP {
            g[i] = f.get(i).cloned().unwrap_or_default();
        }
    }
    unsafe { sync_filter_hwnds() };
}

pub(crate) fn fit_park(a: &mut App) {
    let n = a.s.tabs.len();
    if a.park.len() < n {
        a.park.resize_with(n, no_panes);
    } else if a.park.len() > n {
        a.park.truncate(n);
    }
    let npark = a.park.len();
    a.park_lru.retain(|&i| i < npark);
}

pub(crate) fn lru_note(a: &mut App, i: usize) {
    a.park_lru.retain(|&x| x != i);
    a.park_lru.push(i);
}

pub(crate) fn lru_forget(a: &mut App, i: usize) {
    a.park_lru.retain(|&x| x != i);
}

pub(crate) fn lru_insert_shift(a: &mut App, at: usize) {
    for x in &mut a.park_lru {
        if *x >= at {
            *x += 1;
        }
    }
}

pub(crate) fn lru_remove_shift(a: &mut App, at: usize) {
    a.park_lru.retain(|&x| x != at);
    for x in &mut a.park_lru {
        if *x > at {
            *x -= 1;
        }
    }
}

pub(crate) fn park_has_browsers(set: &[Option<Pane>]) -> bool {
    set.iter().any(|p| p.is_some())
}

/// Oldest parked tab that should drop its browsers, or none when the tab cache holds.
/// `counts[i]` is how many Explorer windows tab `i` still holds (0 = already dropped).
/// The count is not a second cap: one warm tab stays warm however many panes it has.
/// `lru` is oldest first. A tab with windows that is missing from `lru` is taken after it, in index order.
pub(crate) fn park_victim(counts: &[usize], lru: &[usize], tab_cache: usize) -> Option<usize> {
    let live = counts.iter().filter(|&&n| n > 0).count();
    if live <= tab_cache {
        return None;
    }
    lru.iter().copied().find(|&i| counts.get(i).copied().unwrap_or(0) > 0).or_else(|| counts.iter().position(|&n| n > 0))
}

/// Tabs to drop, oldest first, until only `tab_cache` still hold browsers.
/// `counts` and `lru` are not the live app state.
pub(crate) fn park_evictions(mut counts: Vec<usize>, mut lru: Vec<usize>, tab_cache: usize) -> Vec<usize> {
    let mut dropped = Vec::new();
    let limit = counts.len();
    while dropped.len() < limit {
        let Some(i) = park_victim(&counts, &lru, tab_cache) else { break };
        dropped.push(i);
        counts[i] = 0;
        lru.retain(|&x| x != i);
    }
    dropped
}

/// Drop hidden Explorer windows past [`PARK_CACHE`].
/// Their folders, columns and filters stay on the tab.
pub(crate) unsafe fn evict_park() {
    let plan = app(|a| {
        let counts: Vec<usize> = a.park.iter().map(|set| set.iter().filter(|p| p.is_some()).count()).collect();
        park_evictions(counts, a.park_lru.clone(), PARK_CACHE)
    });
    for i in plan {
        let set = app(|a| {
            lru_forget(a, i);
            if i < a.park.len() && park_has_browsers(&a.park[i]) {
                std::mem::replace(&mut a.park[i], no_panes())
            } else {
                no_panes()
            }
        });
        destroy_set(set);
    }
}

thread_local! {
    static TAB_SWITCH_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct TabSwitchBusy;

impl TabSwitchBusy {
    fn enter() -> Self {
        TAB_SWITCH_DEPTH.with(|c| c.set(c.get().saturating_add(1)));
        Self
    }
}

impl Drop for TabSwitchBusy {
    fn drop(&mut self) {
        TAB_SWITCH_DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

fn tab_switch_busy() -> bool {
    TAB_SWITCH_DEPTH.with(|c| c.get() > 0)
}

/// Arm a short timer so browser destruction runs after the switch has returned.
/// With no window, evict now: there is no timer to defer it.
pub(crate) unsafe fn schedule_park_evict() {
    let main = app(|a| a.main);
    if main.0.is_null() || !IsWindow(Some(main)).as_bool() {
        evict_park();
        return;
    }
    SetTimer(Some(main), TIMER_PARK, PARK_EVICT_MS, None);
}

pub(crate) unsafe fn on_shell_timer(main: HWND) {
    _ = KillTimer(Some(main), TIMER_SHELL);
    if tab_switch_busy() {
        if IsWindow(Some(main)).as_bool() {
            SetTimer(Some(main), TIMER_SHELL, SHELL_HOLD_MS, None);
        }
        return;
    }
    if app(|a| a.main) != main {
        return;
    }
    release_shell_hold();
}

pub(crate) unsafe fn on_park_timer(main: HWND) {
    _ = KillTimer(Some(main), TIMER_PARK);
    if tab_switch_busy() {
        if IsWindow(Some(main)).as_bool() {
            SetTimer(Some(main), TIMER_PARK, PARK_EVICT_MS, None);
        }
        return;
    }
    evict_park();
}

pub(crate) unsafe fn pull_filters() -> Vec<String> {
    let (n, stored) = app(|a| {
        let stored = a.s.tabs.get(a.s.tab).map(|t| t.filters.clone()).unwrap_or_default();
        (a.s.panes(), stored)
    });
    let mut f = pad_filters(stored);
    for i in 0..n {
        if let Some(e) = pane_ctl(i, ID_FILTER) {
            f[i] = filter_spec(&text_of(e));
        }
    }
    put_filters(&f);
    f
}

/// Fold the live grid and filters into the active tab so a switch or a save keeps them.
pub(crate) unsafe fn remember_tabs() {
    let n = app(|a| a.s.panes());
    for i in 0..n {
        save_view(i);
    }
    let filters = pull_filters();
    app(|a| {
        let tab = a.s.tab;
        if tab < a.s.tabs.len() {
            a.s.tabs[tab].filters = pad_filters(filters);
        }
        a.s.anchor();
    });
}

pub(crate) unsafe fn detach_live() {
    detach_live_panes(true);
}

/// Unhook the live browsers. `hide` is false for a tab switch: those panes stay up until
/// [`relayout`] has shown the incoming set (`hide_parked_panes`).
unsafe fn detach_live_panes(hide: bool) {
    crate::pane::marks_overlay::note_all_hidden();
    hide_addr_tip();
    for i in 0..MAXP {
        let got = app(|a| {
            a.panes[i].as_mut().map(|p| {
                let eb = p.eb.clone();
                let cookie = std::mem::take(&mut p.cookie);
                (eb, cookie, p.hwnd)
            })
        });
        if let Some((eb, cookie, hwnd)) = got {
            if cookie != 0 {
                _ = eb.Unadvise(cookie);
            }
            _ = IUnknown_SetSite(&eb, None);
            _ = KillTimer(Some(hwnd), TIMER_SETTLE);
            _ = KillTimer(Some(hwnd), TIMER_FILTER);
            if hide {
                _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

pub(crate) unsafe fn attach_live() {
    for i in 0..MAXP {
        let eb = app(|a| a.panes[i].as_ref().map(|p| p.eb.clone()));
        let Some(eb) = eb else { continue };
        let slot = app(|a| a.panes[i].as_ref().map(|p| p.slot.clone()));
        let Some(slot) = slot else { continue };
        slot.store(i, Ordering::SeqCst);
        let site: IServiceProvider = Host(slot.clone()).into();
        _ = IUnknown_SetSite(&eb, &site);
        let ev: IExplorerBrowserEvents = Events(slot).into();
        if let Ok(c) = eb.Advise(&ev) {
            app(|a| {
                if let Some(p) = a.panes[i].as_mut() {
                    p.cookie = c;
                }
            });
        }
    }
    // A tab that was hidden through the theme switch still has the old shell view.
    // Headers and scrollbars follow now. The list itself waits for a restart.
    recolor_due_panes();
    // A delete that finished while this tab was parked can re-list now that the view has a site.
    crate::shell::listsync::relist_shown();
    crate::shell::listsync::retarget(app(|a| a.main));
    crate::pane::marks_overlay::sync_visible();
}

pub(crate) unsafe fn sync_filter_edits() {
    let n = app(|a| a.s.panes());
    for i in 0..n {
        let spec = filter_of(i);
        if let Some(e) = pane_ctl(i, ID_FILTER) {
            if text_of(e) != spec {
                _ = SetWindowTextW(e, &HSTRING::from(spec.as_str()));
            }
        }
    }
}

pub(crate) unsafe fn destroy_set(set: Vec<Option<Pane>>) {
    if set.iter().all(|p| p.is_none()) {
        return;
    }
    let (back, keep) = app(|a| {
        let keep = FILTERS.lock().map(|f| f.clone()).unwrap_or_else(|_| [const { String::new() }; MAXP]);
        (std::mem::replace(&mut a.panes, set), keep)
    });
    // These browsers belong to a tab that is not on screen. Their grid is already on that tab.
    KEEP_VIEWS.with(|c| c.set(true));
    for i in 0..MAXP {
        destroy_pane(i);
    }
    KEEP_VIEWS.with(|c| c.set(false));
    app(|a| a.panes = back);
    put_filters(&keep);
}

pub(crate) unsafe fn destroy_every_pane() {
    clear_shell_hold();
    for i in 0..MAXP {
        destroy_pane(i);
    }
    let sets = app(|a| a.park.drain(..).collect::<Vec<_>>());
    for set in sets {
        destroy_set(set);
    }
}

pub(crate) fn tabs_invalidate() {
    let h = app(|a| a.tabs_hwnd);
    if !h.0.is_null() {
        unsafe { _ = InvalidateRect(Some(h), None, false) };
    }
    unsafe { crate::tablist::refresh() };
}

pub(crate) unsafe fn switch_tab(j: usize) {
    let (cur, n, main) = app(|a| (a.s.tab, a.s.tabs.len(), a.main));
    if j >= n {
        return;
    }
    // A click, a menu, or a flush replaces a deferred Ctrl+Tab. Do not also run the pending one.
    cancel_deferred_tab();
    // The hold paints once on drop, including the same-tab path and any early return.
    // Busy keeps a park-evict timer that fires from a nested message pump from destroying browsers mid-switch.
    let _busy = TabSwitchBusy::enter();
    let _hold = RedrawHold::new(main);
    // A previous cold switch may still be hiding its shell view. Show that view
    // before this switch parks the panes, so a hidden list is not what comes back.
    release_shell_hold();
    if j == cur {
        show_active_tab();
        tabs_invalidate();
        return;
    }
    end_rename(true);
    remember_tabs();
    // Leave the outgoing panes visible until relayout has shown the incoming ones.
    detach_live_panes(false);
    let filters = app(|a| {
        fit_park(a);
        let live = std::mem::replace(&mut a.panes, no_panes());
        let tab = a.s.tab;
        a.park[tab] = live;
        lru_note(a, tab);
        a.panes = std::mem::take(&mut a.park[j]);
        if a.panes.len() != MAXP {
            a.panes = no_panes();
        }
        lru_forget(a, j);
        let old = a.s.tab;
        stash_tab_order(a, old);
        a.s.tab = j;
        a.s.publish();
        restore_tab_order(a, j);
        let _ = pull_dirty(a);
        a.zoom = None;
        a.s.tabs[j].filters.clone()
    });
    put_filters(&filters);
    attach_live();
    show_active_tab();
    // Empty incoming slots are created in relayout. Their shell views stay hidden
    // until navigation finishes. The parked windows are not kept on top.
    arm_shell_hold();
    relayout();
    sync_filter_edits();
    focus_view(active_pane());
    tabs_invalidate();
    note_save();
    refresh_save_button();
    drop(_hold);
    drop(_busy);
    schedule_shell_hold();
    schedule_park_evict();
}

/// Strip selection versus the tab whose panes are on screen.
/// `pending` is the tab Ctrl+Tab has highlighted and not switched to yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabCoalesce {
    pub live: usize,
    pub n: usize,
    pub pending: Option<usize>,
}

/// What one coalesced Ctrl+Tab event should do. The strip update is immediate either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabCoalesceOp {
    /// Restart the one-shot timer. The strip shows `shown`. Panes stay on `live`.
    Defer { shown: usize },
    /// Stop the timer. The strip shows `live`. Panes stay.
    Idle,
    /// Stop the timer and switch the panes to `to`.
    Switch { to: usize },
}

impl TabCoalesce {
    /// Tab the strip draws as selected.
    pub(crate) fn shown(self) -> usize {
        match self.pending {
            Some(i) if self.n > 0 && i < self.n => i,
            _ => {
                if self.n == 0 {
                    self.live
                } else {
                    self.live.min(self.n - 1)
                }
            }
        }
    }

    /// `tab.next` / `tab.prev`. Repeats advance from the strip, not from the panes.
    pub(crate) fn step(self, delta: i32) -> (Self, TabCoalesceOp) {
        if self.n == 0 {
            return (self, TabCoalesceOp::Idle);
        }
        let shown = (self.shown() as i32 + delta).rem_euclid(self.n as i32) as usize;
        if shown == self.live {
            (Self { pending: None, ..self }, TabCoalesceOp::Idle)
        } else {
            (Self { pending: Some(shown), ..self }, TabCoalesceOp::Defer { shown })
        }
    }

    /// Timer, Ctrl key-up, or a command that reads the active tab.
    pub(crate) fn commit(self) -> (Self, TabCoalesceOp) {
        match self.pending {
            Some(to) if to != self.live && to < self.n => (Self { live: to, pending: None, n: self.n }, TabCoalesceOp::Switch { to }),
            _ => (Self { pending: None, ..self }, TabCoalesceOp::Idle),
        }
    }
}

/// Tab the strip draws as selected. `pending` wins while a Ctrl+Tab has not landed yet.
pub(crate) fn shown_tab(live: usize, pending: Option<usize>, n: usize) -> usize {
    TabCoalesce { live, n, pending }.shown()
}

pub(crate) unsafe fn switch_rel(d: i32) {
    let (live, n, pending) = app(|a| (a.s.tab, a.s.tabs.len(), a.tab_pending));
    if n == 0 {
        return;
    }
    let (next, op) = TabCoalesce { live, n, pending }.step(d);
    apply_tab_coalesce(next, op);
}

unsafe fn apply_tab_coalesce(state: TabCoalesce, op: TabCoalesceOp) {
    match op {
        TabCoalesceOp::Defer { shown } => {
            let main = app(|a| {
                a.tab_pending = state.pending;
                a.main
            });
            // Reset before painting. A timer message already queued would switch at the old index.
            if !main.0.is_null() {
                _ = KillTimer(Some(main), TIMER_TAB_SWITCH);
            }
            let armed = !main.0.is_null() && SetTimer(Some(main), TIMER_TAB_SWITCH, TAB_SWITCH_MS, None) != 0;
            if !armed {
                // No timer: the strip must not stay ahead of the panes.
                app(|a| a.tab_pending = None);
                switch_tab(shown);
                return;
            }
            show_tab(shown);
            tabs_invalidate();
        }
        TabCoalesceOp::Idle => {
            let had = app(|a| a.tab_pending.take().is_some());
            if had {
                disarm_tab_switch_timer();
                show_active_tab();
                tabs_invalidate();
            }
        }
        TabCoalesceOp::Switch { to } => {
            // Clear before the switch so a paint inside it does not still show the old highlight.
            app(|a| a.tab_pending = None);
            disarm_tab_switch_timer();
            switch_tab(to);
        }
    }
}

unsafe fn disarm_tab_switch_timer() {
    let main = app(|a| a.main);
    if !main.0.is_null() {
        _ = KillTimer(Some(main), TIMER_TAB_SWITCH);
    }
}

/// Drop a deferred Ctrl+Tab without switching. A direct `switch_tab` is the switch.
pub(crate) unsafe fn cancel_deferred_tab() {
    let had = app(|a| a.tab_pending.take().is_some());
    if had {
        disarm_tab_switch_timer();
    }
}

/// Run a deferred Ctrl+Tab now. No-op when the strip already matches the panes.
pub(crate) unsafe fn flush_pending_tab() {
    let (live, n, pending) = app(|a| (a.s.tab, a.s.tabs.len(), a.tab_pending));
    if pending.is_none() {
        return;
    }
    let (_state, op) = TabCoalesce { live, n, pending }.commit();
    apply_tab_coalesce(TabCoalesce { live, n, pending }, op);
}

pub(crate) unsafe fn on_tab_switch_timer(main: HWND) {
    _ = KillTimer(Some(main), TIMER_TAB_SWITCH);
    // A switch already on the stack can pump this timer. Run it after that switch returns.
    if tab_switch_busy() {
        if IsWindow(Some(main)).as_bool() {
            SetTimer(Some(main), TIMER_TAB_SWITCH, TAB_SWITCH_MS, None);
        }
        return;
    }
    if app(|a| a.main) != main {
        return;
    }
    flush_pending_tab();
}

pub(crate) unsafe fn new_tab() {
    new_tab_at(None);
}

/// A new window tab. `path` is pane 0's folder. `None` leaves every path empty, as `new_tab` does.
pub(crate) unsafe fn new_tab_at(path: Option<&str>) {
    flush_pending_tab();
    let _busy = TabSwitchBusy::enter();
    end_rename(true);
    remember_tabs();
    detach_live();
    let first = path.unwrap_or("").to_string();
    app(|a| {
        fit_park(a);
        let live = std::mem::replace(&mut a.panes, no_panes());
        let tab = a.s.tab;
        a.park[tab] = live;
        lru_note(a, tab);
        // Settings pane count, with even splits. Not the tab on screen.
        // Paths stay empty unless the caller named a folder for the first pane.
        // Pane colours start with no label.
        let fresh = session_for_new_tab(&a.s, &first);
        let old = a.s.tab;
        stash_tab_order(a, old);
        let name = next_tab_name(&a.s.tabs);
        let tab = Tab::from_live(&fresh, name, false, vec![String::new(); MAXP]);
        a.s.tabs.push(tab.clone());
        a.park.push(no_panes());
        let at = a.s.tabs.len() - 1;
        insert_tab_base(a, at, &tab);
        a.s.tab = a.s.tabs.len() - 1;
        a.s.publish();
        a.s.order.clear();
        let _ = pull_dirty(a);
        a.zoom = None;
    });
    put_filters(&vec![String::new(); MAXP]);
    attach_live();
    show_active_tab();
    relayout();
    redraw_all();
    focus_view(active_pane());
    tabs_invalidate();
    note_save();
    refresh_save_button();
    drop(_busy);
    schedule_park_evict();
}

pub(crate) unsafe fn duplicate_tab(i: usize) {
    flush_pending_tab();
    end_rename(true);
    remember_tabs();
    let j = app(|a| {
        if i >= a.s.tabs.len() {
            return None;
        }
        fit_park(a);
        let Some(t) = duplicated_tab(&a.s, i) else {
            return None;
        };
        let j = (i + 1).min(a.s.tabs.len());
        a.s.tabs.insert(j, t.clone());
        a.park.insert(j, no_panes());
        insert_tab_base(a, j, &t);
        lru_insert_shift(a, j);
        if j <= a.s.tab {
            a.s.tab += 1;
        }
        Some(j)
    });
    if let Some(j) = j {
        switch_tab(j);
    }
}

/// Insert `tabs` immediately after the tab on screen and show the first of them.
/// Tabs that were already open are not changed.
pub(crate) unsafe fn open_tabs_beside(tabs: Vec<Tab>) {
    if tabs.is_empty() {
        return;
    }
    flush_pending_tab();
    end_rename(true);
    remember_tabs();
    let at = app(|a| {
        fit_park(a);
        let at = if a.s.tabs.is_empty() { 0 } else { (a.s.tab + 1).min(a.s.tabs.len()) };
        for (k, t) in tabs.iter().enumerate() {
            let j = at + k;
            a.s.tabs.insert(j, t.clone());
            a.park.insert(j, no_panes());
            insert_tab_base(a, j, t);
            lru_insert_shift(a, j);
        }
        at
    });
    if at == app(|a| a.s.tab) {
        let filters = app(|a| {
            let i = at.min(a.s.tabs.len().saturating_sub(1));
            a.s.tab = i;
            a.s.publish();
            a.s.order.clear();
            mark_tab_clean(a, i);
            let _ = pull_dirty(a);
            a.s.tabs[i].filters.clone()
        });
        put_filters(&filters);
        show_active_tab();
        relayout();
        sync_filter_edits();
        redraw_all();
        focus_view(active_pane());
        tabs_invalidate();
        note_save();
        refresh_save_button();
    } else {
        switch_tab(at);
    }
}

pub(crate) unsafe fn close_tab(i: usize) {
    end_rename(true);
    if !app(|a| can_close_tab(&a.s.tabs, i)) {
        return;
    }
    // Callers that close "the active tab" flush before they read the index.
    // A close of a specific tab still lands the deferred switch first, so indexes stay aligned.
    flush_pending_tab();
    let active = app(|a| a.s.tab);
    if i == active {
        remember_tabs();
        detach_live();
    }
    let (doomed, filters, switched) = app(|a| {
        fit_park(a);
        let tab = a.s.tabs.remove(i);
        let mut parked = a.park.remove(i);
        remove_tab_base(a, i);
        lru_remove_shift(a, i);
        a.closed.insert(0, tab);
        a.closed.truncate(CLOSED_KEEP);
        let was = a.s.tab;
        let switched = i == was;
        if switched {
            let j = i.min(a.s.tabs.len() - 1);
            a.s.tab = j;
            let next = std::mem::replace(&mut a.park[j], no_panes());
            lru_forget(a, j);
            parked = std::mem::replace(&mut a.panes, next);
            a.s.publish();
            restore_tab_order(a, j);
            let _ = pull_dirty(a);
        } else if i < was {
            a.s.tab = was - 1;
        }
        let filters = a.s.tabs[a.s.tab].filters.clone();
        (parked, filters, switched)
    });
    destroy_set(doomed);
    show_active_tab();
    if switched {
        put_filters(&filters);
        attach_live();
        relayout();
        sync_filter_edits();
        redraw_all();
        focus_view(active_pane());
    }
    tabs_invalidate();
    note_save();
    refresh_save_button();
}

pub(crate) unsafe fn close_other_tabs(keep: usize) {
    let victims = app(|a| close_other_indexes(&a.s.tabs, keep));
    for i in victims.into_iter().rev() {
        close_tab(i);
    }
}

/// Close every unlocked tab. Locked tabs stay. If that would leave none, open one fresh tab and close the rest.
pub(crate) unsafe fn close_all_tabs() {
    if app(|a| close_all_needs_fresh(&a.s.tabs)) {
        new_tab();
        let keep = app(|a| a.s.tabs.len().saturating_sub(1));
        close_other_tabs(keep);
        return;
    }
    let victims = app(|a| close_all_indexes(&a.s.tabs));
    for i in victims.into_iter().rev() {
        close_tab(i);
    }
}

pub(crate) unsafe fn reopen_closed_at(k: usize) {
    flush_pending_tab();
    end_rename(true);
    let j = app(|a| {
        if k >= a.closed.len() {
            return None;
        }
        let t = a.closed.remove(k);
        fit_park(a);
        let j = (a.s.tab + 1).min(a.s.tabs.len());
        a.s.tabs.insert(j, t.clone());
        a.park.insert(j, no_panes());
        insert_tab_base(a, j, &t);
        lru_insert_shift(a, j);
        Some(j)
    });
    if let Some(j) = j {
        switch_tab(j);
    }
}

pub(crate) unsafe fn reorder_tab(from: usize, to: usize) {
    if from == to {
        return;
    }
    flush_pending_tab();
    app(|a| {
        if from >= a.s.tabs.len() || to >= a.s.tabs.len() {
            return;
        }
        fit_park(a);
        reorder_vec(&mut a.s.tabs, from, to);
        reorder_vec(&mut a.park, from, to);
        reorder_vec(&mut a.tab_base, from, to);
        a.park_lru = a.park_lru.iter().copied().map(|i| moved_active(i, from, to)).collect();
        a.s.tab = moved_active(a.s.tab, from, to);
    });
    tabs_invalidate();
    note_save();
}

pub(crate) unsafe fn end_rename(save: bool) {
    let (edit, which) = app(|a| {
        a.tab_rename_skip = !save;
        (a.tab_edit, a.tab_renaming.take())
    });
    if edit.0.is_null() || which.is_none() {
        return;
    }
    if save {
        if let Some(i) = which {
            let name = text_of(edit);
            app(|a| {
                if let Some(t) = a.s.tabs.get_mut(i) {
                    t.name = stored_tab_name(&name);
                }
            });
        }
    }
    _ = ShowWindow(edit, SW_HIDE);
    tabs_invalidate();
    if save {
        note_save();
    }
}

/// Store a typed tab title the way the rename dialog does.
/// Empty, and a legacy all-digit title, clear the custom name so the tab follows its folder again.
/// The tab is the one still called `prev` when the index has moved. False when nothing changed.
pub(crate) unsafe fn apply_tab_rename(i: usize, prev: &str, typed: &str) -> bool {
    let next = stored_tab_name(typed);
    if stored_tab_name(prev) == next {
        return false;
    }
    // The prompt runs a message loop; tabs may have moved. Rename the tab still named `prev`.
    let done = app(|a| {
        let at = if a.s.tabs.get(i).is_some_and(|t| t.name == prev) {
            Some(i)
        } else {
            let mut m = a.s.tabs.iter().enumerate().filter(|(_, t)| t.name == prev).map(|(k, _)| k);
            m.next().filter(|_| m.next().is_none())
        };
        let Some(t) = at.and_then(|k| a.s.tabs.get_mut(k)) else { return false };
        t.name = next;
        true
    });
    if !done {
        return false;
    }
    tabs_invalidate();
    note_save();
    true
}

pub(crate) unsafe fn begin_rename(i: usize) {
    path_tooltip(None);
    let (owner, prev) = app(|a| {
        let prev = a.s.tabs.get(i).map(|t| t.name.clone()).unwrap_or_default();
        (a.main, prev)
    });
    if owner.0.is_null() || app(|a| i >= a.s.tabs.len()) {
        return;
    }
    // Empty when the tab is automatic, so confirming without typing leaves the folder name in charge.
    let initial = custom_tab_name(&prev).unwrap_or("").to_string();
    let title = tr(crate::Key::RenameTab);
    let Some(text) = crate::settings::ask(owner, title, &initial) else { return };
    let _ = apply_tab_rename(i, &prev, &text);
}

pub(crate) unsafe fn set_tab_color(i: usize, color: Option<u32>) {
    app(|a| {
        if let Some(t) = a.s.tabs.get_mut(i) {
            t.color = color;
        }
    });
    tabs_invalidate();
    note_save();
}

pub(crate) unsafe fn tab_menu(i: usize, x: i32, y: i32) {
    path_tooltip(None);
    let (hwnd, locked, others, any_unlocked, color) = app(|a| {
        let tab = a.s.tabs.get(i);
        let locked = tab.is_some_and(|t| t.locked);
        let others = !close_other_indexes(&a.s.tabs, i).is_empty();
        let any_unlocked = !close_all_indexes(&a.s.tabs).is_empty();
        (a.tabs_hwnd, locked, others, any_unlocked, tab.and_then(|t| t.color))
    });
    let Ok(menu) = CreatePopupMenu() else { return };
    let add = |id: usize, text: &str, on: bool| {
        let flags = if on { MF_STRING } else { MF_STRING | MF_GRAYED };
        _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
    };
    add(1, tr(crate::Key::RenameTab), true);
    add(2, if locked { tr(crate::Key::UnlockTab) } else { tr(crate::Key::LockTab) }, true);
    let dup_key = crate::keys::index("tab.dup").map(crate::keys::chord_text).unwrap_or_default();
    let dup = if dup_key.is_empty() {
        tr(crate::Key::DuplicateTab).to_string()
    } else {
        format!("{}\t{dup_key}", tr(crate::Key::DuplicateTab))
    };
    add(3, &dup, true);
    add(6, tr(crate::Key::MoveNewWindow), true);
    let list_key = crate::keys::index("tab.list").map(crate::keys::chord_text).unwrap_or_default();
    let list_item = if list_key.is_empty() {
        tr(crate::Key::TabListMenu).to_string()
    } else {
        format!("{}\t{list_key}", tr(crate::Key::TabListMenu))
    };
    add(7, &list_item, true);
    _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    add(8, tr(crate::Key::CloseAllTabs), any_unlocked);
    add(4, tr(crate::Key::CloseOtherTabs), others);
    _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    add(5, tr(crate::Key::SaveThisTab3), true);
    let Ok(colors) = CreatePopupMenu() else {
        _ = DestroyMenu(menu);
        return;
    };
    let mark = |menu: HMENU, id: usize, on: bool| {
        if on {
            _ = CheckMenuItem(menu, id as u32, (MF_BYCOMMAND | MF_CHECKED).0);
        }
    };
    _ = AppendMenuW(colors, MF_STRING, 19, &HSTRING::from(tr(crate::Key::None)));
    mark(colors, 19, color.is_none());
    for (k, &(c, label)) in TAB_COLORS.iter().enumerate() {
        let id = 20 + k;
        _ = AppendMenuW(colors, MF_STRING, id, &HSTRING::from(tr(label)));
        mark(colors, id, color == Some(c));
    }
    _ = AppendMenuW(menu, MF_POPUP, colors.0 as usize, &HSTRING::from(tr(crate::Key::TabColor)));
    let mut pt = POINT { x, y };
    _ = ClientToScreen(hwnd, &mut pt);
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, hwnd, None).0 as usize;
    _ = DestroyMenu(menu);
    match pick {
        1 => begin_rename(i),
        2 => {
            app(|a| {
                if let Some(t) = a.s.tabs.get_mut(i) {
                    t.locked ^= true;
                }
            });
            tabs_invalidate();
            note_save();
        }
        3 => duplicate_tab(i),
        6 => move_tab_to_new(i),
        7 => crate::tablist::open_menu(),
        8 => close_all_tabs(),
        4 => close_other_tabs(i),
        5 => save_tab_as_session(i),
        19 => set_tab_color(i, None),
        n if (20..20 + TAB_COLORS.len()).contains(&n) => set_tab_color(i, Some(TAB_COLORS[n - 20].0)),
        _ => {}
    }
}

pub(crate) unsafe fn install_session(s: Session) {
    cancel_deferred_tab();
    clear_shell_hold();
    destroy_every_pane();
    let filters = s.tabs.get(s.tab.min(s.tabs.len().saturating_sub(1))).map(|t| t.filters.clone()).unwrap_or_default();
    let n = s.tabs.len().max(1);
    put_filters(&filters);
    app(|a| {
        a.s = s;
        a.park = (0..n).map(|_| no_panes()).collect();
        a.park_lru.clear();
        a.zoom = None;
        let order = a.s.order.clone();
        let tab = a.s.tab;
        a.tab_base = a.s.tabs.iter().enumerate().map(|(i, t)| TabBase {
            grid: t.clone(),
            order: if i == tab { order.clone() } else { Vec::new() },
        }).collect();
        let _ = pull_dirty(a);
    });
    show_active_tab();
}

/// Scroll so `index` is on screen. Uses the main window's width, so it is valid before the strip is moved.
pub(crate) fn show_tab(index: usize) {
    app(|a| {
        if a.main.0.is_null() {
            return;
        }
        let mut rc = RECT::default();
        unsafe { _ = GetClientRect(a.main, &mut rc) };
        if rc.right <= 0 {
            return;
        }
        let locked: Vec<bool> = a.s.tabs.iter().map(|t| t.locked).collect();
        let n = locked.len().max(1);
        let lay = if a.s.vertical_tabs {
            let w = geo(a).vtab.max(1);
            window_tab_lay(true, w, vtab_body_h(rc.bottom, a.dpi), n, &locked, a.dpi, a.tab_scroll)
        } else {
            let w = tab_area_width(rc.right, a.dpi, a.s.bar, drive_slots(a.s.drive_buttons, a.drives.len()));
            window_tab_lay(false, w, rc.bottom, n, &locked, a.dpi, a.tab_scroll)
        };
        let index = index.min(n - 1);
        a.tab_scroll = if lay.overflow.is_none() { 0 } else { scroll_to_show(lay.first, index, lay.count.max(1), n) };
    });
}

pub(crate) fn show_active_tab() {
    let i = app(|a| a.s.tab);
    show_tab(i);
}

/// One wheel notch moves one tab. Positive delta (wheel up) reveals earlier tabs.
pub(crate) fn tab_wheel(delta: i32) {
    if delta == 0 {
        return;
    }
    let mut steps = delta / 120;
    if steps == 0 {
        steps = delta.signum();
    }
    app(|a| {
        if steps > 0 {
            a.tab_scroll = a.tab_scroll.saturating_sub(steps as usize);
        } else {
            a.tab_scroll = a.tab_scroll.saturating_add((-steps) as usize);
        }
    });
    let _ = current_tab_lay();
    tabs_invalidate();
}

/// Every window tab, by name. The current one is checked and a locked one is marked.
pub(crate) unsafe fn open_tab_list() {
    flush_pending_tab();
    let (hwnd, names, locked, cur) = app(|a| {
        (
            a.tabs_hwnd,
            (0..a.s.tabs.len()).map(|i| tab_text(a, i).0).collect::<Vec<_>>(),
            a.s.tabs.iter().map(|t| t.locked).collect::<Vec<_>>(),
            a.s.tab,
        )
    });
    if hwnd.0.is_null() || names.is_empty() {
        return;
    }
    let Ok(menu) = CreatePopupMenu() else { return };
    for (i, name) in names.iter().enumerate() {
        let shown = name.replace('&', "&&");
        let label = if locked.get(i).copied().unwrap_or(false) {
            format!("{shown} — {}", tr(crate::Key::Locked))
        } else {
            shown
        };
        _ = AppendMenuW(menu, MF_STRING, i + 1, &HSTRING::from(label));
        if i == cur {
            _ = CheckMenuItem(menu, (i + 1) as u32, (MF_BYCOMMAND | MF_CHECKED).0);
        }
    }
    let lay = current_tab_lay();
    let (x, y) = lay.overflow.map(|r| (r.left, r.bottom)).unwrap_or((0, 0));
    let mut pt = POINT { x, y };
    _ = ClientToScreen(hwnd, &mut pt);
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, hwnd, None).0 as usize;
    _ = DestroyMenu(menu);
    if (1..=names.len()).contains(&pick) {
        switch_tab(pick - 1);
    }
}

pub(crate) fn current_tab_lay() -> TabLay {
    app(|a| {
        let mut rc = RECT::default();
        unsafe { _ = GetClientRect(a.tabs_hwnd, &mut rc) };
        let locked: Vec<bool> = a.s.tabs.iter().map(|t| t.locked).collect();
        let n = locked.len().max(1);
        let lay = if a.s.vertical_tabs {
            window_tab_lay(true, rc.right.max(1), rc.bottom.max(0), n, &locked, a.dpi, a.tab_scroll)
        } else {
            let strip_h = 32 * a.dpi / 96;
            let btn_top = (strip_h - tool_icon_px(a.dpi).1) / 2;
            let icons = icon_slots(rc.right.max(1), btn_top, a.dpi, drive_slots(a.s.drive_buttons, a.drives.len()), a.s.bar);
            window_tab_lay(false, icons.tab_w, rc.bottom, n, &locked, a.dpi, a.tab_scroll)
        };
        a.tab_scroll = lay.first;
        lay
    })
}

/// Title and path tooltip for tab `i`. The tab on screen uses the live grid; the others use what was saved on the tab.
fn tab_text(a: &App, i: usize) -> (String, String) {
    let (name, parked_paths, parked_n, parked_active, live) = {
        let Some(t) = a.s.tabs.get(i) else { return (String::new(), String::new()) };
        let n = t.heights.iter().sum::<usize>().clamp(1, MAXP);
        (t.name.clone(), t.paths.clone(), n, t.active.min(n - 1), i == a.s.tab)
    };
    let (paths, n, active) = if live {
        let n = a.s.panes().clamp(1, MAXP);
        (a.s.paths.clone(), n, a.s.active.min(n - 1))
    } else {
        (parked_paths, parked_n, parked_active)
    };
    let path = paths.get(active).map(String::as_str).unwrap_or("");
    let label = tab_label(&name, path);
    let shown: Vec<String> = paths.iter().take(n).map(|p| folder_tip_text(p)).collect();
    let refs: Vec<&str> = shown.iter().map(String::as_str).collect();
    (label, tab_tip_text(&refs))
}

pub(crate) unsafe fn paint_tabs(h: HWND) {
    let (active, names, colors, mut hot, mut hot_close, hot_plus, hot_over, dpi, font, icon, accent, vertical, edge_hot) = app(|a| {
        (
            shown_tab(a.s.tab, a.tab_pending, a.s.tabs.len()),
            (0..a.s.tabs.len()).map(|i| tab_text(a, i).0).collect::<Vec<_>>(),
            a.s.tabs.iter().map(|t| t.color).collect::<Vec<_>>(),
            a.tab_hot,
            a.tab_hot_close,
            a.tab_hot_plus,
            a.tab_hot_over,
            a.dpi,
            a.font,
            a.icon_font,
            a.s.accent_now(),
            a.s.vertical_tabs,
            a.s.vertical_tabs && a.gap_hot == Some(Hit::VTab) && a.drag.is_none(),
        )
    });
    // A file drag warms the tab under the pointer without touching mouse-hover state.
    if let Some(i) = crate::file_window_hot(h) {
        hot = Some(i);
        hot_close = false;
    }
    let lay = current_tab_lay();
    buffered(h, |dc, rc| {
        let s = |v: i32| v * dpi / 96;
        fill(dc, rc, pal().bg);
        if vertical {
            // Both edges. The column sits between the side panel and the panes, and `pal().line`
            // is the same hairline the plates use, in either theme. A one-pixel strip is just that line.
            let hair = VTAB_HAIRLINE_PX;
            if rc.right - rc.left > hair {
                fill(dc, RECT { right: rc.left + hair, ..rc }, pal().line);
                fill(dc, RECT { left: rc.right - hair, ..rc }, pal().line);
            }
            // The main window's hover line sits under this child. Paint the grip here.
            if edge_hot {
                let t = (2 * dpi / 96).max(1);
                fill(dc, RECT { left: rc.right.saturating_sub(t), ..rc }, accent);
            }
        } else {
            fill(dc, RECT { top: rc.bottom - 1, ..rc }, pal().line);
        }
        for (i, r) in lay.body.iter().enumerate() {
            if r.right <= r.left {
                continue;
            }
            let on = i == active;
            let over = hot == Some(i);
            let col = colors.get(i).copied().flatten();
            // One selected edge. Horizontal: the top bar. Vertical: the left bar.
            // The edge is the tab colour, or the accent when the tab has none.
            let face = tab_face(on, over, col, vertical, pal(), accent);
            fill(dc, *r, face.bg);
            if !on {
                frame(dc, *r, 1, pal().line);
            }
            if let Some(c) = face.top_bar {
                fill(dc, RECT { bottom: r.top + s(2), ..*r }, c);
                fill(dc, RECT { top: r.bottom - 1, bottom: r.bottom, ..*r }, pal().card);
            }
            if let Some(c) = face.left_bar {
                fill(dc, tab_color_stripe_rect(*r, dpi), c);
            }
            let lock = lay.lock.get(i).copied().unwrap_or(false);
            let nudge = tab_color_label_nudge(col.is_some(), vertical, dpi);
            let mut label = *r;
            label.left += s(8) + nudge;
            if let Some(c) = lay.close[i] {
                label.right = c.left;
                let ink = close_ink(lock, hot_close && hot == Some(i), accent, pal().sub);
                text(dc, c, close_glyph(lock), icon, ink, DT_CENTER);
            } else {
                label.right -= s(8);
            }
            // At the minimum width a locked tab keeps the lock and drops the name.
            let room = label.right - label.left;
            if room >= s(8) && !(lock && room < s(18)) {
                text(dc, label, names.get(i).map(|n| n.as_str()).unwrap_or(""), font, face.title, DT_LEFT | DT_END_ELLIPSIS);
            }
        }
        // Same hover and the same icon ink for the + and the chevron.
        let hot_bg = blend(pal().card, pal().bg, 0.45);
        let ink = pal().text;
        fill(dc, lay.plus, if hot_plus { hot_bg } else { pal().bg });
        text(dc, lay.plus, ICON_PLUS, icon, ink, DT_CENTER);
        if let Some(r) = lay.overflow {
            fill(dc, r, if hot_over { hot_bg } else { pal().bg });
            text(dc, r, ICON_CHEVRON_DOWN, icon, ink, DT_CENTER);
        }
    });
}

/// "Tab list" plus its chord. The button is a tool-row control, so this is its tooltip text.
pub(crate) fn tab_list_tip() -> String {
    let chord = crate::keys::index("tab.list").map(crate::keys::chord_text).unwrap_or_default();
    let name = tr(crate::Key::TabList);
    if chord.is_empty() { name.to_string() } else { format!("{name} ({chord})") }
}

/// Put the current tab-list name and chord on that button. No-op before the button exists.
pub(crate) unsafe fn refresh_tab_list_tip() {
    let main = app(|a| a.main);
    if main.0.is_null() {
        return;
    }
    crate::toolbar::refresh_tips();
    let Ok(b) = GetDlgItem(Some(main), ID_TB_LIST) else { return };
    _ = SetWindowTextW(b, &HSTRING::from(tr(crate::Key::TabList)));
    tip_text(b, &tab_list_tip(), TTM_UPDATETIPTEXTW);
    // Basket and watch tooltips carry their chords, so a rebind updates them here too.
    for (id, _, hint) in crate::toolbar_tips() {
        if id == ID_TB_BASKET || id == ID_TB_WATCH {
            if let Ok(t) = GetDlgItem(Some(main), id) {
                tip_text(t, &hint, TTM_UPDATETIPTEXTW);
            }
        }
    }
}

pub(crate) unsafe fn tab_mouse(x: i32, y: i32) -> TabHit {
    tab_hit(&current_tab_lay(), x, y)
}

/// Screen rect of window-tab body `i`, or None when off-screen.
unsafe fn tab_body_screen(i: usize) -> Option<RECT> {
    let tabs = app(|a| a.tabs_hwnd);
    if tabs.0.is_null() {
        return None;
    }
    let lay = current_tab_lay();
    let r = lay.body.get(i).copied().filter(|r| r.right > r.left)?;
    Some(crate::ui::tip_screen_rect(tabs, r))
}

/// Screen rect of the lock glyph on tab `i` (drawn in the close-box slot).
unsafe fn tab_lock_screen(i: usize) -> Option<RECT> {
    let tabs = app(|a| a.tabs_hwnd);
    if tabs.0.is_null() {
        return None;
    }
    let lay = current_tab_lay();
    if !lay.lock.get(i).copied().unwrap_or(false) {
        return None;
    }
    let r = lay.close.get(i).copied().flatten().filter(|r| r.right > r.left)?;
    Some(crate::ui::tip_screen_rect(tabs, r))
}

/// Beside a vertical tab, under a tab on the top row. The tip never covers the title.
fn tab_tip_side() -> TipSide {
    if app(|a| a.s.vertical_tabs) { TipSide::Right } else { TipSide::Below }
}

/// The drawn title does not show every character. Measure uses the same font as the strip.
unsafe fn tab_name_clipped(i: usize) -> bool {
    let (name, font, dpi, colored, vertical, tabs) = app(|a| {
        let name = tab_text(a, i).0;
        let colored = a.s.tabs.get(i).and_then(|t| t.color).is_some();
        (name, a.font, a.dpi, colored, a.s.vertical_tabs, a.tabs_hwnd)
    });
    if name.is_empty() || tabs.0.is_null() {
        return false;
    }
    let lay = current_tab_lay();
    let Some(body) = lay.body.get(i).copied().filter(|r| r.right > r.left && r.bottom > r.top) else {
        return false;
    };
    let close = lay.close.get(i).copied().flatten();
    let room = tab_label_room(body, close, dpi, colored, vertical);
    let dc = GetDC(Some(tabs));
    if dc.is_invalid() {
        return title_clipped(1, room);
    }
    let px = text_width(dc, font, &name.encode_utf16().collect::<Vec<_>>());
    let _ = ReleaseDC(Some(tabs), dc);
    title_clipped(px, room)
}

/// "Locked" beside the lock glyph. Anchored below the glyph with a 4px gap.
unsafe fn lock_tooltip(show: bool, tab: Option<usize>) {
    let (tip, tabs, added) = app(|a| (a.tip, a.tabs_hwnd, a.tab_lock_tip));
    if tip.0.is_null() || tabs.0.is_null() {
        return;
    }
    const LOCK_ID: usize = 7;
    if !show {
        let (now, _) = crate::ui::tip_leave(tabs, LOCK_ID, added);
        app(|a| a.tab_lock_tip = now);
        return;
    }
    let Some(i) = tab else { return };
    let Some(item) = tab_lock_screen(i) else { return };
    let label = tr(crate::Key::Locked2);
    let key = 0x7000_0000_0000_0000u64 | (i as u64);
    let (now, _) = crate::ui::tip_enter(tabs, LOCK_ID, key, label, item, tab_tip_side(), added);
    app(|a| a.tab_lock_tip = now);
}

/// Full paths of the panes in `tab`, one per line. `None` hides it.
/// Anchored below the tab body with a 4px gap; never covers the tab title.
unsafe fn path_tooltip(tab: Option<usize>) {
    let (tip, tabs, added) = app(|a| (a.tip, a.tabs_hwnd, a.tab_path_tip));
    if tip.0.is_null() || tabs.0.is_null() {
        return;
    }
    let text = tab.filter(|_| app(|a| a.tab_drag.is_none())).and_then(|i| {
        let (name, paths) = app(|a| tab_text(a, i));
        let clipped = tab_name_clipped(i);
        let body = tab_hover_text(&name, &paths, clipped);
        (!body.is_empty()).then_some((i, body))
    });
    let Some((i, text)) = text else {
        let (now, _) = crate::ui::tip_leave(tabs, TAB_TIP_ID, added);
        app(|a| a.tab_path_tip = now);
        return;
    };
    let Some(item) = tab_body_screen(i) else {
        let (now, _) = crate::ui::tip_leave(tabs, TAB_TIP_ID, added);
        app(|a| a.tab_path_tip = now);
        return;
    };
    let key = 0x8000_0000_0000_0000u64 | (i as u64);
    let (now, _) = crate::ui::tip_enter(
        tabs,
        TAB_TIP_ID,
        key,
        &text,
        item,
        tab_tip_side(),
        added,
    );
    app(|a| a.tab_path_tip = now);
}

/// Inline tab rename, centred in the same rectangle the title is drawn in. No-op while hidden.
pub(crate) unsafe fn place_tab_edit() {
    let (edit, which, dpi, color, vertical) = app(|a| {
        let which = a.tab_renaming;
        let color = which.and_then(|i| a.s.tabs.get(i).and_then(|t| t.color));
        (a.tab_edit, which, a.dpi, color, a.s.vertical_tabs)
    });
    let Some(i) = which else { return };
    if edit.0.is_null() || !IsWindowVisible(edit).as_bool() {
        return;
    }
    let lay = current_tab_lay();
    let Some(r) = lay.body.get(i).copied().filter(|r| r.right > r.left) else { return };
    let s = |v: i32| v * dpi / 96;
    let nudge = tab_color_label_nudge(color.is_some(), vertical, dpi);
    let left = r.left + s(8) + nudge;
    let right = lay.close.get(i).and_then(|c| *c).map(|c| c.left).unwrap_or(r.right - s(8)).max(left + 1);
    place_edit(edit, RECT { left, top: r.top, right, bottom: r.bottom }, true);
}

thread_local! {
    static GRIP_CLICK: Cell<(u32, i32, i32)> = const { Cell::new((0, 0, 0)) };
}

/// Second press on the grip within the system double-click time. Capture moves the
/// first mouse-up to the main window, so the strip may not see `WM_LBUTTONDBLCLK`.
unsafe fn grip_second_click(x: i32, y: i32) -> bool {
    let now = windows::Win32::System::SystemInformation::GetTickCount();
    let (then, px, py) = GRIP_CLICK.with(|c| c.get());
    GRIP_CLICK.with(|c| c.set((now, x, y)));
    if then == 0 {
        return false;
    }
    let ms = GetDoubleClickTime();
    let (cx, cy) = (GetSystemMetrics(SM_CXDOUBLECLK), GetSystemMetrics(SM_CYDOUBLECLK));
    now.wrapping_sub(then) <= ms && (x - px).abs() * 2 <= cx.max(1) && (y - py).abs() * 2 <= cy.max(1)
}

/// Right-edge grip, in this strip's client coordinates. Off when the tabs are on the top row.
unsafe fn on_vtab_grip(h: HWND, x: i32, y: i32) -> bool {
    if !app(|a| a.s.vertical_tabs) {
        return false;
    }
    let mut rc = RECT::default();
    _ = GetClientRect(h, &mut rc);
    let (edge, after) = app(|a| {
        let g = geo(a);
        let strip = tab_strip_rect(g.w, g.top, g.bot, true, g.vtab, g.side, g.gap);
        (vtab_edge_px(a.dpi), vtab_gap_after(strip.right, g.gx, g.gap))
    });
    vtab_edge_hit(true, rc, edge, after, x, y)
}

/// Double-click on the grip. Other windows share the width, same as the vertical-tabs switch.
unsafe fn reset_vtab_width() {
    app(|a| {
        a.vtab_live = None;
        a.s.vertical_tabs_width = VTAB_W;
    });
    publish_shared();
    relayout();
    redraw_all();
    across_windows(|| {
        relayout();
        redraw_all();
    });
    note_save();
}

/// The strip owns the pixels, so the click starts here. The main window then tracks the pointer.
/// The strip and the panes move while the button is down. The width is stored when it is released.
unsafe fn begin_vtab_drag() {
    let main = app(|a| a.main);
    gap_hover(main, None);
    app(|a| {
        a.tab_press = None;
        a.tab_drag = None;
        a.drag = Some(Hit::VTab);
        let px = geo(a).vtab;
        a.vtab_live = Some(VtabLive::begin(a.s.vertical_tabs_width, px));
    });
    SetCapture(main);
    // A guide bar belongs to the other splitters. This drag moves the strip itself.
    for b in app(|a| a.bars) {
        _ = ShowWindow(b, SW_HIDE);
    }
}

pub(crate) extern "system" fn tabs_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| tabs_proc_inner(h, msg, wp, lp)))
}
fn tabs_proc_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        bind_hwnd(h);
        match msg {
            WM_PAINT => paint_tabs(h),
            WM_DESTROY => {
                crate::revoke_tab_drop(h);
                return DefWindowProcW(h, msg, wp, lp);
            }
            WM_SIZE => place_tab_edit(),
            WM_ERASEBKGND => return LRESULT(1),
            // The side splitter's slop reaches a few pixels into this column. The grip stays here.
            WM_NCHITTEST if crate::nchit_passes_to_splitter(h, lp) => return LRESULT(HTTRANSPARENT as isize),
            WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
                let resize = app(|a| a.drag == Some(Hit::VTab)) || {
                    let mut pt = POINT::default();
                    _ = GetCursorPos(&mut pt);
                    _ = ScreenToClient(h, &mut pt);
                    on_vtab_grip(h, pt.x, pt.y)
                };
                if resize {
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                    return LRESULT(1);
                }
                return DefWindowProcW(h, msg, wp, lp);
            }
            WM_MOUSEMOVE => {
                let (x, y) = xy(lp);
                if on_vtab_grip(h, x, y) && app(|a| a.tab_drag.is_none() && a.drag.is_none()) {
                    let paint = app(|a| {
                        let paint = a.tab_hot.is_some() || a.tab_hot_close || a.tab_hot_plus || a.tab_hot_over;
                        a.tab_hot = None;
                        a.tab_hot_close = false;
                        a.tab_hot_plus = false;
                        a.tab_hot_over = false;
                        paint
                    });
                    let was = app(|a| a.gap_hot);
                    gap_hover(app(|a| a.main), Some(Hit::VTab));
                    if paint || was != Some(Hit::VTab) {
                        tabs_invalidate();
                    }
                    lock_tooltip(false, None);
                    path_tooltip(None);
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                    _ = TrackMouseEvent(&mut t);
                    return LRESULT(0);
                }
                if app(|a| a.gap_hot == Some(Hit::VTab)) {
                    gap_hover(app(|a| a.main), None);
                    tabs_invalidate();
                }
                let hit = tab_mouse(x, y);
                let (hot, close, plus, over) = match hit {
                    TabHit::Tab(i) | TabHit::Lock(i) => (Some(i), false, false, false),
                    TabHit::Close(i) => (Some(i), true, false, false),
                    TabHit::Plus => (None, false, true, false),
                    TabHit::Overflow => (None, false, false, true),
                    TabHit::Empty => (None, false, false, false),
                };
                lock_tooltip(matches!(hit, TabHit::Lock(_)), match hit { TabHit::Lock(i) => Some(i), _ => None });
                let path_tab = match hit {
                    TabHit::Tab(i) | TabHit::Close(i) => Some(i),
                    _ => None,
                };
                let (from, paint) = app(|a| {
                    if let Some((pi, px, py, _)) = a.tab_press {
                        if a.tab_drag.is_none() && (x - px).abs() + (y - py).abs() > (4 * a.dpi / 96).max(4) {
                            a.tab_drag = Some(pi);
                        }
                    }
                    let from = a.tab_drag;
                    let same = a.tab_hot == hot && a.tab_hot_close == close && a.tab_hot_plus == plus && a.tab_hot_over == over;
                    if !same {
                        a.tab_hot = hot;
                        a.tab_hot_close = close;
                        a.tab_hot_plus = plus;
                        a.tab_hot_over = over;
                    }
                    (from, !same)
                });
                if let Some(from) = from {
                    let mut rc = RECT::default();
                    _ = GetClientRect(h, &mut rc);
                    let vertical = app(|a| a.s.vertical_tabs);
                    // Reorder only while the pointer stays in this strip, and only across tabs that are on screen.
                    let inside = if vertical { x >= 0 && x < rc.right } else { y >= 0 && y < rc.bottom };
                    if inside {
                        let lay = current_tab_lay();
                        let (to, before_buttons) = if vertical {
                            let edge = lay.overflow.map(|r| r.top).unwrap_or(lay.plus.bottom);
                            (vtab_drag_index(&lay.body, y), y < edge)
                        } else {
                            let edge = lay.overflow.map(|r| r.left).unwrap_or(lay.plus.right);
                            (lay.drag_target(x), x < edge)
                        };
                        if before_buttons && to != from && to < app(|a| a.s.tabs.len()) {
                            reorder_tab(from, to);
                            app(|a| a.tab_drag = Some(to));
                        }
                    }
                }
                if paint || from.is_some() {
                    tabs_invalidate();
                }
                path_tooltip(path_tab);
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                _ = TrackMouseEvent(&mut t);
            }
            WM_MOUSELEAVE => {
                if app(|a| a.gap_hot == Some(Hit::VTab)) {
                    gap_hover(app(|a| a.main), None);
                }
                lock_tooltip(false, None);
                path_tooltip(None);
                app(|a| {
                    a.tab_hot = None;
                    a.tab_hot_close = false;
                    a.tab_hot_plus = false;
                    a.tab_hot_over = false;
                });
                tabs_invalidate();
            }
            WM_LBUTTONDOWN => {
                let (x, y) = xy(lp);
                end_rename(true);
                if on_vtab_grip(h, x, y) {
                    lock_tooltip(false, None);
                    path_tooltip(None);
                    if grip_second_click(x, y) {
                        reset_vtab_width();
                    } else {
                        begin_vtab_drag();
                    }
                    return LRESULT(0);
                }
                match tab_mouse(x, y) {
                    TabHit::Tab(i) | TabHit::Close(i) | TabHit::Lock(i) => {
                        // The lock occupies the close slot and does not arm a close.
                        let on_close = matches!(tab_mouse(x, y), TabHit::Close(_));
                        app(|a| a.tab_press = Some((i, x, y, on_close)));
                        SetCapture(h);
                        if !on_close {
                            switch_tab(i);
                        }
                    }
                    TabHit::Plus => new_tab(),
                    TabHit::Overflow => open_tab_list(),
                    TabHit::Empty => {}
                }
            }
            WM_MOUSEWHEEL => tab_wheel((wp.0 >> 16) as i16 as i32),
            WM_LBUTTONUP => {
                let (x, y) = xy(lp);
                let press = app(|a| a.tab_press.take());
                let dragged = app(|a| a.tab_drag.take());
                _ = ReleaseCapture();
                if let Some(from) = dragged {
                    finish_tab_drag(h, from);
                } else if let Some((i, _, _, on_close)) = press {
                    if on_close {
                        if let TabHit::Close(j) = tab_mouse(x, y) {
                            if j == i {
                                close_tab(i);
                            }
                        }
                    }
                }
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = xy(lp);
                if on_vtab_grip(h, x, y) {
                    reset_vtab_width();
                } else if let TabHit::Tab(i) = tab_mouse(x, y) {
                    begin_rename(i);
                }
            }
            WM_MBUTTONUP => {
                let (x, y) = xy(lp);
                if on_vtab_grip(h, x, y) {
                    return LRESULT(0);
                }
                let hit = tab_mouse(x, y);
                if middle_closes(&current_tab_lay(), hit) {
                    if let TabHit::Tab(i) | TabHit::Close(i) = hit {
                        close_tab(i);
                    }
                }
            }
            WM_RBUTTONUP => {
                let (x, y) = xy(lp);
                if on_vtab_grip(h, x, y) {
                    return LRESULT(0);
                }
                let i = match tab_mouse(x, y) {
                    TabHit::Tab(i) | TabHit::Close(i) | TabHit::Lock(i) => i,
                    _ => app(|a| a.s.tab),
                };
                tab_menu(i, x, y);
            }
            WM_CAPTURECHANGED => {
                app(|a| {
                    a.tab_press = None;
                    a.tab_drag = None;
                });
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as _);
                SetBkColor(dc, cref(pal().card));
                SetTextColor(dc, cref(pal().text));
                SetDCBrushColor(dc, cref(pal().card));
                return LRESULT(GetStockObject(DC_BRUSH).0 as isize);
            }
            WM_COMMAND => match ((wp.0 & 0xffff) as i32, ((wp.0 >> 16) & 0xffff) as u32) {
                (ID_TAB_EDIT, EN_KILLFOCUS) => {
                    let skip = app(|a| a.tab_rename_skip);
                    if !skip {
                        end_rename(true);
                    } else {
                        app(|a| a.tab_rename_skip = false);
                    }
                }
                _ => {}
            },
            _ => return DefWindowProcW(h, msg, wp, lp),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(lay: &TabLay) -> i32 {
        lay.overflow.map(|r| r.right).unwrap_or(lay.plus.right)
    }

    #[test]
    fn icon_slots_stay_right_aligned_without_drives() {
        let lay = icon_slots(1000, 6, 96, 0, true);
        assert_eq!(lay.drive_n, 0);
        assert_eq!(lay.tab_w, 696);
        assert_eq!(lay.list, RECT { left: 698, top: 6, right: 728, bottom: 30 });
        assert_eq!(lay.list.right + 2, lay.icons[0].1.left);
        assert_eq!(lay.icons.map(|s| s.0), [ID_TB_LAYOUT, ID_TB_SAVE, ID_TB_LOAD, ID_TB_FAV, ID_TB_BASKET, ID_TB_WATCH, ID_TB_SET]);
        assert_eq!(lay.icons[0].1, RECT { left: 730, top: 6, right: 800, bottom: 30 });
        assert_eq!(lay.icons[1].1.left, 802);
        assert_eq!(lay.icons[GEAR].1.right, 992);
        // Star, basket, watch, gear: 30px buttons, 2px gaps, one line.
        assert_eq!(lay.icons[3].0, ID_TB_FAV);
        assert_eq!(lay.icons[4].0, ID_TB_BASKET);
        assert_eq!(lay.icons[5].0, ID_TB_WATCH);
        assert_eq!(lay.icons[5].1.right + 2, lay.icons[GEAR].1.left);
        for i in 3..ICON_N {
            assert_eq!(lay.icons[i].1.right - lay.icons[i].1.left, 30);
            assert_eq!((lay.icons[i].1.top, lay.icons[i].1.bottom), (lay.icons[GEAR].1.top, lay.icons[GEAR].1.bottom));
        }
        assert!(lay.icons.windows(2).all(|w| w[0].1.right + 2 == w[1].1.left));
        assert_eq!(tab_area_width(1000, 96, true, 0), lay.tab_w);
        // Hidden toolbar keeps the gear (right 992, 30px) and the gap in front of it.
        assert_eq!(tab_area_width(1000, 96, false, 8), 928);
    }

    #[test]
    fn icon_group_min_keeps_one_tab_clear_of_the_icons() {
        for (dpi, bar) in [(96, true), (96, false), (144, true), (192, false)] {
            let w = icon_group_min(dpi, bar);
            let lay = icon_slots(w, 0, dpi, 26, bar);
            assert!(lay.tab_w >= tab_reserve(dpi), "{dpi} bar={bar} tab {} reserve {}", lay.tab_w, tab_reserve(dpi));
            assert_eq!(lay.drive_n, 0, "{dpi} bar={bar}");
            let tight = icon_slots((w - 1).max(1), 0, dpi, 0, bar);
            assert!(tight.tab_w < tab_reserve(dpi), "{dpi} bar={bar} tab {} reserve {}", tight.tab_w, tab_reserve(dpi));
        }
    }

    #[test]
    fn icon_slots_with_n_drives() {
        // Letters sit in the group, immediately left of Layout, and do not move the icons.
        let bare = icon_slots(1000, 4, 96, 0, true);
        let n = 5;
        let lay = icon_slots(1000, 4, 96, n, true);
        assert_eq!(lay.drive_n, n);
        assert_eq!(lay.icons, bare.icons);
        assert_eq!(lay.drives[n - 1].right + 2, lay.icons[0].1.left);
        assert_eq!(lay.list.right + 2, lay.drives[0].left);
        assert_eq!(lay.tab_w, lay.list.left - 2);
        for i in 0..n {
            assert_eq!(lay.drives[i].right - lay.drives[i].left, 30);
            assert_eq!((lay.drives[i].top, lay.drives[i].bottom), (4, 28));
            if i > 0 {
                assert_eq!(lay.drives[i - 1].right + 2, lay.drives[i].left);
            }
        }
        // One minimum tab, +, and ▾ (116px) stay clear of the buttons. Tabs paint only in tab_w.
        let reserve = 116;
        assert!(lay.tab_w >= reserve && lay.drives[0].left >= reserve);
        let tabs = tab_lay(lay.tab_w, 30, &[false; 30], 96, 0);
        assert!(edge(&tabs) <= lay.drives[0].left);

        // A narrow row hides the last letters first. The ones that remain are the prefix.
        let tight = icon_slots(500, 4, 96, 8, true);
        assert!(tight.drive_n > 0 && tight.drive_n < 8);
        assert_eq!(tight.icons[0].1.left, icon_slots(500, 4, 96, 0, true).icons[0].1.left);
        assert!(tight.tab_w >= reserve && tight.drives[0].left >= reserve);
        assert_eq!(icon_slots(500, 4, 96, tight.drive_n + 1, true).drive_n, tight.drive_n);
        let shown = tab_lay(tight.tab_w, 12, &[false; 12], 96, 0);
        assert!(edge(&shown) <= tight.drives[0].left);

        // Too narrow for even one letter: the icons stay, every drive is hidden.
        let tiny = icon_slots(220, 0, 96, 4, true);
        assert_eq!(tiny.drive_n, 0);
        assert!(tiny.tab_w >= 1 && tiny.icons[0].1.left < reserve);

        // The same row at double DPI is the 96dpi layout scaled by two.
        let hi = icon_slots(2000, 8, 192, n, true);
        assert_eq!(hi.drive_n, n);
        assert_eq!(hi.tab_w, lay.tab_w * 2);
        assert_eq!(hi.drives[0].right - hi.drives[0].left, 60);
        assert_eq!(hi.drives[n - 1].right + 4, hi.icons[0].1.left);
        assert_eq!(tab_area_width(2000, 192, true, n), hi.tab_w);
        assert_eq!(tab_area_width(2000, 192, false, n), 1856);
    }

    #[test]
    fn icon_slots_with_drives_hidden() {
        let n = 5;
        let shown = icon_slots(1000, 4, 96, drive_slots(true, n), true);
        let hidden = icon_slots(1000, 4, 96, drive_slots(false, n), true);
        let bare = icon_slots(1000, 4, 96, 0, true);
        assert_eq!(drive_slots(false, n), 0);
        assert_eq!(drive_slots(true, n), n);
        assert_eq!(hidden, bare);
        assert_eq!(hidden.drive_n, 0);
        assert!(hidden.tab_w > shown.tab_w);
        assert_eq!(hidden.icons, shown.icons);
        // The width the letters occupied is now part of the tab strip. The icons stay put.
        assert_eq!(hidden.tab_w - shown.tab_w, shown.icons[0].1.left - shown.drives[0].left);
        assert_eq!(tab_area_width(1000, 96, true, drive_slots(false, n)), hidden.tab_w);
        assert_eq!(tab_area_width(1000, 96, true, drive_slots(true, n)), shown.tab_w);
    }

    #[test]
    fn hidden_bar_keeps_the_gear_and_gives_the_rest_to_the_tabs() {
        let on = icon_slots(1000, 6, 96, 5, true);
        let off = icon_slots(1000, 6, 96, 5, false);
        assert_eq!(off.drive_n, 0);
        assert_eq!(off.icons[GEAR].0, ID_TB_SET);
        // Same gear slot as with the toolbar on. The other icons, including the star, collapse to nothing.
        assert_eq!(off.icons[GEAR].1, on.icons[GEAR].1);
        assert!(off.icons[GEAR].1.right > off.icons[GEAR].1.left);
        for i in 0..GEAR {
            assert_eq!(off.icons[i].1.left, off.icons[GEAR].1.left);
            assert_eq!(off.icons[i].1.right, off.icons[i].1.left);
        }
        assert_eq!(off.list.right + 2, off.icons[GEAR].1.left);
        assert_eq!(off.tab_w, off.list.left - 2);
        assert!(off.tab_w > on.tab_w);
        assert_eq!(tab_area_width(1000, 96, false, 5), off.tab_w);
        assert_eq!(tab_area_width(1000, 96, true, 5), on.tab_w);

        let hi_on = icon_slots(2000, 8, 192, 5, true);
        let hi_off = icon_slots(2000, 8, 192, 5, false);
        assert_eq!(hi_off.icons[GEAR].1, hi_on.icons[GEAR].1);
        assert_eq!(hi_off.list.right + 4, hi_off.icons[GEAR].1.left);
        assert_eq!(hi_off.tab_w, hi_off.list.left - 4);
        assert_eq!(tab_area_width(2000, 192, false, 5), hi_off.tab_w);
    }

    #[test]
    fn tab_lay_plus_width_matches_when_overflowing() {
        let fit = tab_lay(800, 3, &[false; 3], 96, 0);
        assert!(fit.overflow.is_none());
        assert_eq!(fit.plus.right - fit.plus.left, 28);
        assert_eq!(fit.plus.bottom - fit.plus.top, 26);

        let over = tab_lay(240, 12, &[false; 12], 96, 0);
        let o = over.overflow.expect("overflow");
        assert_eq!(over.plus.right - over.plus.left, fit.plus.right - fit.plus.left);
        assert_eq!(over.plus.bottom - over.plus.top, fit.plus.bottom - fit.plus.top);
        assert_eq!(over.plus.top, fit.plus.top);
        // Same square as the +. Wide enough for the icon, and it does not cover the +.
        assert_eq!(o.right - o.left, over.plus.right - over.plus.left);
        assert_eq!(o.bottom - o.top, over.plus.bottom - over.plus.top);
        assert!(over.plus.right <= o.left && o.right <= 240);
        assert!(o.right - o.left >= 20);

        let hi_fit = tab_lay(1600, 3, &[false; 3], 192, 0);
        let hi = tab_lay(480, 12, &[false; 12], 192, 0);
        let ho = hi.overflow.expect("overflow");
        assert_eq!(hi.plus.right - hi.plus.left, hi_fit.plus.right - hi_fit.plus.left);
        assert_eq!(hi.plus.right - hi.plus.left, 56);
        assert_eq!(ho.right - ho.left, 56);
        assert_eq!(ho.bottom - ho.top, hi.plus.bottom - hi.plus.top);
        assert!(hi.plus.right <= ho.left);
    }

    #[test]
    fn tab_list_button_matches_the_toolbar_icon_buttons() {
        // Drawn like the gear: draw_button paints both from the line masters on the shared icon face.
        assert_eq!((tool_line_icon(ID_TB_LIST), tool_line_icon(ID_TB_SET)), (Some(LineIcon::List), Some(LineIcon::Gear)));
        assert_eq!(tool_line_icon(ID_TB_LAYOUT), None);
        for id in [ID_TB_SAVE, ID_TB_LOAD, ID_TB_FAV, ID_TB_BASKET, ID_TB_WATCH] {
            assert!(tool_line_icon(id).is_some(), "{id}");
        }
        assert_eq!(glyph(ID_TB_LIST), ICON_LIST);
        assert_ne!(glyph(ID_TB_LIST), glyph(ID_TB_SET));
        for (dpi, top, bar) in [(96, 4, true), (96, 4, false), (144, 6, true), (192, 8, false)] {
            let lay = icon_slots(2400, top, dpi, 4, bar);
            let gear = lay.icons[GEAR].1;
            let list = lay.list;
            let (w, h) = tool_icon_px(dpi);
            assert!(list.right > list.left, "dpi {dpi} bar {bar}");
            assert_eq!(list.right - list.left, gear.right - gear.left, "dpi {dpi}");
            assert_eq!(list.bottom - list.top, gear.bottom - gear.top, "dpi {dpi}");
            assert_eq!((list.top, list.bottom), (gear.top, gear.bottom), "dpi {dpi}");
            assert_eq!((list.right - list.left, list.bottom - list.top), (w, h), "dpi {dpi}");
            assert_eq!(h, BTN_H_TOOL * dpi / 96);
            assert_eq!(w, TOOL_ICON_W * dpi / 96);
            if bar {
                // Same row as the layout button. That one is wider because it shows the grid.
                assert_eq!((lay.icons[0].1.top, lay.icons[0].1.bottom), (list.top, list.bottom));
                assert!(lay.icons[0].1.right - lay.icons[0].1.left > w);
                let star = lay.icons[3].1;
                assert_eq!((star.right - star.left, star.top, star.bottom), (w, list.top, list.bottom));
                assert_eq!((lay.drives[0].right - lay.drives[0].left, lay.drives[0].top, lay.drives[0].bottom), (w, list.top, list.bottom));
            }
            // Not the tab-row + (28×26 DIP).
            let plus = tab_lay(900, 2, &[false; 2], dpi, 0).plus;
            assert_ne!(list.bottom - list.top, plus.bottom - plus.top);
            assert_ne!(list.right - list.left, plus.right - plus.left);
        }
    }

    fn same_lay(a: &TabLay, b: &TabLay) -> bool {
        a.body == b.body && a.close == b.close && a.lock == b.lock && a.plus == b.plus && a.overflow == b.overflow && a.first == b.first && a.count == b.count
    }

    #[test]
    fn vertical_off_matches_the_horizontal_layout() {
        let locked = [false, true, false, false];
        for (w, n, scroll, dpi) in [(800, 3, 0, 96), (240, 12, 0, 96), (240, 12, 5, 96), (100, 2, 0, 96), (480, 12, 3, 192), (1600, 4, 0, 144)] {
            let flags = &locked[..n.min(locked.len())];
            let row = tab_lay(w, n, flags, dpi, scroll);
            let via = window_tab_lay(false, w, 400, n, flags, dpi, scroll);
            assert!(same_lay(&row, &via), "w={w} n={n} scroll={scroll} dpi={dpi}");
        }
    }

    #[test]
    fn vertical_strip_stacks_rows_and_scrolls_when_they_do_not_fit() {
        assert_eq!(vtab_width(96), 180);
        assert_eq!(vtab_width(144), 270);
        assert_eq!(vtab_width(192), 360);
        assert_eq!(vtab_body_h(600, 96), 544);
        assert_eq!(vtab_body_h(40, 96), 0);
        assert_eq!(vtab_body_h(800, 192), 688);

        let fit = vtab_lay(180, 600, 3, &[false, true, false], 96, 9);
        assert!(fit.overflow.is_none());
        assert_eq!((fit.first, fit.count), (0, 3));
        // 1px hairline + 4px pad on the sides. Top has no hairline, so it is 4. The row gap stays 2.
        assert_eq!(fit.body[0], RECT { left: 5, top: 4, right: 175, bottom: 30 });
        assert_eq!(fit.body[1].top - fit.body[0].bottom, 2);
        assert_eq!(fit.body[1].top, 32);
        assert_eq!(fit.body[2], RECT { left: 5, top: 60, right: 175, bottom: 86 });
        assert_eq!(fit.body[0].bottom - fit.body[0].top, 26);
        assert_eq!(fit.plus, RECT { left: 5, top: 88, right: 175, bottom: 114 });
        assert!(fit.lock[1] && !fit.lock[0]);
        let c0 = fit.close[0].unwrap();
        assert!(c0.left >= fit.body[0].left && c0.right <= fit.body[0].right);
        assert!(c0.top >= fit.body[0].top && c0.bottom <= fit.body[0].bottom);
        assert_eq!(tab_hit(&fit, 20, 10), TabHit::Tab(0));
        assert_eq!(tab_hit(&fit, c0.left + 1, c0.top + 1), TabHit::Close(0));
        let c1 = fit.close[1].unwrap();
        assert_eq!(tab_hit(&fit, c1.left + 1, c1.top + 1), TabHit::Lock(1));
        assert_eq!(tab_hit(&fit, 40, 100), TabHit::Plus);
        assert_eq!(tab_hit(&fit, 2, 10), TabHit::Empty);
        assert_eq!(vtab_drag_index(&fit.body, 10), 0);
        assert_eq!(vtab_drag_index(&fit.body, 19), 1);
        assert_eq!(vtab_drag_index(&fit.body, 40), 1);
        assert_eq!(vtab_drag_index(&fit.body, 200), 2);
        assert_eq!(fit.insert_at_y(10), 0);
        assert_eq!(fit.insert_at_y(200), 3);

        // 220px shows five rows; + and ▾ sit under them. Scroll 4 starts at tab 4.
        let over = vtab_lay(180, 220, 12, &[false; 12], 96, 4);
        let o = over.overflow.expect("overflow");
        assert_eq!((over.first, over.count), (4, 5));
        assert!(over.body[3].bottom <= over.body[3].top);
        assert!(over.body[4].bottom > over.body[4].top);
        assert!(over.body[8].bottom > over.body[8].top);
        assert!(over.body[9].bottom <= over.body[9].top);
        assert!(over.body[8].bottom <= over.plus.top);
        assert!(over.plus.bottom <= o.top);
        assert_eq!(o.bottom, 216);
        assert_eq!(tab_hit(&over, 40, o.top + 2), TabHit::Overflow);
        assert_eq!(vtab_drag_index(&over.body, over.body[4].top + 1), 4);
        assert_eq!(over.insert_at_y(o.bottom + 4), 9);
        let hi = vtab_lay(360, 440, 12, &[false; 12], 192, 4);
        assert_eq!((hi.first, hi.count), (4, 5));
        assert_eq!(hi.body[4].bottom - hi.body[4].top, 52);
        assert_eq!(hi.body[4].right - hi.body[4].left, 342);
        assert_eq!(hi.body[5].top - hi.body[4].bottom, 4, "row gap stays 2 DIP, scaled");
        assert_eq!(hi.body[4].top, vtab_pad_px(192));
        assert_eq!(hi.body[4].left, vtab_inset_x(192));

        let crushed = vtab_lay(1, 0, 4, &[false; 4], 96, 3);
        assert_eq!(crushed.count, 0);
        for r in crushed.body.iter().chain([&crushed.plus]).chain(crushed.overflow.iter()) {
            assert!(r.right >= r.left && r.bottom >= r.top);
        }
    }

    #[test]
    fn a_clipped_title_is_the_first_line_of_the_hover_text() {
        assert!(!title_clipped(80, 80));
        assert!(title_clipped(81, 80));
        assert!(!title_clipped(0, 0));
        assert!(title_clipped(1, 0));
        let body = RECT { left: 6, top: 6, right: 174, bottom: 32 };
        let close = RECT { left: 154, top: 11, right: 170, bottom: 27 };
        assert_eq!(tab_label_room(body, Some(close), 96, false, false), 140);
        assert_eq!(tab_label_room(body, Some(close), 96, true, false), 140);
        assert_eq!(tab_label_room(body, Some(close), 96, true, true), 135);
        assert_eq!(tab_label_room(body, None, 96, false, false), 152);
        assert_eq!(tab_hover_text("Photos", "C:\\Photos", false), "C:\\Photos");
        assert_eq!(tab_hover_text("Photos 2026 archive", "C:\\Photos", true), "Photos 2026 archive\nC:\\Photos");
        assert_eq!(tab_hover_text("Photos 2026 archive", "", true), "Photos 2026 archive");
        assert_eq!(tab_hover_text("", "C:\\Photos", true), "C:\\Photos");
    }

    #[test]
    fn vertical_strip_width_clamps_to_the_window_and_the_grip_is_only_the_right_edge() {
        use crate::{vtab_stored_dip, VTAB_MAX_DIP, VTAB_MIN_DIP, VTAB_W};
        assert_eq!(vtab_stored_dip(None), VTAB_W);
        assert_eq!(vtab_stored_dip(Some(i64::from(VTAB_W))), VTAB_W);
        assert_eq!(vtab_stored_dip(Some(320)), 320);
        assert_eq!(vtab_stored_dip(Some(i64::from(VTAB_MIN_DIP))), VTAB_MIN_DIP);
        assert_eq!(vtab_stored_dip(Some(i64::from(VTAB_MAX_DIP))), VTAB_MAX_DIP);
        assert_eq!(vtab_stored_dip(Some(99)), VTAB_MIN_DIP);
        assert_eq!(vtab_stored_dip(Some(601)), VTAB_MAX_DIP);
        assert_eq!(vtab_stored_dip(Some(-4)), VTAB_MIN_DIP);
        assert_eq!(vtab_stored_dip(Some(i64::MAX)), VTAB_MAX_DIP);

        assert_eq!(vtab_max_dip(2000, 96), VTAB_MAX_DIP);
        assert_eq!(vtab_max_dip(1200, 96), VTAB_MAX_DIP);
        assert_eq!(vtab_max_dip(1000, 96), 500);
        assert_eq!(vtab_max_dip(800, 96), 400);
        assert_eq!(vtab_max_dip(1600, 192), 400);
        assert_eq!(vtab_max_dip(800, 144), 266);
        assert_eq!(vtab_max_dip(100, 96), 50);
        assert_eq!(vtab_max_dip(0, 96), VTAB_MAX_DIP);
        assert_eq!(vtab_max_dip(-10, 192), VTAB_MAX_DIP);

        assert_eq!(vtab_pref_px(VTAB_W, 96, 2000), 180);
        assert_eq!(vtab_pref_px(VTAB_W, 192, 4000), 360);
        assert_eq!(vtab_pref_px(VTAB_W, 144, 2000), 270);
        assert_eq!(vtab_pref_px(500, 96, 800), 400);
        assert_eq!(vtab_pref_px(900, 96, 2000), VTAB_MAX_DIP);
        assert_eq!(vtab_pref_px(50, 96, 2000), VTAB_MIN_DIP);
        assert_eq!(vtab_pref_px(VTAB_W, 96, 100), 50);

        assert_eq!(vtab_display_px(false, 320, 96, 2000, 2000), 0);
        assert_eq!(vtab_display_px(true, VTAB_W, 96, 2000, 2000), 180);
        assert_eq!(vtab_display_px(true, VTAB_W, 96, 2000, 120), 120);
        assert_eq!(vtab_display_px(true, VTAB_W, 96, 2000, 0), 0);
        assert_eq!(vtab_display_px(true, 400, 96, 800, 800), 400);
        assert_eq!(vtab_display_px(true, 400, 96, 800, 150), 150);
        assert_eq!(vtab_display_px(true, VTAB_W, 192, 4000, 4000), 360);

        let left = 248;
        assert_eq!(vtab_drag_dip(left + 180, left, 96, 2000, 800), 180);
        assert_eq!(vtab_drag_dip(left + 50, left, 96, 2000, 800), VTAB_MIN_DIP);
        assert_eq!(vtab_drag_dip(left + 700, left, 96, 2000, 800), VTAB_MAX_DIP);
        assert_eq!(vtab_drag_dip(left + 500, left, 96, 800, 800), 400);
        assert_eq!(vtab_drag_dip(left + 400, left, 192, 4000, 2000), 200);
        assert_eq!(vtab_drag_dip(left + 150, left, 96, 2000, 150), 150);
        assert_eq!(vtab_drag_dip(left + 50, left, 96, 2000, 50), VTAB_MIN_DIP);
        assert_eq!(vtab_drag_dip(left + 10, left, 96, 100, 80), VTAB_MIN_DIP);
        assert_eq!(vtab_drag_dip(left - 20, left, 96, 2000, 800), VTAB_MIN_DIP);

        for dpi in [96, 120, 144, 192] {
            let pad = 8 * dpi / 96;
            assert_eq!(vtab_edge_from_pad(pad), vtab_edge_px(dpi), "dpi {dpi}");
        }
        assert_eq!(vtab_edge_px(96), 6);
        assert_eq!(vtab_edge_px(192), 12);
        assert_eq!(vtab_edge_from_pad(0), 0);

        let strip = RECT { left: 248, top: 32, right: 428, bottom: 576 };
        let edge = vtab_edge_px(96);
        assert!(vtab_edge_hit(true, strip, edge, 0, 427, 40));
        assert!(vtab_edge_hit(true, strip, edge, 0, 422, 575));
        assert!(!vtab_edge_hit(true, strip, edge, 0, 421, 40));
        assert!(!vtab_edge_hit(true, strip, edge, 0, 428, 40));
        assert!(!vtab_edge_hit(true, strip, edge, 0, 244, 40));
        assert!(!vtab_edge_hit(true, strip, edge, 0, 427, 31));
        assert!(!vtab_edge_hit(true, strip, edge, 0, 427, 576));
        assert!(!vtab_edge_hit(false, strip, edge, 0, 427, 40));
        assert!(!vtab_edge_hit(true, strip, 0, 0, 427, 40));
        assert!(!vtab_edge_hit(true, RECT { right: 248, ..strip }, edge, 0, 247, 40));

        // The plate gap is part of the same 6px grip. The strip owns only what is inside it.
        assert_eq!(vtab_pad_px(96), 4);
        assert_eq!(vtab_pad_px(144), 6);
        assert_eq!(vtab_pad_px(192), 8);
        assert_eq!(vtab_inset_x(96), 5);
        assert_eq!(vtab_inset_x(144), 7);
        assert_eq!(vtab_inset_x(192), 9);
        for dpi in [96, 144, 192] {
            let gap = crate::plate_gap_px(dpi);
            let edge = vtab_edge_px(dpi);
            let width = vtab_width(dpi);
            let strip = RECT { left: 0, top: 0, right: width, bottom: 400 };
            let (left, right) = vtab_grip_span(strip, edge, gap);
            assert_eq!(right - left, edge, "dpi {dpi}");
            assert_eq!(right, width + gap, "dpi {dpi}");
            assert!(vtab_edge_hit(true, strip, edge, gap, width - 1, 20), "dpi {dpi}");
            let inside = edge - gap;
            assert!(!vtab_edge_hit(true, strip, edge, gap, width - inside - 1, 20), "dpi {dpi}");
            assert!(!vtab_edge_hit(true, strip, edge, gap, width, 20), "the gap is not the strip's");
            assert_eq!(vtab_gap_after(width, width + gap, gap), gap);
            assert_eq!(vtab_gap_after(width, width + gap + 40, gap), gap);
            assert_eq!(vtab_gap_after(width, width, gap), 0);
        }

        for (width, dpi) in [(100, 96), (180, 96), (320, 96), (600, 96), (200, 192), (360, 144)] {
            let lay = vtab_lay(width, 400, 2, &[false, true], dpi, 0);
            let edge = vtab_edge_px(dpi);
            let gap = crate::plate_gap_px(dpi);
            let strip = RECT { left: 0, top: 0, right: width, bottom: 400 };
            let inside = (edge - gap).max(0);
            for c in lay.close.into_iter().flatten() {
                assert!(c.right <= width - edge, "close {c:?} overlaps a {edge}px inside grip at {width}px dpi {dpi}");
                assert!(c.right <= width - inside, "close {c:?} overlaps the grip at {width}px dpi {dpi}");
                assert!(!vtab_edge_hit(true, strip, edge, gap, c.left, c.top));
                assert!(!vtab_edge_hit(true, strip, edge, gap, c.right - 1, (c.top + c.bottom) / 2));
            }
            assert!(vtab_edge_hit(true, strip, edge, 0, width - 1, 20));
            assert!(!vtab_edge_hit(true, strip, edge, 0, width - edge - 1, 20));
            if inside > 0 {
                assert!(vtab_edge_hit(true, strip, edge, gap, width - 1, 20));
                assert!(!vtab_edge_hit(true, strip, edge, gap, width - inside - 1, 20));
            }
        }
    }

    #[test]
    fn vertical_strip_drag_moves_when_the_pixels_change_and_keeps_that_width_when_it_ends() {
        let start = VtabLive::begin(180, 180);
        assert_eq!(start.pending, 180);
        // The pixel width changed, so the strip and the panes move. The session value is still 180.
        let (placed, place) = start.moved(240, 240);
        assert!(place);
        assert_eq!((placed.shown, placed.pending, placed.placed_px), (240, 240, 240));
        assert_eq!(vtab_drag_finish(180, Some(placed.pending)), 240);
        // Same pixel width (a DIP step the screen cannot show) does not move the windows.
        let (same, place) = placed.moved(241, 240);
        assert!(!place);
        assert_eq!(same.shown, 240);
        assert_eq!(same.pending, 241);
        assert_eq!(same.placed_px, 240);
        // Button up, Esc, and a lost capture share this. None of them put 180 back.
        assert_eq!(vtab_drag_finish(180, Some(same.pending)), 241);
        assert_eq!(vtab_drag_finish(180, Some(100)), 100);
        // A press that never moved, or a second finish after the gesture was cleared.
        assert_eq!(vtab_drag_finish(180, Some(start.pending)), 180);
        assert_eq!(vtab_drag_finish(180, None), 180);
        assert!(!vtab_should_place(180, 180));
        assert!(vtab_should_place(180, 240));
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let chan = |c: u32| {
            let f = c as f32 / 255.0;
            if f <= 0.04045 { f / 12.92 } else { ((f + 0.055) / 1.055).powf(2.4) }
        };
        let rel = |rgb: u32| 0.2126 * chan(rgb >> 16) + 0.7152 * chan(rgb >> 8 & 0xFF) + 0.0722 * chan(rgb & 0xFF);
        let (x, y) = (rel(a), rel(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn assert_one_edge(face: &TabFace, vertical: bool, ink: u32) {
        match selected_tab_edge(vertical) {
            TabEdge::Top => {
                assert_eq!(face.top_bar, Some(ink));
                assert_eq!(face.left_bar, None);
            }
            TabEdge::Left => {
                assert_eq!(face.left_bar, Some(ink));
                assert_eq!(face.top_bar, None);
            }
        }
    }

    #[test]
    fn selected_colored_tab_keeps_a_light_tint_and_one_edge() {
        let red = TAB_COLORS[0].0;
        let accent = 0x0A9BD6;
        for (name, pal) in [("light", &LIGHT), ("dark", &DARK)] {
            for vertical in [false, true] {
                let face = tab_face(true, false, Some(red), vertical, pal, accent);
                let plain = tab_face(true, false, None, vertical, pal, accent);
                assert_one_edge(&face, vertical, red);
                assert_one_edge(&plain, vertical, accent);
                assert_ne!(face.bg, red, "{name} vertical={vertical}");
                assert_ne!(face.bg, plain.bg, "{name} vertical={vertical}");
                assert_eq!(face.bg, blend(pal.card, red, 0.18), "{name} vertical={vertical}");
                assert_eq!(face.title, pal.text);
                // Hover on the selected tab does not drop the edge or change the wash.
                let hot = tab_face(true, true, Some(red), vertical, pal, accent);
                assert_eq!((hot.left_bar, hot.bg, hot.top_bar), (face.left_bar, face.bg, face.top_bar));
            }
        }
    }

    #[test]
    fn unselected_colored_tab_is_a_tint_without_an_edge() {
        let red = TAB_COLORS[0].0;
        let accent = 0x0A9BD6;
        for (name, pal) in [("light", &LIGHT), ("dark", &DARK)] {
            for vertical in [false, true] {
                let face = tab_face(false, false, Some(red), vertical, pal, accent);
                let plain = tab_face(false, false, None, vertical, pal, accent);
                assert_eq!((face.left_bar, face.top_bar), (None, None), "{name} vertical={vertical}");
                assert_eq!((plain.left_bar, plain.top_bar), (None, None));
                assert_ne!(face.bg, red);
                assert_ne!(face.bg, plain.bg);
                assert_eq!(face.bg, blend(pal.bg, red, 0.18));
                assert_eq!(plain.bg, pal.bg);
                assert_eq!(face.title, pal.sub);
            }
        }
    }

    #[test]
    fn hovered_colored_tab_is_a_different_tint_without_an_edge() {
        let red = TAB_COLORS[0].0;
        let accent = 0x0A9BD6;
        for (name, pal) in [("light", &LIGHT), ("dark", &DARK)] {
            for vertical in [false, true] {
                let hover = tab_face(false, true, Some(red), vertical, pal, accent);
                let idle = tab_face(false, false, Some(red), vertical, pal, accent);
                let selected = tab_face(true, false, Some(red), vertical, pal, accent);
                let plain = tab_face(false, true, None, vertical, pal, accent);
                assert_eq!((hover.left_bar, hover.top_bar), (None, None), "{name} vertical={vertical}");
                assert_one_edge(&selected, vertical, red);
                assert_ne!(hover.bg, idle.bg);
                assert_ne!(hover.bg, selected.bg);
                assert_ne!(hover.bg, red);
                assert_ne!(hover.bg, plain.bg);
                let wash = blend(pal.card, pal.bg, 0.45);
                assert_eq!(hover.bg, blend(wash, red, 0.16));
                assert_eq!(plain.bg, wash);
                assert_eq!(hover.title, pal.sub);
            }
        }
    }

    #[test]
    fn uncolored_tabs_keep_their_faces_and_one_selected_edge() {
        let accent = 0x0A9BD6;
        for pal in [&LIGHT, &DARK] {
            let selected_h = tab_face(true, false, None, false, pal, accent);
            let selected_v = tab_face(true, false, None, true, pal, accent);
            let hover = tab_face(false, true, None, false, pal, accent);
            let idle = tab_face(false, false, None, true, pal, accent);
            assert_one_edge(&selected_h, false, accent);
            assert_one_edge(&selected_v, true, accent);
            assert_eq!((hover.left_bar, hover.top_bar), (None, None));
            assert_eq!((idle.left_bar, idle.top_bar), (None, None));
            assert_eq!(selected_h.bg, pal.card);
            assert_eq!(selected_v.bg, pal.card);
            assert_eq!(hover.bg, blend(pal.card, pal.bg, 0.45));
            assert_eq!(idle.bg, pal.bg);
            assert_eq!(selected_h.title, pal.text);
            assert_eq!(hover.title, pal.sub);
            assert_eq!(idle.title, pal.sub);
        }
    }

    #[test]
    fn colored_tab_ink_stays_readable_on_light_and_dark_washes() {
        let accent = 0x0A9BD6;
        for pal in [&LIGHT, &DARK] {
            for &(color, _) in &TAB_COLORS {
                for vertical in [false, true] {
                    let selected = tab_face(true, false, Some(color), vertical, pal, accent);
                    let idle = tab_face(false, false, Some(color), vertical, pal, accent);
                    let hover = tab_face(false, true, Some(color), vertical, pal, accent);
                    assert!(contrast(selected.title, selected.bg) >= 4.5, "selected {color:#08X} on {:#08X} vs {:#08X}", pal.bg, selected.bg);
                    assert!(contrast(idle.title, idle.bg) >= 3.0, "idle {color:#08X} on {:#08X} vs {:#08X}", pal.bg, idle.bg);
                    assert!(contrast(hover.title, hover.bg) >= 3.0, "hover {color:#08X} on {:#08X} vs {:#08X}", pal.bg, hover.bg);
                    assert_ne!(selected.bg, color);
                    assert_ne!(idle.bg, color);
                    assert_ne!(hover.bg, color);
                }
            }
        }
        let body = RECT { left: 10, top: 4, right: 140, bottom: 30 };
        assert_eq!(tab_color_stripe_rect(body, 96), RECT { left: 10, top: 4, right: 14, bottom: 30 });
        assert_eq!(tab_color_stripe_rect(body, 192), RECT { left: 10, top: 4, right: 18, bottom: 30 });
        // The title inset is only for a coloured vertical tab, selected or not, so the label does not jump.
        // A horizontal coloured tab has no left bar, so it is not nudged.
        assert_eq!(tab_color_label_nudge(true, true, 96), 5);
        assert_eq!(tab_color_label_nudge(true, false, 96), 0);
        assert_eq!(tab_color_label_nudge(false, true, 96), 0);
        assert_eq!(tab_color_label_nudge(false, false, 192), 0);
        assert_eq!(tab_color_label_nudge(true, true, 192), 10);
        let close = RECT { left: 154, top: 11, right: 170, bottom: 27 };
        let labeled = RECT { left: 6, top: 6, right: 174, bottom: 32 };
        assert_eq!(tab_label_room(labeled, Some(close), 96, true, true), 135);
        assert_eq!(tab_label_room(labeled, Some(close), 96, true, false), 140);
        assert_eq!(tab_label_room(labeled, Some(close), 96, false, true), 140);
    }

    #[test]
    fn park_cache_matches_v0_3_0() {
        assert_eq!(PARK_CACHE, 2);
        assert_eq!(PARK_EVICT_MS, 100);
    }

    #[test]
    fn park_victim_keeps_two_tabs_and_drops_the_oldest() {
        let counts = vec![0, 2, 0, 4];
        // Two warm tabs fit. A full grid of panes is still one tab.
        assert_eq!(park_victim(&counts, &[3, 1], PARK_CACHE), None);
        assert_eq!(park_victim(&[16, 16], &[0, 1], PARK_CACHE), None);
        assert_eq!(park_victim(&counts, &[3, 1], 1), Some(3));
        // An lru entry with no browsers is skipped. A stale index is ignored.
        assert_eq!(park_victim(&counts, &[0, 9, 1], 1), Some(1));
        assert_eq!(park_victim(&counts, &[], 0), Some(1));
    }

    #[test]
    fn park_evictions_stop_at_the_tab_cache() {
        let counts = vec![1, 4, 16, 2];
        let lru = vec![0, 1, 2, 3];
        // Four warm tabs, cache of two: drop the two oldest. Pane count does not matter.
        assert_eq!(park_evictions(counts, lru, PARK_CACHE), vec![0, 1]);
        assert!(park_evictions(vec![16, 16], vec![1, 0], PARK_CACHE).is_empty());
        assert!(park_evictions(vec![0, 0], vec![], 0).is_empty());
    }

    #[test]
    fn navigating_slots_stay_tracked_until_they_finish() {
        // Bits 0 and 2 were created for this switch and are still browsing.
        assert_eq!(slots_still_busy(0b101, &[true, false, true]), 0b101);
        // Pane 0 finished. Only pane 2 is still browsing.
        assert_eq!(slots_still_busy(0b101, &[false, false, true]), 0b100);
        assert_eq!(slots_still_busy(0b101, &[false, false, false]), 0);
        // A busy pane this switch did not create is not held.
        assert_eq!(slots_still_busy(0b001, &[true, true]), 0b001);
        assert_eq!(slots_still_busy(0, &[true, true, true]), 0);
    }

    #[test]
    fn cold_switch_shows_the_new_chrome_not_the_outgoing_tab() {
        assert_eq!(SHELL_HOLD_MS, 50);
        // Still browsing, under the cap: new address bar, neutral list. Not the old tab.
        assert_eq!(cold_face(true, 0, SHELL_HOLD_MS), ColdFace::Neutral);
        assert_eq!(cold_face(true, 49, SHELL_HOLD_MS), ColdFace::Neutral);
        // Cap elapsed, or the folder is ready: show the new shell view (the list may be empty).
        assert_eq!(cold_face(true, 50, SHELL_HOLD_MS), ColdFace::Shell);
        assert_eq!(cold_face(false, 0, SHELL_HOLD_MS), ColdFace::Shell);
        assert_eq!(shell_still_held(0b101, &[true, false, true], 0, SHELL_HOLD_MS), 0b101);
        assert_eq!(shell_still_held(0b101, &[false, false, true], 10, SHELL_HOLD_MS), 0b100);
        assert_eq!(shell_still_held(0b101, &[true, false, true], SHELL_HOLD_MS, SHELL_HOLD_MS), 0);
        assert_eq!(shell_still_held(0b101, &[false, false, false], 0, SHELL_HOLD_MS), 0);
    }

    #[test]
    fn three_tabs_keep_both_parked_browsers() {
        // Counts are hidden tabs only. The tab on screen is not in the list.
        // Cache of two hidden tabs: cycling 0→1→2→0 never drops the one we return to.
        let steps = [
            (vec![1, 0, 0], vec![0]),
            (vec![1, 1, 0], vec![0, 1]),
            (vec![0, 1, 1], vec![1, 2]),
            (vec![1, 0, 1], vec![2, 0]),
        ];
        for (counts, lru) in steps {
            assert!(
                park_evictions(counts.clone(), lru.clone(), PARK_CACHE).is_empty(),
                "dropped a warm tab {counts:?} lru={lru:?}"
            );
        }
        // A fourth warm hidden tab is the oldest, not the one just parked.
        assert_eq!(park_evictions(vec![1, 1, 1], vec![0, 1, 2], PARK_CACHE), vec![0]);
    }

    fn coal(live: usize, n: usize, pending: Option<usize>) -> TabCoalesce {
        TabCoalesce { live, n, pending }
    }

    #[test]
    fn ctrl_tab_defers_and_a_repeat_moves_the_strip() {
        assert_eq!(TAB_SWITCH_MS, 70);
        let (s, op) = coal(0, 4, None).step(1);
        assert_eq!(op, TabCoalesceOp::Defer { shown: 1 });
        assert_eq!(s.pending, Some(1));
        assert_eq!(s.live, 0);
        assert_eq!(s.shown(), 1);
        let (s, op) = s.step(1);
        assert_eq!(op, TabCoalesceOp::Defer { shown: 2 });
        assert_eq!(s.shown(), 2);
        // A repeat steps from the strip, not from the panes still on 0.
        let (s, op) = s.step(-1);
        assert_eq!(op, TabCoalesceOp::Defer { shown: 1 });
        let (s, op) = s.step(-1);
        assert_eq!(op, TabCoalesceOp::Idle);
        assert_eq!(s.pending, None);
        assert_eq!(s.live, 0);
        assert_eq!(s.shown(), 0);
    }

    #[test]
    fn ctrl_tab_wraps_and_commit_switches_once() {
        let (s, op) = coal(0, 4, None).step(-1);
        assert_eq!(op, TabCoalesceOp::Defer { shown: 3 });
        let (s, op) = s.commit();
        assert_eq!(op, TabCoalesceOp::Switch { to: 3 });
        assert_eq!((s.live, s.pending), (3, None));
        // Ctrl key-up after the switch has landed does nothing.
        let (s, op) = s.commit();
        assert_eq!(op, TabCoalesceOp::Idle);
        assert_eq!(s.live, 3);
        // A pending index past the end is dropped, not switched to.
        let (s, op) = coal(1, 3, Some(9)).commit();
        assert_eq!(op, TabCoalesceOp::Idle);
        assert_eq!(s.pending, None);
        assert_eq!(s.live, 1);
    }

    #[test]
    fn one_tab_does_not_arm_a_switch() {
        let (s, op) = coal(0, 1, None).step(1);
        assert_eq!(op, TabCoalesceOp::Idle);
        assert_eq!(s.pending, None);
        assert_eq!(s.shown(), 0);
        let (s, op) = coal(0, 0, None).step(1);
        assert_eq!(op, TabCoalesceOp::Idle);
        assert_eq!(s.live, 0);
    }

    #[test]
    fn shown_tab_ignores_a_stale_pending_index() {
        assert_eq!(shown_tab(1, Some(3), 4), 3);
        assert_eq!(shown_tab(1, Some(9), 4), 1);
        assert_eq!(shown_tab(1, None, 4), 1);
        assert_eq!(shown_tab(0, None, 0), 0);
    }
}

