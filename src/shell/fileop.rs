use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, IWICBitmapEncoder, IWICBitmapFrameEncode, IWICImagingFactory, WICBitmapEncoderNoCache,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{STREAM_SEEK_END, STREAM_SEEK_SET};
use windows::Win32::System::DataExchange::{GetClipboardSequenceNumber, IsClipboardFormatAvailable};
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::imports::*;
use crate::app::*;
use crate::ui::*;
use crate::pane::*;
use crate::session::*;
use crate::i18n::*;
use crate::{settings, store};

// ---------- background paste ----------

use super::inuse::Pass;

// Copies running on worker threads; the status line says so and skips its disk calls meanwhile.
pub(crate) static COPYING: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Busy;

impl Busy {
    pub(crate) fn new() -> Busy {
        COPYING.fetch_add(1, Ordering::Relaxed);
        Busy
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        COPYING.fetch_sub(1, Ordering::Relaxed);
    }
}

thread_local! {
    // joined after the message loop so closing Maguro doesn't cut a copy short
    pub(crate) static JOBS: RefCell<Vec<std::thread::JoinHandle<()>>> = const { RefCell::new(Vec::new()) };
}

// Ctrl+V in a file list: a held key's repeats are swallowed; the first press starts a copy, or goes
// to the shell when `start` can't take it. True = handled, don't pass it on.
pub(crate) fn paste_key(repeat: bool, start: impl FnOnce() -> bool) -> bool {
    repeat || start()
}

// Some(true): every source sits in `dst` (copy with "- Copy" names), Some(false): none does,
// None: empty, not a file system path or mixed (left to the shell).
pub(crate) fn paste_mode(srcs: &[String], dst: &str) -> Option<bool> {
    let dst = dst.trim_end_matches('\\').to_lowercase();
    let parent = |p: &String| Some(p.trim_end_matches('\\').rsplit_once('\\')?.0.trim_end_matches('\\').to_lowercase());
    let same: Vec<bool> = srcs.iter().map(|p| if is_fs(p) { parent(p).map(|d| d == dst) } else { None }).collect::<Option<_>>()?;
    match (same.iter().all(|&s| s), same.iter().any(|&s| s)) {
        _ if same.is_empty() => None,
        (true, _) => Some(true),
        (false, false) => Some(false),
        _ => None,
    }
}

// Copy or move, like a paste, but a move into the folder the files already sit in is refused.
pub(crate) fn send_mode(srcs: &[String], dst: &str, mv: bool) -> Option<bool> {
    paste_mode(srcs, dst).filter(|&same| !(mv && same))
}

// A send is the whole selection or nothing: never a partial list from a capped or unreadable read.
pub(crate) const SEND_MAX: u32 = 10_000;

#[derive(Debug, PartialEq)]
pub(crate) enum Pick {
    Empty,
    TooMany,
    Unreadable,
}

pub(crate) fn collect_all(n: u32, mut get: impl FnMut(u32) -> Option<String>) -> std::result::Result<Vec<String>, Pick> {
    match n {
        0 => Err(Pick::Empty),
        n if n > SEND_MAX => Err(Pick::TooMany),
        n => (0..n).map(|k| get(k).ok_or(Pick::Unreadable)).collect(),
    }
}

/// Plain text on the clipboard. A folder path is not quoted, so a shell parsing name stays pasteable.
pub(crate) unsafe fn copy_plain(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let main = app(|a| a.main);
    let mut ok = false;
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            _ = GlobalUnlock(h);
            if OpenClipboard(Some(main)).is_ok() {
                ok = EmptyClipboard().is_ok() && SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok();
                _ = CloseClipboard();
            }
        }
        if !ok {
            _ = GlobalFree(Some(h));
        }
    }
    ok
}

/// Selected file or folder paths, or the open folder when nothing is selected. Not quoted.
pub(crate) unsafe fn copy_location(a: usize) {
    let Some(fv) = view_of::<IFolderView2>(a) else { return };
    let path = |it: IShellItem| it.GetDisplayName(SIGDN_FILESYSPATH).ok().map(|p| pwstr(p));
    let mut paths: Vec<String> = Vec::new();
    if let Ok(arr) = fv.GetSelection(false) {
        paths = (0..arr.GetCount().unwrap_or(0)).filter_map(|k| path(arr.GetItemAt(k).ok()?)).collect();
    }
    if fv.ItemCount(SVGIO_SELECTION).unwrap_or(0) == 0 {
        paths = fv.GetFolder::<IShellItem>().ok().and_then(path).into_iter().collect();
    }
    if paths.is_empty() {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    let text: Vec<u16> = paths.join("\r\n").encode_utf16().chain([0]).collect();
    let main = app(|x| x.main);
    let mut ok = false;
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            _ = GlobalUnlock(h);
            if OpenClipboard(Some(main)).is_ok() {
                ok = EmptyClipboard().is_ok() && SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok();
                _ = CloseClipboard();
            }
        }
        if !ok {
            _ = GlobalFree(Some(h));
        }
    }
    if !ok {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    app(|x| x.note = Some((tr(Key::CopiedPath).to_string(), Instant::now() + std::time::Duration::from_secs(3))));
    update_status();
}

// Ctrl+Shift+C in a file list: the selected items' paths (or the folder's), quoted, one per line.
// Virtual items without a file path are skipped. The clipboard is only touched once the text is ready.
pub(crate) unsafe fn copy_paths(a: usize) {
    let Some(fv) = view_of::<IFolderView2>(a) else { return };
    let path = |it: IShellItem| it.GetDisplayName(SIGDN_FILESYSPATH).ok().map(|p| pwstr(p));
    let mut paths: Vec<String> = Vec::new();
    if let Ok(arr) = fv.GetSelection(false) {
        paths = (0..arr.GetCount().unwrap_or(0)).filter_map(|k| path(arr.GetItemAt(k).ok()?)).collect();
    }
    if fv.ItemCount(SVGIO_SELECTION).unwrap_or(0) == 0 {
        paths = fv.GetFolder::<IShellItem>().ok().and_then(path).into_iter().collect();
    }
    if paths.is_empty() {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    let text: Vec<u16> = paths.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join("\r\n").encode_utf16().chain([0]).collect();
    let main = app(|x| x.main);
    let mut ok = false;
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            _ = GlobalUnlock(h);
            if OpenClipboard(Some(main)).is_ok() {
                ok = EmptyClipboard().is_ok() && SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok();
                _ = CloseClipboard();
            }
        }
        if !ok {
            _ = GlobalFree(Some(h)); // after a successful SetClipboardData the system owns it
        }
    }
    if !ok {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    let n = paths.len();
    let note = if n == 1 {
        tr(Key::CopiedPath).to_string()
    } else {
        tf(Key::CopiedPaths, &[("n", &n.to_string())])
    };
    app(|x| x.note = Some((note, Instant::now() + std::time::Duration::from_secs(3))));
    update_status();
}

// Plain file copies and moves (Ctrl+C or Ctrl+X, then Ctrl+V into a folder) run on their own thread,
// so Maguro stays usable. Links, virtual items and anything else stay with the shell.
pub(crate) unsafe fn try_paste(a: usize) -> bool {
    paste_clipboard(a, None, false)
}

/// The context menu's Paste, into `dest` (the clicked folder or the pane's folder).
/// False leaves the paste with the shell.
pub(crate) unsafe fn try_paste_to(a: usize, dest: String) -> bool {
    paste_clipboard(a, Some(dest), false)
}

/// What the clipboard's "Preferred DropEffect" asks for.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum ClipAct {
    Copy,
    Move,
    Shell,
}

