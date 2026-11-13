//! Joined segment groups.
//!
//! Item kind, server type, icon source, and the settings context-menu switch share
//! one painter. The cells sit in a single rounded frame (card fill, line border,
//! shared [`RADIUS`]). The selected cell is a pale accent wash, accent bold text,
//! and a 2 DIP underline. That is not a primary button: a primary face is solid
//! accent with white text.

use super::button::{ButtonState, FocusRing, visual_from_pointer};
use super::tokens::*;
use crate::imports::*;

/// Horizontal inset of the underline, DIP. A little air inside the cell.
pub(crate) const SEGMENT_UNDERLINE_INSET: i32 = SPACE_8;
/// Underline thickness, DIP. `px(dpi, 2)` at the cell's bottom edge.
pub(crate) const SEGMENT_UNDERLINE_DIP: i32 = 2;
/// Divider stops this many DIP short of the frame, top and bottom.
pub(crate) const SEGMENT_DIVIDER_INSET: i32 = 5;
/// Padding on each side of a label, DIP. The cell is at least the ink plus this twice.
pub(crate) const SEGMENT_PAD_X: i32 = SPACE_8;
/// Narrowest cell, DIP. Short labels (URL, RDP) still get a clickable face.
pub(crate) const SEGMENT_MIN_DIP: i32 = 40;

/// ASCII is one cell. Hangul, kana, and han are two. Same measure as the settings chips.
const GLYPH_ASCII: i32 = 7;
const GLYPH_OTHER: i32 = 14;

pub(crate) struct SegmentColors {
    pub fill: u32,
    pub ink: u32,
    pub bold: bool,
    /// Accent bar under a selected label. `None` when the cell is not selected.
    pub underline: Option<u32>,
}

pub(crate) struct SegmentCell<'a> {
    pub rect: RECT,
    pub label: &'a str,
    pub disabled: bool,
}

pub(crate) fn segment_colors(selected: bool, state: ButtonState, dark: bool) -> SegmentColors {
    segment_colors_for(selected, state, dark, ACCENT)
}

/// Selected fill is accent mixed into the card (the shared mist), never the solid accent.
/// Hover and press lay the secondary wash on top of that fill. Disabled fades toward the ground.
pub(crate) fn segment_colors_for(
    selected: bool,
    state: ButtonState,
    dark: bool,
    accent: u32,
) -> SegmentColors {
    let t = tokens_for(dark);
    let base = if selected {
        crate::blend(t.card, accent, t.mist)
    } else {
        t.card
    };
    let (ink, bold, underline) = if selected {
        (accent, true, Some(accent))
    } else {
        (t.text, false, None)
    };
    if state == ButtonState::Disabled {
        return SegmentColors {
            fill: crate::blend(t.bg, base, DISABLED_ALPHA),
            ink: crate::blend(t.bg, ink, DISABLED_ALPHA),
            bold: false,
            underline: underline.map(|c| crate::blend(t.bg, c, DISABLED_ALPHA)),
        };
    }
    let fill = match state {
        ButtonState::Hover => crate::blend(base, t.text, SECONDARY_HOVER),
        ButtonState::Pressed => crate::blend(base, t.text, SECONDARY_PRESSED),
        _ => base,
    };
    SegmentColors {
        fill,
        ink,
        bold,
        underline,
    }
}

/// Group border. Focus turns it accent, the same signal a secondary button uses.
pub(crate) fn segment_border(focused: bool, line: u32, accent: u32) -> u32 {
    if focused {
        accent
    } else {
        line
    }
}

/// Ink width of `label` in pixels. Pure: no device context.
pub(crate) fn segment_label_px(label: &str, dpi: i32) -> i32 {
    let mut w = 0i32;
    for ch in label.chars() {
        w += px(
            dpi,
            if ch.is_ascii() {
                GLYPH_ASCII
            } else {
                GLYPH_OTHER
            },
        );
    }
    w
}

/// Width a cell needs so `label` is not clipped, and not narrower than [`SEGMENT_MIN_DIP`].
pub(crate) fn segment_need_px(label: &str, dpi: i32) -> i32 {
    let ink = segment_label_px(label, dpi);
    let pad = px(dpi, SEGMENT_PAD_X).saturating_mul(2);
    ink.saturating_add(pad).max(px(dpi, SEGMENT_MIN_DIP))
}

