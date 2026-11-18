//! Approved toolbar-pack icons, drawn with the same pen as the strip.
//! IconMCP SVGs are a shape reference only. These are not traced outlines and
//! the executable does not embed or render SVG.
//!
//! Coordinates are icon units in the 17-unit box (origin at the centre, +y down).
//! Whole-unit strokes land on the same hinted grid as the strip icons.

use std::f32::consts::PI;

use super::LineIcon::*;

pub(super) fn draw(kind: super::LineIcon, p: &mut super::Pen) {
    match kind {
        NewFile => {
            page(p, -5.0, -6.0, 5.0, 6.0);
            p.cross((0.0, 2.0), 2.0);
        }
        Duplicate => {
            page(p, -6.0, -6.0, 1.0, 1.0);
            p.cut_slab((-3.0, -2.0), (6.0, 6.0), 1.6);
            page(p, -1.0, -1.0, 6.0, 6.0);
        }
        Move => {
            folder_at(p, -6.0, -1.0, 3.0, 6.0);
            p.seg((1.0, -4.0), (5.0, 0.0));
            p.path(&[(2.0, -1.0), (5.0, 0.0), (4.0, -3.0)], 0.0, false);
        }
        Properties => {
            page(p, -5.0, -6.0, 5.0, 6.0);
            p.seg((-2.0, -1.0), (2.0, -1.0));
            p.seg((-2.0, 1.5), (2.0, 1.5));
            p.seg((-2.0, 4.0), (0.5, 4.0));
        }
        Edit => {
            p.path(&[(-5.0, 5.0), (-2.0, 6.0), (6.0, -2.0), (5.0, -5.0), (-3.0, 3.0)], 0.8, true);
            p.seg((-2.0, 2.0), (2.0, -2.0));
            p.path(&[(-5.0, 5.0), (-6.0, 6.0), (-3.0, 5.0)], 0.4, false);
        }
        SaveAs => {
            p.frame((-5.0, -6.0), (5.0, 6.0), 1.3);
            p.seg((-5.0, -2.0), (5.0, -2.0));
            p.frame((-2.0, -5.0), (2.0, -3.0), 0.4);
            p.frame((-3.0, 1.0), (3.0, 5.0), 0.6);
        }
        Print => {
            p.path(&[(-3.0, -1.0), (-3.0, -6.0), (3.0, -6.0), (3.0, -1.0)], 0.8, false);
            p.frame((-6.0, -2.0), (6.0, 3.0), 1.3);
            p.path(&[(-3.0, 3.0), (-3.0, 6.0), (3.0, 6.0), (3.0, 3.0)], 0.6, false);
            p.dot((3.5, 0.0), 0.7);
        }
        Restore => {
            bin(p, 0.0, 0.0);
            p.seg((0.0, 1.0), (0.0, -3.0));
            p.path(&[(-2.0, -1.0), (0.0, -3.0), (2.0, -1.0)], 0.0, false);
        }
        EmptyTrash => {
            bin(p, 0.0, 1.0);
            p.seg((3.0, -6.0), (5.0, -4.0));
            p.seg((5.0, -6.0), (3.0, -4.0));
            p.dot((6.0, -2.0), 0.55);
        }
        Shred => {
            page(p, -5.0, -6.0, 5.0, 2.0);
            p.seg((-3.0, 3.0), (-3.0, 6.0));
            p.seg((-1.0, 3.0), (-1.0, 6.0));
            p.seg((1.0, 3.0), (1.0, 6.0));
            p.seg((3.0, 3.0), (3.0, 6.0));
        }
        Recent => {
            page(p, -6.0, -5.0, 2.0, 6.0);
            p.ring((3.5, 2.5), 2.4);
            p.seg((3.5, 2.5), (3.5, 1.0));
            p.seg((3.5, 2.5), (4.8, 3.2));
        }
        Compare => {
            page(p, -6.0, -6.0, 0.0, 6.0);
            page(p, 0.0, -6.0, 6.0, 6.0);
            p.seg((-4.0, -1.0), (-2.0, -1.0));
            p.seg((2.0, 1.0), (4.0, 1.0));
            p.seg((2.0, 3.0), (4.5, 3.0));
        }
        Hidden => {
            page(p, -5.0, -6.0, 5.0, 6.0);
            p.lens((0.0, 2.0), 3.2, 1.6);
            p.dot((0.0, 2.0), 0.7);
        }
        FileLink => {
            page(p, -6.0, -6.0, 2.0, 6.0);
            p.ring((3.5, -1.0), 1.6);
            p.ring((5.0, 2.5), 1.6);
            p.seg((2.6, 0.0), (4.2, 1.6));
        }
        FindDupes => {
            page(p, -6.0, -6.0, 0.0, 5.0);
            page(p, 0.0, -4.0, 6.0, 6.0);
        }
        OpenFolder => {
            p.path(&[(-6.0, 5.0), (-6.0, -1.0), (6.0, -1.0), (6.0, 5.0)], 1.2, true);
            p.path(&[(-6.0, -1.0), (-4.0, -5.0), (1.0, -5.0), (3.0, -1.0)], 1.0, false);
        }
        FolderTree => {
            p.frame((0.0, -6.0), (6.0, -2.0), 0.8);
            p.frame((0.0, 2.0), (6.0, 6.0), 0.8);
            p.seg((-6.0, -4.0), (-6.0, 4.0));
            p.seg((-6.0, -4.0), (0.0, -4.0));
            p.seg((-6.0, 4.0), (0.0, 4.0));
        }
        Root => {
            folder_at(p, -6.0, -5.0, 6.0, 6.0);
            p.seg((1.0, 0.0), (-2.0, 4.0));
        }
        NetworkFolder => {
            folder_at(p, -6.0, -5.0, 6.0, 6.0);
            p.dot((-2.0, 1.0), 0.8);
            p.dot((2.0, 1.0), 0.8);
            p.dot((0.0, 3.5), 0.8);
            p.seg((-2.0, 1.0), (2.0, 1.0));
            p.seg((0.0, 1.0), (0.0, 3.5));
        }
        SymlinkFolder => {
            folder_at(p, -6.0, -5.0, 6.0, 6.0);
            p.seg((-3.0, 3.0), (1.0, 3.0));
            p.path(&[(0.0, 1.5), (2.5, 3.0), (0.0, 4.5)], 0.0, false);
        }
        FolderSize => {
            folder_at(p, -6.0, -6.0, 6.0, 6.0);
            p.arc((0.0, 3.0), 3.0, PI, PI);
            p.seg((0.0, 3.0), (2.0, 1.0));
        }
        Cut => {
            p.ring((-2.2, 3.2), 1.8);
            p.ring((2.2, 3.2), 1.8);
            p.seg((-1.0, 1.6), (5.0, -5.5));
            p.seg((1.0, 1.6), (-5.0, -5.5));
        }
        Copy => {
            p.frame((-1.0, -6.0), (6.0, 1.0), 1.2);
            p.cut_slab((-6.0, -1.5), (1.5, 6.0), 1.8);
            p.frame((-6.0, -1.0), (1.0, 6.0), 1.2);
        }
        Paste => {
            p.frame((-5.0, -3.0), (5.0, 6.0), 1.3);
            p.frame((-2.0, -6.0), (2.0, -2.0), 0.8);
            p.seg((-2.0, 0.5), (2.0, 0.5));
            p.seg((-2.0, 3.0), (2.0, 3.0));
        }
        ClipHistory => {
            p.frame((-6.0, -3.0), (2.0, 6.0), 1.2);
            p.frame((-3.5, -6.0), (0.0, -2.0), 0.7);
            p.ring((4.0, 3.0), 2.0);
            p.seg((4.0, 3.0), (4.0, 1.8));
            p.seg((4.0, 3.0), (5.2, 3.8));
        }
        View => {
            p.frame((-6.0, -5.0), (-2.0, -1.0), 0.7);
            p.seg((0.0, -3.0), (6.0, -3.0));
            p.frame((-6.0, 1.0), (-2.0, 5.0), 0.7);
            p.seg((0.0, 3.0), (6.0, 3.0));
        }
        ViewList => {
            p.seg((-6.0, -4.0), (6.0, -4.0));
            p.seg((-6.0, 0.0), (6.0, 0.0));
            p.seg((-6.0, 4.0), (6.0, 4.0));
            p.dot((-6.0, -4.0), 0.7);
            p.dot((-6.0, 0.0), 0.7);
            p.dot((-6.0, 4.0), 0.7);
        }
        ViewIcons => {
            for (x, y) in [(-5.0, -5.0), (2.0, -5.0), (-5.0, 2.0), (2.0, 2.0)] {
                p.frame((x, y), (x + 3.0, y + 3.0), 0.6);
            }
        }
        Preview => {
            page(p, -6.0, -6.0, 2.0, 6.0);
            p.seg((-3.0, -1.0), (0.0, -1.0));
            p.seg((-3.0, 1.5), (-0.5, 1.5));
            p.ring((3.5, 1.5), 2.2);
            p.seg((5.1, 3.1), (6.0, 4.5));
        }
        SidebarLeft => {
            p.frame((-6.0, -5.0), (6.0, 5.0), 1.5);
            p.seg((-2.0, -5.0), (-2.0, 5.0));
        }
        SidebarRight => {
            p.frame((-6.0, -5.0), (6.0, 5.0), 1.5);
            p.seg((2.0, -5.0), (2.0, 5.0));
        }
        Fullscreen => {
            p.path(&[(-2.0, -6.0), (-6.0, -6.0), (-6.0, -2.0)], 0.8, false);
            p.path(&[(2.0, -6.0), (6.0, -6.0), (6.0, -2.0)], 0.8, false);
            p.path(&[(-2.0, 6.0), (-6.0, 6.0), (-6.0, 2.0)], 0.8, false);
            p.path(&[(2.0, 6.0), (6.0, 6.0), (6.0, 2.0)], 0.8, false);
        }
        ZoomIn => {
            p.ring((-1.0, -1.0), 4.0);
            p.seg((2.0, 2.0), (6.0, 6.0));
            p.cross((-1.0, -1.0), 1.6);
        }
        ZoomOut => {
            p.ring((-1.0, -1.0), 4.0);
            p.seg((2.0, 2.0), (6.0, 6.0));
            p.seg((-2.6, -1.0), (0.6, -1.0));
        }
        ThemeLight => {
            p.ring((0.0, 0.0), 2.5);
            p.seg((0.0, -4.0), (0.0, -6.0));
            p.seg((0.0, 4.0), (0.0, 6.0));
            p.seg((-4.0, 0.0), (-6.0, 0.0));
            p.seg((4.0, 0.0), (6.0, 0.0));
            p.seg((-3.0, -3.0), (-5.0, -5.0));
            p.seg((3.0, -3.0), (5.0, -5.0));
            p.seg((-3.0, 3.0), (-5.0, 5.0));
            p.seg((3.0, 3.0), (5.0, 5.0));
        }
        ThemeDark => {
            p.ring((-0.4, 0.2), 5.0);
            p.cut_disc((1.8, -0.6), 4.0);
        }
        Refresh => {
            p.arc((0.0, 0.0), 5.0, 0.8, 4.8);
            p.path(&[(1.5, -5.5), (5.0, -4.0), (2.5, -2.5)], 0.0, false);
        }
        Home => {
            p.path(&[(-6.0, 0.0), (0.0, -6.0), (6.0, 0.0)], 0.0, false);
            p.path(&[(-4.0, 0.0), (-4.0, 6.0), (4.0, 6.0), (4.0, 0.0)], 1.0, false);
            p.path(&[(-1.5, 6.0), (-1.5, 2.5), (1.5, 2.5), (1.5, 6.0)], 0.5, false);
        }
        Breadcrumb => {
            p.path(&[(-6.0, -3.0), (-3.0, -3.0), (-1.0, 0.0), (-3.0, 3.0), (-6.0, 3.0)], 0.8, true);
            p.path(&[(-1.0, -3.0), (2.0, -3.0), (4.0, 0.0), (2.0, 3.0), (-1.0, 3.0), (1.0, 0.0)], 0.8, true);
        }
        Drive => {
            p.frame((-6.0, -3.0), (6.0, 2.0), 1.4);
            p.seg((-6.0, 0.0), (6.0, 0.0));
            p.dot((3.5, -1.5), 0.7);
            p.seg((-3.0, 2.0), (-3.0, 5.0));
            p.seg((3.0, 2.0), (3.0, 5.0));
        }
        Eject => {
            p.path(&[(-5.0, 1.0), (0.0, -5.0), (5.0, 1.0)], 0.8, true);
            p.seg((-5.0, 4.0), (5.0, 4.0));
        }
        Server => {
            p.frame((-6.0, -6.0), (6.0, -2.0), 1.2);
            p.frame((-6.0, -1.0), (6.0, 3.0), 1.2);
            p.frame((-6.0, 4.0), (6.0, 6.0), 0.8);
            p.dot((3.5, -4.0), 0.6);
            p.dot((3.5, 1.0), 0.6);
        }
        DiskUsage => {
            p.frame((-6.0, -5.0), (6.0, -1.0), 1.2);
            p.seg((-3.0, -3.0), (1.0, -3.0));
            p.dot((3.5, -3.0), 0.7);
            p.frame((-6.0, 2.0), (6.0, 5.0), 1.0);
            p.fill_round((-5.2, 2.7), (1.0, 4.3), 0.4);
        }
        SelectAll => {
            corners(p);
            p.frame((-2.0, -2.0), (2.0, 2.0), 0.6);
        }
        Invert => {
            p.path(&[(-5.0, -5.0), (4.0, -5.0), (-5.0, 4.0)], 0.8, true);
            p.path(&[(2.0, 6.0), (6.0, 6.0), (6.0, 2.0)], 0.6, false);
            p.seg((1.0, 6.0), (3.0, 6.0));
            p.seg((-5.0, 1.0), (-5.0, 3.0));
        }
        Deselect => {
            corners(p);
            p.seg((-2.0, -2.0), (2.0, 2.0));
            p.seg((2.0, -2.0), (-2.0, 2.0));
        }
        Search => {
            p.ring((-1.0, -1.0), 4.0);
            p.seg((2.0, 2.0), (6.0, 6.0));
        }
        Filter => {
            p.path(&[(-5.0, -6.0), (5.0, -6.0), (1.0, -1.0), (1.0, 5.0), (-1.0, 5.0), (-1.0, -1.0)], 0.9, true);
        }
        FilterClear => {
            p.path(&[(-6.0, -6.0), (2.0, -6.0), (-1.0, -2.0), (-1.0, 2.0), (-3.0, 2.0), (-3.0, -2.0)], 0.8, true);
            p.seg((1.0, 1.0), (5.0, 5.0));
            p.seg((5.0, 1.0), (1.0, 5.0));
        }
        Glob => {
            folder_at(p, -6.0, -5.0, 6.0, 6.0);
            p.seg((0.0, 0.0), (0.0, 4.0));
            p.seg((-1.6, 1.2), (1.6, 2.8));
            p.seg((-1.6, 2.8), (1.6, 1.2));
        }
        SortName => {
            p.seg((-5.0, -5.0), (-5.0, 3.0));
            p.path(&[(-6.5, 2.0), (-5.0, 5.0), (-3.5, 2.0)], 0.0, false);
            p.path(&[(0.0, -1.0), (2.0, -5.0), (4.0, -1.0)], 0.0, false);
            p.seg((0.8, -2.5), (3.2, -2.5));
            p.path(&[(0.0, 1.0), (4.0, 1.0), (0.0, 5.0), (4.0, 5.0)], 0.0, false);
        }
        SortDate => {
            p.frame((-6.0, -3.0), (2.0, 6.0), 1.2);
            p.seg((-6.0, -0.5), (2.0, -0.5));
            p.seg((-4.0, -6.0), (-4.0, -2.0));
            p.seg((0.0, -6.0), (0.0, -2.0));
            p.seg((4.5, -1.0), (4.5, 5.0));
            p.path(&[(2.5, 3.0), (4.5, 5.0), (6.0, 3.0)], 0.0, false);
        }
        SortSize => {
            p.seg((-6.0, -4.0), (0.0, -4.0));
            p.seg((-6.0, 0.0), (-2.0, 0.0));
            p.seg((-6.0, 4.0), (-4.0, 4.0));
            p.seg((3.0, -6.0), (3.0, 3.0));
            p.path(&[(1.0, 2.0), (3.0, 6.0), (5.0, 2.0)], 0.0, false);
        }
        SortAsc => {
            p.seg((-6.0, -4.0), (-2.0, -4.0));
            p.seg((-6.0, 0.0), (0.0, 0.0));
            p.seg((-6.0, 4.0), (2.0, 4.0));
            p.seg((4.0, 5.0), (4.0, -4.0));
            p.path(&[(2.0, -2.0), (4.0, -5.0), (6.0, -2.0)], 0.0, false);
        }
        SortDesc => {
            p.seg((-6.0, -4.0), (2.0, -4.0));
            p.seg((-6.0, 0.0), (0.0, 0.0));
            p.seg((-6.0, 4.0), (-2.0, 4.0));
            p.seg((4.0, -5.0), (4.0, 4.0));
            p.path(&[(2.0, 2.0), (4.0, 5.0), (6.0, 2.0)], 0.0, false);
        }
        NewTab => {
            p.path(&[(-6.0, 5.0), (-4.5, -3.0), (-1.0, -3.0), (0.5, 5.0)], 1.0, true);
            p.seg((0.5, 5.0), (6.0, 5.0));
            p.cross((3.5, 0.0), 1.8);
        }
        Close => {
            p.seg((-4.0, -4.0), (4.0, 4.0));
            p.seg((4.0, -4.0), (-4.0, 4.0));
        }
        SplitSide => {
            p.frame((-6.0, -5.0), (-1.0, 5.0), 1.3);
            p.frame((1.0, -5.0), (6.0, 5.0), 1.3);
        }
        SplitStack => {
            p.frame((-5.0, -6.0), (5.0, -1.0), 1.3);
            p.frame((-5.0, 1.0), (5.0, 6.0), 1.3);
        }
        Important => {
            p.seg((-1.0, -6.0), (-1.0, 6.0));
            p.path(&[(-1.0, -6.0), (5.0, -4.0), (5.0, 0.0), (-1.0, -1.0)], 0.8, true);
        }
        Bookmark => {
            p.path(&[(-4.0, -6.0), (4.0, -6.0), (4.0, 6.0), (0.0, 2.0), (-4.0, 6.0)], 1.2, true);
        }
        Tag => {
            p.path(&[(6.0, -1.0), (1.0, -6.0), (-6.0, -6.0), (-6.0, 1.0), (-1.0, 6.0)], 1.2, true);
            p.dot((-3.0, -3.0), 0.9);
        }
        PinFile => {
            page(p, -5.0, -4.0, 5.0, 6.0);
            p.seg((0.0, -6.0), (0.0, -1.0));
            p.dot((0.0, -6.0), 1.1);
        }
        Compress => {
            page(p, -5.0, -6.0, 5.0, 6.0);
            p.seg((-3.0, -1.0), (0.0, 2.0));
            p.path(&[(-1.0, 0.5), (0.0, 2.0), (0.0, 0.0)], 0.0, false);
            p.seg((3.0, 4.0), (0.0, 1.0));
            p.path(&[(1.0, 2.5), (0.0, 1.0), (0.0, 3.0)], 0.0, false);
        }
        Extract => {
            p.frame((-6.0, -6.0), (6.0, -2.0), 1.0);
            p.path(&[(-5.0, -2.0), (-5.0, 4.0), (5.0, 4.0), (5.0, -2.0)], 1.0, false);
            p.seg((0.0, 3.0), (0.0, -1.0));
            p.path(&[(-2.0, 1.0), (0.0, -1.0), (2.0, 1.0)], 0.0, false);
        }
        Terminal => {
            p.frame((-6.0, -5.0), (6.0, 5.0), 1.5);
            p.path(&[(-3.0, -2.0), (-1.0, 0.0), (-3.0, 2.0)], 0.0, false);
            p.seg((1.0, 2.0), (4.0, 2.0));
        }
        TerminalAdmin => {
            // Terminal one unit up and left. A shield takes the freed lower-right corner, its
            // top one unit above the window's bottom edge.
            let m = moved(p, (-1.0, -1.0));
            p.frame(m((-6.0, -5.0)), m((6.0, 5.0)), 1.5);
            shield(p, m((0.0, 4.0)).1, (7.0, 7.0), 2.0);
            p.path(&[m((-3.0, -2.0)), m((-1.0, 0.0)), m((-3.0, 2.0))], 0.0, false);
            p.seg(m((1.0, 2.0)), m((4.0, 2.0)));
        }
        TabColor => {
            p.path(&[(-6.0, 5.0), (-6.0, -1.0), (-3.0, -5.0), (2.0, -5.0), (5.0, -1.0), (6.0, -1.0), (6.0, 5.0)], 1.1, true);
            p.fill_round((-2.0, -0.4), (2.4, 3.0), 0.45);
        }
        PaneColor => {
            p.frame((-6.0, -6.0), (6.0, 6.0), 1.2);
            p.seg((-6.0, -2.5), (6.0, -2.5));
            // Round body with a point above it.
            p.dot((0.0, 3.0), 1.65);
            p.path(&[(-1.35, 1.7), (0.0, -1.0), (1.35, 1.7)], 0.3, false);
        }
        PaneClose => {
            p.frame((-6.0, -6.0), (6.0, 6.0), 1.15);
            p.seg((-6.0, -2.6), (0.4, -2.6));
            p.seg((1.4, -4.9), (4.0, -2.8));
            p.seg((4.0, -4.9), (1.4, -2.8));
        }
        Help => {
            p.ring((0.0, 0.0), 6.0);
            p.arc((0.0, -1.6), 1.8, 0.3, -2.6);
            p.seg((0.3, 0.4), (0.3, 2.0));
            p.dot((0.3, 3.5), 0.65);
        }
        About => {
            p.ring((0.0, 0.0), 6.0);
            p.dot((0.0, -3.2), 0.75);
            p.seg((0.0, -1.2), (0.0, 3.5));
        }
        Shortcuts => {
            p.frame((-6.0, -6.0), (6.0, 6.0), 1.6);
            p.seg((2.0, -4.0), (-1.0, -0.5));
            p.seg((-1.0, -0.5), (1.5, 0.8));
            p.seg((1.5, 0.8), (-2.0, 4.0));
        }
        Undo => {
            p.seg((-3.0, -2.0), (3.0, -2.0));
            p.arc((3.0, 1.0), 3.0, -PI / 2.0, PI);
            p.seg((3.0, 4.0), (0.0, 4.0));
            p.path(&[(-1.0, -4.5), (-3.0, -2.0), (-1.0, 0.5)], 0.0, false);
        }
        Redo => {
            p.seg((3.0, -2.0), (-3.0, -2.0));
            p.arc((-3.0, 1.0), 3.0, -PI / 2.0, -PI);
            p.seg((-3.0, 4.0), (0.0, 4.0));
            p.path(&[(1.0, -4.5), (3.0, -2.0), (1.0, 0.5)], 0.0, false);
        }
        More => {
            p.dot((-4.0, 0.0), 1.15);
            p.dot((0.0, 0.0), 1.15);
            p.dot((4.0, 0.0), 1.15);
        }
        Lock => {
            p.arc((0.0, -1.0), 3.0, PI, PI);
            p.frame((-5.0, -1.0), (5.0, 6.0), 1.4);
            p.seg((0.0, 1.0), (0.0, 3.0));
        }
        Unlock => {
            p.arc((0.0, -1.0), 3.0, PI, PI * 0.72);
            p.frame((-5.0, -1.0), (5.0, 6.0), 1.4);
            p.seg((0.0, 1.0), (0.0, 3.0));
        }
        Share => {
            p.ring((-3.0, 0.0), 1.7);
            p.ring((3.5, -3.5), 1.7);
            p.ring((3.5, 3.5), 1.7);
            p.seg((-1.5, -0.7), (2.0, -2.8));
            p.seg((-1.5, 0.7), (2.0, 2.8));
        }
        OpenExternal => {
            p.path(&[(-6.0, -2.0), (-6.0, 6.0), (2.0, 6.0), (2.0, 3.0)], 1.0, false);
            p.path(&[(-6.0, -2.0), (-2.0, -2.0)], 0.0, false);
            p.path(&[(0.0, -6.0), (6.0, -6.0), (6.0, 0.0)], 0.8, false);
            p.seg((1.0, -1.0), (6.0, -6.0));
        }
        // The tray at y = 5 is the stroke-rule sample in the catalog tests.
        Download => {
            p.seg((0.0, -6.0), (0.0, 1.0));
            p.path(&[(-4.0, -2.0), (0.0, 2.0), (4.0, -2.0)], 0.0, false);
            p.seg((-5.0, 5.0), (5.0, 5.0));
        }
        Upload => {
            p.seg((0.0, 6.0), (0.0, -1.0));
            p.path(&[(-4.0, 2.0), (0.0, -2.0), (4.0, 2.0)], 0.0, false);
            p.seg((-5.0, -5.0), (5.0, -5.0));
        }
        Sync => {
            p.arc((0.0, 0.0), 4.0, 0.5, 2.3);
            p.path(&[(1.2, -5.0), (4.2, -3.2), (1.2, -2.2)], 0.0, false);
            p.arc((0.0, 0.0), 4.0, PI + 0.5, 2.3);
            p.path(&[(-1.2, 5.0), (-4.2, 3.2), (-1.2, 2.2)], 0.0, false);
        }
        Permissions => {
            p.path(&[(-6.0, -5.0), (-4.5, -3.5), (-2.5, -6.0)], 0.0, false);
            p.seg((-0.5, -4.5), (6.0, -4.5));
            p.path(&[(-6.0, 0.0), (-4.5, 1.5), (-2.5, -1.0)], 0.0, false);
            p.seg((-0.5, 0.0), (6.0, 0.0));
            p.seg((-5.5, 3.0), (-3.0, 5.5));
            p.seg((-3.0, 3.0), (-5.5, 5.5));
            p.seg((-0.5, 4.0), (6.0, 4.0));
        }
        Gear | List | Layout | Add | Group | Up | Down | Rename | Del | Back | Fwd | OpenIn | Save | Load | Basket | Watch | Star | StarFilled | StarAdd | Chevrons => {}
    }
}

