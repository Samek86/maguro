//! Shared scroll state and body painting for Quick Look and the preview pane.
//!
//! Text, tables and lists measure once and then draw only the rows on screen.
//! Each surface keeps its own [`BodyScroll`]; the two previews can be open together.

use crate::app::*;
use crate::i18n::*;
use crate::imports::*;
use crate::ui::*;

use super::peek::Pix;
use super::preview::TextKind;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Style {
    Plain,
    Mono, // not wrapped
    H1,
    H2,
    H3,
    Bullet,
    Quote,
    Code, // fenced markdown: monospace, not wrapped, with a background
}

pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) y: i32,
    pub(crate) h: i32,
    pub(crate) no: usize, // source line number, for the code gutter
    table: Option<(Vec<Vec<String>>, Vec<i32>)>,
    frags: Vec<Frag>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) code: bool,
}

struct Frag {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    x: i32,
    dy: i32,
}

pub(crate) struct MdLine {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) runs: Vec<Run>,
}

/// Scroll, laid-out lines and the heading faces for one preview surface.
#[derive(Default)]
pub(crate) struct BodyScroll {
    pub y: i32,
    pub x: i32,
    pub max_y: i32,
    pub max_x: i32,
    pub page_h: i32,
    pub page_w: i32,
    pub layout: Option<(usize, i32, Vec<Line>, i32)>,
}

/// Heading faces cached by the caller. Quick Look keeps them on its view; the preview pane
/// keeps them in a thread-local so the shared scroll state can cross threads.
/// `(dpi, lfHeight)` so a zoomed body font does not reuse the faces built at 100%.
/// Six faces: mono, bold, heading, subheading, italic, bold-italic.
pub(crate) type FaceCache = Option<((i32, i32), [HFONT; 6])>;

/// Markdown lines: headings, lists, quotes, rules, fenced code. Inline bold, italic, code, and links stay on `runs`.
pub(crate) fn md_lines(text: &str) -> Vec<MdLine> {
    let mut out = Vec::new();
    let mut fence: Option<&'static str> = None;
    for l in text.lines() {
        let t = l.trim_start();
        if let Some(mark) = fence {
            if t.starts_with(mark) {
                fence = None;
                continue;
            }
            out.push(MdLine { text: l.to_string(), style: Style::Code, runs: Vec::new() });
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(if t.starts_with("```") { "```" } else { "~~~" });
            continue;
        }
        let hashes = t.bytes().take_while(|&c| c == b'#').count();
        if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
            let (text, _) = inline_runs(&t[hashes + 1..]);
            let style = match hashes {
                1 => Style::H1,
                2 => Style::H2,
                _ => Style::H3,
            };
            out.push(MdLine { text, style, runs: Vec::new() });
            continue;
        }
        if is_rule(t) {
            out.push(MdLine { text: "────────".into(), style: Style::Quote, runs: Vec::new() });
            continue;
        }
        if let Some(rest) = t.strip_prefix("> ").or_else(|| t.strip_prefix('>')) {
            let (text, runs) = inline_runs(rest.trim_start());
            out.push(MdLine { text, style: Style::Quote, runs });
            continue;
        }
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
            out.push(bullet_line(l, t, "• ", rest));
            continue;
        }
        if let Some((num, rest)) = numbered(t) {
            out.push(bullet_line(l, t, &format!("{num}. "), rest));
            continue;
        }
        let (text, runs) = inline_runs(l);
        out.push(MdLine { text, style: Style::Plain, runs });
    }
    out
}

fn bullet_line(raw: &str, trimmed: &str, marker: &str, rest: &str) -> MdLine {
    let indent = "    ".repeat((raw.len() - trimmed.len()) / 2);
    let (body, mut runs) = inline_runs(rest);
    let prefix = format!("{indent}{marker}");
    let text = format!("{prefix}{body}");
    if !runs.is_empty() {
        runs.insert(0, Run { text: prefix, bold: false, italic: false, code: false });
    }
    MdLine { text, style: Style::Bullet, runs }
}

fn numbered(t: &str) -> Option<(&str, &str)> {
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i > 6 || b.get(i) != Some(&b'.') || b.get(i + 1) != Some(&b' ') {
        return None;
    }
    Some((&t[..i], &t[i + 2..]))
}

