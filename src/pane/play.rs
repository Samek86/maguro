//! Media Foundation playback for Quick Look and the docked preview.
//!
//! The player runs on its own thread, the same way a preview handler does, so a slow
//! source cannot freeze the file list. Stopping sets a flag, destroys the video child,
//! and the thread shuts the session down so the file is released.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::imports::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

const WM_PLAY_SIZE: u32 = WM_APP + 98;

struct Play {
    stop: Arc<AtomicBool>,
    child: isize,
    tid: AtomicU32,
}

fn plays() -> std::sync::MutexGuard<'static, HashMap<isize, Play>> {
    static PLAYS: LazyLock<Mutex<HashMap<isize, Play>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    PLAYS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drop the player for `key` and release the file. Safe to call when nothing is playing.
pub(crate) unsafe fn play_stop(key: isize) {
    let child = {
        let mut map = plays();
        let Some(p) = map.remove(&key) else { return };
        p.stop.store(true, Ordering::SeqCst);
        p.child
    };
    let hwnd = HWND(child as _);
    if child != 0 && IsWindow(Some(hwnd)).as_bool() {
        _ = DestroyWindow(hwnd);
    }
}

/// Tell a running video where to draw. The window move itself is the caller's.
pub(crate) unsafe fn play_layout(key: isize, _w: i32, _h: i32) {
    let tid = {
        let map = plays();
        let Some(p) = map.get(&key) else { return };
        p.tid.load(Ordering::SeqCst)
    };
    if tid != 0 {
        _ = PostThreadMessageW(tid, WM_PLAY_SIZE, WPARAM(0), LPARAM(0));
    }
}

/// Start playback. `child` is the video window, or 0 for audio. `done` is posted to `parent`
/// with wParam = `gen` and lParam = 1 on success, 0 on failure.
pub(crate) unsafe fn play_start(key: isize, gen: u64, parent: isize, child: isize, path: &str, video: bool, _area: RECT, done: u32) {
    play_stop(key);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut map = plays();
        map.insert(key, Play { stop: stop.clone(), child, tid: AtomicU32::new(0) });
    }
    let path = path.to_string();
    if std::thread::Builder::new().name("maguro-play".into()).spawn(move || play_thread(key, gen, parent, child, path, video, stop, done)).is_err() {
        play_stop(key);
        let _ = PostMessageW(Some(HWND(parent as _)), done, WPARAM(gen as usize), LPARAM(0));
    }
}

fn play_thread(key: isize, gen: u64, parent: isize, child: isize, path: String, video: bool, stop: Arc<AtomicBool>, done: u32) {
    unsafe {
        let tid = GetCurrentThreadId();
        if let Some(p) = plays().get(&key) {
            if Arc::ptr_eq(&p.stop, &stop) {
                p.tid.store(tid, Ordering::SeqCst);
            }
        }
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let mut prime = MSG::default();
        _ = PeekMessageW(&mut prime, None, WM_PLAY_SIZE, WM_PLAY_SIZE, PM_NOREMOVE);
        let ok = !stop.load(Ordering::SeqCst) && play_file(&path, video, child, &stop, parent, gen, done).is_ok();
        if !ok && !stop.load(Ordering::SeqCst) {
            let _ = PostMessageW(Some(HWND(parent as _)), done, WPARAM(gen as usize), LPARAM(0));
        }
        if plays().get(&key).is_some_and(|p| Arc::ptr_eq(&p.stop, &stop)) {
            if let Some(p) = plays().get(&key) {
                p.tid.store(0, Ordering::SeqCst);
            }
        }
        if com {
            CoUninitialize();
        }
    }
}

unsafe fn play_file(path: &str, video: bool, child: isize, stop: &AtomicBool, parent: isize, gen: u64, done: u32) -> std::result::Result<(), ()> {
    MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|_| ())?;
    let result = (|| {
        if stop.load(Ordering::SeqCst) {
            return Err(());
        }
        let resolver = MFCreateSourceResolver().map_err(|_| ())?;
        let mut kind = MF_OBJECT_INVALID;
        let mut obj = None;
        let wide = HSTRING::from(path);
        let flags = MF_RESOLUTION_MEDIASOURCE.0 | MF_RESOLUTION_CONTENT_DOES_NOT_HAVE_TO_MATCH_EXTENSION_OR_MIME_TYPE.0;
        resolver.CreateObjectFromURL(PCWSTR(wide.as_ptr()), flags as u32, None, &mut kind, &mut obj).map_err(|_| ())?;
        let source: IMFMediaSource = obj.ok_or(())?.cast().map_err(|_| ())?;
        let opened = open_session(&source, video, HWND(child as _), stop, parent, gen, done);
        let _ = source.Shutdown();
        opened
    })();
    let _ = MFShutdown();
    result
}