/// Folded document. `(x0, y0)` is the top-left, `(x1, y1)` the bottom-right.
fn page(p: &mut super::Pen, x0: f32, y0: f32, x1: f32, y1: f32) {
    let fold = 3.0_f32.min((x1 - x0) * 0.4).min((y1 - y0) * 0.4);
    p.path(&[(x0, y1), (x0, y0), (x1 - fold, y0), (x1, y0 + fold), (x1, y1)], 1.0, true);
    p.path(&[(x1 - fold, y0), (x1 - fold, y0 + fold), (x1, y0 + fold)], 0.4, false);
}

fn folder_at(p: &mut super::Pen, x0: f32, y0: f32, x1: f32, y1: f32) {
    let tab = y0 + 2.0;
    let notch = x0 + (x1 - x0) * 0.42;
    p.path(&[(x0, y1), (x0, tab), (x0 + 1.6, y0), (notch, y0), (notch + 1.2, tab), (x1, tab), (x1, y1)], 1.2, true);
}

/// Lid, handle, and body of a bin. `dy` shifts the whole mark down.
fn bin(p: &mut super::Pen, _dx: f32, dy: f32) {
    p.seg((-4.5, -3.0 + dy), (4.5, -3.0 + dy));
    p.path(&[(-1.5, -3.0 + dy), (-1.5, -5.0 + dy), (1.5, -5.0 + dy), (1.5, -3.0 + dy)], 0.7, false);
    p.path(&[(-3.5, -3.0 + dy), (-2.8, 5.0 + dy), (2.8, 5.0 + dy), (3.5, -3.0 + dy)], 1.0, false);
}