/// No effect is a copy. Cut is move alone. Copy (with or without link) is a copy.
/// Anything else (link only, copy and move both) is left with the shell.
pub(crate) fn clip_act(effect: Option<u32>) -> ClipAct {
    match effect {
        None => ClipAct::Copy,
        Some(e) if e & DROPEFFECT_MOVE.0 != 0 && e & DROPEFFECT_COPY.0 == 0 => ClipAct::Move,
        Some(e) if e & DROPEFFECT_MOVE.0 == 0 && e & DROPEFFECT_COPY.0 != 0 => ClipAct::Copy,
        Some(_) => ClipAct::Shell,
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum PastePlan {
    /// Copy on the worker. `true` names the copies "- Copy" (the files already sit there).
    Copy(bool),
    /// Move on the worker.
    Move,
    /// A cut pasted into the folder it came from: Explorer does nothing, and so do we.
    Nothing,
    /// Mixed, virtual or not a file system path: the shell's paste.
    Shell,
}

/// `mode` is `paste_mode(srcs, dst)`.
pub(crate) fn paste_plan(act: ClipAct, mode: Option<bool>) -> PastePlan {
    match (act, mode) {
        (ClipAct::Copy, Some(rename)) => PastePlan::Copy(rename),
        (ClipAct::Move, Some(false)) => PastePlan::Move,
        (ClipAct::Move, Some(true)) => PastePlan::Nothing,
        _ => PastePlan::Shell,
    }
}

/// After a cut is pasted, every source is gone. A cancel or a partial failure leaves some behind.
pub(crate) fn cut_finished(srcs: &[String], exists: impl Fn(&str) -> bool) -> bool {
    !srcs.is_empty() && srcs.iter().all(|s| !exists(s))
}

/// A finished cut-and-paste: tell the source (Explorer clears its cut state on "Paste Succeeded")
/// and empty the clipboard, as Explorer does, so a second paste cannot move nothing.
/// Only when the clipboard still holds the cut that was pasted (`seq`).
unsafe fn finish_cut(seq: u32) {
    if GetClipboardSequenceNumber() != seq {
        return;
    }
    let ole = OleInitialize(None).is_ok();
    if let Ok(obj) = OleGetClipboard() {
        let _ = super::askdrop::set_drop_dword(&obj, w!("Performed DropEffect"), DROPEFFECT_MOVE.0);
        let _ = super::askdrop::set_drop_dword(&obj, w!("Paste Succeeded"), DROPEFFECT_MOVE.0);
    }
    // The source may have cleared it on "Paste Succeeded". Anything newer is someone else's.
    if GetClipboardSequenceNumber() == seq && OpenClipboard(None).is_ok() {
        if GetClipboardSequenceNumber() == seq {
            let _ = EmptyClipboard();
        }
        let _ = CloseClipboard();
    }
    if ole {
        OleUninitialize();
    }
}

/// Context-menu Fast paste. Robocopy even when the fast-copy setting is off, and for a local folder.
pub(crate) unsafe fn try_fast_paste(a: usize, dest: Option<String>) -> bool {
    let ok = paste_clipboard(a, dest, true);
    if !ok {
        let _ = MessageBeep(MB_ICONWARNING);
    }
    ok
}

unsafe fn paste_clipboard(a: usize, dest: Option<String>, force: bool) -> bool {
    let (dst, main) = app(|x| {
        let folder = match &dest {
            Some(d) => d.clone(),
            None => x.s.paths[a].clone(),
        };
        (folder, x.main)
    });
    if crate::pane::recent::path_is_recent(&dst) {
        return false;
    }
    if !is_fs(&dst) || OpenClipboard(Some(main)).is_err() {
        return false;
    }
    let mut srcs = Vec::new();
    if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
        let drop = HDROP(h.0);
        for k in 0..DragQueryFileW(drop, u32::MAX, None) {
            let mut buf = vec![0u16; DragQueryFileW(drop, k, None) as usize + 1];
            let n = DragQueryFileW(drop, k, Some(&mut buf)) as usize;
            srcs.push(String::from_utf16_lossy(&buf[..n]));
        }
    }
    let seq = GetClipboardSequenceNumber();
    let mut effect = None;
    if let Ok(h) = GetClipboardData(RegisterClipboardFormatW(w!("Preferred DropEffect"))) {
        let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
        if !p.is_null() {
            effect = Some(*p);
            _ = GlobalUnlock(HGLOBAL(h.0));
        }
    }
    let has = |f: PCWSTR| IsClipboardFormatAvailable(RegisterClipboardFormatW(f)).is_ok();
    let pic = picture_only(
        !srcs.is_empty(),
        has(w!("Shell IDList Array")),
        has(w!("FileGroupDescriptorW")) || has(w!("FileGroupDescriptor")),
        has(w!("PNG")) || IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() || IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok(),
    )
    .then(|| clip_picture())
    .flatten();
    _ = CloseClipboard();
    if let Some(pic) = pic {
        if force {
            return false;
        }
        return start_picture(pic, dst);
    }
    let act = clip_act(effect);
    if force {
        if act == ClipAct::Move {
            let Some(_) = send_mode(&srcs, &dst, true) else { return false };
            return start_transfer(srcs, dst, false, true, false, true, Some(seq));
        }
        let Some(rename) = paste_mode(&srcs, &dst).filter(|_| act == ClipAct::Copy) else { return false };
        return start_transfer(srcs, dst, rename, false, false, true, None);
    }
    match paste_plan(act, paste_mode(&srcs, &dst)) {
        PastePlan::Copy(rename) => start_copy(srcs, dst, rename, false),
        PastePlan::Move => start_transfer(srcs, dst, false, true, false, false, Some(seq)),
        PastePlan::Nothing => true,
        PastePlan::Shell => false,
    }
}

/// A picture with no real files. Text or HTML beside it (a browser's Copy image) does not matter.
/// Files, virtual files, and text without a picture stay with the shell paste.
pub(crate) fn picture_only(has_hdrop_items: bool, has_idlist: bool, has_fgd: bool, has_picture: bool) -> bool {
    has_picture && !(has_hdrop_items || has_idlist || has_fgd)
}

pub(crate) enum Pic {
    Png(Vec<u8>),
    Dib(Vec<u8>),
}

/// The clipboard's PNG bytes, else CF_DIB, else CF_DIBV5. The clipboard is already open.
unsafe fn clip_picture() -> Option<Pic> {
    let bytes = |fmt: u32| {
        let h = HGLOBAL(GetClipboardData(fmt).ok()?.0);
        let p = GlobalLock(h) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec();
        _ = GlobalUnlock(h);
        Some(v)
    };
    if let Some(mut png) = bytes(RegisterClipboardFormatW(w!("PNG"))).filter(|b| b.starts_with(b"\x89PNG\r\n\x1a\n")) {
        if let Some(i) = png.windows(4).rposition(|w| w == b"IEND").filter(|i| i + 8 <= png.len()) {
            png.truncate(i + 8);
        }
        return Some(Pic::Png(png));
    }
    bytes(CF_DIB.0 as u32).or_else(|| bytes(CF_DIBV5.0 as u32)).map(Pic::Dib)
}

/// A CF_DIB as a .bmp file. None when the header or the palette is cut short.
pub(crate) fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |i: usize| Some(u32::from_le_bytes(dib.get(i..i + 4)?.try_into().ok()?));
    let size = u32_at(0)? as usize;
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let (compression, used) = (u32_at(16)?, u32_at(32)? as usize);
    if size < 40 {
        return None;
    }
    let colors = if used > 0 { used } else if bits <= 8 { 1usize << bits } else { 0 };
    let masks = if size == 40 && compression == 3 { 12 } else { 0 };
    let off = colors.checked_mul(4)?.checked_add(size + masks)?;
    if off >= dib.len() {
        return None;
    }
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&u32::try_from(14 + dib.len()).ok()?.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(14 + off as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

/// `Clipboard image 2026-10-21 153012.png`, with ` (k)` from the second try on.
pub(crate) fn image_name(head: &str, t: &SYSTEMTIME, k: u32) -> String {
    let n = if k >= 2 { format!(" ({k})") } else { String::new() };
    format!("{head} {:04}-{:02}-{:02} {:02}{:02}{:02}{n}.png", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Writes `bytes` under the first free name. Never replaces a file. Returns the name used.
pub(crate) fn save_unique(dir: &Path, name_of: impl Fn(u32) -> String, bytes: &[u8]) -> std::io::Result<String> {
    use std::io::{ErrorKind, Write};
    for k in 1..=999 {
        let name = name_of(k);
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes) {
                    drop(f);
                    _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(name);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(ErrorKind::AlreadyExists.into())
}

/// The status line shows this on its next tick. Set by the worker, taken by `update_status`.
pub(crate) static SAVED: Mutex<Option<String>> = Mutex::new(None);

/// Encodes and saves on a worker. The pane's selection, focus and folder stay as they are.
pub(crate) unsafe fn start_picture(pic: Pic, dst: String) -> bool {
    let head = tr(Key::ClipboardImage).to_string();
    let unreadable = tr(Key::UnreadablePicture).to_string();
    let saved = tr(Key::SavedName).to_string();
    let fail = tr(Key::ClipboardSaveFail).to_string();
    let job = std::thread::Builder::new().name("maguro-image".into()).spawn(move || {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let _ = crate::crash::guard(std::panic::AssertUnwindSafe(|| unsafe {
        let png = match pic {
            Pic::Png(b) => Ok(b),
            Pic::Dib(d) => dib_to_bmp(&d).ok_or_else(|| unreadable.clone()).and_then(|b| to_png(&b).map_err(|e| e.message())),
        };
        let now = GetLocalTime();
        match png.and_then(|b| save_unique(Path::new(&dst), |k| image_name(&head, &now, k), &b).map_err(|e| e.to_string())) {
            Ok(name) => {
                let note = saved.replace("{name}", &name);
                *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
            }
            Err(e) => {
                MessageBoxW(None, &HSTRING::from(format!("{fail}{e}")), w!("Maguro"), MB_ICONWARNING);
            }
        }
        }));
        if com {
            CoUninitialize();
        }
    });
    let Ok(job) = job else { return false };
    keep_job(job);
    true
}

#[repr(C)]
struct EncVtbl {
    unk: [usize; 3],
    initialize: usize,
    get_container: usize,
    get_info: usize,
    set_colors: usize,
    set_palette: usize,
    set_thumb: usize,
    set_preview: usize,
    create_frame: unsafe extern "system" fn(*mut core::ffi::c_void, *mut *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HRESULT,
}

#[repr(C)]
struct FrameVtbl {
    unk: [usize; 3],
    initialize: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> HRESULT,
}

unsafe fn wic_frame(enc: &IWICBitmapEncoder) -> Result<IWICBitmapFrameEncode> {
    use windows::core::Interface;
    let vt = &*(Interface::vtable(enc) as *const _ as *const EncVtbl);
    let mut raw = std::ptr::null_mut();
    let mut bag = std::ptr::null_mut();
    (vt.create_frame)(Interface::as_raw(enc), &mut raw, &mut bag).ok()?;
    if !bag.is_null() {
        // bag points at the object; its first field is the vtable, and Release is slot 2.
        let release = *(*(bag as *const *const usize)).add(2);
        let release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = std::mem::transmute(release);
        release(bag);
    }
    let frame: IWICBitmapFrameEncode = Interface::from_raw(raw);
    let init = (*(Interface::vtable(&frame) as *const _ as *const FrameVtbl)).initialize;
    init(Interface::as_raw(&frame), std::ptr::null_mut()).ok()?;
    Ok(frame)
}

unsafe fn to_png(bmp: &[u8]) -> Result<Vec<u8>> {
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let input = SHCreateMemStream(Some(bmp)).ok_or_else(|| Error::from(E_OUTOFMEMORY))?;
    let src = wic.CreateDecoderFromStream(&input, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?.GetFrame(0)?;
    let out = SHCreateMemStream(None).ok_or_else(|| Error::from(E_OUTOFMEMORY))?;
    let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
    enc.Initialize(&out, WICBitmapEncoderNoCache)?;
    // CreateNewFrame's options bag is IPropertyBag2. Passing null avoids the StructuredStorage
    // feature, which is large enough that a release build of it is killed on a small machine.
    let frame = wic_frame(&enc)?;
    frame.WriteSource(&src, std::ptr::null())?;
    frame.Commit()?;
    enc.Commit()?;
    let mut len = 0u64;
    out.Seek(0, STREAM_SEEK_END, Some(&mut len))?;
    out.Seek(0, STREAM_SEEK_SET, None)?;
    let mut png = vec![0u8; len as usize];
    let mut read = 0u32;
    out.Read(png.as_mut_ptr().cast(), len as u32, Some(&mut read)).ok()?;
    png.truncate(read as usize);
    Ok(png)
}

fn keep_job(job: std::thread::JoinHandle<()>) {
    JOBS.with_borrow_mut(|j| {
        j.retain(|j| !j.is_finished());
        j.push(job);
    });
}

// Copies (or, with `mv`, moves) `srcs` into `dst` on a worker thread. False = the thread didn't start.
pub(crate) unsafe fn start_copy(srcs: Vec<String>, dst: String, rename: bool, mv: bool) -> bool {
    start_transfer(srcs, dst, rename, mv, false, false, None)
}

/// Move started from a drop. `MoveItem` runs only after the drag source can release the
/// file. Doing it inside `Drop` reports the item missing and leaves it in place.
pub(crate) unsafe fn start_drop_move(srcs: Vec<String>, dst: String) -> bool {
    start_transfer(srcs, dst, false, true, true, false, None)
}

/// `cut` is the clipboard sequence number of a pasted cut. When every source is gone afterwards,
/// the clipboard is told and emptied (see `finish_cut`).
unsafe fn start_transfer(srcs: Vec<String>, dst: String, rename: bool, mv: bool, after_drop: bool, force: bool, cut: Option<u32>) -> bool {
    let (owner, accent, setting) = app(|a| (a.main.0 as isize, a.s.accent_now(), a.s.fast_copy));
    // Captured here, on the UI thread. The worker realizes its own copies if FastCopy runs.
    let snap = unsafe { crate::dialog_snap() };
    let busy = Busy::new();
    // The worker must not call `app()`. The compare is posted when this drops, after
    // PerformOperations returns, including cancel and a partial failure.
    let hwnds = super::listsync::list_hwnds();
    let folders = super::listsync::transfer_drift(&srcs, &dst, mv);
    let job = std::thread::Builder::new().spawn(move || {
        let later = super::listsync::RefreshLater::new(hwnds, folders);
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let originals = if cut.is_some() { srcs.clone() } else { Vec::new() };
        let _ = crate::crash::guard(std::panic::AssertUnwindSafe(|| unsafe {
        if after_drop {
            settle_drop_move(&srcs);
        }
        // Network is only asked when the setting is on and this is not Fast paste.
        // `share_path` can call GetDriveTypeW, which must stay off the UI thread.
        let network = !force && setting && super::fastcopy::network_transfer(&srcs, &dst);
        if super::fastcopy::decide_fast(force, setting, network) {
            let srcs: Vec<String> = srcs.iter().map(|s| final_dos(s)).collect();
            let dst = final_dos(&dst);
            // `busy` stays raised through the copy. run_fast drops it before the completion linger.
            super::fastcopy::run_fast(super::fastcopy::FastJob { owner, accent, snap, srcs, dst, rename, mv, busy });
            return;
        }
        let _busy = busy;
        let owner_hwnd = HWND(owner as *mut _);
        let prepared: Vec<String> = srcs.iter().map(|s| final_dos(s)).collect();
        let prepared_dst = final_dos(&dst);
        let kind = if mv { super::inuse::OpKind::Move } else { super::inuse::OpKind::Copy };
        let fallback = transfer_fallback(&prepared, &prepared_dst);
        // A drop may still be holding the file after the first settle. Try once quietly,
        // then ask which program has it if the move is still blocked.
        let result = if after_drop && mv {
            let first = copy_once(&prepared, &prepared_dst, rename, true, Pass::Quiet, owner_hwnd);
            if first.result.is_ok() && first.locked.is_empty() && first.denied().is_empty() {
                Ok(())
            } else {
                settle_drop_move(&srcs);
                super::inuse::recover_with(owner_hwnd, kind, &fallback, |pass, only| copy_once(&pick(&prepared, only), &prepared_dst, rename, true, pass, owner_hwnd))
            }
        } else {
            super::inuse::recover_with(owner_hwnd, kind, &fallback, |pass, only| copy_once(&pick(&prepared, only), &prepared_dst, rename, mv, pass, owner_hwnd))
        };
        if let Err(e) = result {
            if !super::inuse::is_cancel(&e) {
                let what = if mv { tr(crate::Key::MovingFailed) } else { tr(crate::Key::CopyingFailed) };
                let text = HSTRING::from(format!("{what}{}", e.message()));
                MessageBoxW(None, &text, w!("Maguro"), MB_ICONWARNING);
            }
        }
        }));
        if let Some(seq) = cut {
            if mv && cut_finished(&originals, |p| std::fs::symlink_metadata(p).is_ok()) {
                let _ = crate::crash::guard(std::panic::AssertUnwindSafe(|| unsafe { finish_cut(seq) }));
            }
        }
        drop(later);
        if com {
            CoUninitialize();
        }
    });
    let Ok(job) = job else { return false };
    keep_job(job);
    true
}

/// `GetFinalPathNameByHandle` returns `\\?\C:\...` or `\\?\UNC\server\share\...`.
/// The copy engine wants the DOS form. A device path is left unchanged.
pub(crate) fn dos_from_final(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// One path for a subst drive and for the folder it points at. A favourite stored as
/// `G:\Docs\a.txt` and a pane on `C:\WORK\pgreadme\data\Docs\a.txt` are the same file.
unsafe fn final_dos(path: &str) -> String {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFinalPathNameByHandleW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        VOLUME_NAME_DOS,
    };
    let opened = crate::winpath::win_long_path(path);
    let Ok(handle) = CreateFileW(
        &HSTRING::from(&opened),
        FILE_READ_ATTRIBUTES.0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    ) else {
        return path.to_string();
    };
    let mut buf = [0u16; 1024];
    let n = GetFinalPathNameByHandleW(handle, &mut buf, VOLUME_NAME_DOS) as usize;
    let _ = CloseHandle(handle);
    // The count does not include the terminating NUL. Stop at a NUL anyway, so a
    // count that does include it cannot leave a trailing zero in the path.
    if n == 0 || n >= buf.len() {
        return path.to_string();
    }
    let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
    let wide = String::from_utf16_lossy(&buf[..end]);
    let dos = dos_from_final(&wide);
    if dos.len() >= 2 && (dos.as_bytes()[1] == b':' || dos.starts_with(r"\\")) {
        dos
    } else {
        path.to_string()
    }
}

/// True while some other handle has the file open without `FILE_SHARE_DELETE`.
/// A drag from the folder view holds the item that way until `Drop` returns.
unsafe fn drag_still_holds(path: &str) -> bool {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let opened = crate::winpath::win_long_path(path);
    match CreateFileW(
        &HSTRING::from(&opened),
        DELETE.0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    ) {
        Ok(handle) => {
            let _ = CloseHandle(handle);
            false
        }
        Err(e) => e.code() == ERROR_SHARING_VIOLATION.to_hresult(),
    }
}

/// Let `Drop` return, and let the drag source close its handle, before `MoveItem`.
fn settle_drop_move(srcs: &[String]) {
    let start = Instant::now();
    let min = std::time::Duration::from_millis(500);
    let max = std::time::Duration::from_millis(5000);
    loop {
        let held = srcs.iter().any(|p| unsafe { drag_still_holds(p) });
        let elapsed = start.elapsed();
        if !held && elapsed >= min {
            return;
        }
        if elapsed >= max {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

// Paths go through `final_dos` first, so a subst drive and its target folder are one place.
// The progress window stays (FOF_SILENT is off). FOF_NOERRORUI suppresses the shell's
// "File In Use" box so Maguro can name the program and offer Retry.
// The worker calls `copy_once` itself. These wrappers exist so the drop-move test can
// exercise that path without a window, and they stay out of the release binary.
#[cfg(test)]
pub(crate) unsafe fn run_copy(srcs: &[String], dst: &str, rename: bool, mv: bool) -> Result<()> {
    run_copy_at(srcs, dst, rename, mv, HWND::default())
}

#[cfg(test)]
unsafe fn run_copy_at(srcs: &[String], dst: &str, rename: bool, mv: bool, owner: HWND) -> Result<()> {
    let srcs: Vec<String> = srcs.iter().map(|s| final_dos(s)).collect();
    let dst = final_dos(dst);
    let fallback = transfer_fallback(&srcs, &dst);
    let kind = if mv { super::inuse::OpKind::Move } else { super::inuse::OpKind::Copy };
    super::inuse::recover_with(owner, kind, &fallback, |pass, only| copy_once(&pick(&srcs, only), &dst, rename, mv, pass, owner))
}

/// The sources for one pass: all of them first, then only the ones that need elevation.
fn pick(srcs: &[String], only: &[String]) -> Vec<String> {
    if only.is_empty() { srcs.to_vec() } else { super::inuse::retry_targets(srcs, only) }
}

/// Copy or move flags. The quiet pass adds `FOF_NOERRORUI`; the elevated pass swaps it for
/// `FOFX_SHOWELEVATIONPROMPT`. `confirmed` = conflicts were already answered (Fast copy).
pub(crate) fn copy_flags(rename: bool, confirmed: bool, pass: Pass) -> FILEOPERATION_FLAGS {
    let mut base = FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | FOFX_ADDUNDORECORD;
    if rename {
        base = base | FOF_RENAMEONCOLLISION;
    }
    if confirmed {
        base = base | FOF_NOCONFIRMATION;
    }
    super::inuse::pass_flags(base, pass)
}

/// Delete flags. `shell_confirms` = a permanent delete nobody confirmed yet, so the shell
/// asks. The elevated pass never asks again: the first pass already got the answer.
pub(crate) fn delete_flags(permanent: bool, shell_confirms: bool, pass: Pass) -> FILEOPERATION_FLAGS {
    let mut base = FOF_NOCONFIRMMKDIR;
    if !shell_confirms || pass == Pass::Elevated {
        base = base | FOF_NOCONFIRMATION;
    }
    if !permanent {
        base = base | FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
    }
    super::inuse::pass_flags(base, pass)
}

pub(crate) fn rename_flags(pass: Pass) -> FILEOPERATION_FLAGS {
    super::inuse::pass_flags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | FOF_NOCONFIRMATION | FOFX_ADDUNDORECORD, pass)
}

/// The elevated pass shows Windows' own prompts. An owner keeps them in front of Maguro.
unsafe fn own_elevated(op: &IFileOperation, pass: Pass, owner: HWND) -> Result<()> {
    if pass == Pass::Elevated && !owner.0.is_null() {
        op.SetOwnerWindow(owner)?;
    }
    Ok(())
}

/// Fast copy hit access denied. Copy or move `items` (source, destination name) into `dst`
/// once, elevated, with Windows' prompt. Conflicts were already answered in the Fast copy window.
pub(crate) unsafe fn elevated_transfer(items: &[(String, String)], dst: &str, mv: bool, owner: HWND) -> Result<()> {
    let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
    op.SetOperationFlags(copy_flags(false, true, Pass::Elevated))?;
    own_elevated(&op, Pass::Elevated, owner)?;
    let to: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(dst), None)?;
    for (src, name) in items {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(src.as_str()), None)?;
        let same = Path::new(src).file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name));
        let wide = HSTRING::from(name.as_str());
        let new_name = if same || name.is_empty() { PCWSTR::null() } else { PCWSTR(wide.as_ptr()) };
        if mv { op.MoveItem(&item, &to, new_name, None)? } else { op.CopyItem(&item, &to, new_name, None)? }
    }
    let attempt = super::inuse::perform_watched(&op);
    attempt.result?;
    if op.GetAnyOperationsAborted().unwrap_or(BOOL(0)).as_bool() {
        return Err(Error::from(COPYENGINE_E_USER_CANCELLED));
    }
    Ok(())
}

/// Fast move copied these but could not remove the sources (access denied). Delete them
/// elevated, without another confirmation: the move already decided it.
pub(crate) unsafe fn elevated_remove(srcs: &[String], owner: HWND) -> Result<()> {
    delete_once(srcs, true, 0, Pass::Elevated, owner).result
}

fn transfer_fallback(srcs: &[String], dst: &str) -> Vec<String> {
    let mut out = srcs.to_vec();
    if !dst.is_empty() {
        out.push(dst.to_string());
    }
    for src in srcs {
        let Some(name) = Path::new(src).file_name() else { continue };
        let name = name.to_string_lossy();
        if name.is_empty() {
            continue;
        }
        let folder = dst.trim_end_matches(['\\', '/']);
        out.push(format!("{folder}\\{name}"));
    }
    out
}

unsafe fn copy_once(srcs: &[String], dst: &str, rename: bool, mv: bool, pass: Pass, owner: HWND) -> super::inuse::Attempt {
    let op: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
        Ok(op) => op,
        Err(e) => return super::inuse::Attempt::err(e),
    };
    if let Err(e) = op.SetOperationFlags(copy_flags(rename, false, pass)) {
        return super::inuse::Attempt::err(e);
    }
    if let Err(e) = own_elevated(&op, pass, owner) {
        return super::inuse::Attempt::err(e);
    }
    let to: IShellItem = match SHCreateItemFromParsingName(&HSTRING::from(dst), None) {
        Ok(it) => it,
        Err(e) => return super::inuse::Attempt::err(e),
    };
    for s in srcs {
        let item: IShellItem = match SHCreateItemFromParsingName(&HSTRING::from(s), None) {
            Ok(it) => it,
            Err(e) => return super::inuse::Attempt::err(e),
        };
        let queued = if mv { op.MoveItem(&item, &to, PCWSTR::null(), None) } else { op.CopyItem(&item, &to, PCWSTR::null(), None) };
        if let Err(e) = queued {
            return super::inuse::Attempt::err(e);
        }
    }
    super::inuse::perform_watched(&op)
}

/// Shell Properties for the active pane's selection.
pub(crate) unsafe fn show_selection_properties(i: usize) {
    let Some(sv) = view_of::<IShellView>(i) else {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    };
    let Ok(cm) = sv.GetItemObject::<IContextMenu>(SVGIO_SELECTION) else {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    };
    let Ok(menu) = CreatePopupMenu() else { return };
    let verb = b"properties\0";
    let hwnd = sv.GetWindow().unwrap_or_else(|_| app(|a| a.main));
    if cm.QueryContextMenu(menu, 0, 1, 0x7fff, CMF_NORMAL).is_ok() {
        let ci = CMINVOKECOMMANDINFO {
            cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd,
            lpVerb: PCSTR(verb.as_ptr()),
            nShow: SW_SHOWNORMAL.0 as i32,
            ..Default::default()
        };
        if cm.InvokeCommand(&ci).is_err() {
            _ = MessageBeep(MB_ICONWARNING);
        }
    } else {
        _ = MessageBeep(MB_ICONWARNING);
    }
    _ = DestroyMenu(menu);
}

pub(crate) unsafe fn delete_selection(i: usize, shift: bool) {
    if crate::pane::recent::locked(i) {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    }
    let arr = view_of::<IFolderView2>(i).and_then(|fv| fv.GetSelection(false).ok());
    let count = arr.as_ref().and_then(|a| a.GetCount().ok()).unwrap_or(0);
    let Ok(paths) = collect_all(count, |k| unsafe { arr.as_ref()?.GetItemAt(k).ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok().map(|p| pwstr(p)) }) else {
        _ = MessageBeep(MB_ICONWARNING);
        return;
    };
    if paths.is_empty() {
        return;
    }
    let (ask, permanent) = app(|a| (a.s.confirm, shift || a.s.del == 1));
    if ask {
        let n = paths.len();
        let verb = if permanent {
            tr(crate::Key::PermanentlyDelete)
        } else {
            tr(crate::Key::MoveRecycleBin)
        };
        let text = format!("{verb} ({n})");
        let owner = app(|a| a.main);
        if !ask_confirm(owner, Confirm::danger("Maguro", text)) {
            return;
        }
    }
    // Permanent delete nobody confirmed yet: let the shell show its own confirmation.
    let owner = if permanent && !ask { app(|a| a.main.0 as isize) } else { 0 };
    start_delete(paths, permanent, owner);
}

pub(crate) unsafe fn start_delete(srcs: Vec<String>, permanent: bool, ask_owner: isize) -> bool {
    start_delete_notify(srcs, permanent, ask_owner, HWND::default(), 0)
}

/// Like `start_delete`, then `PostMessageW(notify, msg, success, 0)` when the worker finishes.
/// `success` is 1 only when `PerformOperations` returned Ok (cancelled/failed = 0).
pub(crate) unsafe fn start_delete_notify(
    srcs: Vec<String>,
    permanent: bool,
    ask_owner: isize,
    notify: HWND,
    msg: u32,
) -> bool {
    let busy = Busy::new();
    let notify_hwnd = notify.0 as isize;
    // Captured here. The worker must not call `app()` while the UI thread is in a dialog.
    let dialog_owner = app(|a| a.main.0 as isize);
    let hwnds = super::listsync::list_hwnds();
    let job = std::thread::Builder::new().spawn(move || {
        let _busy = busy;
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let folders = super::listsync::source_drift(&srcs);
        let result = run_delete(&srcs, permanent, ask_owner, dialog_owner);
        // Before the warning dialog, so a stale pane can drop rows while that box is up.
        // Cancel and a partial failure are included: some of the files may already be gone.
        super::listsync::post_refresh(&hwnds, &folders);
        let ok = match result {
            Ok(()) => true,
            Err(e) => {
                if !super::inuse::is_cancel(&e) {
                    let text = HSTRING::from(format!("{}{}", tr(crate::Key::DeleteFailed), e.message()));
                    _ = MessageBoxW(None, &text, w!("Maguro"), MB_ICONWARNING);
                }
                false
            }
        };
        if com {
            CoUninitialize();
        }
        if notify_hwnd != 0 && msg != 0 {
            _ = PostMessageW(Some(HWND(notify_hwnd as _)), msg, WPARAM(ok as usize), LPARAM(0));
        }
    });
    let Ok(job) = job else { return false };
    keep_job(job);
    true
}

/// Rename a single filesystem item via IFileOperation (Recycle-friendly undo).
pub(crate) unsafe fn run_rename_one(path: &str, new_name: &str, owner: HWND) -> Result<()> {
    let mut fallback = vec![path.to_string()];
    if let Some(dir) = Path::new(path).parent() {
        fallback.push(dir.join(new_name).to_string_lossy().into_owned());
    }
    super::inuse::recover_with(owner, super::inuse::OpKind::Rename, &fallback, |pass, _| rename_once(path, new_name, pass, owner))
}

unsafe fn rename_once(path: &str, new_name: &str, pass: Pass, owner: HWND) -> super::inuse::Attempt {
    let op: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
        Ok(op) => op,
        Err(e) => return super::inuse::Attempt::err(e),
    };
    if let Err(e) = op.SetOperationFlags(rename_flags(pass)) {
        return super::inuse::Attempt::err(e);
    }
    if let Err(e) = own_elevated(&op, pass, owner) {
        return super::inuse::Attempt::err(e);
    }
    let item: IShellItem = match SHCreateItemFromParsingName(&HSTRING::from(path), None) {
        Ok(it) => it,
        Err(e) => return super::inuse::Attempt::err(e),
    };
    let name = HSTRING::from(new_name);
    if let Err(e) = op.RenameItem(&item, PCWSTR(name.as_ptr()), None) {
        return super::inuse::Attempt::err(e);
    }
    super::inuse::perform_watched(&op)
}

pub(crate) unsafe fn run_delete(srcs: &[String], permanent: bool, ask_owner: isize, dialog_owner: isize) -> Result<()> {
    let fallback = srcs.to_vec();
    let owner = HWND(dialog_owner as *mut _);
    super::inuse::recover_with(owner, super::inuse::OpKind::Delete, &fallback, |pass, only| {
        // An item that was deleted before the failure is gone; the elevated pass skips it.
        let targets: Vec<String> = pick(srcs, only).into_iter().filter(|p| pass == Pass::Quiet || Path::new(p).exists()).collect();
        if targets.is_empty() {
            return super::inuse::Attempt { result: Ok(()), locked: Vec::new(), failed: Vec::new() };
        }
        delete_once(&targets, permanent, ask_owner, pass, owner)
    })
}

unsafe fn delete_once(srcs: &[String], permanent: bool, ask_owner: isize, pass: Pass, owner: HWND) -> super::inuse::Attempt {
    let op: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
        Ok(op) => op,
        Err(e) => return super::inuse::Attempt::err(e),
    };
    let shell_confirms = ask_owner != 0 && permanent;
    if shell_confirms {
        if let Err(e) = op.SetOwnerWindow(HWND(ask_owner as _)) {
            return super::inuse::Attempt::err(e);
        }
    } else if let Err(e) = own_elevated(&op, pass, owner) {
        return super::inuse::Attempt::err(e);
    }
    if let Err(e) = op.SetOperationFlags(delete_flags(permanent, shell_confirms, pass)) {
        return super::inuse::Attempt::err(e);
    }
    for s in srcs {
        let item: IShellItem = match SHCreateItemFromParsingName(&HSTRING::from(s), None) {
            Ok(it) => it,
            Err(e) => return super::inuse::Attempt::err(e),
        };
        if let Err(e) = op.DeleteItem(&item, None) {
            return super::inuse::Attempt::err(e);
        }
    }
    super::inuse::perform_watched(&op)
}

pub(crate) fn default_new_name() -> String {
    tr(crate::Key::NewTextDocumentTxt).into()
}

pub(crate) unsafe fn pick_folder(owner: HWND) -> Option<String> {
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let opts = dlg.GetOptions().unwrap_or_default();
    dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
    dlg.Show(Some(owner)).ok()?;
    let name = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    Some(pwstr(name))
}

/// Multi-select file picker. Cancel and a failed dialog both return an empty list.
pub(crate) unsafe fn pick_files(owner: HWND) -> Vec<String> {
    let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return Vec::new() };
    let opts = dlg.GetOptions().unwrap_or_default();
    if dlg.SetOptions(opts | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).is_err() {
        return Vec::new();
    }
    if dlg.Show(Some(owner)).is_err() {
        return Vec::new();
    }
    let Ok(arr) = dlg.GetResults() else { return Vec::new() };
    let n = arr.GetCount().unwrap_or(0).min(10_000);
    let mut out = Vec::new();
    for i in 0..n {
        let Ok(item) = arr.GetItemAt(i) else { continue };
        let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) else { continue };
        let path = pwstr(name);
        if !path.is_empty() {
            out.push(path);
        }
    }
    out
}

