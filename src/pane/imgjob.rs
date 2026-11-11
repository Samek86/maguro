//! Picture bytes are decoded in a child process.
//!
//! A property handler or codec that faults while the hosted Details view reads a JPEG
//! takes down this process; that path is Windows. Our own preview must not do the same.
//! The child is killed if it runs too long. Its death does not kill Maguro.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use super::jpeg::{self, JpegFault};
use super::peek::Pix;
use super::preview::{self, ext_of};
use crate::i18n::tr;
use crate::winpath::win_long_path;

const FILE_CAP: u64 = 80 * 1024 * 1024;
const CHILD_MS: u64 = 8_000;

pub(crate) fn image_main(args: &[String]) -> i32 {
    if args.get(1).map(String::as_str) == Some("worker") {
        worker_loop();
        return 0;
    }
    let Some(w) = args.get(1).and_then(|s| s.parse::<i32>().ok()) else { return 1 };
    let Some(h) = args.get(2).and_then(|s| s.parse::<i32>().ok()) else { return 1 };
    let Some(flags) = args.get(3).and_then(|s| s.parse::<u32>().ok()) else { return 1 };
    let Some(out) = args.get(4) else { return 1 };
    let Some(path) = args.get(5) else { return 1 };
    let w = w.clamp(1, 4096);
    let h = h.clamp(1, 4096);
    let com = com_init();
    let code = match decode_to_pix(path, w, h, flags) {
        Ok(pix) if write_pix(Path::new(out), &pix) => 0,
        Ok(_) => 1,
        Err(code) => code,
    };
    com_fini(com);
    code
}

fn com_init() -> bool {
    unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED).is_ok() }
}

