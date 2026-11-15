//! One click-through layered window per pane. Stars are blitted into a premultiplied 32bpp DIB.
//! A layered child (Windows 8+) stays under Maguro's popups. If the child cannot be created,
//! an owned popup is used and the caller hides it while a menu, drag, preview, or rename is open.

use std::collections::HashMap;
use std::sync::Once;

use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WS_CHILD,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP, WNDCLASSW,
};

use crate::imports::*;

use super::geom::{IRect, usable_icon};
use super::star_draw::{render_star_bgra, StarBitmap, StarLook, MAGURO_STAR};

const CLASS: PCWSTR = windows::core::w!("MaguroStarLayer");

static REGISTERED: Once = Once::new();

#[derive(Clone, Debug)]
pub(crate) struct DrawnStar {
    pub star: IRect,
}

pub(crate) struct Layer {
    hwnd: HWND,
    /// True when `hwnd` is a child of the items view. False for the owned-popup fallback.
    child: bool,
    mem: HDC,
    bmp: HBITMAP,
    old: HBITMAP,
    bits: *mut u8,
    width: i32,
    height: i32,
    screen: HDC,
    sprites: HashMap<(i32, i32), StarBitmap>,
}

impl Layer {
    pub(crate) fn attach(parent: HWND) -> windows::core::Result<Self> {
        ensure_class();
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None)?.into() };
        let screen = unsafe { GetDC(None) };
        let mem = unsafe { CreateCompatibleDC(Some(screen)) };
        let (hwnd, child) = match create_window(parent, instance, true) {
            Ok(hwnd) => (hwnd, true),
            Err(_) => (create_window(parent, instance, false)?, false),
        };
        Ok(Self {
            hwnd,
            child,
            mem,
            bmp: HBITMAP::default(),
            old: HBITMAP::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
            screen,
            sprites: HashMap::new(),
        })
    }

    pub(crate) fn is_child(&self) -> bool {
        self.child
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub(crate) fn present(&mut self, viewport: IRect, stars: &[DrawnStar]) -> bool {
        if viewport.w < 2 || viewport.h < 2 || stars.is_empty() {
            self.hide();
            return false;
        }
        if !self.ensure_surface(viewport.w, viewport.h) {
            return false;
        }
        self.paint(stars, viewport);
        self.blit_at(viewport)
    }

    pub(crate) fn hide(&self) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
    }

    pub(crate) fn destroy(&mut self) {
        self.hide();
        if !self.hwnd.0.is_null() && unsafe { IsWindow(Some(self.hwnd)).as_bool() } {
            unsafe { let _ = DestroyWindow(self.hwnd); }
        }
        self.hwnd = HWND::default();
        self.release_bitmap();
        unsafe {
            if !self.mem.0.is_null() {
                let _ = DeleteDC(self.mem);
                self.mem = HDC::default();
            }
            if !self.screen.0.is_null() {
                ReleaseDC(None, self.screen);
                self.screen = HDC::default();
            }
        }
        self.sprites.clear();
    }

    fn blit_at(&self, viewport: IRect) -> bool {
        if self.hwnd.0.is_null() || self.bits.is_null() {
            return false;
        }
        let (x, y) = layered_dest(self.child, viewport, parent_client_origin(self.hwnd));
        unsafe {
            let insert = if self.child { Some(HWND_TOP) } else { None };
            let _ = SetWindowPos(self.hwnd, insert, x, y, viewport.w, viewport.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        // `UpdateLayeredWindow` takes screen coordinates for a popup and the parent's
        // client coordinates for a layered child. A screen point on the child shifts
        // every star by the list's screen origin.
        let dst = POINT { x, y };
        let src = POINT { x: 0, y: 0 };
        let size = SIZE { cx: self.width, cy: self.height };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let ok = unsafe {
            UpdateLayeredWindow(self.hwnd, Some(self.screen), Some(&dst), Some(&size), Some(self.mem), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA).is_ok()
        };
        if let Ok(path) = std::env::var("MAGURO_STAR_LOG") {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                use std::io::Write;
                let _ = writeln!(
                    file,
                    "present ok={ok} child={} at {},{} {}x{} dest={},{}",
                    self.child, viewport.x, viewport.y, viewport.w, viewport.h, x, y
                );
            }
        }
        if ok {
            unsafe { let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE); }
        }
        ok
    }

    fn ensure_surface(&mut self, width: i32, height: i32) -> bool {
        if width == self.width && height == self.height && !self.bits.is_null() {
            return true;
        }
        self.release_bitmap();
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = match unsafe { CreateDIBSection(Some(self.mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) } {
            Ok(bmp) => bmp,
            Err(_) => return false,
        };
        let old = unsafe { SelectObject(self.mem, HGDIOBJ(bmp.0)) };
        self.bmp = bmp;
        self.old = HBITMAP(old.0);
        self.bits = bits as *mut u8;
        self.width = width;
        self.height = height;
        true
    }

    fn paint(&mut self, stars: &[DrawnStar], viewport: IRect) {
        let stride = self.width * 4;
        let len = (stride * self.height) as usize;
        if self.bits.is_null() || len == 0 {
            return;
        }
        unsafe { std::ptr::write_bytes(self.bits, 0, len) };
        for star in stars {
            let local = IRect { x: star.star.x - viewport.x, y: star.star.y - viewport.y, w: star.star.w, h: star.star.h };
            if local.w < 2 || local.h < 2 {
                continue;
            }
            let bitmap = self.sprite(local.w, local.h);
            let pad = ink_pad(local.w, local.h).max(bitmap.pad as i32);
            blit_premul(self.bits, self.width, self.height, &bitmap, local.x - pad, local.y - pad);
        }
    }

    fn sprite(&mut self, w: i32, h: i32) -> StarBitmap {
        if let Some(found) = self.sprites.get(&(w, h)) {
            return found.clone();
        }
        let bitmap = render_star_bgra(&MAGURO_STAR, w as u32, h as u32);
        self.sprites.insert((w, h), bitmap.clone());
        bitmap
    }

    fn release_bitmap(&mut self) {
        if self.bmp.0.is_null() {
            return;
        }
        unsafe {
            if !self.old.0.is_null() {
                SelectObject(self.mem, HGDIOBJ(self.old.0));
            }
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
        }
        self.bmp = HBITMAP::default();
        self.old = HBITMAP::default();
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        self.destroy();
    }
}