/// Maps a whole-unit point to units the pen hints onto that point's own pixel, moved by `d`
/// whole units. A moved shape keeps the pixels it has in place at every DPI.
fn moved(p: &super::Pen, d: (f32, f32)) -> impl Fn((f32, f32)) -> (f32, f32) {
    let s = p.s;
    let (dx, dy) = ((d.0 * s).round(), (d.1 * s).round());
    move |(x, y)| (((x * s).round() + dx) / s, ((y * s).round() + dy) / s)
}

/// Solid shield badge from `top` down to `corner`, `half` wide each side of its point, with 45
/// degree lower edges. An outline this small has no room for a hole, so it is filled. The width
/// is hinted as one length about the point, so the point stays centred at every DPI, and rounds
/// down so the badge never grows toward the shape it sits on. Everything drawn before it is
/// cleared for one unit around it.
fn shield(p: &mut super::Pen, top: f32, corner: (f32, f32), half: f32) {
    let s = p.s;
    let w = (half * s).floor().max(1.0);
    let (x1, y1, y0) = ((corner.0 * s).round(), (corner.1 * s).round(), (top * s).round());
    let (x0, cx, sh) = (x1 - 2.0 * w, x1 - w, y1 - w);
    let clear = p.hw + p.len(1.0);
    // The pen takes units. A whole pixel divided back by the scale lands on that same pixel.
    let u = |px: f32| px / s;
    p.cut_slab((u(x0 - clear), u(y0 - clear)), (u(x1 + clear), u(y1 + clear)), 0.0);
    p.path(&[(u(x0), u(y0)), (u(x1), u(y0)), (u(x1), u(sh)), (u(cx), u(y1)), (u(x0), u(sh))], 0.5, true);
    p.fill_round((u(x0), u(y0)), (u(x1), u(sh)), 0.5);
    // One stroke per pixel row closes the point. The strokes overlap, so no row stays open.
    for d in 0..w as i32 {
        let d = d as f32;
        p.seg((u(x0 + d), u(sh + d)), (u(x1 - d), u(sh + d)));
    }
}