#[allow(dead_code)] // sessions use the list window now
pub(crate) unsafe fn pick_open(owner: HWND) -> Option<String> {
    file_dialog(owner, false)
}

#[allow(dead_code)]
pub(crate) unsafe fn file_dialog(owner: HWND, save: bool) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let mut buf = [0u16; 520];
    if save {
        for (d, c) in buf.iter_mut().zip("maguro.json".encode_utf16()) {
            *d = c;
        }
    }
    let filter: Vec<u16> = "Settings\0*.ini\0All\0*.*\0\0".encode_utf16().chain(std::iter::once(0)).collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: if save { OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST } else { OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST },
        ..Default::default()
    };
    let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
    ok.as_bool().then(|| {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    })
}

pub(crate) fn settings_filter() -> Vec<u16> {
    let mut s = String::new();
    for (label, spec) in [(tr(crate::Key::Json), "*.json"), (tr(crate::Key::Settings2), "*.json;*.ini"), (tr(crate::Key::All), "*.*")] {
        s.push_str(label);
        s.push('\0');
        s.push_str(spec);
        s.push('\0');
    }
    s.push('\0');
    s.encode_utf16().collect()
}

/// Export / import picker. Save defaults to maguro.json. Open accepts JSON and a legacy ini.
pub(crate) unsafe fn pick_settings(owner: HWND, save: bool) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let mut buf = [0u16; 520];
    if save {
        for (d, c) in buf.iter_mut().zip("maguro.json".encode_utf16()) {
            *d = c;
        }
    }
    let filter = settings_filter();
    let def = w!("json");
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: if save { OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST } else { OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST },
        lpstrDefExt: if save { def } else { PCWSTR::null() },
        ..Default::default()
    };
    let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
    ok.as_bool().then(|| {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    })
}