/// Share `total` pixels across `labels`. Longer ink gets a wider cell.
/// The widths sum to `total`. When the row is wide enough, each cell is at least [`segment_need_px`].
pub(crate) fn segment_widths(labels: &[&str], total: i32, dpi: i32) -> Vec<i32> {
    let needs: Vec<i32> = labels.iter().map(|s| segment_need_px(s, dpi)).collect();
    distribute(&needs, total)
}

/// Largest-remainder split. `weights` and `total` are non-negative. The result sums to `total`.
fn distribute(weights: &[i32], total: i32) -> Vec<i32> {
    let n = weights.len();
    if n == 0 || total <= 0 {
        return vec![0; n];
    }
    let w: Vec<i64> = weights.iter().map(|x| (*x).max(0) as i64).collect();
    let sum: i64 = w.iter().sum();
    if sum <= 0 {
        let base = total / n as i32;
        let mut out = vec![base; n];
        let mut left = total - base * n as i32;
        for slot in &mut out {
            if left <= 0 {
                break;
            }
            *slot += 1;
            left -= 1;
        }
        return out;
    }
    let mut out = vec![0i32; n];
    let mut rem = Vec::with_capacity(n);
    let mut used = 0i32;
    for (i, weight) in w.iter().enumerate() {
        let prod = weight * total as i64;
        let base = (prod / sum) as i32;
        out[i] = base;
        used += base;
        rem.push((prod % sum, i));
    }
    rem.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = total.saturating_sub(used);
    let mut k = 0usize;
    while left > 0 {
        out[rem[k % n].1] += 1;
        left -= 1;
        k += 1;
    }
    out
}

/// Cells in one row, left to right, no gap. Widths come from [`segment_widths`].
pub(crate) fn segment_row(left: i32, top: i32, height: i32, widths: &[i32]) -> Vec<RECT> {
    let mut x = left;
    let mut out = Vec::with_capacity(widths.len());
    for w in widths {
        let w = (*w).max(0);
        out.push(RECT {
            left: x,
            top,
            right: x + w,
            bottom: top + height,
        });
        x += w;
    }
    out
}

/// Outer frame: the union of the cells. One rounded rect, not one frame per cell.
pub(crate) fn segment_group_bounds(rects: impl IntoIterator<Item = RECT>) -> RECT {
    let mut out: Option<RECT> = None;
    for r in rects {
        if r.right <= r.left || r.bottom <= r.top {
            continue;
        }
        out = Some(match out {
            None => r,
            Some(a) => RECT {
                left: a.left.min(r.left),
                top: a.top.min(r.top),
                right: a.right.max(r.right),
                bottom: a.bottom.max(r.bottom),
            },
        });
    }
    out.unwrap_or_default()
}

/// Hairline between two cells. Vertical inset is [`SEGMENT_DIVIDER_INSET`] DIP.
pub(crate) fn segment_divider_rect(left_cell: RECT, right_cell: RECT, dpi: i32) -> RECT {
    let inset = px(dpi, SEGMENT_DIVIDER_INSET);
    let w = px(dpi, 1).max(1);
    let top = left_cell.top + inset;
    let bottom = (left_cell.bottom - inset).max(top);
    let edge = if right_cell.left > left_cell.right {
        (left_cell.right + right_cell.left) / 2
    } else {
        left_cell.right
    };
    let x = edge - w / 2;
    RECT {
        left: x,
        top,
        right: x + w,
        bottom,
    }
}