/// Corner ticks of a selection marquee.
fn corners(p: &mut super::Pen) {
    let (o, t) = (6.0, 2.2);
    p.path(&[(-o, -o + t), (-o, -o), (-o + t, -o)], 0.5, false);
    p.path(&[(o, -o + t), (o, -o), (o - t, -o)], 0.5, false);
    p.path(&[(-o, o - t), (-o, o), (-o + t, o)], 0.5, false);
    p.path(&[(o, o - t), (o, o), (o - t, o)], 0.5, false);
}

#[cfg(test)]
mod tests {
    use super::super::{line_icon_coverage, line_icon_px, LineIcon};

    const DPIS: [i32; 8] = [96, 120, 144, 168, 192, 216, 240, 288];
    const INK: u8 = 128;

    fn cov(dpi: i32, kind: LineIcon) -> (i32, Vec<u8>) {
        let n = line_icon_px(dpi);
        (n, line_icon_coverage(n, dpi, kind))
    }

    /// The shield's rows, top to point, as inclusive runs of solid ink through its axis, and
    /// twice the axis column. Found by walking up from the lowest solid row.
    fn shield_runs(n: i32, c: &[u8]) -> (Vec<(i32, i32, i32)>, i32) {
        let at = |x: i32, y: i32| c[(y * n + x) as usize];
        let run = |y: i32, x: i32| {
            let (mut a, mut b) = (x, x);
            while a > 0 && at(a - 1, y) >= INK {
                a -= 1;
            }
            while b < n - 1 && at(b + 1, y) >= INK {
                b += 1;
            }
            (a, b)
        };
        let tip = (0..n).rev().find(|&y| (0..n).any(|x| at(x, y) >= INK)).unwrap();
        let first = (0..n).find(|&x| at(x, tip) >= INK).unwrap();
        let (a, b) = run(tip, first);
        let axis2 = a + b;
        let mut rows = Vec::new();
        let mut y = tip;
        while y >= 0 && at(axis2 / 2, y) >= INK {
            let (a, b) = run(y, axis2 / 2);
            rows.push((y, a, b));
            y -= 1;
        }
        rows.reverse();
        (rows, axis2)
    }