pub(crate) unsafe fn reset_prefs() {
    let fresh = Session::default();
    app(|a| {
        a.s.theme = fresh.theme;
        a.s.font_face.clear();
        a.s.bar = fresh.bar;
        a.s.drive_buttons = fresh.drive_buttons;
        a.s.view_new = fresh.view_new;
        a.s.view_icon = fresh.view_icon;
        a.s.start_new = fresh.start_new;
        a.s.start_path.clear();
        // `ext` mirrors Explorer's system-wide HideFileExt; resetting app settings must not change it.
        a.s.tree_on = fresh.tree_on;
        a.s.fav_pane = fresh.fav_pane;
        a.s.tree_pane = fresh.tree_pane;
        a.s.preview_on = fresh.preview_on;
        a.s.preview_w = fresh.preview_w;
        a.s.fav_top = fresh.fav_top;
        a.s.edge = fresh.edge;
        a.s.confirm = fresh.confirm;
        a.s.del = fresh.del;
        a.s.new_name.clear();
        a.s.dropm = fresh.dropm;
        a.s.fast_copy = fresh.fast_copy;
        a.s.layout_new = fresh.layout_new;
        a.s.boot = fresh.boot;
        a.s.boot_file.clear();
        a.s.autosave = fresh.autosave;
        a.s.dot = fresh.dot;
        a.s.update_check = fresh.update_check;
        a.s.single = fresh.single;
        a.s.one_window = fresh.one_window;
        a.s.dblup = fresh.dblup;
        a.s.space_preview = fresh.space_preview;
        a.s.peek_wheel_nav = fresh.peek_wheel_nav;
        a.s.remember_views = fresh.remember_views;
        a.s.folder_sizes = fresh.folder_sizes;
        a.s.folder_sizes_network = fresh.folder_sizes_network;
        a.s.show_mark_stars = fresh.show_mark_stars;
        a.s.peek_image = None;
        a.s.peek_doc = None;
        a.s.accent = fresh.accent;
        a.s.accent_dark = fresh.accent_dark;
        a.s.winmerge_path.clear();
        a.s.ctx_off.clear();
        a.s.ctx_on.clear();
        a.s.ctx_builtin_off.clear();
        a.s.ctx_maguro_off.clear();
        // Packaged Windows 11 commands default off. Reset drops whatever was turned on.
        // The migration flag stays recorded so the next launch does not run that switch again.
        a.s.ctx_packaged_on.clear();
        a.s.ctx_win11_pack_migrated = true;
    });
    crate::winmerge::forget_status();
    // The folder association is not a saved preference. Reset turns it off.
    _ = crate::folderapp::set_registered(false);
    set_language(fresh.lang);
    crate::keys::reset_all();
    crate::cliphot::apply(false);
    relayout();
    publish_shared();
    across_windows(|| {
        relayout();
        redraw_all();
    });
    save_session();
    settings::refresh();
}