fn is_rule(t: &str) -> bool {
    let u: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    u.len() >= 3 && (u.chars().all(|c| c == '-') || u.chars().all(|c| c == '*') || u.chars().all(|c| c == '_'))
}

/// Visible text plus styled runs. A line with no bold, italic, or code returns an empty run list.
pub(crate) fn inline_runs(input: &str) -> (String, Vec<Run>) {
    let b = input.as_bytes();
    let mut i = 0;
    let mut runs: Vec<Run> = Vec::new();
    let mut cur = String::new();
    let mut bold = false;
    let mut italic = false;
    let flush = |runs: &mut Vec<Run>, cur: &mut String, bold: bool, italic: bool, code: bool| {
        if cur.is_empty() {
            return;
        }
        if let Some(last) = runs.last_mut() {
            if last.bold == bold && last.italic == italic && last.code == code {
                last.text.push_str(cur);
                cur.clear();
                return;
            }
        }
        runs.push(Run { text: std::mem::take(cur), bold, italic, code });
    };
    while i < b.len() {
        if b[i] == b'`' {
            if let Some(end) = b[i + 1..].iter().position(|&c| c == b'`') {
                flush(&mut runs, &mut cur, bold, italic, false);
                let inner = input[i + 1..i + 1 + end].to_string();
                if !inner.is_empty() {
                    runs.push(Run { text: inner, bold: false, italic: false, code: true });
                }
                i += end + 2;
                continue;
            }
        }
        if b[i..].starts_with(b"**") || b[i..].starts_with(b"__") {
            flush(&mut runs, &mut cur, bold, italic, false);
            bold = !bold;
            i += 2;
            continue;
        }
        if b[i] == b'*' || (b[i] == b'_' && underscore_mark(b, i, italic)) {
            flush(&mut runs, &mut cur, bold, italic, false);
            italic = !italic;
            i += 1;
            continue;
        }
        if b[i] == b'[' {
            if let Some((shown, next)) = link_text(&input[i..]) {
                flush(&mut runs, &mut cur, bold, italic, false);
                cur.push_str(&shown);
                flush(&mut runs, &mut cur, bold, italic, false);
                i += next;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
        cur.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut runs, &mut cur, bold, italic, false);
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    if runs.iter().any(|r| r.bold || r.italic || r.code) { (text, runs) } else { (text, Vec::new()) }
}

fn underscore_mark(b: &[u8], i: usize, italic: bool) -> bool {
    if italic {
        return true;
    }
    if i > 0 && b[i - 1].is_ascii_alphanumeric() {
        return false;
    }
    b[i + 1..].contains(&b'_')
}

fn link_text(s: &str) -> Option<(String, usize)> {
    let end = s.find(']')?;
    if end < 1 || !s[end..].starts_with("](") {
        return None;
    }
    let url_at = end + 2;
    let close = s[url_at..].find(')')?;
    let label = s[1..end].to_string();
    let url = s[url_at..url_at + close].to_string();
    let shown = if label.is_empty() { url } else if label == url { label } else { format!("{label} ({url})") };
    Some((shown, url_at + close + 1))
}

/// Source text, or the laid-out lines inside `sel` (1-based, inclusive) when a range is set.
pub(crate) fn selected_text(source: &str, lines: &[Line], sel: Option<(usize, usize)>) -> String {
    let Some((a, b)) = sel else { return source.to_string() };
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if lo == 0 || hi == 0 {
        return source.to_string();
    }
    let picked: Vec<&str> = lines.iter().filter(|l| l.no >= lo && l.no <= hi).map(|l| l.text.as_str()).collect();
    if picked.is_empty() {
        source.to_string()
    } else {
        picked.join("\n")
    }
}

/// Row under the pointer. `scroll_y` is the list's vertical scroll. `None` past the last row.
pub(crate) fn list_row_at(scroll_y: i32, area_top: i32, y: i32, row_h: i32, len: usize) -> Option<usize> {
    if row_h <= 0 || y < area_top || len == 0 {
        return None;
    }
    let i = scroll_y.saturating_add(y - area_top) / row_h;
    if i < 0 {
        return None;
    }
    let i = i as usize;
    (i < len).then_some(i)
}

/// Source line number under the pointer, in the same space [`paint_text`] lays lines out.
pub(crate) fn line_no_at(lines: &[Line], scroll_y: i32, area_top: i32, y: i32) -> Option<usize> {
    if y < area_top {
        return None;
    }
    let rel = scroll_y.saturating_add(y - area_top);
    if rel < 0 {
        return None;
    }
    lines.iter().find(|l| rel >= l.y && rel < l.y.saturating_add(l.h)).map(|l| l.no)
}

pub(crate) fn row_h(fh: i32, dpi: i32) -> i32 {
    fh + SPACE_8 * dpi / 96
}

pub(crate) unsafe fn fonts(cache: &mut FaceCache, dpi: i32, base: HFONT) -> [HFONT; 6] {
    let mut lf = LOGFONTW::default();
    GetObjectW(base.into(), size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as _));
    let key = (dpi, lf.lfHeight);
    if let Some((k, f)) = *cache {
        if k == key {
            return f;
        }
        for x in f {
            delete_font(x);
        }
    }
    let f = preview_fonts(&lf);
    *cache = Some((key, f));
    f
}

/// A body font scaled by `zoom` (1.0 returns `base` and does not own a new handle).
pub(crate) unsafe fn font_at_zoom(base: HFONT, zoom: f64) -> (HFONT, bool) {
    crate::ui::font_at_zoom(base, zoom)
}

pub(crate) unsafe fn clear_faces(cache: &mut FaceCache) {
    if let Some((_, fonts)) = cache.take() {
        for f in fonts {
            delete_font(f);
        }
    }
}

fn style_font(st: Style, font: HFONT, f: &[HFONT; 6]) -> HFONT {
    match st {
        Style::Mono | Style::Code => f[0],
        Style::H1 => f[2],
        Style::H2 => f[3],
        Style::H3 => f[1],
        _ => font,
    }
}

fn run_font(bold: bool, italic: bool, code: bool, font: HFONT, f: &[HFONT; 6]) -> HFONT {
    if code {
        f[0]
    } else if bold && italic {
        f[5]
    } else if bold {
        f[1]
    } else if italic {
        f[4]
    } else {
        font
    }
}

// Heights of every line at this width, measured once per item and width; painting then draws only what shows.
unsafe fn layout(scroll: &mut BodyScroll, dc: HDC, gen: usize, width: i32, text: &str, kind: TextKind, font: HFONT, f: &[HFONT; 6], dpi: i32) -> i32 {
    if let Some(total) = scroll.layout.as_ref().filter(|(g, w, ..)| *g == gen && *w == width).map(|l| l.3) {
        return total;
    }
    let plain = |l: &str, st: Style| MdLine { text: l.to_string(), style: st, runs: Vec::new() };
    let src: Vec<MdLine> = match kind {
        TextKind::Markdown => md_lines(text),
        TextKind::Code => text.lines().map(|l| plain(l, Style::Mono)).collect(),
        TextKind::Plain => text.lines().map(|l| plain(l, Style::Plain)).collect(),
        TextKind::Document => text.lines().map(|l| plain(l, Style::Plain)).collect(),
    };
    let mut y = 0;
    let mut lines = Vec::with_capacity(src.len().min(40_000));
    let mut source = src.into_iter().take(40_000).enumerate().peekable();
    while let Some((no, line)) = source.next() {
        let (t, st, runs) = (line.text, line.style, line.runs);
        if kind == TextKind::Document && t.starts_with("| ") {
            let mut rows = Vec::new();
            let parse = |line: &str| -> Vec<String> {
                line.strip_prefix("| ").unwrap_or(line).strip_suffix('|').unwrap_or(line).split(" | ").map(|cell| cell.trim().to_string()).collect()
            };
            rows.push(parse(&t));
            while source.peek().is_some_and(|(_, line)| line.text.starts_with("| ")) {
                if let Some((_, line)) = source.next() {
                    rows.push(parse(&line.text));
                }
            }
            let scale = |v: i32| v * dpi / 96;
            let rows: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|c| super::docparse::grid_cell_shown(&c)).collect()).collect();
            let inset = scale(7);
            // Header is drawn bold, so it is measured bold. The pane width is not an input.
            let widths = grid_column_widths(&rows, |cell, header| unsafe { text_w(dc, cell, if header { f[1] } else { font }) }, scale(36), scale(260), inset * 2 + scale(4));
            let rh = app(|a| a.fh) + scale(SPACE_8);
            let h = rh * rows.len() as i32;
            lines.push(Line { text: String::new(), style: st, y, h, no: no + 1, table: Some((rows, widths)), frags: Vec::new() });
            y += h;
            continue;
        }
        let old = SelectObject(dc, style_font(st, font, f).into());
        let mut tm = TEXTMETRICW::default();
        _ = GetTextMetricsW(dc, &mut tm);
        let lh = tm.tmHeight + tm.tmExternalLeading;
        let (frags, run_h) = if !runs.is_empty() && !matches!(st, Style::Mono | Style::Code | Style::H1 | Style::H2 | Style::H3) {
            let (frags, h) = wrap_runs(dc, width, &runs, font, f, lh);
            (frags, Some(h))
        } else {
            (Vec::new(), None)
        };
        let h = if let Some(h) = run_h {
            h.max(lh)
        } else if matches!(st, Style::Mono | Style::Code) || t.is_empty() {
            lh
        } else {
            let mut w: Vec<u16> = t.encode_utf16().collect();
            let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
            DrawTextW(dc, &mut w, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_EXPANDTABS | DT_NOPREFIX | DT_EDITCONTROL);
            (r.bottom - r.top).max(lh)
        };
        SelectObject(dc, old);
        y += match st {
            Style::H1 | Style::H2 => lh / 2,
            Style::H3 => lh / 3,
            _ => 0,
        };
        lines.push(Line { text: t, style: st, y, h, no: no + 1, table: None, frags });
        y += h + if matches!(st, Style::H1 | Style::H2) { SPACE_4 * dpi / 96 } else { 0 };
    }
    scroll.layout = Some((gen, width, lines, y));
    y
}

