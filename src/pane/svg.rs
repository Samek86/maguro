//! Rasterize an SVG with Direct2D. The image worker calls this before WIC.
//!
//! Windows on this machine has no WIC SVG decoder, and the `.svg` association
//! (Microsoft Edge) has no thumbnail provider. The drawing does not ask the shell
//! which program owns the extension. A file is SVG when its bytes start like one.
//!
//! `ID2D1DeviceContext5` (Windows 10 1703) is the API that can `CreateSvgDocument`.
//! A WIC render target is only an `ID2D1RenderTarget`, so the context is created
//! on a D3D11 WARP device and the pixels are read back from a CPU bitmap.

use std::mem::size_of;

use windows::core::{w, Interface};
use windows::Win32::Foundation::{HMODULE, S_OK};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_MAPPED_RECT, D2D1_MAP_OPTIONS_READ, D2D1_SVG_ATTRIBUTE_POD_TYPE_LENGTH,
    D2D1_SVG_LENGTH, D2D1_SVG_LENGTH_UNITS_PERCENTAGE, ID2D1Device5, ID2D1DeviceContext5, ID2D1Factory6, ID2D1SvgDocument, ID2D1SvgElement,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::SHCreateMemStream;

use super::peek::Pix;

/// Longest side of a rasterized SVG, in pixels. Larger documents are scaled down to this.
pub(crate) const SVG_MAX_EDGE: i32 = 2048;

const SVG_DEFAULT: f32 = 512.0;
/// How much of the start of the file is inspected for the root `<svg` tag.
const SNIFF_MAX: usize = 256 * 1024;

struct ComGuard {
    uninit: bool,
}

impl ComGuard {
    fn enter() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_FALSE means this thread already has COM. RPC_E_CHANGED_MODE means it
        // was initialized as an STA. Either way the caller owns that init.
        Self { uninit: hr == S_OK }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}

struct SvgSize {
    w: f32,
    h: f32,
    /// The root tag has a `viewBox`, so the graphic can be scaled to the viewport.
    view_box: bool,
}

/// True when `bytes` open as an SVG document. The extension is not consulted.
pub(crate) fn looks_like_svg(bytes: &[u8]) -> bool {
    let Some(text) = decode_head(bytes) else { return false };
    skip_to_root(&text).is_some()
}

/// Draw `bytes` so the long side fills `box_w`×`box_h` without passing [`SVG_MAX_EDGE`].
/// `None` when the bytes are not an SVG or Direct2D rejects them.
pub(crate) fn render_svg(bytes: &[u8], box_w: i32, box_h: i32) -> Option<Pix> {
    if !looks_like_svg(bytes) {
        return None;
    }
    let _com = ComGuard::enter();
    let size = svg_size_of(bytes);
    let (ow, oh) = svg_output_px(size.w, size.h, box_w, box_h);
    unsafe { rasterize(bytes, &size, ow, oh) }
}

fn svg_size_of(bytes: &[u8]) -> SvgSize {
    let Some(text) = decode_head(bytes) else {
        return SvgSize { w: SVG_DEFAULT, h: SVG_DEFAULT, view_box: false };
    };
    let Some(tag) = root_open_tag(&text) else {
        return SvgSize { w: SVG_DEFAULT, h: SVG_DEFAULT, view_box: false };
    };
    let width = attr_value(tag, "width").and_then(parse_svg_length);
    let height = attr_value(tag, "height").and_then(parse_svg_length);
    let view = attr_value(tag, "viewBox").and_then(parse_view_box);
    let (w, h) = document_px(width, height, view);
    SvgSize { w, h, view_box: view.is_some() }
}

/// Pixel size of the document before it is fitted to the preview.
fn document_px(width: Option<f32>, height: Option<f32>, view_box: Option<(f32, f32)>) -> (f32, f32) {
    let view = view_box.filter(|(w, h)| w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0);
    let (w, h) = match (width, height, view) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (None, None, Some((vw, vh))) => (vw, vh),
        (Some(w), None, None) => (w, w),
        (None, Some(h), None) => (h, h),
        (None, None, None) => (SVG_DEFAULT, SVG_DEFAULT),
    };
    if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 { (w, h) } else { (SVG_DEFAULT, SVG_DEFAULT) }
}