/// JSON replaces the settings. A file that is not JSON is read as a legacy ini.
pub(crate) unsafe fn import_settings(text: &str) -> bool {
    let Ok(mut s) = store::import_text(text) else { return false };
    let (win, lang_now) = app(|a| (a.s.win, a.s.lang));
    s.win = win;
    s.dark = resolve_dark(s.theme);
    let lang = s.lang;
    // set_language drops a chosen face. An imported file keeps the face it named.
    let face = s.font_face.clone();
    install_session(s);
    if lang != lang_now {
        set_language(lang);
        if !face.is_empty() {
            app(|a| a.s.font_face = face);
            publish_shared();
            apply_font_all();
            save_session();
        }
    } else {
        apply_theme();
        apply_font();
    }
    apply_ext(app(|a| a.s.ext));
    crate::cliphot::apply(false);
    relayout();
    save_session();
    settings::refresh();
    crate::watch::resync();
    true
}

pub(crate) fn apply_boot(s: &mut Session) {
    match s.boot {
        2 => {
            s.set_heights(s.layout_heights());
            s.paths = vec![String::new(); MAXP];
            s.views = vec![(0, 0); MAXP];
            s.cols = vec![String::new(); MAXP];
            s.pane_folders = vec![Vec::new(); MAXP];
            s.pane_cur = vec![0; MAXP];
            s.pane_locks = vec![Vec::new(); MAXP];
            s.pane_names = vec![Vec::new(); MAXP];
            s.active = 0;
            s.file.clear();
            s.tabs.clear();
            s.tab = 0;
            s.anchor();
        }
        1 if !s.boot_file.is_empty() => {
            if let Ok(t) = std::fs::read_to_string(&s.boot_file) {
                let file = s.boot_file.clone();
                s.take_layout(&Session::parse(&t));
                link_loaded_window(s, &file);
            }
        }
        // 0, the default: every tab loaded from maguro.json stays. Only the active one is shown.
        _ => {
            if s.tabs.is_empty() {
                s.anchor();
            }
            s.publish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_effect_picks_copy_move_or_shell() {
        assert_eq!(clip_act(None), ClipAct::Copy);
        // Explorer's Copy is copy | link; Cut is move alone.
        assert_eq!(clip_act(Some(DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0)), ClipAct::Copy);
        assert_eq!(clip_act(Some(DROPEFFECT_COPY.0)), ClipAct::Copy);
        assert_eq!(clip_act(Some(DROPEFFECT_MOVE.0)), ClipAct::Move);
        assert_eq!(clip_act(Some(DROPEFFECT_LINK.0)), ClipAct::Shell);
        assert_eq!(clip_act(Some(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0)), ClipAct::Shell);
    }

    #[test]
    fn cut_paste_runs_on_the_worker_and_same_folder_cut_does_nothing() {
        let srcs = vec![r"C:\a\x.txt".to_string()];
        let act = clip_act(Some(DROPEFFECT_MOVE.0));
        assert_eq!(paste_plan(act, paste_mode(&srcs, r"D:\b")), PastePlan::Move);
        assert_eq!(paste_plan(act, paste_mode(&srcs, r"C:\a")), PastePlan::Nothing);
        let mixed = vec![r"C:\a\x.txt".to_string(), r"C:\b\y.txt".to_string()];
        assert_eq!(paste_plan(act, paste_mode(&mixed, r"C:\a")), PastePlan::Shell);
        assert_eq!(paste_plan(act, None), PastePlan::Shell);
    }

    #[test]
    fn copy_paste_keeps_the_copy_names_rule() {
        let srcs = vec![r"C:\a\x.txt".to_string()];
        assert_eq!(paste_plan(ClipAct::Copy, paste_mode(&srcs, r"D:\b")), PastePlan::Copy(false));
        assert_eq!(paste_plan(ClipAct::Copy, paste_mode(&srcs, r"C:\a")), PastePlan::Copy(true));
        assert_eq!(paste_plan(ClipAct::Shell, paste_mode(&srcs, r"D:\b")), PastePlan::Shell);
        // Virtual items carry no file system path.
        assert_eq!(paste_plan(ClipAct::Copy, paste_mode(&["::{x}".to_string()], r"D:\b")), PastePlan::Shell);
    }

    #[test]
    fn cut_clears_the_clipboard_only_when_every_source_moved() {
        let srcs = vec!["a".to_string(), "b".to_string()];
        assert!(cut_finished(&srcs, |_| false));
        assert!(!cut_finished(&srcs, |p| p == "b"), "cancel or partial failure keeps the cut");
        assert!(!cut_finished(&[], |_| false));
    }

    fn dib(size: u32, bits: u16, comp: u32, used: u32, extra: usize) -> Vec<u8> {
        let mut d = vec![0u8; size as usize + extra];
        d[0..4].copy_from_slice(&size.to_le_bytes());
        d[14..16].copy_from_slice(&bits.to_le_bytes());
        d[16..20].copy_from_slice(&comp.to_le_bytes());
        d[32..36].copy_from_slice(&used.to_le_bytes());
        d
    }

    #[test]
    fn paste_picture() {
        let off = |b: Vec<u8>| (u32::from_le_bytes(b[2..6].try_into().unwrap()), u32::from_le_bytes(b[10..14].try_into().unwrap()), b[..2].to_vec());
        assert_eq!(dib_to_bmp(&dib(40, 24, 0, 0, 12)).map(off), Some((66, 54, b"BM".to_vec())));
        assert_eq!(dib_to_bmp(&dib(40, 32, 3, 0, 16)).map(off), Some((70, 66, b"BM".to_vec())));
        assert_eq!(dib_to_bmp(&dib(124, 32, 3, 0, 16)).map(|b| off(b).1), Some(138));
        assert_eq!(dib_to_bmp(&dib(40, 8, 0, 0, 1024 + 4)).map(|b| off(b).1), Some(1078));
        assert_eq!(dib_to_bmp(&dib(40, 8, 0, 2, 8 + 4)).map(|b| off(b).1), Some(62));
        assert_eq!(dib_to_bmp(&dib(40, 8, 0, 0, 1024)), None);
        assert_eq!(dib_to_bmp(&dib(40, 24, 0, 0, 0)), None);
        assert_eq!(dib_to_bmp(&dib(40, 24, 0, 0, 4)[..20]), None);
        assert_eq!(dib_to_bmp(&[]), None);
        // A picture with text or HTML beside it (browser Copy image) still saves.
        assert!(picture_only(false, false, false, true));
        assert!(!picture_only(false, false, false, false));
        for k in 0..3 {
            assert!(!picture_only(k == 0, k == 1, k == 2, true));
        }
        let t = SYSTEMTIME { wYear: 2026, wMonth: 10, wDay: 21, wHour: 15, wMinute: 30, wSecond: 2, ..Default::default() };
        assert_eq!(image_name("Clipboard image", &t, 1), "Clipboard image 2026-10-21 153002.png");
        assert_eq!(image_name("클립보드 이미지", &t, 2), "클립보드 이미지 2026-10-21 153002 (2).png");
        assert_eq!(image_name("クリップボード画像", &t, 1), "クリップボード画像 2026-10-21 153002.png");
        let dir = std::env::temp_dir().join(format!("maguro-picture-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), vec![0u8; 3]).unwrap();
        let name = |k: u32| if k == 1 { "a.png".to_string() } else { format!("a ({k}).png") };
        assert_eq!(save_unique(&dir, name, b"new").unwrap(), "a (2).png");
        assert_eq!((std::fs::read(dir.join("a.png")).unwrap(), std::fs::read(dir.join("a (2).png")).unwrap()), (vec![0; 3], b"new".to_vec()));
        assert_eq!(save_unique(&dir, name, b"x").unwrap(), "a (3).png");
        _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_path_drops_the_verbatim_prefix() {
        assert_eq!(dos_from_final(r"\\?\C:\Work\a.txt"), r"C:\Work\a.txt");
        assert_eq!(dos_from_final(r"\\?\UNC\server\share\a"), r"\\server\share\a");
        assert_eq!(dos_from_final(r"C:\Work\a.txt"), r"C:\Work\a.txt");
    }

    #[test]
    fn drop_move_waits_until_the_drag_releases_the_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = std::env::temp_dir().join(format!("maguro-drop-move-{}", std::process::id()));
        let src_dir = root.join("src");
        let dst_dir = root.join("dst");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();
        let file = src_dir.join("moved.txt");
        std::fs::write(&file, b"drag").unwrap();
        let locked = file.clone();
        let hold = std::thread::spawn(move || {
            let f = std::fs::OpenOptions::new().read(true).share_mode(1).open(&locked).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(350));
            drop(f);
        });
        std::thread::sleep(std::time::Duration::from_millis(40));
        let src = file.to_string_lossy().to_string();
        assert!(unsafe { drag_still_holds(&src) });
        settle_drop_move(&[src.clone()]);
        assert!(!unsafe { drag_still_holds(&src) });
        let dst = dst_dir.to_string_lossy().to_string();
        let src_final = unsafe { final_dos(&src) };
        let dst_final = unsafe { final_dos(&dst) };
        assert!(std::path::Path::new(&src_final).is_file(), "src final missing {src_final} from {src}");
        assert!(std::path::Path::new(&dst_final).is_dir(), "dst final missing {dst_final} from {dst}");
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
        let moved = unsafe { run_copy(&[src], &dst, false, true) };
        if com {
            unsafe { CoUninitialize() };
        }
        hold.join().unwrap();
        assert!(moved.is_ok(), "{moved:?}");
        assert!(!file.exists());
        assert_eq!(std::fs::read(dst_dir.join("moved.txt")).unwrap(), b"drag");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subst_and_its_target_are_one_path() {
        let subst = std::path::Path::new(r"G:\Documents\F29test\f31_copy_me.txt");
        let real = std::path::Path::new(r"C:\WORK\pgreadme\data\Documents\F29test\f31_copy_me.txt");
        if !subst.is_file() || !real.is_file() {
            return;
        }
        let a = unsafe { final_dos(&subst.to_string_lossy()) };
        let b = unsafe { final_dos(&real.to_string_lossy()) };
        assert_eq!(a.to_lowercase(), b.to_lowercase());
        assert!(a.to_lowercase().starts_with(r"c:\work\pgreadme\data\"));
        assert!(!a.to_lowercase().starts_with("g:"));
    }

    #[test]
    fn locked_delete_reports_in_use_without_hanging() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::sync::mpsc::channel;
        let dir = std::env::temp_dir().join(format!("maguro-lockdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("locked.txt");
        std::fs::write(&path, b"locked").unwrap();
        let hold_path = path.clone();
        let (ready_tx, ready_rx) = channel();
        let (release_tx, release_rx) = channel::<()>();
        let holder = std::thread::spawn(move || {
            let f = std::fs::OpenOptions::new().read(true).share_mode(0).open(&hold_path).unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv();
            drop(f);
        });
        ready_rx.recv().unwrap();
        let shown = path.to_string_lossy().to_string();
        let (done_tx, done_rx) = channel();
        let _worker = std::thread::spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            let attempt = unsafe { delete_once(&[shown], true, 0, Pass::Quiet, HWND::default()) };
            if com {
                unsafe { CoUninitialize() };
            }
            let code = attempt.result.as_ref().err().map(|e| e.code());
            let msg = attempt.result.as_ref().err().map(|e| e.message()).unwrap_or_default();
            let _ = done_tx.send((code, msg, attempt.locked));
        });
        let got = done_rx.recv_timeout(std::time::Duration::from_secs(12));
        let _ = release_tx.send(());
        let _ = holder.join();
        let Ok((code, msg, locked)) = got else {
            // A system "File In Use" dialog would keep this process alive. Exit so the suite can finish.
            eprintln!("locked delete did not return; a system dialog may be showing");
            std::process::exit(2);
        };
        let in_use = code.is_some_and(|c| super::super::inuse::is_in_use_code(c)) || super::super::inuse::message_says_in_use(&msg) || !locked.is_empty();
        assert!(in_use, "code={code:?} msg={msg} locked={locked:?}");
        assert!(path.exists(), "locked file should still be there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// None when this process can create a file in `C:\` (elevated): a probe would write there.
    fn protected_root_probe(tag: &str) -> Option<std::path::PathBuf> {
        let target = std::path::Path::new(r"C:\").join(format!("maguro-uac-{tag}-{}.txt", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(f) => {
                drop(f);
                let _ = std::fs::remove_file(&target);
                eprintln!("C:\\ is writable here (elevated?); skipping the protected-root probe");
                None
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Some(target),
            Err(e) => {
                eprintln!("C:\\ probe skipped: {e}");
                None
            }
        }
    }

    #[test]
    fn flags_keep_noerrorui_first_and_swap_it_for_the_elevation_prompt_on_retry() {
        let has = |f: FILEOPERATION_FLAGS, bit: FILEOPERATION_FLAGS| f.0 & bit.0 == bit.0;
        for rename in [false, true] {
            let q = copy_flags(rename, false, Pass::Quiet);
            let e = copy_flags(rename, false, Pass::Elevated);
            assert!(has(q, FOF_NOERRORUI) && !has(q, FOFX_SHOWELEVATIONPROMPT));
            assert!(!has(e, FOF_NOERRORUI) && has(e, FOFX_SHOWELEVATIONPROMPT));
            for f in [q, e] {
                assert!(has(f, FOF_ALLOWUNDO) && has(f, FOF_NOCONFIRMMKDIR) && has(f, FOFX_ADDUNDORECORD));
                assert_eq!(has(f, FOF_RENAMEONCOLLISION), rename);
                assert!(!has(f, FOF_NOCONFIRMATION));
            }
        }
        // Fast copy already asked about conflicts.
        assert!(has(copy_flags(false, true, Pass::Elevated), FOF_NOCONFIRMATION));
        // Shift+Delete the shell confirms: it asks on the first pass only.
        let q = delete_flags(true, true, Pass::Quiet);
        let e = delete_flags(true, true, Pass::Elevated);
        assert!(has(q, FOF_NOERRORUI) && !has(q, FOF_NOCONFIRMATION) && !has(q, FOF_ALLOWUNDO));
        assert!(!has(e, FOF_NOERRORUI) && has(e, FOF_NOCONFIRMATION) && has(e, FOFX_SHOWELEVATIONPROMPT) && !has(e, FOF_ALLOWUNDO));
        // Shift+Delete Maguro confirmed, and Delete to the recycle bin.
        assert!(has(delete_flags(true, false, Pass::Quiet), FOF_NOCONFIRMATION));
        assert!(has(delete_flags(true, false, Pass::Elevated), FOF_NOCONFIRMATION));
        let r = delete_flags(false, false, Pass::Elevated);
        assert!(has(r, FOF_ALLOWUNDO) && has(r, FOFX_ADDUNDORECORD) && has(r, FOF_NOCONFIRMATION) && has(r, FOFX_SHOWELEVATIONPROMPT) && !has(r, FOF_NOERRORUI));
        assert!(has(rename_flags(Pass::Quiet), FOF_NOERRORUI));
        let r = rename_flags(Pass::Elevated);
        assert!(!has(r, FOF_NOERRORUI) && has(r, FOFX_SHOWELEVATIONPROMPT) && has(r, FOF_NOCONFIRMATION));
    }

    /// The quiet first pass into `C:\` fails without any UI, and the code it reports is one
    /// `recover_with` turns into the elevated pass. The elevated pass itself is not run here:
    /// it would raise the administrator prompt.
    #[test]
    fn quiet_copy_into_protected_root_fails_with_an_elevation_code() {
        use std::sync::mpsc::channel;
        let Some(target) = protected_root_probe("copy") else { return };
        let dir = std::env::temp_dir().join(format!("maguro-uac-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(target.file_name().unwrap());
        std::fs::write(&src, b"probe").unwrap();
        let shown = src.to_string_lossy().into_owned();
        let (tx, rx) = channel();
        let _worker = std::thread::spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            for mv in [false, true] {
                let a = unsafe { copy_once(&[shown.clone()], r"C:\", false, mv, Pass::Quiet, HWND::default()) };
                let code = a.result.as_ref().err().map(|e| e.code()).unwrap_or(S_OK);
                let _ = tx.send((mv, code, a.failed.clone(), a.locked.clone(), a.denied()));
            }
            if com {
                unsafe { CoUninitialize() };
            }
        });
        let mut seen = Vec::new();
        for _ in 0..2 {
            match rx.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(r) => seen.push(r),
                Err(_) => {
                    let _ = std::fs::remove_file(&target);
                    eprintln!("quiet copy into C:\\ did not return; a system dialog may be showing");
                    std::process::exit(2);
                }
            }
        }
        let wrote = target.exists();
        if wrote {
            let _ = std::fs::remove_file(&target);
        }
        let src_left = src.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!wrote, "copy into C:\\ succeeded unelevated");
        assert!(src_left, "quiet move into C:\\ removed the source");
        for (mv, code, failed, locked, denied) in seen {
            let items: Vec<String> = failed.iter().map(|(p, hr)| format!("{:#010x} {p}", hr.0 as u32)).collect();
            eprintln!("UAC probe: mv={mv} PerformOperations={:#010x} items={items:?} locked={locked:?}", code.0 as u32);
            let next = super::super::inuse::next_step(code, "", !locked.is_empty(), !denied.is_empty(), false);
            assert_eq!(next, super::super::inuse::Next::Elevate, "mv={mv} code={:#010x} items={items:?}", code.0 as u32);
        }
    }

    /// Manual: `cargo test --bin maguro -- --ignored elevated_copy_into_protected_root`.
    /// The elevated pass must show Windows' own prompt instead of failing silently. The test
    /// closes that prompt itself (Cancel), so nothing is elevated and nothing is written to `C:\`.
    #[test]
    #[ignore]
    fn elevated_copy_into_protected_root_shows_the_windows_prompt() {
        use std::sync::mpsc::channel;
        unsafe extern "system" fn ours(hwnd: HWND, lp: LPARAM) -> BOOL {
            let found = unsafe { &mut *(lp.0 as *mut Vec<(HWND, String, String)>) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == std::process::id() && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                let mut class = [0u16; 128];
                let n = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
                let mut title = [0u16; 256];
                let t = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
                found.push((hwnd, String::from_utf16_lossy(&class[..n]), String::from_utf16_lossy(&title[..t])));
            }
            BOOL(1)
        }
        let Some(target) = protected_root_probe("elev") else { return };
        let dir = std::env::temp_dir().join(format!("maguro-uac-elev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(target.file_name().unwrap());
        std::fs::write(&src, b"probe").unwrap();
        let shown = src.to_string_lossy().into_owned();
        let (tx, rx) = channel();
        let _worker = std::thread::spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            let a = unsafe { copy_once(&[shown], r"C:\", false, false, Pass::Elevated, HWND::default()) };
            let code = a.result.as_ref().err().map(|e| e.code()).unwrap_or(S_OK);
            if com {
                unsafe { CoUninitialize() };
            }
            let _ = tx.send(code);
        });
        let start = Instant::now();
        let mut prompt: Option<(HWND, String, String)> = None;
        let mut early = None;
        let mut seen: Vec<String> = Vec::new();
        while start.elapsed() < std::time::Duration::from_secs(15) && prompt.is_none() && early.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            early = rx.try_recv().ok();
            let mut found: Vec<(HWND, String, String)> = Vec::new();
            unsafe {
                let _ = EnumWindows(Some(ours), LPARAM(&mut found as *mut _ as isize));
            }
            for (_, class, title) in &found {
                let k = format!("{class} \"{title}\"");
                if !seen.contains(&k) {
                    seen.push(k);
                }
            }
            // The copy engine draws its "Continue" (administrator) prompt in its own
            // OperationStatusWindow. A 5-byte copy that is still up after 1.5 s is waiting there.
            if start.elapsed() > std::time::Duration::from_millis(1500) {
                prompt = found.into_iter().find(|(_, class, _)| class == "OperationStatusWindow" || class == "#32770");
            }
        }
        let Some((hwnd, class, title)) = prompt else {
            let wrote = target.exists();
            let _ = std::fs::remove_file(&target);
            let _ = std::fs::remove_dir_all(&dir);
            panic!("no Windows prompt appeared for the elevated pass: early={:?} wrote={wrote} windows={seen:?}", early.map(|c: HRESULT| format!("{:#010x}", c.0 as u32)));
        };
        eprintln!("UAC probe: elevated pass showed {class} \"{title}\"; cancelling it");
        // Close it, never Continue: nothing gets elevated.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let code = match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(code) => code,
            Err(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                eprintln!("the prompt did not close; exiting so it does not stay on screen");
                std::process::exit(2);
            }
        };
        let wrote = target.exists();
        if wrote {
            let _ = std::fs::remove_file(&target);
        }
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("UAC probe: elevated pass after Cancel -> {:#010x}", code.0 as u32);
        assert!(!wrote, "the cancelled elevated copy wrote into C:\\");
    }
}