    #[test]
    fn admin_terminal_is_terminal_moved_up_left_outside_the_badge() {
        for dpi in DPIS {
            let (n, term) = cov(dpi, LineIcon::Terminal);
            let (_, admin) = cov(dpi, LineIcon::TerminalAdmin);
            let (rows, _) = shield_runs(n, &admin);
            let (gap, left) = (rows[0].0 - 1, rows.iter().map(|r| r.1).min().unwrap());
            // One unit up and left, hinted the way the pen hints it.
            let d = (dpi as f32 / 96.0).round() as i32;
            let at = |c: &[u8], x: i32, y: i32| {
                if (0..n).contains(&x) && (0..n).contains(&y) {
                    c[(y * n + x) as usize]
                } else {
                    0
                }
            };
            for y in 0..n {
                for x in 0..n {
                    if x < n / 2 || y < n / 2 || (y < gap && x < left) {
                        let (a, t) = (at(&admin, x, y), at(&term, x + d, y + d));
                        assert!(a.abs_diff(t) <= 1, "{dpi} {x},{y}: {a} vs {t}");
                    }
                }
            }
        }
    }

    #[test]
    fn admin_terminal_shield_is_solid_mirrored_and_clear_of_the_prompt() {
        for dpi in DPIS {
            let (n, c) = cov(dpi, LineIcon::TerminalAdmin);
            let at = |x: i32, y: i32| c[(y * n + x) as usize];
            for k in 0..n {
                for (x, y) in [(k, 0), (0, k), (k, n - 1), (n - 1, k)] {
                    assert!(at(x, y) < 96, "{dpi} edge {x},{y} is {}", at(x, y));
                }
            }
            let (rows, axis2) = shield_runs(n, &c);
            let (top, tip) = (rows[0], rows[rows.len() - 1]);
            let (left, right) = (rows.iter().map(|r| r.1).min().unwrap(), rows.iter().map(|r| r.2).max().unwrap());
            assert!(right - left + 1 >= n / 4, "{dpi}: a badge that reads at toolbar size");
            assert!(tip.0 - top.0 + 2 >= right - left + 1, "{dpi}: not squat");
            let widest = rows.iter().position(|r| r.1 == left).unwrap();
            for w in rows[widest..].windows(2) {
                assert!(w[1].2 - w[1].1 <= w[0].2 - w[0].1, "{dpi}: the sides only close toward the point");
            }
            assert!(tip.2 - tip.1 + 2 <= right - left, "{dpi}: a point under the body");
            for &(y, a, b) in &rows {
                assert_eq!(a + b, axis2, "{dpi} row {y}: mirrored about the point");
                assert!((a..=b).all(|x| at(x, y) >= INK), "{dpi} row {y}: solid");
            }
            // A light row above the badge parts it from the cursor and the window's right side,
            // and a light column to its left parts it from the window's bottom.
            assert!((left..=right).all(|x| at(x, top.0 - 1) < INK), "{dpi}: clear above");
            assert!(rows.iter().all(|&(y, ..)| at(left - 1, y) < INK), "{dpi}: clear to the left");
        }
    }
}