/// Extra pixels around a star rect. Same formula as `render_star_bgra`: `ceil(bleed) + 1`.
pub(crate) fn ink_pad(w: i32, h: i32) -> i32 {
    if w <= 0 || h <= 0 {
        return 0;
    }
    let look = StarLook::resolve(&MAGURO_STAR, 0.0, 0.0, w as f32, h as f32);
    look.bleed(0.0, 0.0, w as f32, h as f32).ceil() as i32 + 1
}

pub(crate) fn place_stars(icons: &[(IRect, bool)], viewport: IRect, dpi: u32) -> Vec<DrawnStar> {
    let mut out = Vec::new();
    for &(icon, large) in icons {
        if !usable_icon(icon, viewport) {
            continue;
        }
        let star = super::geom::star_rect(icon, large, dpi);
        if star.w < 2 {
            continue;
        }
        out.push(DrawnStar { star });
    }
    out
}

fn parent_client_origin(hwnd: HWND) -> (i32, i32) {
    let parent = unsafe { GetParent(hwnd).unwrap_or_default() };
    if parent.0.is_null() {
        return (0, 0);
    }
    let mut origin = POINT { x: 0, y: 0 };
    unsafe { let _ = ClientToScreen(parent, &mut origin); }
    (origin.x, origin.y)
}

/// Where to put the layered window. A popup uses the viewport's screen origin.
/// A child uses the same point in its parent's client coordinates.
pub(crate) fn layered_dest(child: bool, viewport: IRect, parent_screen: (i32, i32)) -> (i32, i32) {
    if child {
        (viewport.x - parent_screen.0, viewport.y - parent_screen.1)
    } else {
        (viewport.x, viewport.y)
    }
}

fn create_window(parent: HWND, instance: HINSTANCE, child: bool) -> windows::core::Result<HWND> {
    unsafe {
        if child {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
                CLASS,
                windows::core::w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(instance),
                None,
            )
        } else {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS,
                windows::core::w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(instance),
                None,
            )
        }
    }
}

fn ensure_class() {
    REGISTERED.call_once(|| {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else { return };
        let instance: HINSTANCE = module.into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(star_wnd_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..WNDCLASSW::default()
        };
        unsafe { let _ = RegisterClassW(&class); }
    });
}

unsafe extern "system" fn star_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Src-over of an already premultiplied BGRA sprite.
fn blit_premul(dst: *mut u8, dst_w: i32, dst_h: i32, src: &StarBitmap, dx: i32, dy: i32) {
    if dst.is_null() || dst_w <= 0 || dst_h <= 0 {
        return;
    }
    let dst_stride = dst_w as usize * 4;
    let src_stride = src.width as usize * 4;
    let src_px = src.bgra.as_slice();
    let dst_px = unsafe { std::slice::from_raw_parts_mut(dst, dst_stride * dst_h as usize) };
    for sy in 0..src.height as i32 {
        let y = dy + sy;
        if y < 0 || y >= dst_h {
            continue;
        }
        for sx in 0..src.width as i32 {
            let x = dx + sx;
            if x < 0 || x >= dst_w {
                continue;
            }
            let si = sy as usize * src_stride + sx as usize * 4;
            let di = y as usize * dst_stride + x as usize * 4;
            let sa = src_px[si + 3] as u32;
            if sa == 0 {
                continue;
            }
            if sa == 255 || dst_px[di + 3] == 0 {
                dst_px[di..di + 4].copy_from_slice(&src_px[si..si + 4]);
                continue;
            }
            let inv = 255 - sa;
            for c in 0..4 {
                let v = src_px[si + c] as u32 + (dst_px[di + c] as u32 * inv + 127) / 255;
                dst_px[di + c] = v.min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_window_uses_parent_client_origin() {
        let view = IRect { x: 59, y: 149, w: 1000, h: 600 };
        assert_eq!(layered_dest(true, view, (59, 149)), (0, 0));
        assert_eq!(layered_dest(true, view, (40, 80)), (19, 69));
        assert_eq!(layered_dest(false, view, (59, 149)), (59, 149));
    }
}