fn com_fini(on: bool) {
    if on {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

fn decode_to_pix(path: &str, w: i32, h: i32, flags: u32) -> Result<Pix, i32> {
    let w = w.clamp(1, 4096);
    let h = h.clamp(1, 4096);
    let path = win_long_path(path);
    let Ok(meta) = std::fs::metadata(&path) else { return Err(1) };
    if meta.len() == 0 || meta.len() > FILE_CAP {
        return Err(2);
    }
    let Ok(bytes) = std::fs::read(&path) else { return Err(1) };
    if preview_ext(&path) {
        match jpeg::inspect(&bytes) {
            Err(JpegFault::Huge) => return Err(3),
            Err(JpegFault::ZeroSize) | Err(JpegFault::Empty) => return Err(2),
            _ => {}
        }
    }
    unsafe {
        preview::decode_pic(&bytes, w, h)
            .map(|pix| apply_orient(&bytes, pix))
            .or_else(|| preview::shell_pic(&path, w, h, shell_flags(flags)))
            .ok_or(4)
    }
}

fn preview_ext(path: &str) -> bool {
    matches!(ext_of(path).as_str(), "jpg" | "jpeg" | "jpe" | "jfif")
}

fn apply_orient(bytes: &[u8], mut pix: Pix) -> Pix {
    let Ok(meta) = jpeg::inspect(bytes) else { return pix };
    if let Some((w, h, bgra)) = jpeg::orient_bgra(pix.w, pix.h, &pix.bgra, meta.orientation) {
        pix = Pix::new(w, h, bgra);
    }
    pix
}

fn shell_flags(bits: u32) -> windows::Win32::UI::Shell::SIIGBF {
    windows::Win32::UI::Shell::SIIGBF(bits as i32)
}

fn write_pix(path: &Path, pix: &Pix) -> bool {
    let Ok(mut f) = std::fs::File::create(path) else { return false };
    write_pix_to(&mut f, pix)
}

fn write_pix_to(w: &mut impl Write, pix: &Pix) -> bool {
    let Some(n) = pix_len(pix.w, pix.h) else { return false };
    if pix.bgra.len() != n {
        return false;
    }
    w.write_all(b"PGIM").is_ok()
        && w.write_all(&(pix.w as u32).to_le_bytes()).is_ok()
        && w.write_all(&(pix.h as u32).to_le_bytes()).is_ok()
        && w.write_all(&1u16.to_le_bytes()).is_ok()
        && w.write_all(&pix.bgra).is_ok()
}

fn pix_len(w: i32, h: i32) -> Option<usize> {
    if !(1..=jpeg::MAX_EDGE as i32).contains(&w) || !(1..=jpeg::MAX_EDGE as i32).contains(&h) {
        return None;
    }
    (w as usize).checked_mul(h as usize)?.checked_mul(4)
}

fn read_pix_from(r: &mut impl Read) -> Option<Pix> {
    let mut head = [0u8; 14];
    r.read_exact(&mut head).ok()?;
    if &head[..4] != b"PGIM" {
        return None;
    }
    let w = u32::from_le_bytes(head[4..8].try_into().ok()?) as i32;
    let h = u32::from_le_bytes(head[8..12].try_into().ok()?) as i32;
    let n = pix_len(w, h)?;
    let mut data = vec![0u8; n];
    r.read_exact(&mut data).ok()?;
    Some(Pix::new(w, h, data))
}

/// Decode `path` off this process. `Err` is an in-pane message, not a crash.
pub(crate) fn isolated_pic(path: &str, w: i32, h: i32, flags: u32) -> Result<Pix, String> {
    if let Err(msg) = reject_header(path) {
        return Err(msg);
    }
    let w = w.clamp(1, 4096);
    let h = h.clamp(1, 4096);
    let meta = std::fs::metadata(path).map_err(|_| msg_open())?;
    let key = PicKey {
        path: path.to_string(),
        len: meta.len(),
        modified: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis()),
        w,
        h,
        flags,
    };
    if let Some(pix) = cache_get(&key) {
        return Ok(pix);
    }
    let pix = ask_worker(path, w, h, flags)?;
    cache_put(key, &pix);
    Ok(pix)
}

const CACHE_N: usize = 8;
const CACHE_MAX: usize = 32 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
struct PicKey {
    path: String,
    len: u64,
    modified: Option<u128>,
    w: i32,
    h: i32,
    flags: u32,
}

struct Cached {
    key: PicKey,
    pix: Pix,
}

fn cache() -> &'static Mutex<VecDeque<Cached>> {
    static CACHE: std::sync::OnceLock<Mutex<VecDeque<Cached>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn clone_pix(pix: &Pix) -> Pix {
    let mut out = Pix::new(pix.w, pix.h, pix.bgra.clone());
    out.lay_w = pix.lay_w;
    out.lay_h = pix.lay_h;
    out
}

fn cache_get(key: &PicKey) -> Option<Pix> {
    let mut g = cache().lock().unwrap_or_else(|p| p.into_inner());
    let i = g.iter().position(|e| e.key == *key)?;
    let e = g.remove(i)?;
    let pix = clone_pix(&e.pix);
    g.push_back(e);
    Some(pix)
}

fn cache_put(key: PicKey, pix: &Pix) {
    let n = pix.bgra.len();
    if n > CACHE_MAX {
        return;
    }
    let mut g = cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = g.iter().position(|e| e.key == key) {
        g.remove(i);
    }
    g.push_back(Cached { key, pix: clone_pix(pix) });
    while g.len() > CACHE_N || g.iter().map(|e| e.pix.bgra.len()).sum::<usize>() > CACHE_MAX {
        if g.pop_front().is_none() {
            break;
        }
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: Option<ChildStdout>,
}

fn workers() -> &'static Mutex<Option<Worker>> {
    static WORKER: std::sync::OnceLock<Mutex<Option<Worker>>> = std::sync::OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

fn ask_worker(path: &str, w: i32, h: i32, flags: u32) -> Result<Pix, String> {
    let mut slot = workers().lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_mut().is_some_and(|worker| worker.child.try_wait().ok().flatten().is_some()) {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(spawn_worker()?);
    }
    let sent = slot.as_mut().is_some_and(|worker| write_request(&mut worker.stdin, path, w, h, flags));
    if !sent {
        drop_worker(&mut slot);
        return Err(msg_spawn());
    }
    let stdout = match slot.as_mut().and_then(|worker| worker.stdout.take()) {
        Some(stdout) => stdout,
        None => {
            drop_worker(&mut slot);
            return Err(msg_spawn());
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new().name("maguro-img-read".into()).spawn(move || {
        let mut stdout = stdout;
        let result = read_response(&mut stdout);
        let _ = tx.send(result);
        stdout
    });
    let reader = match reader {
        Ok(t) => t,
        Err(_) => {
            drop_worker(&mut slot);
            return Err(msg_spawn());
        }
    };
    // The read blocks in the kernel until the child writes or the pipe breaks.
    // Killing the child on timeout unblocks that read. There is no 30ms poll.
    match rx.recv_timeout(Duration::from_millis(CHILD_MS)) {
        Ok(Ok(pix)) => {
            if let Ok(stdout) = reader.join() {
                if let Some(worker) = slot.as_mut() {
                    worker.stdout = Some(stdout);
                }
            }
            Ok(pix)
        }
        Ok(Err(msg)) => {
            drop_worker(&mut slot);
            let _ = reader.join();
            Err(msg)
        }
        Err(_) => {
            drop_worker(&mut slot);
            let _ = reader.join();
            Err(msg_slow())
        }
    }
}

fn drop_worker(slot: &mut Option<Worker>) {
    if let Some(worker) = slot.take() {
        let mut child = worker.child;
        drop(worker.stdin);
        drop(worker.stdout);
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn spawn_worker() -> Result<Worker, String> {
    let exe = std::env::current_exe().map_err(|_| msg_spawn())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--maguro-image").arg("worker");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|_| msg_spawn())?;
    let stdin = child.stdin.take().ok_or_else(msg_spawn)?;
    let stdout = child.stdout.take().ok_or_else(msg_spawn)?;
    Ok(Worker { child, stdin, stdout: Some(stdout) })
}

fn write_request(w: &mut impl Write, path: &str, width: i32, height: i32, flags: u32) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() > 32 * 1024 {
        return false;
    }
    w.write_all(b"RQIM").is_ok()
        && w.write_all(&(bytes.len() as u32).to_le_bytes()).is_ok()
        && w.write_all(bytes).is_ok()
        && w.write_all(&width.to_le_bytes()).is_ok()
        && w.write_all(&height.to_le_bytes()).is_ok()
        && w.write_all(&flags.to_le_bytes()).is_ok()
        && w.flush().is_ok()
}

fn read_response(r: &mut impl Read) -> Result<Pix, String> {
    let mut code = [0u8; 4];
    r.read_exact(&mut code).map_err(|_| msg_stop("?"))?;
    let code = i32::from_le_bytes(code);
    if code != 0 {
        return Err(msg_stop(&code.to_string()));
    }
    read_pix_from(r).ok_or_else(|| msg_stop("0"))
}

fn worker_loop() {
    let com = com_init();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    loop {
        let mut magic = [0u8; 4];
        if input.read_exact(&mut magic).is_err() || &magic != b"RQIM" {
            break;
        }
        let mut len_b = [0u8; 4];
        if input.read_exact(&mut len_b).is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_b) as usize;
        if len > 32 * 1024 {
            break;
        }
        let mut path = vec![0u8; len];
        if input.read_exact(&mut path).is_err() {
            break;
        }
        let mut rest = [0u8; 12];
        if input.read_exact(&mut rest).is_err() {
            break;
        }
        let width = i32::from_le_bytes(rest[0..4].try_into().unwrap());
        let height = i32::from_le_bytes(rest[4..8].try_into().unwrap());
        let flags = u32::from_le_bytes(rest[8..12].try_into().unwrap());
        let Ok(path) = String::from_utf8(path) else { break };
        let result = decode_to_pix(&path, width, height, flags);
        let ok = match result {
            Ok(pix) => output.write_all(&0i32.to_le_bytes()).is_ok() && write_pix_to(&mut output, &pix),
            Err(code) => output.write_all(&code.to_le_bytes()).is_ok(),
        };
        if !ok || output.flush().is_err() {
            break;
        }
    }
    com_fini(com);
}

fn reject_header(path: &str) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|_| msg_open())?;
    if meta.len() == 0 {
        return Err(msg_empty());
    }
    if meta.len() > FILE_CAP {
        return Err(msg_big());
    }
    let ext = ext_of(path);
    let head = read_prefix(path, 1024 * 1024).unwrap_or_default();
    if matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif") {
        return match jpeg::inspect(&head) {
            Ok(_) => Ok(()),
            Err(JpegFault::Truncated) if (head.len() as u64) < meta.len() => Ok(()),
            Err(JpegFault::Huge) => Err(msg_huge()),
            Err(JpegFault::Empty) => Err(msg_empty()),
            Err(JpegFault::ZeroSize) | Err(JpegFault::NotJpeg) | Err(JpegFault::Truncated) => Err(msg_bad()),
        };
    }
    if let Some(Err(JpegFault::Huge)) = jpeg::probe_edges(&head) {
        return Err(msg_huge());
    }
    if let Some(Err(JpegFault::ZeroSize)) = jpeg::probe_edges(&head) {
        return Err(msg_bad());
    }
    Ok(())
}

fn read_prefix(path: &str, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut b = Vec::new();
    f.take(max as u64).read_to_end(&mut b).ok()?;
    Some(b)
}

fn msg_empty() -> String {
    tr(crate::Key::FileIsEmpty).into()
}
fn msg_huge() -> String {
    tr(crate::Key::PictureIsTooLarge).into()
}
fn msg_big() -> String {
    tr(crate::Key::SkippedLargerThan80Mb).into()
}
fn msg_bad() -> String {
    tr(crate::Key::PictureCouldNotBeRead).into()
}
fn msg_slow() -> String {
    tr(crate::Key::PictureTookTooLong).into()
}
fn msg_open() -> String {
    tr(crate::Key::FileCouldNotBeOpened).into()
}
fn msg_spawn() -> String {
    tr(crate::Key::CouldNotStartPictureDecoder).into()
}
fn msg_stop(code: &str) -> String {
    format!("{} ({code})", tr(crate::Key::DecoderStopped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pix_frame_round_trips() {
        let pix = Pix::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let mut buf = Vec::new();
        assert!(write_pix_to(&mut buf, &pix));
        let back = read_pix_from(&mut buf.as_slice()).unwrap();
        assert_eq!((back.w, back.h, back.bgra), (2, 1, pix.bgra));
    }

    #[test]
    fn cache_drops_the_oldest_past_eight() {
        let pix = Pix::new(1, 1, vec![9, 9, 9, 9]);
        for i in 0..10 {
            cache_put(PicKey { path: format!("p{i}"), len: i, modified: Some(1), w: 1, h: 1, flags: 0 }, &pix);
        }
        let g = cache().lock().unwrap();
        assert_eq!(g.len(), 8);
        assert!(g.iter().all(|e| e.key.path != "p0" && e.key.path != "p1"));
        assert!(g.back().unwrap().key.path == "p9");
    }
}
