//! Item rectangles, star size, and which rows may take a star.
//! Screen pixels, origin top-left. No window handles.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const EMPTY: Self = Self { x: 0, y: 0, w: 0, h: 0 };

    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    pub fn is_degenerate(self) -> bool {
        self.w < 2 || self.h < 2
    }

    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right() && self.right() > other.x && self.y < other.bottom() && self.bottom() > other.y
    }

    pub fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn from_ltrb(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { x: left, y: top, w: right.saturating_sub(left), h: bottom.saturating_sub(top) }
    }
}

pub(crate) fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

pub(crate) fn file_name(path: &str) -> &str {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Details, list, and small icons stay on the 10 px star. Medium and larger use the icon.
/// `FVM_SMALLICON` is 2, `FVM_LIST` is 3, `FVM_DETAILS` is 4.
pub(crate) fn large_badge(view_mode: i32, icon_size: i32) -> bool {
    if view_mode == 2 || view_mode == 3 || view_mode == 4 {
        return false;
    }
    icon_size > 32
}

pub(crate) fn dpi_scale(dpi: u32) -> f32 {
    (dpi.max(96) as f32) / 96.0
}

/// Star box centred on `icon`. Small views are 10 px at 96 DPI. Larger icons are about 22%
/// of the icon side, clamped to 12–32 px, and both limits scale with DPI.
pub(crate) fn star_rect(icon: IRect, large: bool, dpi: u32) -> IRect {
    let scale = dpi_scale(dpi);
    let size = if !large {
        (10.0 * scale).round().max(8.0) as i32
    } else {
        let side = icon.w.max(icon.h).max(1) as f32;
        let min_px = 12.0 * scale;
        let max_px = 32.0 * scale;
        (side * 0.22).clamp(min_px, max_px).round() as i32
    };
    let size = size.max(4);
    IRect { x: icon.x + (icon.w - size) / 2, y: icon.y + (icon.h - size) / 2, w: size, h: size }
}

/// Side of the star for one view. `rect_side` is the on-screen icon, in pixels.
#[cfg(test)]
pub(crate) fn star_px(view_mode: i32, icon_size: i32, dpi: u32, rect_side: i32) -> i32 {
    let icon = IRect { x: 0, y: 0, w: rect_side.max(1), h: rect_side.max(1) };
    star_rect(icon, large_badge(view_mode, icon_size), dpi).w
}

/// When the icon element is missing, place a square from the item rect and the shell icon size.
pub(crate) fn estimate_icon(item: IRect, view_mode: i32, icon_size: i32, dpi: u32) -> IRect {
    let scale = dpi_scale(dpi);
    let logical = icon_size.max(16) as f32;
    let side = (logical * scale).round().max(8.0) as i32;
    if !large_badge(view_mode, icon_size) {
        let pad = (4.0 * scale).round() as i32;
        let y = item.y + (item.h - side).max(0) / 2;
        IRect { x: item.x + pad, y, w: side, h: side }
    } else {
        let pad = (2.0 * scale).round() as i32;
        let w = side.min(item.w.max(side));
        let h = side.min(item.h.max(side));
        let x = item.x + (item.w - w).max(0) / 2;
        IRect { x, y: item.y + pad, w, h }
    }
}

/// The icon element has to sit on its own row. A recycled UI Automation rect can
/// name one file and carry the box of another.
pub(crate) fn icon_on_row(icon: IRect, item: IRect) -> bool {
    if icon.is_degenerate() {
        return false;
    }
    if item.is_degenerate() {
        return true;
    }
    let cx = icon.x + icon.w / 2;
    let cy = icon.y + icon.h / 2;
    cx >= item.x && cx < item.right() && cy >= item.y && cy < item.bottom()
}

pub(crate) fn usable_icon(icon: IRect, viewport: IRect) -> bool {
    if icon.is_degenerate() {
        return false;
    }
    if icon.x <= -20_000 || icon.y <= -20_000 {
        return false;
    }
    if icon.w > 8_000 || icon.h > 8_000 {
        return false;
    }
    if viewport.is_degenerate() {
        return true;
    }
    // A sliver that only intersects the viewport paints the star on the next row.
    let overlap_h = icon.bottom().min(viewport.bottom()) - icon.y.max(viewport.y);
    let overlap_w = icon.right().min(viewport.right()) - icon.x.max(viewport.x);
    overlap_h * 2 >= icon.h && overlap_w * 2 >= icon.w
}

/// One visible row. `path` is set only after `IFolderView2` resolved it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row<'a> {
    pub display: &'a str,
    pub path: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MarkRef<'a> {
    pub path: &'a str,
    pub name: &'a str,
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn paths_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).replace('/', "\\");
    norm(a).eq_ignore_ascii_case(&norm(b))
}

