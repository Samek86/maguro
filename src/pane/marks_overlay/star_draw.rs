//! Maguro "important" star mark.
//!
//! Shape model: a sharp regular five-point star `P`. Every visible layer (halo, rim, body) is
//! `P` grown outward by some distance: the Minkowski sum with a disc, which gives round tips
//! and keeps the valleys sharp. The body can be split into ten bevel facets (two per arm).
//!
//! `render_star_bgra` is the back end the pane overlay uses: a pure-Rust rasteriser with an
//! exact signed distance to `P`, producing premultiplied BGRA for `UpdateLayeredWindow`.
//! The GDI+ `draw_star` adapter from the round-3 prototype is not compiled here and is not called.

use core::f32::consts::PI;

/// Inner/outer radius ratio of the regular star, (3 - sqrt 5) / 2. With it the tops of the two
/// side arms lie on one horizontal line, which the small-size hinting snaps to the pixel grid.
pub const INNER_RATIO: f32 = 0.381_966;

/// Direction the bevel light comes from, in degrees, y pointing down (-90 = straight up).
pub const LIGHT_ANGLE_DEG: f32 = -120.0;

const SIN18: f32 = 0.309_017;
const COS18: f32 = 0.951_057;
const SIN54: f32 = 0.809_017;

pub type Argb = u32;

/// A pixel length that depends on the star size: values at 10, 16 and 32 px, linear in between
/// and clamped outside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp(pub f32, pub f32, pub f32);

impl Ramp {
    pub const fn flat(v: f32) -> Ramp {
        Ramp(v, v, v)
    }

    pub fn at(self, size: f32) -> f32 {
        if size <= 16.0 {
            lerp(self.0, self.1, ((size - 10.0) / 6.0).clamp(0.0, 1.0))
        } else {
            lerp(self.1, self.2, ((size - 16.0) / 16.0).clamp(0.0, 1.0))
        }
    }
}