/// Accent bar near the bottom of a selected cell, inset on the left and right.
pub(crate) fn segment_underline_rect(cell: RECT, dpi: i32) -> RECT {
    let thick = px(dpi, SEGMENT_UNDERLINE_DIP).max(1);
    let inset_x = px(dpi, SEGMENT_UNDERLINE_INSET);
    let inset_b = px(dpi, 1).max(1);
    let bottom = cell.bottom - inset_b;
    let top = bottom - thick;
    let left = cell.left + inset_x;
    let right = (cell.right - inset_x).max(left);
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn cell_state(
    i: usize,
    cell: &SegmentCell<'_>,
    hot: Option<usize>,
    down: Option<usize>,
) -> ButtonState {
    if cell.disabled {
        ButtonState::Disabled
    } else {
        visual_from_pointer(i, hot, down).state
    }
}

fn cell_colors(
    i: usize,
    cell: &SegmentCell<'_>,
    selected: Option<usize>,
    hot: Option<usize>,
    down: Option<usize>,
    dark: bool,
    accent: u32,
) -> SegmentColors {
    segment_colors_for(
        selected == Some(i),
        cell_state(i, cell, hot, down),
        dark,
        accent,
    )
}

/// Draw the rounded frame and the cell washes. `scale` is 1 on the window DC, or the
/// supersample factor inside [`crate::aa_blit`]. Dividers, the underline, and the text
/// are drawn afterwards on the window DC so a 2 DIP bar stays crisp.
unsafe fn paint_group_face(
    dc: HDC,
    bounds: RECT,
    cells: &[SegmentCell<'_>],
    selected: Option<usize>,
    hot: Option<usize>,
    down: Option<usize>,
    border: u32,
    card: u32,
    dark: bool,
    accent: u32,
    dpi: i32,
    scale: i32,
    ox: i32,
    oy: i32,
) {
    let scale = scale.max(1);
    let gr = crate::scaled_from(bounds, bounds, scale, ox, oy);
    let border_px = scale;
    let rad = px(dpi, RADIUS).max(1) * scale;
    crate::rfill(dc, gr, rad, border, None);
    let inner = RECT {
        left: gr.left + border_px,
        top: gr.top + border_px,
        right: gr.right - border_px,
        bottom: gr.bottom - border_px,
    };
    if inner.right <= inner.left || inner.bottom <= inner.top {
        return;
    }
    let inner_rad = (rad - border_px).max(1);
    crate::rfill(dc, inner, inner_rad, card, None);

    let mut order: Vec<usize> = (0..cells.len())
        .filter(|&i| {
            let r = cells[i].rect;
            r.right > r.left && r.bottom > r.top
        })
        .collect();
    order.sort_by_key(|&i| (cells[i].rect.left, cells[i].rect.top));
    if order.is_empty() {
        return;
    }
    let first = order[0];
    let last = order[order.len() - 1];
    for &i in &order {
        let colors = cell_colors(i, &cells[i], selected, hot, down, dark, accent);
        if colors.fill == card {
            continue;
        }
        let cr = crate::scaled_from(bounds, cells[i].rect, scale, ox, oy);
        let box_r = RECT {
            left: cr.left.max(inner.left),
            top: cr.top.max(inner.top),
            right: cr.right.min(inner.right),
            bottom: cr.bottom.min(inner.bottom),
        };
        if box_r.right <= box_r.left || box_r.bottom <= box_r.top {
            continue;
        }
        let saved = SaveDC(dc);
        let _ = IntersectClipRect(dc, box_r.left, box_r.top, box_r.right, box_r.bottom);
        let end_left = i == first;
        let end_right = i == last;
        if end_left || end_right {
            let mut rr = box_r;
            if end_left && !end_right {
                rr.right = rr.right.saturating_add(inner_rad);
            }
            if end_right && !end_left {
                rr.left = rr.left.saturating_sub(inner_rad);
            }
            crate::rfill(dc, rr, inner_rad, colors.fill, None);
        } else {
            crate::fill(dc, box_r, colors.fill);
        }
        if saved != 0 {
            let _ = RestoreDC(dc, saved);
        }
    }
}

/// Paint one joined group. `selected`, `hot`, `down`, and `focused` are indexes into `cells`.
/// `ring` matches [`crate::paint_button`]: [`FocusRing::Inside`] clears the ground first
/// because an owner-draw child clips to its window. Segments on a window pass [`FocusRing::Outside`].
pub(crate) unsafe fn paint_segment_group(
    dc: HDC,
    cells: &[SegmentCell<'_>],
    selected: Option<usize>,
    hot: Option<usize>,
    down: Option<usize>,
    focused: Option<usize>,
    dpi: i32,
    ground: u32,
    ring: FocusRing,
) {
    if cells.is_empty() {
        return;
    }
    let bounds = segment_group_bounds(cells.iter().map(|c| c.rect));
    if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
        return;
    }
    let dark = ui_tokens().dark;
    let accent = crate::live_accent();
    let t = ui_tokens();
    let group_focus = focused.is_some_and(|i| cells.get(i).is_some_and(|c| !c.disabled));
    let border = segment_border(group_focus, t.line, accent);
    if ring == FocusRing::Inside {
        crate::fill(dc, bounds, ground);
    }
    let painted = crate::aa_blit(dc, bounds, ground, |mem, scale, ox, oy| unsafe {
        paint_group_face(
            mem, bounds, cells, selected, hot, down, border, t.card, dark, accent, dpi, scale, ox,
            oy,
        );
    });
    if !painted {
        paint_group_face(
            dc, bounds, cells, selected, hot, down, border, t.card, dark, accent, dpi, 1, 0, 0,
        );
    }
    let mut order: Vec<usize> = (0..cells.len())
        .filter(|&i| {
            let r = cells[i].rect;
            r.right > r.left && r.bottom > r.top
        })
        .collect();
    order.sort_by_key(|&i| (cells[i].rect.left, cells[i].rect.top));
    for pair in order.windows(2) {
        let (a, b) = (cells[pair[0]].rect, cells[pair[1]].rect);
        if a.top != b.top || a.bottom != b.bottom {
            continue;
        }
        let d = segment_divider_rect(a, b, dpi);
        if d.bottom > d.top && d.right > d.left {
            crate::fill(dc, d, t.line);
        }
    }
    for (i, cell) in cells.iter().enumerate() {
        let colors = cell_colors(i, cell, selected, hot, down, dark, accent);
        if let Some(ink) = colors.underline {
            let u = segment_underline_rect(cell.rect, dpi);
            if u.right > u.left && u.bottom > u.top && u.top >= cell.rect.top {
                crate::fill(dc, u, ink);
            }
        }
        if cell.label.is_empty() {
            continue;
        }
        let font = if colors.bold {
            crate::bold_ui_font()
        } else {
            crate::body_font()
        };
        crate::text(
            dc,
            cell.rect,
            cell.label,
            font,
            colors.ink,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        button_colors, px, tokens_for, ButtonKind, ButtonState, ACCENT, ACCENT_HOVER_DARK,
        ACCENT_HOVER_LIGHT, ACCENT_PRESSED, BG_DARK, BG_LIGHT, CARD_DARK, CARD_LIGHT, DARK, LIGHT,
        LINE_DARK, LINE_LIGHT, MIST_DARK, MIST_LIGHT, TEXT_DARK, TEXT_LIGHT,
    };

    fn lum(c: u32) -> f32 {
        0.2126 * (c >> 16 & 0xFF) as f32
            + 0.7152 * (c >> 8 & 0xFF) as f32
            + 0.0722 * (c & 0xFF) as f32
    }

    #[test]
    fn selected_cell_is_a_pale_accent_not_a_primary_button() {
        let mint = 0x2EC4A6u32;
        for dark in [false, true] {
            let ground = if dark { BG_DARK } else { BG_LIGHT };
            let rest = segment_colors(true, ButtonState::Normal, dark);
            let hover = segment_colors(true, ButtonState::Hover, dark);
            let pressed = segment_colors(true, ButtonState::Pressed, dark);
            let t = tokens_for(dark);
            let primary = button_colors(ButtonKind::Primary, ButtonState::Normal, dark, ground);
            let primary_hover =
                button_colors(ButtonKind::Primary, ButtonState::Hover, dark, ground);
            let primary_down =
                button_colors(ButtonKind::Primary, ButtonState::Pressed, dark, ground);
            assert_eq!(rest.fill, crate::blend(t.card, ACCENT, t.mist));
            assert_ne!(rest.fill, ACCENT);
            assert_ne!(rest.fill, t.card);
            assert_ne!(Some(rest.fill), primary.fill);
            assert_eq!(rest.ink, ACCENT);
            assert_ne!(rest.ink, primary.ink);
            assert!(rest.bold);
            assert_eq!(rest.underline, Some(ACCENT));
            assert_ne!(hover.fill, rest.fill);
            assert_ne!(pressed.fill, hover.fill);
            assert_ne!(Some(hover.fill), primary_hover.fill);
            assert_ne!(Some(pressed.fill), primary_down.fill);
            assert_eq!(hover.ink, ACCENT);
            assert_eq!(pressed.ink, ACCENT);
            assert!(hover.bold && pressed.bold);
            assert_eq!(hover.underline, Some(ACCENT));
            assert_eq!(pressed.underline, Some(ACCENT));
            for c in [rest.fill, hover.fill, pressed.fill] {
                assert_ne!(c, mint);
                assert_ne!(c, ACCENT_HOVER_LIGHT);
                assert_ne!(c, ACCENT_HOVER_DARK);
                assert_ne!(c, ACCENT_PRESSED);
            }
        }
        let light = segment_colors(true, ButtonState::Normal, false);
        let dark = segment_colors(true, ButtonState::Normal, true);
        assert_eq!(light.fill, crate::blend(CARD_LIGHT, ACCENT, MIST_LIGHT));
        assert_eq!(dark.fill, crate::blend(CARD_DARK, ACCENT, MIST_DARK));
        assert!(lum(light.fill) > lum(ACCENT));
        assert!(lum(dark.fill) > lum(CARD_DARK) + 8.0);
        assert!(lum(dark.fill) < lum(ACCENT) - 40.0);
    }

    #[test]
    fn resting_cell_uses_card_and_body_text_and_hover_matches_secondary() {
        let rest = segment_colors(false, ButtonState::Normal, false);
        let hover = segment_colors(false, ButtonState::Hover, false);
        let pressed = segment_colors(false, ButtonState::Pressed, false);
        let dark = segment_colors(false, ButtonState::Normal, true);
        assert_eq!(rest.fill, CARD_LIGHT);
        assert_eq!(rest.ink, TEXT_LIGHT);
        assert_eq!(rest.ink, LIGHT.text);
        assert!(!rest.bold);
        assert!(rest.underline.is_none());
        assert_eq!(dark.ink, TEXT_DARK);
        assert_eq!(dark.ink, DARK.text);
        assert_eq!(
            hover.fill,
            crate::blend(CARD_LIGHT, TEXT_LIGHT, SECONDARY_HOVER)
        );
        assert_eq!(
            pressed.fill,
            crate::blend(CARD_LIGHT, TEXT_LIGHT, SECONDARY_PRESSED)
        );
        assert_ne!(hover.fill, rest.fill);
        assert_ne!(pressed.fill, hover.fill);
        assert!(hover.underline.is_none());
        assert!(!hover.bold);
    }

    #[test]
    fn disabled_cell_fades_toward_the_ground_and_drops_bold() {
        let rest = segment_colors(true, ButtonState::Normal, false);
        let off = segment_colors(true, ButtonState::Disabled, false);
        let dark_rest = segment_colors(false, ButtonState::Normal, true);
        let idle = segment_colors(false, ButtonState::Disabled, true);
        assert!(rest.bold);
        assert!(!off.bold);
        assert!(!idle.bold);
        assert_ne!(off.fill, rest.fill);
        assert_ne!(off.ink, rest.ink);
        assert_ne!(off.underline, rest.underline);
        assert!(off.underline.is_some());
        assert!(idle.underline.is_none());
        assert_ne!(idle.fill, dark_rest.fill);
        assert_ne!(idle.ink, dark_rest.ink);
    }

    #[test]
    fn focus_turns_the_group_border_accent() {
        assert_eq!(segment_border(false, LINE_LIGHT, ACCENT), LINE_LIGHT);
        assert_eq!(segment_border(false, LINE_DARK, ACCENT), LINE_DARK);
        assert_eq!(segment_border(true, LINE_LIGHT, ACCENT), ACCENT);
        assert_eq!(segment_border(true, LINE_DARK, ACCENT), ACCENT);
        assert_ne!(segment_border(true, LINE_LIGHT, ACCENT), CARD_LIGHT);
    }

    #[test]
    fn group_frame_is_one_rect_and_cells_touch() {
        let labels = ["Folder", "File", "Special folder", "Command", "URL", "RDP"];
        let total = 488;
        let widths = segment_widths(&labels, total, 96);
        let cells = segment_row(16, 62, 28, &widths);
        let frame = segment_group_bounds(cells.iter().copied());
        assert_eq!(
            frame,
            RECT {
                left: 16,
                top: 62,
                right: 16 + total,
                bottom: 90
            }
        );
        for (i, c) in cells.iter().enumerate() {
            assert_eq!(c.top, frame.top);
            assert_eq!(c.bottom, frame.bottom);
            assert!(c.left >= frame.left && c.right <= frame.right);
            if i + 1 < cells.len() {
                assert_eq!(c.right, cells[i + 1].left, "cells share an edge");
                let div = segment_divider_rect(*c, cells[i + 1], 96);
                assert_eq!(div.top, c.top + SEGMENT_DIVIDER_INSET);
                assert_eq!(div.bottom, c.bottom - SEGMENT_DIVIDER_INSET);
                assert_eq!(div.right - div.left, 1);
            }
        }
        let under = segment_underline_rect(cells[0], 96);
        assert_eq!(under.bottom - under.top, SEGMENT_UNDERLINE_DIP);
        assert!(under.left > cells[0].left && under.right < cells[0].right);
        assert_eq!(under.bottom, cells[0].bottom - 1);
    }

    #[test]
    fn underline_is_two_dip_at_every_scale() {
        let cell = RECT {
            left: 0,
            top: 0,
            right: 120,
            bottom: 40,
        };
        for dpi in [96, 144, 192] {
            let u = segment_underline_rect(cell, dpi);
            assert_eq!(u.bottom - u.top, px(dpi, SEGMENT_UNDERLINE_DIP), "{dpi}");
            assert_eq!(u.left - cell.left, px(dpi, SEGMENT_UNDERLINE_INSET));
            assert_eq!(cell.right - u.right, px(dpi, SEGMENT_UNDERLINE_INSET));
            let div = segment_divider_rect(
                cell,
                RECT {
                    left: cell.right,
                    ..cell
                },
                dpi,
            );
            assert_eq!(div.top - cell.top, px(dpi, SEGMENT_DIVIDER_INSET));
            assert_eq!(cell.bottom - div.bottom, px(dpi, SEGMENT_DIVIDER_INSET));
        }
    }

    fn ini_val(file: &str, key: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("lang")
            .join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            if k.trim() == key {
                return v.trim().to_string();
            }
        }
        panic!("{key} missing in {file}");
    }

    fn kind_labels(lang: &str) -> Vec<String> {
        if lang == "en" {
            return ["Folder", "File", "Special folder", "Command", "URL", "RDP"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }
        let file = format!("lang.{lang}.ini");
        [
            "folder",
            "file",
            "kind_special_folder",
            "kind_command",
            "kind_url",
            "kind_rdp",
        ]
        .into_iter()
        .map(|k| ini_val(&file, k))
        .collect()
    }

    #[test]
    fn widths_follow_the_label_and_do_not_clip_at_96_144_192() {
        for lang in ["en", "ko", "ja", "fr", "de"] {
            let owned = kind_labels(lang);
            let labels: Vec<&str> = owned.iter().map(String::as_str).collect();
            for dpi in [96, 144, 192] {
                let inner = px(dpi, 520) - px(dpi, PAD_BODY_X) * 2;
                let widths = segment_widths(&labels, inner, dpi);
                assert_eq!(widths.len(), labels.len(), "{lang} {dpi}");
                assert_eq!(widths.iter().sum::<i32>(), inner, "{lang} {dpi}");
                for (label, w) in labels.iter().zip(&widths) {
                    let ink = segment_label_px(label, dpi);
                    let need = segment_need_px(label, dpi);
                    assert!(
                        *w >= ink,
                        "{lang} {dpi} {label:?} width {w} clips ink {ink}"
                    );
                    assert!(*w >= need, "{lang} {dpi} {label:?} width {w} < need {need}");
                    assert!(*w >= px(dpi, SEGMENT_MIN_DIP), "{lang} {dpi}");
                }
                for i in 0..labels.len() {
                    for j in 0..labels.len() {
                        let ai = segment_label_px(labels[i], dpi);
                        let aj = segment_label_px(labels[j], dpi);
                        if ai > aj {
                            assert!(
                                widths[i] >= widths[j],
                                "{lang} {dpi} {:?} is longer than {:?} but not as wide",
                                labels[i],
                                labels[j]
                            );
                        }
                    }
                }
                assert!(segment_label_px(labels[2], dpi) > segment_label_px("URL", dpi));
                assert!(
                    widths[2] > widths[4],
                    "{lang} {dpi} special {:?} vs URL",
                    labels[2]
                );
                assert!(widths[2] > widths[5], "{lang} {dpi} special vs RDP");
                assert_eq!(
                    widths[2],
                    *widths.iter().max().unwrap(),
                    "{lang} {dpi} special should be the widest"
                );
            }
        }
    }
}