/// Which visible rows get a star.
/// A shared display name with no resolved path draws nothing: `a.txt` and `a.png` both
/// showing as "a" must not put the star on the wrong file. A path from `IFolderView2`
/// wins. A display that is not the mark's full file name (hidden extension) also draws
/// nothing until a path is supplied.
pub(crate) fn assign_stars(rows: &[Row<'_>], marks: &[MarkRef<'_>]) -> Vec<bool> {
    let mut out = vec![false; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        if row.display.trim().is_empty() {
            continue;
        }
        if let Some(path) = row.path {
            out[i] = !path.is_empty() && marks.iter().any(|m| paths_eq(m.path, path));
            continue;
        }
        let siblings = rows.iter().filter(|o| eq_ci(o.display, row.display)).count();
        if siblings != 1 {
            continue;
        }
        let exact = marks.iter().filter(|m| eq_ci(m.name, row.display)).count();
        out[i] = exact == 1;
    }
    out
}

/// A folder item whose display name is shared, or whose extension is hidden, needs a path.
pub(crate) fn needs_folder_walk(displays: &[&str], mark_names: &[&str]) -> bool {
    for display in displays {
        if display.trim().is_empty() {
            continue;
        }
        let marked = mark_names.iter().any(|n| eq_ci(n, display) || eq_ci(stem(n), display));
        if !marked {
            continue;
        }
        let siblings = displays.iter().filter(|o| eq_ci(o, display)).count();
        if siblings != 1 || !mark_names.iter().any(|n| eq_ci(n, display)) {
            return true;
        }
    }
    false
}

/// One shell item with a screen point (`IFolderView::GetItemPosition` plus the view origin).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placed<'a> {
    pub display: &'a str,
    pub path: &'a str,
    pub x: i32,
    pub y: i32,
}