/// Output pixels. The long side is the document's long side or the requested box, and never above [`SVG_MAX_EDGE`].
pub(crate) fn svg_output_px(doc_w: f32, doc_h: f32, box_w: i32, box_h: i32) -> (i32, i32) {
    let doc_w = if doc_w.is_finite() && doc_w > 0.0 { doc_w } else { SVG_DEFAULT };
    let doc_h = if doc_h.is_finite() && doc_h > 0.0 { doc_h } else { SVG_DEFAULT };
    let box_long = (box_w.max(1).min(SVG_MAX_EDGE) as f32).max(box_h.max(1).min(SVG_MAX_EDGE) as f32);
    let doc_long = doc_w.max(doc_h);
    let long_out = box_long.max(doc_long).min(SVG_MAX_EDGE as f32);
    let scale = long_out / doc_long;
    let mut ow = (doc_w * scale).round().max(1.0) as i32;
    let mut oh = (doc_h * scale).round().max(1.0) as i32;
    let edge = ow.max(oh);
    if edge > SVG_MAX_EDGE {
        let k = SVG_MAX_EDGE as f32 / edge as f32;
        ow = ((ow as f32) * k).round().max(1.0) as i32;
        oh = ((oh as f32) * k).round().max(1.0) as i32;
    }
    (ow.max(1), oh.max(1))
}

unsafe fn rasterize(bytes: &[u8], size: &SvgSize, ow: i32, oh: i32) -> Option<Pix> {
    let mut d3d_device = None;
    let mut d3d_context = None;
    D3D11CreateDevice(
        None::<&IDXGIAdapter>,
        D3D_DRIVER_TYPE_WARP,
        HMODULE(std::ptr::null_mut()),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut d3d_device),
        None,
        Some(&mut d3d_context),
    )
    .ok()?;
    let d3d_device = d3d_device?;
    let _d3d_context = d3d_context;
    let dxgi: IDXGIDevice = d3d_device.cast().ok()?;
    let factory: ID2D1Factory6 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
    let device: ID2D1Device5 = factory.CreateDevice(&dxgi).ok()?;
    let dc: ID2D1DeviceContext5 = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).ok()?;
    // 96 DPI makes one SVG unit one pixel, which is how the size above was measured.
    dc.SetDpi(96.0, 96.0);
    dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

    let stream = SHCreateMemStream(Some(bytes))?;
    let fill_view = size.view_box;
    let viewport = if fill_view { D2D_SIZE_F { width: ow as f32, height: oh as f32 } } else { D2D_SIZE_F { width: size.w, height: size.h } };
    let doc: ID2D1SvgDocument = dc.CreateSvgDocument(&stream, viewport).ok()?;
    if fill_view {
        if let Ok(root) = doc.GetRoot() {
            // A fixed width would stay at its own pixel size inside a larger viewport.
            // 100% plus the viewBox scales the graphic to the bitmap.
            force_percent(&root);
        }
        doc.SetViewportSize(D2D_SIZE_F { width: ow as f32, height: oh as f32 }).ok()?;
    } else {
        doc.SetViewportSize(D2D_SIZE_F { width: size.w, height: size.h }).ok()?;
    }

    let target = dc.CreateBitmap(D2D_SIZE_U { width: ow as u32, height: oh as u32 }, None, 0, &bitmap_props(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW)).ok()?;
    dc.SetTarget(&target);
    dc.BeginDraw();
    if !fill_view {
        let scale = [ow as f32 / size.w, 0.0f32, 0.0, oh as f32 / size.h, 0.0, 0.0];
        dc.SetTransform(scale.as_ptr().cast());
    }
    // Transparent, so the preview composites it the same way as a PNG.
    dc.Clear(None);
    dc.DrawSvgDocument(&doc);
    dc.EndDraw(None, None).ok()?;

    let cpu = dc
        .CreateBitmap(D2D_SIZE_U { width: ow as u32, height: oh as u32 }, None, 0, &bitmap_props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))
        .ok()?;
    cpu.CopyFromBitmap(None, &target, None).ok()?;
    let mapped = cpu.Map(D2D1_MAP_OPTIONS_READ).ok()?;
    let bgra = copy_mapped(&mapped, ow, oh);
    cpu.Unmap().ok()?;
    let bgra = bgra?;
    Some(Pix::new(ow, oh, bgra))
}

fn force_percent(root: &ID2D1SvgElement) {
    let pct = D2D1_SVG_LENGTH { value: 100.0, units: D2D1_SVG_LENGTH_UNITS_PERCENTAGE };
    let n = size_of::<D2D1_SVG_LENGTH>() as u32;
    unsafe {
        let _ = root.SetAttributeValue2(w!("width"), D2D1_SVG_ATTRIBUTE_POD_TYPE_LENGTH, &pct as *const D2D1_SVG_LENGTH as *const _, n);
        let _ = root.SetAttributeValue2(w!("height"), D2D1_SVG_ATTRIBUTE_POD_TYPE_LENGTH, &pct as *const D2D1_SVG_LENGTH as *const _, n);
    }
}