unsafe fn open_session(source: &IMFMediaSource, video: bool, child: HWND, stop: &AtomicBool, parent: isize, gen: u64, done: u32) -> std::result::Result<(), ()> {
    let pd = source.CreatePresentationDescriptor().map_err(|_| ())?;
    let count = pd.GetStreamDescriptorCount().map_err(|_| ())?;
    let session = MFCreateMediaSession(None).map_err(|_| ())?;
    let topo = MFCreateTopology().map_err(|_| ())?;
    let mut connected = 0u32;
    for i in 0..count {
        let mut selected = BOOL(0);
        let mut sd = None;
        if pd.GetStreamDescriptorByIndex(i, &mut selected, &mut sd).is_err() {
            continue;
        }
        let Some(sd) = sd else { continue };
        let major = sd.GetMediaTypeHandler().and_then(|h| h.GetMajorType()).unwrap_or(GUID::zeroed());
        let want = major == MFMediaType_Audio || (video && major == MFMediaType_Video);
        if selected.as_bool() && !want {
            let _ = pd.DeselectStream(i);
            continue;
        }
        if !want {
            continue;
        }
        if !selected.as_bool() {
            let _ = pd.SelectStream(i);
        }
        let id = sd.GetStreamIdentifier().unwrap_or(i);
        let src_node = MFCreateTopologyNode(MF_TOPOLOGY_SOURCESTREAM_NODE).map_err(|_| ())?;
        src_node.SetUnknown(&MF_TOPONODE_SOURCE, source).map_err(|_| ())?;
        src_node.SetUnknown(&MF_TOPONODE_PRESENTATION_DESCRIPTOR, &pd).map_err(|_| ())?;
        src_node.SetUINT32(&MF_TOPONODE_STREAMID, id).map_err(|_| ())?;
        src_node.SetUINT32(&MF_TOPONODE_NOSHUTDOWN_ON_REMOVE, 1).map_err(|_| ())?;
        let out = MFCreateTopologyNode(MF_TOPOLOGY_OUTPUT_NODE).map_err(|_| ())?;
        if major == MFMediaType_Video {
            if child.0.is_null() {
                continue;
            }
            let activate = MFCreateVideoRendererActivate(child).map_err(|_| ())?;
            out.SetObject(&activate).map_err(|_| ())?;
        } else {
            let activate = MFCreateAudioRendererActivate().map_err(|_| ())?;
            out.SetObject(&activate).map_err(|_| ())?;
        }
        src_node.ConnectOutput(0, &out, 0).map_err(|_| ())?;
        topo.AddNode(&src_node).map_err(|_| ())?;
        topo.AddNode(&out).map_err(|_| ())?;
        connected += 1;
    }
    if connected == 0 || stop.load(Ordering::SeqCst) {
        let _ = session.Shutdown();
        return Err(());
    }
    session.SetTopology(0, &topo).map_err(|_| ())?;
    let pos: PROPVARIANT = std::mem::zeroed();
    session.Start(std::ptr::null(), &pos).map_err(|_| ())?;
    if stop.load(Ordering::SeqCst) {
        let _ = session.Shutdown();
        return Err(());
    }
    if video && !child.0.is_null() {
        place_video(&session, child);
    }
    let _ = PostMessageW(Some(HWND(parent as _)), done, WPARAM(gen as usize), LPARAM(1));
    let mut msg = MSG::default();
    while !stop.load(Ordering::SeqCst) && (child.0.is_null() || IsWindow(Some(child)).as_bool()) {
        while let Ok(ev) = session.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
            let ty = ev.GetType().unwrap_or(0);
            if ty == MESessionStarted.0 as u32 && video {
                place_video(&session, child);
            } else if ty == MEError.0 as u32 {
                let _ = session.Shutdown();
                return Err(());
            }
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_PLAY_SIZE {
                place_video(&session, child);
            }
        }
        let _ = MsgWaitForMultipleObjects(None, false, 80, QS_ALLINPUT);
    }
    let _ = session.Stop();
    let _ = session.Close();
    let _ = session.Shutdown();
    Ok(())
}

unsafe fn place_video(session: &IMFMediaSession, child: HWND) {
    if child.0.is_null() || !IsWindow(Some(child)).as_bool() {
        return;
    }
    let Ok(gs) = session.cast::<IMFGetService>() else { return };
    let Ok(vc) = gs.GetService::<IMFVideoDisplayControl>(&MR_VIDEO_RENDER_SERVICE) else { return };
    let mut rc = RECT::default();
    if GetClientRect(child, &mut rc).is_err() {
        return;
    }
    let _ = vc.SetVideoWindow(child);
    let _ = vc.SetVideoPosition(std::ptr::null(), &rc);
}