/// Path of the shell item that sits on this row, when the point match is unique.
pub(crate) fn path_for_rect<'a>(display: &str, rect: IRect, placed: &'a [Placed<'a>]) -> Option<&'a str> {
    let same: Vec<&Placed> = placed.iter().filter(|p| eq_ci(p.display, display) && !p.path.is_empty()).collect();
    if same.is_empty() {
        return None;
    }
    if same.len() == 1 && placed.iter().filter(|p| eq_ci(p.display, display)).count() == 1 {
        return Some(same[0].path);
    }
    let inside: Vec<&Placed> = same.iter().copied().filter(|p| rect.contains_point(p.x, p.y)).collect();
    let had_inside = !inside.is_empty();
    let pool: Vec<&Placed> = if had_inside { inside } else { same };
    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let dist = |p: &Placed| {
        let dx = p.x as i64 - cx as i64;
        let dy = p.y as i64 - cy as i64;
        dx * dx + dy * dy
    };
    let mut best: Option<&Placed> = None;
    let mut second = i64::MAX;
    for p in &pool {
        let d = dist(p);
        if best.is_none() || d < dist(best.unwrap()) {
            second = best.map(dist).unwrap_or(i64::MAX);
            best = Some(p);
        } else if d < second {
            second = d;
        }
    }
    let best = best?;
    let d = dist(best);
    // Two points in the same cell, or a point far from the row, is not a safe match.
    if second != i64::MAX && second.saturating_sub(d) < 64 {
        return None;
    }
    let limit = (rect.w.max(rect.h).max(8) as i64) * 2;
    if !had_inside && d > limit * limit {
        return None;
    }
    Some(best.path)
}

/// A display name that might belong to one of these mark file names.
pub(crate) fn could_match(display: &str, mark_names: &[String]) -> bool {
    let display = display.trim();
    if display.is_empty() {
        return false;
    }
    mark_names.iter().any(|n| eq_ci(n, display) || eq_ci(stem(n), display))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_centred(star: IRect, icon: IRect) {
        assert!((star.x * 2 + star.w - (icon.x * 2 + icon.w)).abs() <= 1);
        assert!((star.y * 2 + star.h - (icon.y * 2 + icon.h)).abs() <= 1);
    }

    #[test]
    fn star_size_follows_view_mode_and_dpi() {
        // Details, list, small icons: 10 px at 96 DPI.
        for mode in [2, 3, 4] {
            assert_eq!(star_px(mode, 16, 96, 16), 10, "mode {mode}");
        }
        // Medium (48) clamps 22% up to 12. Large (96) is 21. Extra large clamps at 32.
        assert_eq!(star_px(1, 48, 96, 48), 12);
        assert_eq!(star_px(1, 96, 96, 96), 21);
        assert_eq!(star_px(5, 256, 96, 256), 32);
        // 192 DPI doubles the 10 px star and the 12–32 clamp.
        assert_eq!(star_px(4, 16, 192, 32), 20);
        assert_eq!(star_px(5, 256, 192, 256), 56);
        let icon = IRect { x: 40, y: 100, w: 16, h: 16 };
        let star = star_rect(icon, false, 96);
        assert_eq!((star.x, star.y, star.w), (43, 103, 10));
        assert_centred(star, icon);
    }

    #[test]
    fn shared_display_name_draws_nothing_until_a_path_resolves() {
        let marks = [MarkRef { path: r"C:\box\a.txt", name: "a.txt" }];
        let hidden = [Row { display: "a", path: None }, Row { display: "a", path: None }];
        assert_eq!(assign_stars(&hidden, &marks), vec![false, false]);
        // The name alone, even with one row, is not the file name. Do not guess.
        let one = [Row { display: "a", path: None }];
        assert_eq!(assign_stars(&one, &marks), vec![false]);
        let resolved = [
            Row { display: "a", path: Some(r"C:\box\a.txt") },
            Row { display: "a", path: Some(r"C:\box\a.png") },
        ];
        assert_eq!(assign_stars(&resolved, &marks), vec![true, false]);
        let shown = [Row { display: "a.txt", path: None }];
        assert_eq!(assign_stars(&shown, &marks), vec![true]);
    }

    #[test]
    fn two_placed_items_pick_the_rect_they_sit_in() {
        let placed = [
            Placed { display: "a", path: r"C:\box\a.txt", x: 30, y: 40 },
            Placed { display: "a", path: r"C:\box\a.png", x: 30, y: 80 },
        ];
        let top = IRect { x: 10, y: 20, w: 40, h: 36 };
        let bottom = IRect { x: 10, y: 64, w: 40, h: 36 };
        assert_eq!(path_for_rect("a", top, &placed), Some(r"C:\box\a.txt"));
        assert_eq!(path_for_rect("a", bottom, &placed), Some(r"C:\box\a.png"));
        // Both points in one rect: do not guess.
        let wide = IRect { x: 0, y: 0, w: 80, h: 120 };
        assert_eq!(path_for_rect("a", wide, &placed), None);
    }

    #[test]
    fn hidden_extension_asks_for_a_folder_walk() {
        assert!(needs_folder_walk(&["a", "a"], &["a.txt"]));
        assert!(needs_folder_walk(&["a"], &["a.txt"]));
        assert!(!needs_folder_walk(&["a.txt", "b.png"], &["a.txt"]));
    }
}