fn bitmap_props(options: windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        ..Default::default()
    }
}

fn copy_mapped(mapped: &D2D1_MAPPED_RECT, w: i32, h: i32) -> Option<Vec<u8>> {
    let row = (w as usize).checked_mul(4)?;
    let pitch = mapped.pitch as usize;
    if mapped.bits.is_null() || pitch < row || h <= 0 {
        return None;
    }
    let total = pitch.checked_mul(h as usize)?;
    let src = unsafe { std::slice::from_raw_parts(mapped.bits, total) };
    let mut out = vec![0u8; row.checked_mul(h as usize)?];
    for y in 0..h as usize {
        let from = y * pitch;
        out[y * row..(y + 1) * row].copy_from_slice(&src[from..from + row]);
    }
    Some(out)
}

fn decode_head(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let bytes = &bytes[..bytes.len().min(SNIFF_MAX)];
    if let Some(text) = utf16_head(bytes) {
        return Some(text);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let mut n = b.len();
    while n > 0 && std::str::from_utf8(&b[..n]).is_err() {
        n -= 1;
        if b.len() - n > 3 {
            return None;
        }
    }
    std::str::from_utf8(&b[..n]).ok().map(str::to_string)
}

fn utf16_head(bytes: &[u8]) -> Option<String> {
    let (rest, le) = if bytes.starts_with(&[0xFF, 0xFE]) {
        (&bytes[2..], true)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (&bytes[2..], false)
    } else if bytes.len() >= 8 && bytes[0] == b'<' && bytes[1] == 0 && bytes[3] == 0 {
        (bytes, true)
    } else {
        return None;
    };
    let n = rest.len() - rest.len() % 2;
    let units: Vec<u16> = rest[..n]
        .chunks_exact(2)
        .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn skip_to_root(text: &str) -> Option<&str> {
    let mut s = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if s.starts_with("<?") {
            let end = s.find("?>")?;
            s = s[end + 2..].trim_start();
            continue;
        }
        if s.starts_with("<!--") {
            let end = s.find("-->")?;
            s = s[end + 3..].trim_start();
            continue;
        }
        let probe = s.get(..9).unwrap_or(s).to_ascii_lowercase();
        if probe.starts_with("<!doctype") {
            let end = s.find('>')?;
            s = s[end + 1..].trim_start();
            continue;
        }
        break;
    }
    is_svg_tag(s).then_some(s)
}

fn is_svg_tag(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 4 || b[0] != b'<' || !b[1..4].eq_ignore_ascii_case(b"svg") {
        return false;
    }
    match b.get(4) {
        None => true,
        Some(b'>' | b'/') => true,
        Some(c) => c.is_ascii_whitespace(),
    }
}

fn root_open_tag(text: &str) -> Option<&str> {
    let s = skip_to_root(text)?;
    let end = end_of_open_tag(s)?;
    Some(&s[..end])
}

fn end_of_open_tag(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut quote = 0u8;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let b = tag.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] != b'>' && !b[i].is_ascii_whitespace() && b[i] != b'/' {
        i += 1;
    }
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' {
            break;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let key = &tag[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let value = if b[i] == b'"' || b[i] == b'\'' {
            let q = b[i];
            i += 1;
            let vstart = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            let v = &tag[vstart..i];
            if i < b.len() {
                i += 1;
            }
            v
        } else {
            let vstart = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
                i += 1;
            }
            &tag[vstart..i]
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
    }
    None
}

/// An absolute length in CSS pixels at 96 DPI. Percent, em, and ex are not a document size.
fn parse_svg_length(raw: &str) -> Option<f32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let unit_at = s.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(s.len());
    let (num, unit) = s.split_at(unit_at);
    let n: f32 = num.trim().parse().ok()?;
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let px = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "px" => n,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        _ => return None,
    };
    px.is_finite().then_some(px).filter(|v| *v > 0.0)
}

