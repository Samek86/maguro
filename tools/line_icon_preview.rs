//! Host preview of the save/load toolbar icons. Not part of the Windows build.
//!
//! ```
//! rustc --edition 2021 tools/session_icon_preview.rs -o /tmp/session_icon_preview
//! /tmp/session_icon_preview /opt/cursor/artifacts/session-icons.png
//! ```

#[path = "../src/ui/sicon.rs"]
mod sicon;

use sicon::{SessionIcon, session_icon_coverage, session_icon_px};

const LIGHT_BG: u32 = 0xF3F1EC;
const LIGHT_TEXT: u32 = 0x2A2825;
const LIGHT_SUB: u32 = 0x7C776F;
const DARK_BG: u32 = 0x1C1D20;
const DARK_TEXT: u32 = 0xE6E8EB;
const DARK_SUB: u32 = 0x9AA0A8;
const GREEN: u32 = 0x1FA463;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/opt/cursor/artifacts/session-icons.png".into());
    for n in [16, 20, 24, 32] {
        for kind in [SessionIcon::Save, SessionIcon::Load] {
            let c = session_icon_coverage(n, kind);
            let name = match kind {
                SessionIcon::Save => "save",
                SessionIcon::Load => "load",
            };
            println!("--- {name} {n}px ---");
            for y in 0..n {
                for x in 0..n {
                    let a = c[(y * n + x) as usize];
                    let ch = if a > 200 { '#' } else if a > 40 { '+' } else { '.' };
                    print!("{ch}");
                }
                println!();
            }
        }
    }
    let png = render();
    std::fs::create_dir_all("/opt/cursor/artifacts").ok();
    std::fs::write(&path, &png).unwrap_or_else(|e| panic!("write {path}: {e}"));
    println!("wrote {path} ({} bytes)", png.len());
}

struct Theme {
    name: &'static str,
    bg: u32,
    text: u32,
    sub: u32,
}

fn render() -> Vec<u8> {
    let themes = [
        Theme { name: "Light", bg: LIGHT_BG, text: LIGHT_TEXT, sub: LIGHT_SUB },
        Theme { name: "Dark", bg: DARK_BG, text: DARK_TEXT, sub: DARK_SUB },
    ];
    let dpis = [(96, "100%"), (120, "125%"), (144, "150%"), (192, "200%")];
    let margin = 20;
    let label_h = 16;
    let row_gap = 12;
    let col_gap = 16;
    let btn_gap = 8;
    let states = ["Rest", "Hover", "Disabled", "Dirty"];
    // 1x crops first (the icon itself, plus a 4px mat), then an 8× view of the 100% pair,
    // then the icons sitting in the real 32×24 toolbar buttons.
    let crop_pad = 4;
    let crop_cell = 32 + crop_pad * 2;
    let crop_w = margin * 2 + 64 + dpis.len() as i32 * (crop_cell + col_gap);
    let max_bh = 24 * 192 / 96;
    let cell_w = (32 * 192 / 96) * 2 + btn_gap;
    let btn_w = margin * 2 + 72 + dpis.len() as i32 * (cell_w + col_gap);
    let big = 8;
    let big_cell = 16 * big;
    let big_w = margin * 2 + 64 + 3 * (big_cell + col_gap);
    let sheet_w = crop_w.max(btn_w).max(big_w);
    let crop_h = label_h + themes.len() as i32 * (label_h + 3 * (crop_cell + 8) + 8);
    let big_h = label_h + themes.len() as i32 * (label_h + big_cell + 12);
    let block_h = label_h + states.len() as i32 * (max_bh + row_gap);
    let sheet_h = margin + crop_h + 12 + big_h + 12 + themes.len() as i32 * (block_h + 20) + margin;
    let (w, h) = (sheet_w as u32, sheet_h as u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    fill(&mut rgba, w, 0, 0, w, h, 0xE7E4DE);

    let mut y = margin;
    text(&mut rgba, w, h, margin, y, "1x", 0x2A2825);
    y += label_h;
    for theme in &themes {
        text(&mut rgba, w, h, margin, y, theme.name, theme.text);
        y += label_h;
        for (tag, kind, rgb) in [
            ("save", SessionIcon::Save, theme.sub),
            ("load", SessionIcon::Load, theme.sub),
            ("dirty", SessionIcon::Save, GREEN),
        ] {
            text(&mut rgba, w, h, margin, y + crop_pad + 4, tag, 0x5C574F);
            let mut x = margin + 64;
            for (i, &(dpi, name)) in dpis.iter().enumerate() {
                if tag == "save" {
                    text(&mut rgba, w, h, x, y - label_h, name, 0x5C574F);
                }
                let n = session_icon_px(dpi);
                let ox = x + (crop_cell - n) / 2;
                let oy = y + (crop_cell - n) / 2;
                blit_icon(&mut rgba, w, h, ox, oy, n, 1, theme.bg, rgb, kind);
                let _ = i;
                x += crop_cell + col_gap;
            }
            y += crop_cell + 8;
        }
        y += 8;
    }

    text(&mut rgba, w, h, margin, y, "100% x8", 0x2A2825);
    y += label_h;
    for theme in &themes {
        text(&mut rgba, w, h, margin, y, theme.name, theme.text);
        y += label_h;
        let mut x = margin + 64;
        for (tag, kind, rgb) in [
            ("save", SessionIcon::Save, theme.sub),
            ("load", SessionIcon::Load, theme.sub),
            ("dirty", SessionIcon::Save, GREEN),
        ] {
            text(&mut rgba, w, h, x, y - label_h, tag, 0x5C574F);
            blit_icon(&mut rgba, w, h, x, y, 16, big, theme.bg, rgb, kind);
            x += big_cell + col_gap;
        }
        y += big_cell + 12;
    }

    for theme in &themes {
        text(&mut rgba, w, h, margin, y, theme.name, theme.text);
        y += label_h;
        let mut sy = y;
        for state in states {
            text(&mut rgba, w, h, margin, sy + 4, state, 0x5C574F);
            let mut x = margin + 72;
            for &(dpi, name) in &dpis {
                if sy == y {
                    text(&mut rgba, w, h, x, sy - label_h, name, 0x5C574F);
                }
                let bw = 32 * dpi / 96;
                let bh = 24 * dpi / 96;
                let bg = if state == "Hover" { blend(theme.bg, theme.text, 0.07) } else { theme.bg };
                let ink = match state {
                    "Hover" => theme.text,
                    "Disabled" => blend(theme.bg, theme.sub, 0.45),
                    "Dirty" => GREEN,
                    _ => theme.sub,
                };
                paint_button(&mut rgba, w, h, x, sy, bw, bh, dpi, bg, ink, SessionIcon::Save);
                paint_button(
                    &mut rgba,
                    w,
                    h,
                    x + bw + btn_gap,
                    sy,
                    bw,
                    bh,
                    dpi,
                    bg,
                    if state == "Dirty" { theme.sub } else { ink },
                    SessionIcon::Load,
                );
                x += cell_w + col_gap;
            }
            sy += max_bh + row_gap;
        }
        y = sy + 12;
    }
    encode_png(w, h, &rgba)
}

fn paint_button(buf: &mut [u8], sw: u32, sh: u32, x: i32, y: i32, bw: i32, bh: i32, dpi: i32, bg: u32, ink: u32, kind: SessionIcon) {
    fill(buf, sw, x, y, bw as u32, bh as u32, bg);
    let n = session_icon_px(dpi);
    let cov = session_icon_coverage(n, kind);
    let ox = x + (bw - n) / 2;
    let oy = y + (bh - n) / 2;
    for py in 0..n {
        for px in 0..n {
            let a = cov[(py * n + px) as usize];
            if a == 0 {
                continue;
            }
            put(buf, sw, sh, ox + px, oy + py, blend(bg, ink, a as f32 / 255.0));
        }
    }
}

fn blit_icon(buf: &mut [u8], sw: u32, sh: u32, x: i32, y: i32, n: i32, zoom: i32, bg: u32, ink: u32, kind: SessionIcon) {
    let cov = session_icon_coverage(n, kind);
    let side = n * zoom;
    fill(buf, sw, x, y, side as u32, side as u32, bg);
    for py in 0..n {
        for px in 0..n {
            let a = cov[(py * n + px) as usize];
            let c = blend(bg, ink, a as f32 / 255.0);
            for dy in 0..zoom {
                for dx in 0..zoom {
                    put(buf, sw, sh, x + px * zoom + dx, y + py * zoom + dy, c);
                }
            }
        }
    }
}

fn fill(buf: &mut [u8], sw: u32, x: i32, y: i32, w: u32, h: u32, rgb: u32) {
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            put(buf, sw, u32::MAX, x + col, y + row, rgb);
        }
    }
}