/// A translucent ring behind the rim: a contrasting halo (`dy == 0`) or a drop shadow.
#[derive(Clone, Copy, Debug)]
pub struct HaloSpec {
    /// Distance beyond the rim's outer edge.
    pub extent: Ramp,
    pub dy: Ramp,
    pub color: Argb,
    /// Drawn only when the star is at least this many pixels.
    pub min_px: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct StarStyle {
    pub inner_ratio: f32,
    /// Tip rounding radius: how far the body is grown out of the sharp star.
    pub round: Ramp,
    /// Rim (outline) width outside the body.
    pub rim: Ramp,
    pub rim_color: Argb,
    /// Halos and shadows, outermost first.
    pub halos: &'static [HaloSpec],
    /// When true the halos fit inside the rect as well; otherwise only rim and body do and the
    /// halos bleed past it (see `StarLook::bleed`).
    pub fit_halos: bool,
    /// Vertical body gradient.
    pub top: Argb,
    pub bottom: Argb,
    /// Bevel facets are drawn from this size up; `f32::INFINITY` disables them.
    pub bevel_min_px: f32,
    /// Translucent overlays for facets facing towards / away from the light.
    pub bevel_lit: Argb,
    pub bevel_shade: Argb,
    /// Small-size hinting applies at or below this size.
    pub hint_max_px: f32,
}

/// Sizes below this get neither halo nor bevel: at 10-12 px they only blur the silhouette.
pub const DETAIL_MIN_PX: f32 = 14.0;

const MAGURO_HALOS: &[HaloSpec] = &[
    // Soft drop shadow: lifts the star off white pages and yellow folders.
    HaloSpec {
        extent: Ramp(0.5, 1.0, 1.5),
        dy: Ramp(0.5, 0.75, 1.25),
        color: 0x4000_0000,
        min_px: DETAIL_MIN_PX,
    },
    // Cream ring outside the dark rim: invisible on white, a crisp edge on dark and busy photos.
    HaloSpec {
        extent: Ramp(0.75, 0.75, 1.0),
        dy: Ramp::flat(0.0),
        color: 0xB3FF_F2C4,
        min_px: DETAIL_MIN_PX,
    },
];

/// The shipped look: bevelled gold star (variant C, tuned; see DESIGN.md).
pub const MAGURO_STAR: StarStyle = StarStyle {
    inner_ratio: INNER_RATIO,
    round: Ramp(0.25, 0.75, 1.25),
    rim: Ramp(1.0, 1.0, 1.5),
    rim_color: 0xFF5A_2D00,
    halos: MAGURO_HALOS,
    fit_halos: false,
    top: 0xFFFF_E46E,
    bottom: 0xFFFB_A300,
    bevel_min_px: DETAIL_MIN_PX,
    bevel_lit: 0x80FF_F8D6,
    bevel_shade: 0x40A0_4800,
    hint_max_px: 12.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Argb),
    /// Linear gradient from `y0` to `y1`, padded beyond both ends.
    Vertical {
        y0: f32,
        y1: f32,
        top: Argb,
        bottom: Argb,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The whole grown star.
    Outline,
    /// Only the bevel facets facing the light.
    LitFacets,
    /// Only the bevel facets facing away from the light.
    ShadeFacets,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub shape: Shape,
    /// How far the shape is grown out of the sharp star.
    pub offset: f32,
    pub dy: f32,
    pub paint: Paint,
}

pub const MAX_LAYERS: usize = 6;

#[derive(Clone, Copy, Debug)]
pub struct Layers {
    items: [Layer; MAX_LAYERS],
    len: usize,
}

impl Layers {
    fn push(&mut self, l: Layer) {
        self.items[self.len] = l;
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[Layer] {
        &self.items[..self.len]
    }
}

/// A style resolved for one rect: final geometry and the ordered list of fills.
#[derive(Clone, Copy, Debug)]
pub struct StarLook {
    pub cx: f32,
    pub cy: f32,
    pub outer: f32,
    pub inner: f32,
    /// Kept with the resolved look. The rasterizer reads `layers`; these are the inputs that built them.
    #[allow(dead_code)]
    pub round: f32,
    #[allow(dead_code)]
    pub rim: f32,
    layers: Layers,
}

impl StarLook {
    pub fn resolve(style: &StarStyle, x: f32, y: f32, w: f32, h: f32) -> StarLook {
        let size = w.min(h);
        let round = style.round.at(size);
        let mut rim = style.rim.at(size);
        let hint = size <= style.hint_max_px;
        if hint {
            rim = rim.round().max(1.0);
        }

        let halos = style.halos.iter().filter(|s| size >= s.min_px);
        let mut extent = round + rim;
        if style.fit_halos {
            let reach = halos
                .clone()
                .map(|s| s.extent.at(size) + s.dy.at(size).abs());
            extent += reach.fold(0.0, f32::max);
        }
        let outer = ((size - 2.0 * extent) / (2.0 * COS18)).max(1.0);

        // Centre the star's bounding box, not its circumcircle: the bottom tips only reach
        // 0.809 R below the centre, so the centre sits slightly below the middle of the rect.
        let cx = x + w * 0.5;
        let mut cy = y + h * 0.5 + outer * (1.0 - SIN54) * 0.5;
        if hint {
            let arm_top = cy - outer * SIN18 - round - rim;
            cy += arm_top.round() - arm_top;
        }

        let mut layers = Layers {
            items: [Layer {
                shape: Shape::Outline,
                offset: 0.0,
                dy: 0.0,
                paint: Paint::Solid(0),
            }; MAX_LAYERS],
            len: 0,
        };
        for s in halos {
            layers.push(Layer {
                shape: Shape::Outline,
                offset: round + rim + s.extent.at(size),
                dy: s.dy.at(size),
                paint: Paint::Solid(s.color),
            });
        }
        layers.push(Layer {
            shape: Shape::Outline,
            offset: round + rim,
            dy: 0.0,
            paint: Paint::Solid(style.rim_color),
        });
        layers.push(Layer {
            shape: Shape::Outline,
            offset: round,
            dy: 0.0,
            // Endpoints sit 1 px outside the body: GDI+ line brushes wrap at their ends, and
            // flip-tiling just beyond them is indistinguishable from padding.
            paint: Paint::Vertical {
                y0: cy - outer - round - 1.0,
                y1: cy + outer * SIN54 + round + 1.0,
                top: style.top,
                bottom: style.bottom,
            },
        });
        if size >= style.bevel_min_px {
            for (shape, color) in [
                (Shape::LitFacets, style.bevel_lit),
                (Shape::ShadeFacets, style.bevel_shade),
            ] {
                layers.push(Layer {
                    shape,
                    offset: round,
                    dy: 0.0,
                    paint: Paint::Solid(color),
                });
            }
        }

        StarLook {
            cx,
            cy,
            outer,
            inner: outer * style.inner_ratio,
            round,
            rim,
            layers,
        }
    }