pub(crate) fn clamp_scroll(scroll: &mut BodyScroll, total: i32, page: i32) {
    let page = page.max(0);
    scroll.page_h = page;
    scroll.max_y = total.saturating_sub(page).max(0);
    scroll.y = scroll.y.clamp(0, scroll.max_y.max(0));
    // Text, tables and lists keep their own sideways scroll and do not grow this bar.
    (scroll.max_x, scroll.page_w) = (0, 0);
}

unsafe fn wrap_runs(dc: HDC, width: i32, runs: &[Run], font: HFONT, faces: &[HFONT; 6], lh: i32) -> (Vec<Frag>, i32) {
    let width = width.max(1);
    let mut frags = Vec::new();
    let mut x = 0i32;
    let mut dy = 0i32;
    for run in runs {
        for piece in run_pieces(&run.text) {
            let face = run_font(run.bold, run.italic, run.code, font, faces);
            let chunks: Vec<String> = if piece.chars().count() > 1 && text_w(dc, piece, face) > width {
                piece.chars().map(|c| c.to_string()).collect()
            } else {
                vec![piece.to_string()]
            };
            for chunk in chunks {
                if chunk.is_empty() {
                    continue;
                }
                let w = text_w(dc, &chunk, face).max(1);
                if x > 0 && x + w > width && chunk.trim().is_empty() == false {
                    x = 0;
                    dy += lh;
                }
                frags.push(Frag { text: chunk, bold: run.bold, italic: run.italic, code: run.code, x, dy });
                x += w;
            }
        }
    }
    (frags, dy + lh)
}