fn put(buf: &mut [u8], sw: u32, sh: u32, x: i32, y: i32, rgb: u32) {
    let sh = if sh == u32::MAX { (buf.len() / (sw as usize * 4)) as u32 } else { sh };
    if x < 0 || y < 0 || x as u32 >= sw || y as u32 >= sh {
        return;
    }
    let i = ((y as u32 * sw + x as u32) * 4) as usize;
    buf[i] = (rgb >> 16) as u8;
    buf[i + 1] = (rgb >> 8) as u8;
    buf[i + 2] = rgb as u8;
    buf[i + 3] = 255;
}

fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |sh: u32| {
        let (x, y) = ((a >> sh & 0xFF) as f32, (b >> sh & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) << sh
    };
    ch(16) | ch(8) | ch(0)
}

fn text(buf: &mut [u8], sw: u32, sh: u32, x: i32, y: i32, s: &str, rgb: u32) {
    let mut cx = x;
    for ch in s.chars() {
        glyph(buf, sw, sh, cx, y, ch, rgb);
        cx += 6;
    }
}

fn glyph(buf: &mut [u8], sw: u32, sh: u32, x: i32, y: i32, ch: char, rgb: u32) {
    let rows = font(ch);
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                put(buf, sw, sh, x + col, y + row as i32, rgb);
            }
        }
    }
}

fn font(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '%' => [0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'i' => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],
        'g' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x11, 0x0E],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        't' => [0x04, 0x04, 0x1F, 0x04, 0x04, 0x04, 0x03],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        'k' => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'v' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        's' => [0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E],
        'l' => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'd' => [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'x' => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],
        'y' => [0x00, 0x00, 0x11, 0x11, 0x0F, 0x01, 0x0E],
        'c' => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x10, 0x0E],
        'm' => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],
        'p' => [0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((w * 4 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        let start = (y * w * 4) as usize;
        raw.extend_from_slice(&rgba[start..start + (w * 4) as usize]);
    }
    let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_src = kind.to_vec();
    crc_src.extend_from_slice(data);
    out.extend(crc32(&crc_src).to_be_bytes());
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(65535);
        let last = i + n >= data.len();
        out.push(if last { 1 } else { 0 });
        out.extend((n as u16).to_le_bytes());
        out.extend((!(n as u16)).to_le_bytes());
        out.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    out.extend(adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}