    pub fn layers(&self) -> &[Layer] {
        self.layers.as_slice()
    }

    /// The sharp star `P`, top tip first, clockwise on screen.
    pub fn vertices(&self) -> [(f32, f32); 10] {
        star_vertices(self.cx, self.cy, self.outer, self.inner)
    }

    /// How far ink reaches beyond the rect `(x, y, w, h)` on any side, in pixels.
    pub fn bleed(&self, x: f32, y: f32, w: f32, h: f32) -> f32 {
        let mut b: f32 = 0.0;
        for l in self.layers() {
            let half_w = self.outer * COS18 + l.offset;
            let top = self.cy + l.dy - self.outer - l.offset;
            let bottom = self.cy + l.dy + self.outer * SIN54 + l.offset;
            b = b
                .max(x - (self.cx - half_w))
                .max(self.cx + half_w - (x + w))
                .max(y - top)
                .max(bottom - (y + h));
        }
        b.max(0.0)
    }

    /// Emits the path of `layer` into `sink`. The pane blits the rasterizer instead.
    #[allow(dead_code)]
    pub fn emit(&self, layer: &Layer, sink: &mut impl PathSink) {
        let v = self.vertices();
        let dy = layer.dy;
        let n = edge_normals(&v);
        let o = layer.offset;
        let v = v.map(|(x, y)| (x, y + dy));
        let tip_arc = |j: usize| -> [(f32, f32); 7] { arc_cubics(v[j], o, n[(j + 9) % 10], n[j]) };
        let valley = |j: usize| -> (f32, f32) { miter(v[j], o, n[(j + 9) % 10], n[j]) };

        match layer.shape {
            Shape::Outline => {
                for t in 0..5 {
                    let j = 2 * t;
                    let a = tip_arc(j);
                    if t == 0 {
                        sink.move_to(a[0].0, a[0].1);
                    } else {
                        sink.line_to(a[0].0, a[0].1);
                    }
                    if o > 1e-3 {
                        sink.cubic_to(a[1], a[2], a[3]);
                        sink.cubic_to(a[4], a[5], a[6]);
                    }
                    let q = valley(j + 1);
                    sink.line_to(q.0, q.1);
                }
                sink.close();
            }
            Shape::LitFacets | Shape::ShadeFacets => {
                let want_lit = layer.shape == Shape::LitFacets;
                let c = (self.cx, self.cy + dy);
                for t in 0..5 {
                    let j = 2 * t;
                    let a = tip_arc(j);
                    // Sector j - 1 runs valley -> tip (left half of the arm), sector j runs
                    // tip -> valley (right half).
                    if facet_tone(n[(j + 9) % 10]) == Some(want_lit) {
                        let q = valley((j + 9) % 10);
                        sink.move_to(c.0, c.1);
                        sink.line_to(q.0, q.1);
                        sink.line_to(a[0].0, a[0].1);
                        if o > 1e-3 {
                            sink.cubic_to(a[1], a[2], a[3]);
                        }
                        sink.close();
                    }
                    if facet_tone(n[j]) == Some(want_lit) {
                        let q = valley(j + 1);
                        sink.move_to(c.0, c.1);
                        sink.line_to(a[3].0, a[3].1);
                        if o > 1e-3 {
                            sink.cubic_to(a[4], a[5], a[6]);
                        }
                        sink.line_to(q.0, q.1);
                        sink.close();
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub trait PathSink {
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), p: (f32, f32));
    fn close(&mut self);
}

/// The ten vertices of a sharp star: outer tips at even indices, top tip first, clockwise on
/// screen (y down).
pub fn star_vertices(cx: f32, cy: f32, outer: f32, inner: f32) -> [(f32, f32); 10] {
    let mut v = [(0.0, 0.0); 10];
    for (j, p) in v.iter_mut().enumerate() {
        let a = (-90.0 + 36.0 * j as f32).to_radians();
        let r = if j % 2 == 0 { outer } else { inner };
        *p = (cx + r * a.cos(), cy + r * a.sin());
    }
    v
}

/// Outward unit normal of edge `j` (vertex `j` to `j + 1`).
fn edge_normals(v: &[(f32, f32); 10]) -> [(f32, f32); 10] {
    let mut n = [(0.0, 0.0); 10];
    for j in 0..10 {
        let (x0, y0) = v[j];
        let (x1, y1) = v[(j + 1) % 10];
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        n[j] = (dy / len, -dx / len);
    }
    n
}

/// `Some(true)` for a facet facing the light, `Some(false)` facing away, `None` in between.
fn facet_tone(normal: (f32, f32)) -> Option<bool> {
    let a = LIGHT_ANGLE_DEG.to_radians();
    let d = normal.0 * a.cos() + normal.1 * a.sin();
    if d > 0.5 {
        Some(true)
    } else if d < -0.3 {
        Some(false)
    } else {
        None
    }
}

#[allow(dead_code)]
fn miter(v: (f32, f32), o: f32, n0: (f32, f32), n1: (f32, f32)) -> (f32, f32) {
    let k = o / (1.0 + n0.0 * n1.0 + n0.1 * n1.1);
    (v.0 + k * (n0.0 + n1.0), v.1 + k * (n0.1 + n1.1))
}

/// Arc of radius `o` around a tip from normal `n0` to `n1`, as two cubics:
/// `[start, c1, c2, mid, c3, c4, end]`.
#[allow(dead_code)]
fn arc_cubics(c: (f32, f32), o: f32, n0: (f32, f32), n1: (f32, f32)) -> [(f32, f32); 7] {
    let a0 = n0.1.atan2(n0.0);
    let mut sweep = n1.1.atan2(n1.0) - a0;
    while sweep <= 0.0 {
        sweep += 2.0 * PI;
    }
    let half = sweep * 0.5;
    let k = 4.0 / 3.0 * (half / 4.0).tan() * o;
    let pt = |a: f32| (c.0 + o * a.cos(), c.1 + o * a.sin());
    let tan = |a: f32| (-a.sin(), a.cos());
    let (a1, a2) = (a0 + half, a0 + sweep);
    let (p0, p1, p2) = (pt(a0), pt(a1), pt(a2));
    let (t0, t1, t2) = (tan(a0), tan(a1), tan(a2));
    [
        p0,
        (p0.0 + k * t0.0, p0.1 + k * t0.1),
        (p1.0 - k * t1.0, p1.1 - k * t1.1),
        p1,
        (p1.0 + k * t1.0, p1.1 + k * t1.1),
        (p2.0 - k * t2.0, p2.1 - k * t2.1),
        p2,
    ]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------------------------
// Software rasteriser
// ---------------------------------------------------------------------------------------------

/// A premultiplied BGRA star image. Blit it with its top-left at `(rect.left - pad,
/// rect.top - pad)`.
#[derive(Clone, Debug)]
pub struct StarBitmap {
    pub width: u32,
    pub height: u32,
    pub pad: u32,
    /// Premultiplied BGRA, top-down, stride `width * 4`.
    pub bgra: Vec<u8>,
}

/// Renders the star for a `w` x `h` rect, padded so that halos which bleed past the rect fit.
pub fn render_star_bgra(style: &StarStyle, w: u32, h: u32) -> StarBitmap {
    let probe = StarLook::resolve(style, 0.0, 0.0, w as f32, h as f32);
    let pad = probe.bleed(0.0, 0.0, w as f32, h as f32).ceil() as u32 + 1;
    let look = StarLook::resolve(style, pad as f32, pad as f32, w as f32, h as f32);
    let (bw, bh) = (w + 2 * pad, h + 2 * pad);
    let mut bgra = vec![0u8; (bw * bh * 4) as usize];
    raster_into(&look, &mut bgra, bw, bh, bw as usize * 4);
    StarBitmap {
        width: bw,
        height: bh,
        pad,
        bgra,
    }
}

/// Composites `look` (in buffer coordinates) over a premultiplied BGRA buffer.
pub fn raster_into(look: &StarLook, bgra: &mut [u8], width: u32, height: u32, stride: usize) {
    let v = look.vertices();
    let n = edge_normals(&v);
    let tones: [Option<bool>; 10] = core::array::from_fn(|j| facet_tone(n[j]));
    let reach = look
        .layers()
        .iter()
        .map(|l| l.offset + l.dy.abs())
        .fold(0.0, f32::max)
        + 1.0;
    let x0 = ((look.cx - look.outer - reach).floor().max(0.0)) as u32;
    let x1 = ((look.cx + look.outer + reach).ceil() as u32).min(width);
    let y0 = ((look.cy - look.outer - reach).floor().max(0.0)) as u32;
    let y1 = ((look.cy + look.outer + reach).ceil() as u32).min(height);

    for py in y0..y1 {
        for px in x0..x1 {
            let (qx, qy) = (px as f32 + 0.5, py as f32 + 0.5);
            let at = py as usize * stride + px as usize * 4;
            let mut dst = [
                bgra[at + 2] as f32 / 255.0,
                bgra[at + 1] as f32 / 255.0,
                bgra[at] as f32 / 255.0,
                bgra[at + 3] as f32 / 255.0,
            ];
            let d0 = signed_distance(&v, qx, qy);
            if d0 > reach {
                continue;
            }
            let mut sub_tones: Option<[Option<bool>; 4]> = None;
            for l in look.layers() {
                let d = if l.dy == 0.0 {
                    d0
                } else {
                    signed_distance(&v, qx, qy - l.dy)
                };
                let mut cov = (l.offset - d + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                if l.shape != Shape::Outline {
                    let sub = sub_tones.get_or_insert_with(|| {
                        [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)]
                            .map(|(sx, sy)| tones[sector(look.cx, look.cy, qx + sx, qy + sy)])
                    });
                    let want = Some(l.shape == Shape::LitFacets);
                    cov *= sub.iter().filter(|&&t| t == want).count() as f32 / 4.0;
                }
                let c = match l.paint {
                    Paint::Solid(c) => c,
                    Paint::Vertical {
                        y0,
                        y1,
                        top,
                        bottom,
                    } => lerp_argb(top, bottom, ((qy - y0) / (y1 - y0)).clamp(0.0, 1.0)),
                };
                let a = ((c >> 24) & 0xFF) as f32 / 255.0 * cov;
                let src = [
                    ((c >> 16) & 0xFF) as f32 / 255.0 * a,
                    ((c >> 8) & 0xFF) as f32 / 255.0 * a,
                    (c & 0xFF) as f32 / 255.0 * a,
                    a,
                ];
                for i in 0..4 {
                    dst[i] = src[i] + dst[i] * (1.0 - a);
                }
            }
            let q = |f: f32| (f * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            bgra[at] = q(dst[2]);
            bgra[at + 1] = q(dst[1]);
            bgra[at + 2] = q(dst[0]);
            bgra[at + 3] = q(dst[3]);
        }
    }
}

/// Which of the ten angular sectors (vertex `j` to `j + 1`) contains the point.
fn sector(cx: f32, cy: f32, x: f32, y: f32) -> usize {
    let a = (y - cy).atan2(x - cx).to_degrees() + 90.0;
    (a.rem_euclid(360.0) / 36.0) as usize % 10
}

fn signed_distance(v: &[(f32, f32); 10], x: f32, y: f32) -> f32 {
    let mut best = f32::MAX;
    let mut inside = false;
    for j in 0..10 {
        let (ax, ay) = v[j];
        let (bx, by) = v[(j + 1) % 10];
        let (ex, ey) = (bx - ax, by - ay);
        let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (dx, dy) = (x - ax - ex * t, y - ay - ey * t);
        best = best.min(dx * dx + dy * dy);
        if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * ex {
            inside = !inside;
        }
    }
    if inside {
        -best.sqrt()
    } else {
        best.sqrt()
    }
}

fn lerp_argb(a: Argb, b: Argb, t: f32) -> Argb {
    let ch = |s: u32| {
        let (ca, cb) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((ca + (cb - ca) * t + 0.5) as u32) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_is_a_nonempty_premultiplied_sprite() {
        for size in [10u32, 21, 32] {
            let bmp = render_star_bgra(&MAGURO_STAR, size, size);
            assert!(bmp.width > size && bmp.height > size, "{size}: {}x{}", bmp.width, bmp.height);
            assert_eq!(bmp.bgra.len(), (bmp.width * bmp.height * 4) as usize);
            assert!(bmp.pad >= 1);
            assert!(bmp.bgra.iter().any(|&b| b != 0), "{size} rendered empty");
            for px in bmp.bgra.chunks_exact(4) {
                assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3], "{size} not premultiplied");
            }
        }
    }
}