fn parse_view_box(raw: &str) -> Option<(f32, f32)> {
    let nums: Vec<f32> = raw.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).map(|p| p.parse::<f32>().ok()).collect::<Option<_>>()?;
    if nums.len() != 4 {
        return None;
    }
    let (w, h) = (nums[2], nums[3]);
    (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra_at(pix: &Pix, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as usize) * (pix.w as usize) + (x as usize)) * 4;
        pix.bgra[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn svg_size_prefers_width_then_view_box_then_a_default() {
        let both = br#"<svg width="64" height="32" viewBox="0 0 10 10"></svg>"#;
        let size = svg_size_of(both);
        assert_eq!((size.w, size.h), (64.0, 32.0));
        assert!(size.view_box);

        let only = br#"<?xml version="1.0"?>
            <!-- a drawing with no width -->
            <svg viewBox="0, 0, 40, 20"></svg>"#;
        let size = svg_size_of(only);
        assert_eq!((size.w, size.h), (40.0, 20.0));
        assert!(size.view_box);

        let bare = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let size = svg_size_of(bare);
        assert_eq!((size.w, size.h), (512.0, 512.0));
        assert!(!size.view_box);

        let inches = br#"<svg width="2in" height="1in"></svg>"#;
        assert_eq!((svg_size_of(inches).w, svg_size_of(inches).h), (192.0, 96.0));

        let pct = br#"<svg width="100%" height="50%" viewBox="0 0 30 15"></svg>"#;
        assert_eq!((svg_size_of(pct).w, svg_size_of(pct).h), (30.0, 15.0));

        assert_eq!(svg_output_px(4000.0, 2000.0, 100, 80), (2048, 1024));
        assert_eq!(svg_output_px(40.0, 20.0, 80, 80), (80, 40));
        assert_eq!(svg_output_px(40.0, 40.0, 40, 40), (40, 40));
    }

    #[test]
    fn svg_sniff_uses_the_root_tag_not_the_name() {
        assert!(looks_like_svg(br#"<?xml version="1.0"?><svg width="1" height="1"></svg>"#));
        assert!(looks_like_svg(b"\xef\xbb\xbf<svg viewBox=\"0 0 1 1\"/>"));
        assert!(looks_like_svg(b"<!-- note --><svg viewBox=\"0 0 1 1\"/>"));
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n<svg"));
        assert!(!looks_like_svg(b"<html><body><svg></svg></body></html>"));
        assert!(!looks_like_svg(b"just text"));
        let utf16: Vec<u8> = "<svg viewBox=\"0 0 1 1\"/>".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert!(looks_like_svg(&utf16));
    }

    #[test]
    fn svg_red_rect_and_circle_render_at_the_view_box_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">
            <rect x="0" y="0" width="20" height="40" fill="#ff0000" shape-rendering="crispEdges"/>
            <circle cx="30" cy="20" r="8" fill="#0000ff"/>
        </svg>"##;
        let pix = render_svg(svg, 40, 40).expect("viewBox svg renders");
        assert_eq!((pix.w, pix.h), (40, 40));
        assert_eq!(bgra_at(&pix, 8, 20), [0, 0, 255, 255], "left half is red");
        let blue = bgra_at(&pix, 30, 20);
        assert!(blue[0] > 200 && blue[1] < 40 && blue[2] < 40 && blue[3] == 255, "circle center is blue, got {blue:?}");
        let clear = bgra_at(&pix, 38, 2);
        assert!(clear[3] < 16, "outside the shapes stays transparent, got {clear:?}");

        let big = render_svg(svg, 80, 80).expect("scaled viewBox svg renders");
        assert_eq!((big.w, big.h), (80, 80));
        assert_eq!(bgra_at(&big, 16, 40), [0, 0, 255, 255], "scaled rect stays red");
        let scaled_blue = bgra_at(&big, 60, 40);
        assert!(scaled_blue[0] > 200 && scaled_blue[3] == 255, "scaled circle center is blue, got {scaled_blue:?}");
    }

    #[test]
    fn svg_without_a_view_box_scales_its_width_and_height() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
            <rect x="0" y="0" width="20" height="40" fill="#ff0000" shape-rendering="crispEdges"/>
            <circle cx="30" cy="20" r="8" fill="#0000ff"/>
        </svg>"##;
        let pix = render_svg(svg, 80, 80).expect("sized svg renders");
        assert_eq!((pix.w, pix.h), (80, 80));
        assert_eq!(bgra_at(&pix, 30, 40), [0, 0, 255, 255], "the rect covers half the scaled bitmap");
        let blue = bgra_at(&pix, 60, 40);
        assert!(blue[0] > 200 && blue[3] == 255, "circle center scaled from (30,20), got {blue:?}");
    }

    #[test]
    fn malformed_svg_does_not_render() {
        let broken = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"80\"><rect this is not well formed";
        assert!(looks_like_svg(broken));
        assert!(render_svg(broken, 64, 64).is_none());
        // The same shape as samples/broken.svg: a tag that never closes, then trailing text.
        let sample = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"80\">\n  <rect width=\"200\" height=\"80\" fill=\"not-a-color\"/>\n  <text this is not well formed\n</svg>\nbroken on purpose so F3 should show the source\n";
        assert!(looks_like_svg(sample));
        assert!(render_svg(sample, 200, 80).is_none());
    }
}