fn run_pieces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut spaces = None;
    for (i, c) in s.char_indices() {
        let sp = c == ' ' || c == '\t';
        if spaces != Some(sp) {
            if i > start {
                out.push(&s[start..i]);
            }
            start = i;
            spaces = Some(sp);
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

pub(crate) unsafe fn text_w(dc: HDC, s: &str, font: HFONT) -> i32 {
    let w: Vec<u16> = s.chars().take(200).collect::<String>().encode_utf16().collect();
    let old = SelectObject(dc, font.into());
    let mut sz = SIZE::default();
    _ = GetTextExtentPoint32W(dc, &w, &mut sz);
    SelectObject(dc, old);
    sz.cx
}

unsafe fn clip(dc: HDC, r: RECT, draw: impl FnOnce()) {
    let rgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
    SelectClipRgn(dc, Some(rgn));
    draw();
    SelectClipRgn(dc, None);
    _ = DeleteObject(rgn.into());
}

/// `sel` highlights source lines (1-based, inclusive). `accent` `0` draws no highlight.
pub(crate) unsafe fn paint_text(
    scroll: &mut BodyScroll,
    dc: HDC,
    area: RECT,
    gen: usize,
    text: &str,
    kind: TextKind,
    cut: bool,
    font: HFONT,
    dpi: i32,
    pic: Option<&Pix>,
    sel: Option<(usize, usize)>,
    accent: u32,
    faces: &mut FaceCache,
) {
    let s = |v: i32| v * dpi / 96;
    let f = fonts(faces, dpi, font);
    let gutter = if kind == TextKind::Code { s(44) } else { 0 };
    let width = area.right - area.left - gutter - s(SPACE_8);
    let pic_size = pic.map(|p| {
        let k = ((area.right - area.left) as f32 / p.w as f32).min((area.bottom - area.top) as f32 / p.h as f32);
        ((p.w as f32 * k) as i32, (p.h as f32 * k) as i32)
    });
    let offset = pic_size.map_or(0, |(_, h)| h + s(20));
    let total = offset + layout(scroll, dc, gen, width, text, kind, font, &f, dpi) + if cut { s(28) } else { 0 };
    clamp_scroll(scroll, total, area.bottom - area.top);
    if kind == TextKind::Document {
        let full = scroll
            .layout
            .as_ref()
            .map(|(_, _, lines, _)| lines.iter().filter_map(|l| l.table.as_ref()).map(|(_, widths)| widths.iter().sum::<i32>()).max().unwrap_or(0))
            .unwrap_or(0);
        let view_w = (area.right - area.left).max(1);
        scroll.page_w = view_w;
        scroll.max_x = (full - view_w).max(0);
        scroll.x = scroll.x.clamp(0, scroll.max_x);
    } else {
        let wide = scroll
            .layout
            .as_ref()
            .map(|(_, _, lines, _)| {
                lines.iter().filter(|l| matches!(l.style, Style::Mono | Style::Code)).map(|l| unsafe { text_w(dc, &l.text, f[0]) }).max().unwrap_or(0)
            })
            .unwrap_or(0);
        if wide > width {
            scroll.page_w = width.max(1);
            scroll.max_x = wide - width;
            scroll.x = scroll.x.clamp(0, scroll.max_x);
        }
    }
    let (sy, sx) = (scroll.y, scroll.x);
    let mark = sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    clip(dc, area, || {
        if let (Some(p), Some((w, h))) = (pic, pic_size) {
            let x = (area.left + area.right - w) / 2;
            let y = area.top - sy;
            blit_bgra(dc, RECT { left: x, top: y, right: x + w, bottom: y + h }, p.w, p.h, &p.bgra);
        }
        let sy = sy - offset;
        let Some((_, _, lines, _)) = scroll.layout.as_ref() else { return };
        let first = lines.partition_point(|l| l.y + l.h < sy);
        let fh = app(|a| a.fh);
        for l in lines[first..].iter().take_while(|l| l.y - sy < area.bottom - area.top) {
            let top = area.top + l.y - sy;
            if let Some((rows, widths)) = &l.table {
                let rh = fh + s(SPACE_8);
                let first = ((area.top - top).max(0) / rh) as usize;
                let shown = ((area.bottom - area.top) / rh + 2) as usize;
                for (row_no, row) in rows.iter().enumerate().skip(first).take(shown) {
                    let row_top = top + row_no as i32 * rh;
                    let mut x = area.left - sx;
                    for (col, w) in widths.iter().enumerate() {
                        if x + w > area.left && x < area.right {
                            let cell = row.get(col).map(String::as_str).unwrap_or("");
                            let rect = RECT { left: x + s(7), top: row_top, right: x + w - s(7), bottom: row_top + rh };
                            paint_label(dc, rect, cell, if row_no == 0 { f[1] } else { font });
                            fill(dc, RECT { left: x + w - 1, top: row_top, right: x + w, bottom: row_top + rh }, pal().line);
                            fill(dc, RECT { left: x, top: row_top + rh - 1, right: x + w, bottom: row_top + rh }, pal().line);
                        }
                        x += w;
                    }
                    fill(dc, RECT { left: area.left - sx, top: row_top, right: area.left - sx + 1, bottom: row_top + rh }, pal().line);
                }
                let table_width: i32 = widths.iter().sum();
                fill(dc, RECT { left: area.left - sx, top, right: area.left - sx + table_width, bottom: top + 1 }, pal().line);
                continue;
            }
            if mark.is_some_and(|(lo, hi)| accent != 0 && l.no >= lo && l.no <= hi) {
                fill(dc, RECT { left: area.left, top, right: area.right, bottom: top + l.h }, blend(pal().card, accent, 0.22));
            }
            if gutter > 0 {
                let g = RECT { left: area.left, top, right: area.left + gutter - s(10), bottom: top + l.h };
                paint_text_flags(dc, g, &l.no.to_string(), f[0], pal().sub, DT_RIGHT | DT_SINGLELINE);
            }
            let mono = matches!(l.style, Style::Mono | Style::Code);
            if l.style == Style::Code {
                fill(dc, RECT { left: area.left, top, right: area.right, bottom: top + l.h }, pal().pill);
            }
            if !l.frags.is_empty() {
                for frag in &l.frags {
                    let face = run_font(frag.bold, frag.italic, frag.code, font, &f);
                    let ink = if l.style == Style::Quote && !frag.code { pal().sub } else { pal().text };
                    let fw = text_w(dc, &frag.text, face).max(1);
                    let left = area.left + gutter + frag.x;
                    let row_top = top + frag.dy;
                    if frag.code {
                        fill(dc, RECT { left, top: row_top, right: left + fw, bottom: row_top + fh }, blend(pal().pill, pal().line, 0.35));
                    }
                    let r = RECT { left, top: row_top, right: left + fw + s(4), bottom: row_top + fh };
                    paint_text_flags(dc, r, &frag.text, face, ink, DT_SINGLELINE);
                }
                continue;
            }
            let (ink, fnt) = match l.style {
                Style::Quote => (pal().sub, font),
                st => (pal().text, style_font(st, font, &f)),
            };
            let left = area.left + gutter - if mono { sx } else { 0 };
            let r = RECT { left, top, right: if mono { left + 100_000 } else { area.right - s(SPACE_8) }, bottom: top + l.h };
            let flags = if mono { DT_SINGLELINE | DT_EXPANDTABS } else { DT_WORDBREAK | DT_EXPANDTABS | DT_EDITCONTROL };
            paint_text_flags(dc, r, &l.text, fnt, ink, flags);
        }
        if cut {
            let y = area.top + total - offset - s(24) - sy;
            let r = RECT { left: area.left, top: y, right: area.right, bottom: y + s(24) };
            paint_text_flags(dc, r, tr(crate::Key::OnlyFirst512KbIsShown), font, pal().sub, DT_SINGLELINE);
        }
    });
}

/// Column widths in pixels. `measure(text, header)` is the width of one cell; the header row is
/// measured with `header == true` (the bold face). A column is the widest cell plus `pad_px`,
/// at least `min_px` and at most `cap_px`. The view width is not an argument: a narrow pane
/// scrolls sideways instead of squeezing every column into the window.
pub(crate) fn grid_column_widths(rows: &[Vec<String>], mut measure: impl FnMut(&str, bool) -> i32, min_px: i32, cap_px: i32, pad_px: i32) -> Vec<i32> {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let min_px = min_px.max(0);
    let cap_px = cap_px.max(min_px);
    let mut widths = vec![min_px; cols];
    for (k, row) in rows.iter().enumerate() {
        let header = k == 0;
        for (c, cell) in row.iter().enumerate().take(cols) {
            let w = measure(cell, header).saturating_add(pad_px.max(0));
            widths[c] = widths[c].max(w.min(cap_px).max(min_px));
        }
    }
    widths
}

// A grid: column widths from the first rows' text, the first row as a header, scrolled both ways.
pub(crate) unsafe fn paint_table(scroll: &mut BodyScroll, dc: HDC, area: RECT, rows: &[Vec<String>], cut: bool, font: HFONT, dpi: i32, faces: &mut FaceCache) {
    let s = |v: i32| v * dpi / 96;
    let f = fonts(faces, dpi, font);
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        paint_center_meta(dc, area, tr(crate::Key::Empty), font);
        return;
    }
    let measured: Vec<Vec<String>> = rows.iter().take(200).map(|r| r.iter().map(|c| super::docparse::grid_cell_shown(c)).collect()).collect();
    let inset = s(7);
    // Header bold, then the widest cell, capped. A narrow pane scrolls; it does not shrink a column.
    let widths = grid_column_widths(&measured, |cell, header| unsafe { text_w(dc, cell, if header { f[1] } else { font }) }, s(36), s(260), inset * 2 + s(4));
    let num = text_w(dc, &rows.len().to_string(), font) + s(14);
    let rh = app(|a| a.fh) + s(SPACE_8);
    let total = rh * rows.len() as i32 + if cut { rh } else { 0 };
    clamp_scroll(scroll, total, area.bottom - area.top);
    let view_w = (area.right - area.left).max(1);
    let full: i32 = num + widths.iter().sum::<i32>();
    scroll.page_w = view_w;
    scroll.max_x = (full - view_w).max(0);
    scroll.x = scroll.x.clamp(0, scroll.max_x);
    let (sy, sx) = (scroll.y, scroll.x);
    clip(dc, area, || {
        let first = (sy / rh) as usize;
        let shown = ((area.bottom - area.top) / rh + 2) as usize;
        for (k, r) in rows.iter().enumerate().skip(first).take(shown) {
            let top = area.top + k as i32 * rh - sy;
            let row = RECT { left: area.left, top, right: area.right, bottom: top + rh };
            if k == 0 {
                fill(dc, row, pal().pill);
            }
            fill(dc, RECT { top: row.bottom - 1, ..row }, pal().line);
            paint_meta_right(dc, RECT { left: area.left, right: area.left + num - s(6), ..row }, &(k + 1).to_string(), font);
            let mut x = area.left + num - sx;
            for (c, w) in widths.iter().enumerate() {
                if x + w > area.left + num && x < area.right {
                    let cell = super::docparse::grid_cell_shown(r.get(c).map(String::as_str).unwrap_or(""));
                    let cr = RECT { left: x + inset, top, right: x + w - inset, bottom: top + rh };
                    let face = if k == 0 { f[1] } else { font };
                    let right = k > 0 && !cell.is_empty() && cell.replace(',', "").parse::<f64>().is_ok();
                    // The rect is the full column, so a short header is not ellipsized just because
                    // the pane is narrower. The clip keeps the text out of the row-number gutter.
                    let saved = SaveDC(dc);
                    IntersectClipRect(dc, (area.left + num).max(x), top, (x + w).min(area.right), top + rh);
                    if right {
                        text(dc, cr, &cell, face, pal().text, DT_RIGHT);
                    } else {
                        paint_label(dc, cr, &cell, face);
                    }
                    if saved != 0 {
                        let _ = RestoreDC(dc, saved);
                    }
                    fill(dc, RECT { left: x + w - 1, top, right: x + w, bottom: top + rh }, pal().line);
                }
                x += w;
            }
        }
        if cut {
            let top = area.top + rows.len() as i32 * rh - sy;
            let r = RECT { left: area.left + s(6), top, right: area.right, bottom: top + rh };
            paint_meta(dc, r, tr(crate::Key::MoreRowsNotShown), font);
        }
    });
}

pub(crate) unsafe fn paint_list(scroll: &mut BodyScroll, dc: HDC, area: RECT, rows: &[super::preview::ListItem], font: HFONT, dpi: i32) {
    let s = |v: i32| v * dpi / 96;
    let rh = app(|a| a.fh) + s(SPACE_8);
    let top0 = area.top;
    if rows.is_empty() {
        paint_center_meta(dc, area, tr(crate::Key::Empty), font);
        return;
    }
    let page = area.bottom - top0;
    clamp_scroll(scroll, rh * rows.len() as i32, page);
    let sy = scroll.y;
    let dated = rows.iter().any(|r| !r.date.is_empty());
    let dw = if dated { s(132) } else { 0 };
    let sw = s(90);
    clip(dc, RECT { top: top0, ..area }, || {
        for (k, row) in rows.iter().enumerate().skip((sy / rh) as usize).take((page / rh + 2) as usize) {
            let top = top0 + k as i32 * rh - sy;
            let r = RECT { left: area.left, top, right: area.right, bottom: top + rh };
            if k % 2 == 1 {
                fill(dc, RECT { left: r.left - s(SPACE_8), right: r.right + s(SPACE_8), ..r }, blend(pal().card, pal().bg, 0.45));
            }
            let right = r.right - sw - dw - s(SPACE_8);
            paint_label(dc, RECT { right, ..r }, &row.name, font);
            if dated {
                paint_meta_right(dc, RECT { left: r.right - sw - dw, right: r.right - sw, ..r }, &row.date, font);
            }
            paint_meta_right(dc, RECT { left: r.right - sw, ..r }, &row.size, font);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_or_whole_text() {
        let lines = vec![
            Line { text: "one".into(), style: Style::Plain, y: 0, h: 10, no: 1, table: None, frags: Vec::new() },
            Line { text: "two".into(), style: Style::Plain, y: 10, h: 10, no: 2, table: None, frags: Vec::new() },
            Line { text: "three".into(), style: Style::Plain, y: 20, h: 10, no: 3, table: None, frags: Vec::new() },
        ];
        assert_eq!(selected_text("one\ntwo\nthree", &lines, None), "one\ntwo\nthree");
        assert_eq!(selected_text("one\ntwo\nthree", &lines, Some((2, 3))), "two\nthree");
        assert_eq!(selected_text("one\ntwo\nthree", &lines, Some((3, 1))), "one\ntwo\nthree");
        assert_eq!(selected_text("all", &lines, Some((9, 9))), "all");
    }

    #[test]
    fn grid_columns_fit_the_bold_header_and_do_not_shrink_to_the_view() {
        // "Cost" bold is wider than "12.5". "Paint" is wider than "Item". A 40px pane must not change either.
        let rows = vec![vec!["Item".into(), "Cost".into()], vec!["Paint".into(), "12.5".into()]];
        let widths = grid_column_widths(&rows, |text, header| match (text, header) {
            ("Item", true) => 30,
            ("Item", false) => 24,
            ("Paint", _) => 36,
            ("Cost", true) => 32,
            ("Cost", false) => 26,
            ("12.5", _) => 22,
            _ => 0,
        }, 36, 260, 18);
        assert_eq!(widths, vec![54, 50]);
        assert!(widths.iter().sum::<i32>() > 40);
        let long = grid_column_widths(&[vec!["x".repeat(80)]], |text, _| text.len() as i32 * 10, 36, 260, 18);
        assert_eq!(long, vec![260]);
        assert_eq!(grid_column_widths(&[vec![String::new()]], |_, _| 0, 36, 260, 18), vec![36]);
    }

    #[test]
    fn list_and_line_hits() {
        assert_eq!(list_row_at(0, 100, 100, 20, 3), Some(0));
        assert_eq!(list_row_at(20, 100, 110, 20, 3), Some(1));
        assert_eq!(list_row_at(0, 100, 99, 20, 3), None);
        assert_eq!(list_row_at(0, 100, 160, 20, 3), None);
        let lines = vec![Line { text: "a".into(), style: Style::Mono, y: 0, h: 16, no: 1, table: None, frags: Vec::new() }];
        assert_eq!(line_no_at(&lines, 0, 40, 48), Some(1));
        assert_eq!(line_no_at(&lines, 0, 40, 39), None);
    }
}
