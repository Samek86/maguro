//! Audio and video in Quick Look and the preview pane.
//!
//! Windows Media Foundation (MFPlay, `IMFPMediaPlayer`) draws video into a child
//! window and plays audio with no video window. Both views call the same player.
//! Closing the view, stepping to another file, or losing the file shuts the player
//! down before the next file starts, so sound does not keep going.
//!
//! Quick Look already uses Space to close and the arrow keys to move between files
//! or scroll. Those stay. Playback keys are K, J, L, M, and Shift+arrows.
//! The preview pane does not use Space, so Space pauses there as well.
//!
//! A file Media Foundation cannot open leaves this idle. The view then paints the
//! thumbnail or the information card it already built.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Media::MediaFoundation::{
    IMFVideoDisplayControl, IMFPMediaPlayer, IMFPMediaPlayerCallback, IMFPMediaPlayerCallback_Impl, MFPCreateMediaPlayer, MFGetService, MFStartup, MFP_EVENT_HEADER,
    MFP_EVENT_TYPE_ERROR, MFP_EVENT_TYPE_FRAME_STEP, MFP_EVENT_TYPE_MEDIAITEM_SET, MFP_EVENT_TYPE_PAUSE, MFP_EVENT_TYPE_PLAY, MFP_EVENT_TYPE_PLAYBACK_ENDED,
    MFP_EVENT_TYPE_POSITION_SET, MFP_MEDIAPLAYER_STATE_PLAYING, MFP_MEDIAPLAYER_STATE_STOPPED, MFP_OPTION_NONE, MFP_POSITIONTYPE_100NS, MFSTARTUP_FULL,
    MFVideoARMode_PreservePicture, MFVideoRenderPrefs_DoNotRepaintOnStop, MF_VERSION, MR_VIDEO_RENDER_SERVICE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Variant::{VT_I8, VT_UI8};
use windows::core::implement;

use crate::imports::*;
use crate::ui::*;

/// Playback starts as soon as a file opens. Both views share this. There is no settings row.
pub(crate) const MEDIA_AUTOPLAY: bool = true;

/// Posted to the view that owns the player. The event itself sits in a queue.
pub(crate) const WM_MEDIA: u32 = WM_APP + 98;
/// Peek uses 13. The preview pane has no timer of its own. 32 is free in `app::ids`.
pub(crate) const TIMER_MEDIA: usize = 32;
const TIMER_MS: u32 = 200;

/// 100-nanosecond units, which is what MFPlay reports.
pub(crate) const HNS_PER_SEC: i64 = 10_000_000;
pub(crate) const SEEK_STEP: i64 = 5 * HNS_PER_SEC;
const VOLUME_STEP: f32 = 0.05;
/// Pause this close to the end. MFPlay clears the picture when the natural end stops the session.
pub(crate) const END_GUARD: i64 = 80 * HNS_PER_SEC / 1000;
/// After the media item is set, keep re-asserting the video rect. MFPlay can move the child on start.
const STICK_MS: u64 = 800;

const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_VOL: &str = "\u{E767}";
const ICON_MUTE: &str = "\u{E74F}";

const VIDEO: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "flv", "asf",
];
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff", "aif", "mid", "midi"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaKind {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaCmd {
    Toggle,
    SeekBack,
    SeekFwd,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl MediaCmd {
    fn repeats(self) -> bool {
        !matches!(self, Self::Toggle | Self::Mute)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Play,
    Seek,
    Mute,
    Volume,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TransportGeom {
    pub play: RECT,
    pub time: RECT,
    pub seek: RECT,
    pub seek_hit: RECT,
    pub mute: RECT,
    pub volume: RECT,
    pub volume_hit: RECT,
}

impl Default for TransportGeom {
    fn default() -> Self {
        Self {
            play: RECT::default(),
            time: RECT::default(),
            seek: RECT::default(),
            seek_hit: RECT::default(),
            mute: RECT::default(),
            volume: RECT::default(),
            volume_hit: RECT::default(),
        }
    }
}

pub(crate) fn media_kind(ext: &str) -> Option<MediaKind> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let e = e.as_str();
    if VIDEO.contains(&e) {
        Some(MediaKind::Video)
    } else if AUDIO.contains(&e) {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

pub(crate) fn is_video(ext: &str) -> bool {
    media_kind(ext) == Some(MediaKind::Video)
}

pub(crate) fn is_audio(ext: &str) -> bool {
    media_kind(ext) == Some(MediaKind::Audio)
}

/// A zero-length file, or one that is not a file, is not handed to Media Foundation.
pub(crate) fn playable_len(is_file: bool, len: u64) -> bool {
    is_file && len > 0
}

pub(crate) fn transport_height(dpi: i32) -> i32 {
    (40 * dpi.max(1) / 96).max(1)
}

/// Content above, transport along the bottom. A short area gives the whole rect to the bar.
pub(crate) fn split_content(area: RECT, dpi: i32) -> (RECT, RECT) {
    let span = (area.bottom - area.top).max(0);
    let h = transport_height(dpi).min(span);
    let top = area.bottom - h;
    (RECT { bottom: top, ..area }, RECT { top, ..area })
}

/// Push `area` down so its top is at or below `header_bottom`. The video child
/// must not cover the attribute header.
pub(crate) fn below_header(header_bottom: i32, area: RECT) -> RECT {
    let bottom = area.bottom.max(area.top);
    let top = area.top.max(header_bottom).min(bottom);
    RECT { top, ..area }
}

/// Where the video child sits: under the attribute header, above the transport.
///
/// `header_bottom` is a y inside `area` (the full preview client, or Quick Look's
/// stage). Pass `area.top` when `area` is already below the header.
/// The child fills this rect; Media Foundation letterboxes inside it.
pub(crate) fn video_dest(area: RECT, header_bottom: i32, dpi: i32) -> RECT {
    let (content, _) = split_content(below_header(header_bottom, area), dpi);
    content
}

/// The picture is shown as soon as `dest` has area.
///
/// Alt+P used to wait until the preview job had published file facts. Until then
/// the child stayed hidden, the pane filled this rect with its background, and the
/// video area was solid black while the transport already showed the duration.
/// Quick Look never waited. Facts only change `dest` (a taller header). They do
/// not decide whether a frame may be on screen.
pub(crate) fn reveal_video(dest: RECT) -> bool {
    live(dest)
}

/// Where playback sits relative to the end of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndChoice {
    /// Keep the decoder at `pos`.
    Play { pos: i64 },
    /// Pause on a real frame. `player` is the decoder; `shown` is the clock (the duration).
    Hold { player: i64, shown: i64 },
}

/// `Hold` once `pos` is inside the end guard, so the session never reaches the clear-on-stop.
pub(crate) fn end_choice(pos: i64, dur: i64) -> EndChoice {
    let pos = pos.max(0);
    if dur > END_GUARD && pos >= dur - END_GUARD {
        EndChoice::Hold { player: dur - END_GUARD, shown: dur }
    } else {
        EndChoice::Play { pos }
    }
}

/// The session already stopped at the end and rewound. Park on the last frame, or the first if the clip is shorter than the guard.
pub(crate) fn after_end(dur: i64) -> EndChoice {
    if dur <= 0 {
        EndChoice::Hold { player: 0, shown: 0 }
    } else if dur <= END_GUARD {
        EndChoice::Hold { player: 0, shown: dur }
    } else {
        EndChoice::Hold { player: dur - END_GUARD, shown: dur }
    }
}

/// Stage height for an audio card drawn above the transport.
/// `rows` property lines, plus two spare lines so a wrapped path still clears the bar.
/// `icon` is the album-art side. `inset_v` is the padding above and below the card.
pub(crate) fn audio_stage_height(rows: i32, row_h: i32, icon: i32, transport: i32, inset_v: i32) -> i32 {
    let row_h = row_h.max(0);
    let text = row_h.saturating_mul(rows.max(1)).saturating_add(row_h.saturating_mul(2));
    let block = text.max(icon.max(0));
    inset_v.max(0).saturating_add(block).saturating_add(transport.max(0))
}

pub(crate) fn transport_geom(bar: RECT, dpi: i32) -> TransportGeom {
    let s = |v: i32| v * dpi.max(1) / 96;
    let pad = s(8);
    let bh = (bar.bottom - bar.top).max(0);
    let btn = s(28).min((bh - s(4)).max(0));
    if btn < s(16) || bar.right - bar.left < pad * 2 + btn {
        return TransportGeom::default();
    }
    let y = bar.top + (bh - btn) / 2;
    let play = RECT { left: bar.left + pad, top: y, right: bar.left + pad + btn, bottom: y + btn };
    let time_w = s(96);
    let time = RECT { left: play.right + s(6), top: bar.top, right: play.right + s(6) + time_w, bottom: bar.bottom };
    let vol_w = s(72);
    let right = bar.right - pad;
    let volume = RECT {
        left: right - vol_w,
        top: y + btn / 2 - s(3),
        right,
        bottom: y + btn / 2 + s(3),
    };
    let mute = RECT { left: volume.left - s(4) - btn, top: y, right: volume.left - s(4), bottom: y + btn };
    let seek_left = time.right + s(8);
    let seek_right = mute.left - s(8);
    let seek_h = s(6).max(4);
    let sy = bar.top + (bh - seek_h) / 2;
    let seek = if seek_right - seek_left >= s(24) {
        RECT { left: seek_left, top: sy, right: seek_right, bottom: sy + seek_h }
    } else {
        RECT::default()
    };
    let hit_pad = s(8);
    let seek_hit = if seek.right > seek.left {
        RECT {
            left: seek.left,
            top: (seek.top - hit_pad).max(bar.top),
            right: seek.right,
            bottom: (seek.bottom + hit_pad).min(bar.bottom),
        }
    } else {
        RECT::default()
    };
    let volume_hit = RECT {
        left: volume.left,
        top: (volume.top - hit_pad).max(bar.top),
        right: volume.right,
        bottom: (volume.bottom + hit_pad).min(bar.bottom),
    };
    // A narrow bar keeps play, time, and mute. Drop the volume track, then the clock, if they collide.
    if mute.left < time.right + s(4) {
        let mute = RECT { left: (bar.right - pad - btn).max(play.right + s(4)), top: y, right: bar.right - pad, bottom: y + btn };
        return TransportGeom { play, time: RECT::default(), seek: RECT::default(), seek_hit: RECT::default(), mute, volume: RECT::default(), volume_hit: RECT::default() };
    }
    if volume.left < mute.right {
        return TransportGeom { play, time, seek, seek_hit, mute, volume: RECT::default(), volume_hit: RECT::default() };
    }
    TransportGeom { play, time, seek, seek_hit, mute, volume, volume_hit }
}

fn live(r: RECT) -> bool {
    r.right > r.left && r.bottom > r.top
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    live(r) && x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub(crate) fn hit_transport(g: &TransportGeom, x: i32, y: i32) -> Option<Part> {
    if inside(g.play, x, y) {
        Some(Part::Play)
    } else if inside(g.mute, x, y) {
        Some(Part::Mute)
    } else if inside(g.volume_hit, x, y) || inside(g.volume, x, y) {
        Some(Part::Volume)
    } else if inside(g.seek_hit, x, y) || inside(g.seek, x, y) {
        Some(Part::Seek)
    } else {
        None
    }
}

pub(crate) fn ratio_along(track: RECT, x: i32) -> f32 {
    let w = track.right - track.left;
    if w <= 0 {
        return 0.0;
    }
    ((x - track.left) as f32 / w as f32).clamp(0.0, 1.0)
}

pub(crate) fn seek_target(dur: i64, ratio: f32) -> i64 {
    if dur <= 0 || !ratio.is_finite() {
        return 0;
    }
    let t = dur as f64 * (ratio as f64).clamp(0.0, 1.0);
    if !t.is_finite() {
        return 0;
    }
    (t.round() as i64).clamp(0, dur)
}

pub(crate) fn seek_step(pos: i64, dur: i64, delta: i64) -> i64 {
    let next = pos.saturating_add(delta).max(0);
    if dur > 0 { next.min(dur) } else { next }
}

pub(crate) fn volume_set(cur: f32, delta: f32) -> f32 {
    if !cur.is_finite() {
        return 1.0;
    }
    (cur + delta).clamp(0.0, 1.0)
}

/// `hns` is a Media Foundation position. Hours appear only past 60 minutes.
pub(crate) fn format_media_time(hns: i64) -> String {
    let total = if hns <= 0 { 0 } else { (hns / HNS_PER_SEC) as u64 };
    let s = total % 60;
    let m = (total / 60) % 60;
    let h = total / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `space_closes` is Quick Look: Space, and plain arrows, already do something else.
pub(crate) fn media_key(vk: u16, shift: bool, ctrl: bool, alt: bool, space_closes: bool) -> Option<MediaCmd> {
    if ctrl || alt {
        return None;
    }
    const J: u16 = b'J' as u16;
    const K: u16 = b'K' as u16;
    const L: u16 = b'L' as u16;
    const M: u16 = b'M' as u16;
    match vk {
        0x20 if !space_closes && !shift => Some(MediaCmd::Toggle),
        K if !shift => Some(MediaCmd::Toggle),
        J if !shift => Some(MediaCmd::SeekBack),
        L if !shift => Some(MediaCmd::SeekFwd),
        M if !shift => Some(MediaCmd::Mute),
        0x25 if shift => Some(MediaCmd::SeekBack),
        0x27 if shift => Some(MediaCmd::SeekFwd),
        0x26 if shift => Some(MediaCmd::VolumeUp),
        0x28 if shift => Some(MediaCmd::VolumeDown),
        _ => None,
    }
}

fn file_playable(path: &str) -> bool {
    let Ok(f) = std::fs::File::open(path) else { return false };
    match f.metadata() {
        Ok(m) => playable_len(m.is_file(), m.len()),
        Err(_) => false,
    }
}

// ---------- player (UI thread) ----------

struct Ev {
    host: isize,
    gen: u64,
    kind: i32,
    hr: i32,
}

/// One Media Foundation open that the view can fall back from, once.
pub(crate) struct OpenFailure {
    pub view_gen: u64,
    pub path: String,
}

struct Slot {
    gen: u64,
    /// View generation that asked for this open. A later file has a different one.
    view_gen: u64,
    path: String,
    kind: MediaKind,
    player: Option<IMFPMediaPlayer>,
    video: HWND,
    failed: bool,
    /// `fail` set this. The view takes it once, then starts a handler or leaves the info view.
    fail_pending: bool,
    ready: bool,
    playing: bool,
    /// Paused on the last frame. The clock shows the duration. Play starts over at 0.
    holding: bool,
    /// Seek back onto a frame after MFPlay has already cleared the picture.
    want_still: bool,
    /// One restore seek per end, so a second end event cannot loop.
    restore_used: bool,
    muted: bool,
    volume: f32,
    pos: i64,
    dur: i64,
    drag: Option<Part>,
    down: Option<Part>,
    hot: Option<Part>,
    geom: TransportGeom,
    bar: RECT,
    pending: Option<RECT>,
    placed: RECT,
    /// The video child may be visible. Until then WM_WINDOWPOSCHANGING keeps it hidden.
    shown: bool,
    /// Re-apply `placed` even when the rect has not changed (MFPlay moves the child on start).
    force_place: bool,
    /// The view should place the video and repaint the header.
    relayout: bool,
    /// Re-assert the video rect until this instant. `None` once it has passed.
    stick_until: Option<std::time::Instant>,
}

fn slots() -> &'static std::cell::RefCell<HashMap<isize, Slot>> {
    thread_local! {
        static SLOTS: std::cell::RefCell<HashMap<isize, Slot>> = std::cell::RefCell::new(HashMap::new());
    }
    SLOTS.with(|s| {
        // Extend the borrow by leaking a pointer to the thread-local. The RefCell
        // lives for the thread, and every caller drops the borrow before returning.
        let p = s as *const std::cell::RefCell<HashMap<isize, Slot>>;
        unsafe { &*p }
    })
}

fn queue() -> &'static Mutex<Vec<Ev>> {
    static Q: OnceLock<Mutex<Vec<Ev>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

static GEN: AtomicU64 = AtomicU64::new(1);
static MF_ON: std::sync::Once = std::sync::Once::new();
static VID_CLASS: std::sync::Once = std::sync::Once::new();

fn key_of(h: HWND) -> isize {
    h.0 as isize
}

fn ensure_mf() {
    MF_ON.call_once(|| unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    });
}

#[implement(IMFPMediaPlayerCallback)]
struct Cb {
    host: isize,
    gen: u64,
}

impl IMFPMediaPlayerCallback_Impl for Cb_Impl {
    fn OnMediaPlayerEvent(&self, peventheader: *const MFP_EVENT_HEADER) {
        if peventheader.is_null() {
            return;
        }
        let (kind, hr) = unsafe {
            let e = &*peventheader;
            (e.eEventType.0, e.hrEvent.0)
        };
        if let Ok(mut q) = queue().lock() {
            q.push(Ev { host: self.host, gen: self.gen, kind, hr });
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(self.host as *mut std::ffi::c_void)), WM_MEDIA, WPARAM(0), LPARAM(0));
        }
    }
}

pub(crate) fn covers(host: HWND) -> bool {
    slots().borrow().get(&key_of(host)).is_some_and(|s| s.player.is_some() && !s.failed)
}

pub(crate) fn has_video(host: HWND) -> bool {
    slots().borrow().get(&key_of(host)).is_some_and(|s| s.player.is_some() && !s.failed && s.kind == MediaKind::Video && !s.video.0.is_null())
}

/// Open `path` when it is audio or video. Anything else releases a player this view still holds.
/// Tests use this. The views pass a video rect through [`open_in`].
#[cfg(test)]
pub(crate) unsafe fn open(host: HWND, path: &str) {
    open_in(host, path, None);
}

/// Like [`open_bound`] with no view generation. Tests use this.
#[cfg(test)]
pub(crate) unsafe fn open_in(host: HWND, path: &str, dest: Option<RECT>) {
    open_bound(host, path, dest, 0);
}

/// Like [`open_in`]. `view_gen` is the Quick Look or preview-pane generation for this file.
/// A later failure is reported with that generation so a stale open is ignored.
///
/// For video, show the child at `dest` before the player is created.
///
/// MFPlay binds the EVR to the HWND it is given. A window that is still 8×8 and
/// hidden at that moment presents nothing, and a later paint of the pane is all
/// the user sees. `dest` is the rect from [`video_dest`]. `None` leaves placement
/// to the view (audio, or a host that does not have a size yet).
pub(crate) unsafe fn open_bound(host: HWND, path: &str, dest: Option<RECT>, view_gen: u64) {
    let ext = super::preview::ext_of(path);
    let Some(kind) = media_kind(&ext) else {
        close(host);
        return;
    };
    let key = key_of(host);
    // The same generation keeps a player that is already up, or a failure the view already saw.
    // A new generation retries, so a theme change can open the file again.
    let same = slots().borrow().get(&key).is_some_and(|s| s.path == path && s.view_gen == view_gen && (s.player.is_some() || s.failed));
    if same {
        return;
    }
    let (volume, muted) = slots().borrow().get(&key).map(|s| (s.volume, s.muted)).unwrap_or((1.0, false));
    close(host);
    if host.0.is_null() || !file_playable(path) {
        return;
    }
    ensure_mf();
    let gen = GEN.fetch_add(1, Ordering::Relaxed);
    let dest = dest.filter(|r| reveal_video(*r));
    let video = if kind == MediaKind::Video { create_video(host, dest.unwrap_or_default()) } else { HWND::default() };
    if kind == MediaKind::Video && video.0.is_null() {
        return;
    }
    slots().borrow_mut().insert(
        key,
        Slot {
            gen,
            view_gen,
            path: path.to_string(),
            kind,
            player: None,
            video,
            failed: false,
            fail_pending: false,
            ready: false,
            playing: false,
            holding: false,
            want_still: false,
            restore_used: false,
            muted,
            volume,
            pos: 0,
            dur: 0,
            drag: None,
            down: None,
            hot: None,
            geom: TransportGeom::default(),
            bar: RECT::default(),
            pending: None,
            placed: RECT::default(),
            shown: false,
            force_place: false,
            relayout: false,
            stick_until: None,
        },
    );
    // Size and show the child before MFPlay binds the renderer to it.
    if let Some(dest) = dest {
        queue_place(host, dest);
        apply_place(host);
    }
    let cb: IMFPMediaPlayerCallback = Cb { host: key, gen }.into();
    let mut made: Option<IMFPMediaPlayer> = None;
    let hwnd = if video.0.is_null() { None } else { Some(video) };
    let opened = MFPCreateMediaPlayer(&HSTRING::from(path), MEDIA_AUTOPLAY, MFP_OPTION_NONE, &cb, hwnd, Some(&mut made));
    if opened.is_err() || made.is_none() {
        fail(host);
        return;
    }
    if let Some(slot) = slots().borrow_mut().get_mut(&key) {
        slot.player = made;
    } else {
        if let Some(p) = made {
            let _ = p.Shutdown();
        }
        if !video.0.is_null() {
            let _ = DestroyWindow(video);
        }
        return;
    }
    let _ = SetTimer(Some(host), TIMER_MEDIA, TIMER_MS, None);
    apply_events();
    let _ = InvalidateRect(Some(host), None, false);
}

pub(crate) unsafe fn close(host: HWND) {
    if host.0.is_null() {
        return;
    }
    let key = key_of(host);
    let _ = KillTimer(Some(host), TIMER_MEDIA);
    let removed = slots().borrow_mut().remove(&key);
    if let Ok(mut q) = queue().lock() {
        q.retain(|e| e.host != key);
    }
    let Some(slot) = removed else { return };
    if let Some(p) = slot.player {
        let _ = p.Stop();
        let _ = p.Shutdown();
    }
    if !slot.video.0.is_null() && IsWindow(Some(slot.video)).as_bool() {
        let _ = DestroyWindow(slot.video);
    }
}

/// The open that just failed, once. `None` when the player is fine or the view already took it.
pub(crate) fn take_open_failure(host: HWND) -> Option<OpenFailure> {
    let mut map = slots().borrow_mut();
    let slot = map.get_mut(&key_of(host))?;
    if !slot.failed || !slot.fail_pending {
        return None;
    }
    slot.fail_pending = false;
    Some(OpenFailure { view_gen: slot.view_gen, path: slot.path.clone() })
}

pub(crate) unsafe fn on_posted(host: HWND) {
    let _ = host;
    apply_events();
}

pub(crate) unsafe fn tick(host: HWND) {
    let key = key_of(host);
    let Some(path) = slots().borrow().get(&key).filter(|s| s.player.is_some() && !s.failed).map(|s| s.path.clone()) else {
        let _ = KillTimer(Some(host), TIMER_MEDIA);
        return;
    };
    if file_gone(&path) {
        close(host);
        let _ = InvalidateRect(Some(host), None, false);
        return;
    }
    refresh(host);
    arm_clock(host);
    invalidate_bar(host);
}

/// `true` when the view should place the video child again and repaint the header.
pub(crate) unsafe fn take_relayout(host: HWND) -> bool {
    let mut map = slots().borrow_mut();
    let Some(slot) = map.get_mut(&key_of(host)) else { return false };
    let due = slot.relayout;
    slot.relayout = false;
    due
}

pub(crate) unsafe fn command(host: HWND, cmd: MediaCmd, repeat: bool) {
    if !covers(host) || (repeat && !cmd.repeats()) {
        return;
    }
    let key = key_of(host);
    let act = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        let player = slot.player.clone();
        let act = match cmd {
            MediaCmd::Toggle => {
                if slot.holding {
                    slot.holding = false;
                    slot.want_still = false;
                    slot.restore_used = false;
                    slot.playing = true;
                    slot.pos = 0;
                    Act::Replay
                } else if slot.playing {
                    slot.playing = false;
                    Act::Pause
                } else {
                    slot.playing = true;
                    Act::Play
                }
            }
            MediaCmd::SeekBack => choose_seek(slot, seek_step(slot.pos, slot.dur, -SEEK_STEP)),
            MediaCmd::SeekFwd => choose_seek(slot, seek_step(slot.pos, slot.dur, SEEK_STEP)),
            MediaCmd::VolumeUp => {
                slot.volume = volume_set(slot.volume, VOLUME_STEP);
                slot.muted = false;
                Act::Volume(slot.volume)
            }
            MediaCmd::VolumeDown => {
                slot.volume = volume_set(slot.volume, -VOLUME_STEP);
                slot.muted = false;
                Act::Volume(slot.volume)
            }
            MediaCmd::Mute => {
                slot.muted = !slot.muted;
                Act::Mute(slot.muted)
            }
        };
        (player, act)
    };
    let (player, act) = act;
    if let Some(p) = player {
        run_act(&p, act);
    }
    invalidate_bar(host);
}

pub(crate) unsafe fn sync_theme(host: HWND) {
    let p = slots().borrow().get(&key_of(host)).and_then(|s| s.player.clone());
    if let Some(p) = p {
        let _ = p.SetBorderColor(cref(pal().bg));
        let _ = p.UpdateVideo();
    }
}

pub(crate) unsafe fn queue_place(host: HWND, content: RECT) {
    if let Some(slot) = slots().borrow_mut().get_mut(&key_of(host)) {
        if slot.kind == MediaKind::Video && !slot.video.0.is_null() {
            slot.pending = Some(content);
        }
    }
}

pub(crate) unsafe fn apply_place(host: HWND) {
    let key = key_of(host);
    let (video, rect, player) = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        let Some(rect) = slot.pending.take() else { return };
        let force = slot.force_place;
        slot.force_place = false;
        if !force && slot.shown && rect_eq(slot.placed, rect) {
            return;
        }
        slot.placed = rect;
        if !slot.shown {
            // The child becomes visible only once the header is final. Pin it there
            // for a short while: MFPlay moves the window again as the first frame arrives.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(STICK_MS);
            slot.stick_until = Some(slot.stick_until.map(|t| t.max(until)).unwrap_or(until));
            slot.relayout = true;
        }
        slot.shown = true;
        (slot.video, rect, slot.player.clone())
    };
    if video.0.is_null() || !IsWindow(Some(video)).as_bool() {
        return;
    }
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    // NOREDRAW so this does not nest a paint. The caller invalidates the header before BeginPaint.
    let _ = SetWindowPos(video, None, rect.left, rect.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_SHOWWINDOW);
    if let Some(p) = player {
        let _ = p.UpdateVideo();
    }
    let _ = InvalidateRect(Some(video), None, false);
    repaint_header(host);
}

/// Keep the video child hidden until the header height is final. A window shown
/// on the one-line opening header stays on top of the lines that arrive later,
/// and WS_CLIPCHILDREN then skips those lines.
pub(crate) unsafe fn hide_video(host: HWND) {
    let key = key_of(host);
    let video = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        if slot.kind != MediaKind::Video {
            return;
        }
        slot.shown = false;
        slot.placed = RECT::default();
        slot.pending = None;
        slot.force_place = false;
        slot.video
    };
    if video.0.is_null() || !IsWindow(Some(video)).as_bool() {
        return;
    }
    let vis = IsWindowVisible(video).as_bool();
    let _ = SetWindowPos(video, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_HIDEWINDOW);
    if vis {
        let _ = InvalidateRect(Some(host), None, false);
    }
}

/// Repaint the strip above the video child. Call before BeginPaint when the child just moved.
pub(crate) unsafe fn repaint_header(host: HWND) {
    let placed = slots().borrow().get(&key_of(host)).map(|s| s.placed).unwrap_or_default();
    let mut rc = RECT::default();
    let _ = GetClientRect(host, &mut rc);
    if live(placed) {
        rc.bottom = placed.top.max(rc.top);
    }
    if rc.bottom > rc.top {
        let _ = InvalidateRect(Some(host), Some(&rc), false);
    }
}

pub(crate) unsafe fn paint_transport(dc: HDC, host: HWND, bar: RECT, font: HFONT, dpi: i32, accent: u32) {
    let g = transport_geom(bar, dpi);
    let (playing, muted, volume, pos, dur, hot, down) = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key_of(host)) else { return };
        slot.geom = g;
        slot.bar = bar;
        (slot.playing, slot.muted, slot.volume, slot.pos, slot.dur, slot.hot, slot.down.or(slot.drag))
    };
    fill(dc, bar, pal().bg);
    if bar.bottom > bar.top {
        fill(dc, RECT { bottom: bar.top + 1, ..bar }, pal().line);
    }
    let ground = pal().bg;
    let glyphs = icon_font();
    let play_vis = visual_from_pointer(Part::Play, hot, down);
    paint_button(dc, g.play, ButtonKind::Icon, play_vis, if playing { ICON_PAUSE } else { ICON_PLAY }, glyphs, dpi, ground, FocusRing::Inside);
    if live(g.time) {
        let label = format!("{} / {}", format_media_time(pos), format_media_time(dur));
        text(dc, g.time, &label, font, pal().sub, DT_CENTER | DT_VCENTER);
    }
    if live(g.seek) {
        paint_slider(dc, g.seek, if dur > 0 { pos as f32 / dur as f32 } else { 0.0 }, accent, dpi);
    }
    let mute_vis = visual_from_pointer(Part::Mute, hot, down);
    paint_button(dc, g.mute, ButtonKind::Icon, mute_vis, if muted { ICON_MUTE } else { ICON_VOL }, glyphs, dpi, ground, FocusRing::Inside);
    if live(g.volume) {
        paint_slider(dc, g.volume, if muted { 0.0 } else { volume }, accent, dpi);
    }
}

pub(crate) unsafe fn pointer_down(host: HWND, x: i32, y: i32) -> bool {
    if !covers(host) {
        return false;
    }
    let key = key_of(host);
    let hit = slots().borrow().get(&key).and_then(|s| hit_transport(&s.geom, x, y));
    let Some(part) = hit else { return false };
    match part {
        Part::Play => command(host, MediaCmd::Toggle, false),
        Part::Mute => command(host, MediaCmd::Mute, false),
        Part::Seek => set_ratio(host, Part::Seek, x),
        Part::Volume => set_ratio(host, Part::Volume, x),
    }
    if let Some(slot) = slots().borrow_mut().get_mut(&key) {
        slot.down = Some(part);
        slot.drag = matches!(part, Part::Seek | Part::Volume).then_some(part);
        slot.hot = Some(part);
    }
    let _ = SetCapture(host);
    invalidate_bar(host);
    true
}

/// `true` while a seek or volume drag owns the pointer.
pub(crate) unsafe fn pointer_move(host: HWND, x: i32, y: i32) -> bool {
    if !covers(host) {
        return false;
    }
    let key = key_of(host);
    let drag = slots().borrow().get(&key).and_then(|s| s.drag);
    if let Some(part) = drag {
        set_ratio(host, part, x);
        return true;
    }
    let next = slots().borrow().get(&key).and_then(|s| hit_transport(&s.geom, x, y));
    let changed = slots().borrow().get(&key).is_some_and(|s| s.hot != next);
    if changed {
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.hot = next;
        }
        invalidate_bar(host);
    }
    false
}

pub(crate) unsafe fn pointer_up(host: HWND) -> bool {
    let key = key_of(host);
    let had = slots().borrow().get(&key).is_some_and(|s| s.down.is_some() || s.drag.is_some());
    if !had {
        return false;
    }
    if let Some(slot) = slots().borrow_mut().get_mut(&key) {
        slot.down = None;
        slot.drag = None;
    }
    if GetCapture() == host {
        let _ = ReleaseCapture();
    }
    invalidate_bar(host);
    true
}

pub(crate) unsafe fn pointer_cancel(host: HWND) {
    let key = key_of(host);
    if let Some(slot) = slots().borrow_mut().get_mut(&key) {
        slot.down = None;
        slot.drag = None;
    }
}

enum Act {
    Play,
    Pause,
    Seek(i64),
    /// Seek to the start and play. Used after the end-of-file hold.
    Replay,
    /// Seek onto a frame and pause. `want_still` then steps that frame into the window.
    Hold(i64),
    Volume(f32),
    Mute(bool),
}

/// Seek, and pause before the natural end so the picture is not cleared.
fn choose_seek(slot: &mut Slot, pos: i64) -> Act {
    match end_choice(pos, slot.dur) {
        EndChoice::Hold { player, shown } => {
            slot.holding = true;
            slot.playing = false;
            slot.pos = shown;
            slot.want_still = slot.kind == MediaKind::Video;
            Act::Hold(player)
        }
        EndChoice::Play { pos } => {
            slot.holding = false;
            slot.want_still = false;
            slot.restore_used = false;
            slot.pos = pos;
            Act::Seek(pos)
        }
    }
}

unsafe fn run_act(player: &IMFPMediaPlayer, act: Act) {
    match act {
        Act::Play => {
            let _ = player.Play();
        }
        Act::Pause => {
            let _ = player.Pause();
        }
        Act::Seek(pos) => write_hns(player, pos),
        Act::Replay => {
            write_hns(player, 0);
            let _ = player.Play();
        }
        Act::Hold(pos) => {
            write_hns(player, pos);
            let _ = player.Pause();
        }
        Act::Volume(v) => {
            let _ = player.SetMute(false);
            let _ = player.SetVolume(v);
        }
        Act::Mute(m) => {
            let _ = player.SetMute(m);
        }
    }
}

unsafe fn set_ratio(host: HWND, part: Part, x: i32) {
    let key = key_of(host);
    let act = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        let player = slot.player.clone();
        let act = match part {
            Part::Seek => choose_seek(slot, seek_target(slot.dur, ratio_along(slot.geom.seek, x))),
            Part::Volume => {
                let v = ratio_along(slot.geom.volume, x);
                slot.volume = v;
                slot.muted = false;
                Act::Volume(v)
            }
            _ => return,
        };
        (player, act)
    };
    if let (Some(p), act) = act {
        run_act(&p, act);
    }
    invalidate_bar(host);
}

enum Park {
    /// The frame on screen is the last one. Pausing keeps it; stopping would clear it.
    Pause,
    /// The session already rewound and cleared. Seek back onto a frame.
    Restore(i64),
}

unsafe fn refresh(host: HWND) {
    let key = key_of(host);
    let player = slots().borrow().get(&key).and_then(|s| s.player.clone());
    let Some(player) = player else { return };
    let pos = read_hns(&player, false);
    let dur = read_hns(&player, true);
    let state = player.GetState().ok();
    let playing = state == Some(MFP_MEDIAPLAYER_STATE_PLAYING);
    let stopped = state == Some(MFP_MEDIAPLAYER_STATE_STOPPED);
    let park = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        if let Some(dur) = dur {
            if dur > slot.dur {
                slot.dur = dur;
            }
        }
        let dur = slot.dur;
        let was_playing = slot.playing;
        let mut park = None;
        if !slot.holding {
            if let Some(pos) = pos {
                slot.pos = pos;
            }
            slot.playing = playing;
            let near_end = playing && matches!(end_choice(slot.pos, dur), EndChoice::Hold { .. });
            // A natural end rewinds to 0 and reports stopped. A user pause stays paused.
            let rewound = was_playing && stopped && dur > 0 && slot.pos <= END_GUARD;
            if near_end {
                if let EndChoice::Hold { shown, .. } = end_choice(slot.pos, dur) {
                    slot.holding = true;
                    slot.playing = false;
                    slot.pos = shown;
                    park = Some(Park::Pause);
                }
            } else if rewound && !slot.restore_used {
                if let EndChoice::Hold { player, shown } = after_end(dur) {
                    slot.holding = true;
                    slot.playing = false;
                    slot.pos = shown;
                    slot.restore_used = true;
                    slot.want_still = slot.kind == MediaKind::Video;
                    park = Some(Park::Restore(player));
                }
            }
        } else {
            slot.playing = false;
            if slot.pos <= 0 && dur > 0 {
                slot.pos = dur;
            }
        }
        let now = std::time::Instant::now();
        if slot.kind == MediaKind::Video && slot.stick_until.is_some_and(|t| now < t) {
            slot.force_place = true;
            slot.relayout = true;
        } else {
            slot.stick_until = None;
        }
        park
    };
    match park {
        Some(Park::Pause) => {
            let _ = player.Pause();
        }
        Some(Park::Restore(pos)) => {
            write_hns(&player, pos);
            let _ = player.Pause();
        }
        None => {}
    }
}

unsafe fn arm_clock(host: HWND) {
    let (playing, holding, pos, dur) = {
        let map = slots().borrow();
        let Some(slot) = map.get(&key_of(host)) else { return };
        (slot.playing, slot.holding, slot.pos, slot.dur)
    };
    let ms = if playing && !holding && dur > pos {
        let remain_ms = ((dur - pos) / (HNS_PER_SEC / 1000)).max(0) as u32;
        if remain_ms >= TIMER_MS {
            TIMER_MS
        } else {
            remain_ms.saturating_sub((END_GUARD / (HNS_PER_SEC / 1000)) as u32).max(15)
        }
    } else {
        TIMER_MS
    };
    let _ = SetTimer(Some(host), TIMER_MEDIA, ms, None);
}

unsafe fn apply_events() {
    let evs: Vec<Ev> = match queue().lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => return,
    };
    let mut later = Vec::new();
    for ev in evs {
        let host = HWND(ev.host as *mut std::ffi::c_void);
        let ready = slots().borrow().get(&ev.host).is_some_and(|s| s.gen == ev.gen && s.player.is_some());
        let pending = slots().borrow().get(&ev.host).is_some_and(|s| s.gen == ev.gen && s.player.is_none() && !s.failed);
        if pending {
            later.push(ev);
            continue;
        }
        if ready {
            apply_one(host, ev);
        }
    }
    if !later.is_empty() {
        if let Ok(mut q) = queue().lock() {
            q.extend(later);
        }
    }
}

unsafe fn apply_one(host: HWND, ev: Ev) {
    let failed = ev.hr < 0 && (ev.kind == MFP_EVENT_TYPE_ERROR.0 || ev.kind == MFP_EVENT_TYPE_MEDIAITEM_SET.0);
    if ev.kind == MFP_EVENT_TYPE_ERROR.0 || failed {
        fail(host);
        return;
    }
    let key = key_of(host);
    if ev.kind == MFP_EVENT_TYPE_MEDIAITEM_SET.0 {
        let player = slots().borrow().get(&key).and_then(|s| s.player.clone());
        if let Some(p) = player {
            let _ = p.SetAspectRatioMode(MFVideoARMode_PreservePicture.0 as u32);
            let _ = p.SetBorderColor(cref(pal().bg));
            keep_last_frame(&p);
            let vol = slots().borrow().get(&key).map(|s| s.volume).unwrap_or(1.0);
            let muted = slots().borrow().get(&key).map(|s| s.muted).unwrap_or(false);
            let _ = p.SetVolume(vol);
            let _ = p.SetMute(muted);
            let _ = p.UpdateVideo();
        }
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.ready = true;
            if slot.kind == MediaKind::Video {
                // The child is often moved as the first frame appears. Re-place it under the header.
                slot.force_place = true;
                slot.relayout = true;
                slot.stick_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(STICK_MS));
            }
        }
        refresh(host);
        let _ = InvalidateRect(Some(host), None, false);
        return;
    }
    if ev.kind == MFP_EVENT_TYPE_POSITION_SET.0 || ev.kind == MFP_EVENT_TYPE_FRAME_STEP.0 {
        let (player, still) = {
            let mut map = slots().borrow_mut();
            let Some(slot) = map.get_mut(&key) else { return };
            let still = slot.want_still && ev.kind == MFP_EVENT_TYPE_POSITION_SET.0;
            if still {
                slot.want_still = false;
            }
            (slot.player.clone(), still && slot.kind == MediaKind::Video)
        };
        if let Some(p) = player {
            if still {
                let _ = p.FrameStep();
            }
            let _ = p.UpdateVideo();
        }
        invalidate_bar(host);
        return;
    }
    if ev.kind == MFP_EVENT_TYPE_PLAY.0 {
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            if !slot.holding {
                slot.playing = true;
            }
        }
    } else if ev.kind == MFP_EVENT_TYPE_PAUSE.0 {
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.playing = false;
        }
    } else if ev.kind == MFP_EVENT_TYPE_PLAYBACK_ENDED.0 {
        let restore = {
            let mut map = slots().borrow_mut();
            let Some(slot) = map.get_mut(&key) else { return };
            slot.playing = false;
            match after_end(slot.dur) {
                EndChoice::Hold { player, shown } => {
                    slot.holding = true;
                    slot.pos = shown;
                    if slot.restore_used {
                        None
                    } else {
                        slot.restore_used = true;
                        slot.want_still = slot.kind == MediaKind::Video;
                        Some(player)
                    }
                }
                EndChoice::Play { .. } => None,
            }
        };
        if let Some(pos) = restore {
            if let Some(p) = slots().borrow().get(&key).and_then(|s| s.player.clone()) {
                write_hns(&p, pos);
                let _ = p.Pause();
            }
        }
    }
    invalidate_bar(host);
}

/// Ask the EVR not to paint the border colour over the last frame when playback stops.
unsafe fn keep_last_frame(player: &IMFPMediaPlayer) {
    let mut ptr = std::ptr::null_mut();
    if MFGetService(player, &MR_VIDEO_RENDER_SERVICE, &IMFVideoDisplayControl::IID, &mut ptr).is_err() || ptr.is_null() {
        return;
    }
    let display: IMFVideoDisplayControl = IMFVideoDisplayControl::from_raw(ptr);
    let cur = display.GetRenderingPrefs().unwrap_or(0);
    let _ = display.SetRenderingPrefs(cur | MFVideoRenderPrefs_DoNotRepaintOnStop.0 as u32);
}

unsafe fn fail(host: HWND) {
    let key = key_of(host);
    let _ = KillTimer(Some(host), TIMER_MEDIA);
    let (player, video) = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        slot.failed = true;
        slot.fail_pending = true;
        slot.playing = false;
        slot.ready = false;
        let player = slot.player.take();
        let video = slot.video;
        slot.video = HWND::default();
        slot.pending = None;
        (player, video)
    };
    if let Ok(mut q) = queue().lock() {
        q.retain(|e| e.host != key);
    }
    if let Some(p) = player {
        let _ = p.Stop();
        let _ = p.Shutdown();
    }
    if !video.0.is_null() && IsWindow(Some(video)).as_bool() {
        let _ = DestroyWindow(video);
    }
    let _ = InvalidateRect(Some(host), None, false);
}

fn file_gone(path: &str) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => !playable_len(m.is_file(), m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

unsafe fn invalidate_bar(host: HWND) {
    let bar = slots().borrow().get(&key_of(host)).map(|s| s.bar);
    match bar {
        Some(r) if live(r) => {
            let _ = InvalidateRect(Some(host), Some(&r), false);
        }
        Some(_) => {
            let _ = InvalidateRect(Some(host), None, false);
        }
        None => {}
    }
}

unsafe fn paint_slider(dc: HDC, track: RECT, ratio: f32, accent: u32, dpi: i32) {
    let ratio = if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 0.0 };
    fill(dc, track, pal().pill);
    let w = track.right - track.left;
    let played = track.left + (w as f32 * ratio) as i32;
    if played > track.left {
        fill(dc, RECT { right: played.min(track.right), ..track }, accent);
    }
    let tw = (10 * dpi.max(1) / 96).max(6);
    let left = (played - tw / 2).clamp(track.left, (track.right - tw).max(track.left));
    let top = track.top - (tw - (track.bottom - track.top)) / 2;
    fill(dc, RECT { left, top, right: left + tw, bottom: top + tw }, pal().text);
}

fn rect_eq(a: RECT, b: RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

unsafe fn read_hns(player: &IMFPMediaPlayer, duration: bool) -> Option<i64> {
    let mut pv = if duration { player.GetDuration(&MFP_POSITIONTYPE_100NS).ok()? } else { player.GetPosition(&MFP_POSITIONTYPE_100NS).ok()? };
    let v = hns_of(&pv);
    let _ = PropVariantClear(&mut pv);
    v
}

unsafe fn write_hns(player: &IMFPMediaPlayer, ticks: i64) {
    let mut pv = PROPVARIANT::default();
    {
        let inner = &mut pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = ticks.max(0);
    }
    let _ = player.SetPosition(&MFP_POSITIONTYPE_100NS, &pv);
    let _ = PropVariantClear(&mut pv);
}

unsafe fn hns_of(pv: &PROPVARIANT) -> Option<i64> {
    let inner = &pv.Anonymous.Anonymous;
    if inner.vt == VT_I8 {
        Some(inner.Anonymous.hVal)
    } else if inner.vt == VT_UI8 {
        Some(inner.Anonymous.uhVal.min(i64::MAX as u64) as i64)
    } else {
        None
    }
}

fn ensure_class() {
    VID_CLASS.call_once(|| unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = module.into();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let _ = RegisterClassW(&WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(vid_proc),
            hInstance: hinst,
            hCursor: arrow,
            hbrBackground: HBRUSH(GetStockObject(NULL_BRUSH).0),
            lpszClassName: w!("MaguroMediaVid"),
            ..Default::default()
        });
    });
}

unsafe fn create_video(parent: HWND, dest: RECT) -> HWND {
    ensure_class();
    let Ok(module) = GetModuleHandleW(None) else { return HWND::default() };
    let hinst: HINSTANCE = module.into();
    let (x, y, w, h) = if live(dest) {
        (dest.left, dest.top, (dest.right - dest.left).max(1), (dest.bottom - dest.top).max(1))
    } else {
        (0, 0, 8, 8)
    };
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("MaguroMediaVid"),
        w!(""),
        WS_CHILD | WS_CLIPSIBLINGS,
        x,
        y,
        w,
        h,
        Some(parent),
        None,
        Some(hinst),
        None,
    )
    .unwrap_or_default()
}

pub(crate) extern "system" fn vid_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::catch_wnd(AssertUnwindSafe(|| vid_proc_inner(h, msg, wp, lp)))
}

fn vid_proc_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => return LRESULT(1),
            WM_WINDOWPOSCHANGING => {
                // MFPlay repositions this child to the top of the parent when playback starts.
                // Hide it only until the view has published a rect. After that, pin it there
                // so a later header (file facts) is not covered and the picture is not shoved to 0,0.
                let pos = &mut *(lp.0 as *mut WINDOWPOS);
                let parent = GetParent(h).unwrap_or_default();
                let (shown, placed) = slots().borrow().get(&key_of(parent)).map(|s| (s.shown, s.placed)).unwrap_or((false, RECT::default()));
                if !shown {
                    pos.flags = SET_WINDOW_POS_FLAGS(pos.flags.0 & !SWP_SHOWWINDOW.0) | SWP_HIDEWINDOW;
                } else if live(placed) {
                    pos.x = placed.left;
                    pos.y = placed.top;
                    pos.cx = (placed.right - placed.left).max(1);
                    pos.cy = (placed.bottom - placed.top).max(1);
                    pos.flags = SET_WINDOW_POS_FLAGS(pos.flags.0 & !SWP_NOMOVE.0 & !SWP_NOSIZE.0);
                }
                return LRESULT(0);
            }
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let parent = GetParent(h).unwrap_or_default();
                if !parent.0.is_null() {
                    let _ = SetFocus(Some(parent));
                }
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let parent = GetParent(h).unwrap_or_default();
                let player = slots().borrow().get(&key_of(parent)).and_then(|s| s.player.clone());
                if let Some(p) = player {
                    let _ = p.UpdateVideo();
                } else if !dc.0.is_null() {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(h, &mut rc);
                    fill(dc, rc, pal().bg);
                }
                let _ = EndPaint(h, &ps);
            }
            WM_SIZE => {
                let parent = GetParent(h).unwrap_or_default();
                let player = slots().borrow().get(&key_of(parent)).and_then(|s| s.player.clone());
                if let Some(p) = player {
                    let _ = p.UpdateVideo();
                }
            }
            _ => return DefWindowProcW(h, msg, wp, lp),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "system" fn host_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_MEDIA {
            on_posted(h);
            return LRESULT(0);
        }
        if msg == WM_TIMER && wp.0 == TIMER_MEDIA {
            tick(h);
            return LRESULT(0);
        }
        DefWindowProcW(h, msg, wp, lp)
    }

    unsafe fn test_host() -> HWND {
        let Ok(module) = GetModuleHandleW(None) else { return HWND::default() };
        let hinst: HINSTANCE = module.into();
        let _ = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: hinst,
            lpszClassName: w!("MaguroMediaTest"),
            ..Default::default()
        });
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("MaguroMediaTest"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst), None).unwrap_or_default()
    }

    unsafe fn pump(ms: u64) {
        let start = std::time::Instant::now();
        let mut msg = MSG::default();
        while start.elapsed().as_millis() < ms as u128 {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    fn wav_silence() -> Vec<u8> {
        let samples = 1600u32;
        let data = samples * 2;
        let mut b = Vec::with_capacity(44 + data as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&16000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(b.len() + data as usize, 0);
        b
    }

    #[test]
    fn classifies_extensions() {
        for e in ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "mpg", "mpeg", "ts", "m2ts", "3gp", "MP4", ".mkv"] {
            assert_eq!(media_kind(e), Some(MediaKind::Video), "{e}");
        }
        for e in ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff", "aif", "WAV"] {
            assert_eq!(media_kind(e), Some(MediaKind::Audio), "{e}");
        }
        assert!(is_video("webm") && is_audio("flac"));
        assert_eq!(media_kind("txt"), None);
        assert_eq!(media_kind("png"), None);
        assert_eq!(media_kind(""), None);
    }

    #[test]
    fn empty_and_missing_are_not_playable() {
        assert!(!playable_len(true, 0));
        assert!(!playable_len(false, 12));
        assert!(playable_len(true, 1));
        let dir = std::env::temp_dir();
        let empty = dir.join("maguro-media-empty.bin");
        let junk = dir.join("maguro-media-junk.bin");
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&junk, b"not a media file").unwrap();
        assert!(!file_playable(empty.to_str().unwrap()));
        assert!(file_playable(junk.to_str().unwrap()));
        assert!(!file_playable(dir.join("maguro-media-no-such-file.bin").to_str().unwrap()));
        let _ = std::fs::remove_file(&empty);
        let _ = std::fs::remove_file(&junk);
    }

    #[test]
    fn formats_clock_and_seeks() {
        assert_eq!(format_media_time(0), "0:00");
        assert_eq!(format_media_time(-5), "0:00");
        assert_eq!(format_media_time(61 * HNS_PER_SEC), "1:01");
        assert_eq!(format_media_time(3661 * HNS_PER_SEC), "1:01:01");
        assert_eq!(format_media_time(HNS_PER_SEC + HNS_PER_SEC / 2), "0:01");
        let dur = 180 * HNS_PER_SEC;
        assert_eq!(seek_target(dur, 0.5), 90 * HNS_PER_SEC);
        assert_eq!(seek_target(dur, 2.0), dur);
        assert_eq!(seek_target(dur, f32::NAN), 0);
        assert_eq!(seek_target(0, 0.5), 0);
        assert_eq!(seek_step(10 * HNS_PER_SEC, dur, -SEEK_STEP), 5 * HNS_PER_SEC);
        assert_eq!(seek_step(HNS_PER_SEC, dur, -SEEK_STEP), 0);
        assert_eq!(seek_step(dur - HNS_PER_SEC, dur, SEEK_STEP), dur);
        assert_eq!(seek_step(50, 0, SEEK_STEP), 50 + SEEK_STEP);
    }

    #[test]
    fn volume_stays_in_range() {
        assert_eq!(volume_set(1.0, VOLUME_STEP), 1.0);
        assert_eq!(volume_set(0.0, -VOLUME_STEP), 0.0);
        assert!((volume_set(0.5, VOLUME_STEP) - 0.55).abs() < 0.001);
        assert_eq!(volume_set(f32::NAN, 0.1), 1.0);
    }

    #[test]
    fn transport_layout_and_hits() {
        let area = RECT { left: 10, top: 20, right: 810, bottom: 420 };
        let (content, bar) = split_content(area, 96);
        assert_eq!(bar.bottom - bar.top, 40);
        assert_eq!(content.bottom, bar.top);
        assert_eq!((content.left, content.top, content.right), (10, 20, 810));
        let g = transport_geom(bar, 96);
        assert!(g.play.right <= g.time.left);
        assert!(g.time.right < g.seek.left);
        assert!(g.seek.right <= g.mute.left);
        assert!(g.mute.right <= g.volume.left);
        assert_eq!(hit_transport(&g, g.play.left + 2, g.play.top + 2), Some(Part::Play));
        assert_eq!(hit_transport(&g, g.seek.left + 4, (g.seek.top + g.seek.bottom) / 2), Some(Part::Seek));
        assert_eq!(hit_transport(&g, g.seek_hit.left + 4, g.seek_hit.top + 1), Some(Part::Seek));
        assert_eq!(hit_transport(&g, g.mute.left + 2, g.mute.top + 2), Some(Part::Mute));
        assert_eq!(hit_transport(&g, g.volume.left + 2, (g.volume.top + g.volume.bottom) / 2), Some(Part::Volume));
        assert_eq!(hit_transport(&g, 0, 0), None);
        let mid = ratio_along(g.seek, (g.seek.left + g.seek.right) / 2);
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
        assert_eq!(ratio_along(RECT::default(), 10), 0.0);
        let narrow = transport_geom(RECT { left: 0, top: 0, right: 120, bottom: 40 }, 96);
        assert!(hit_transport(&narrow, narrow.play.left + 1, narrow.play.top + 1).is_some() || !live(narrow.play));
        let (c, b) = split_content(RECT { left: 0, top: 0, right: 50, bottom: 10 }, 96);
        assert_eq!(c.bottom, c.top);
        assert_eq!(b.bottom - b.top, 10);
    }

    #[test]
    fn keys_do_not_steal_quick_look_navigation() {
        let space = 0x20u16;
        let left = 0x25u16;
        let up = 0x26u16;
        let right = 0x27u16;
        let down = 0x28u16;
        assert_eq!(media_key(space, false, false, false, true), None);
        assert_eq!(media_key(left, false, false, false, true), None);
        assert_eq!(media_key(right, false, false, false, true), None);
        assert_eq!(media_key(up, false, false, false, true), None);
        assert_eq!(media_key(down, false, false, false, true), None);
        assert_eq!(media_key(left, true, false, false, true), Some(MediaCmd::SeekBack));
        assert_eq!(media_key(right, true, false, false, true), Some(MediaCmd::SeekFwd));
        assert_eq!(media_key(up, true, false, false, true), Some(MediaCmd::VolumeUp));
        assert_eq!(media_key(down, true, false, false, true), Some(MediaCmd::VolumeDown));
        assert_eq!(media_key(b'K' as u16, false, false, false, true), Some(MediaCmd::Toggle));
        assert_eq!(media_key(b'J' as u16, false, false, false, true), Some(MediaCmd::SeekBack));
        assert_eq!(media_key(b'L' as u16, false, false, false, true), Some(MediaCmd::SeekFwd));
        assert_eq!(media_key(b'M' as u16, false, false, false, true), Some(MediaCmd::Mute));
        assert_eq!(media_key(b'K' as u16, false, true, false, true), None);
        assert_eq!(media_key(b'K' as u16, false, false, true, true), None);
        assert_eq!(media_key(space, false, false, false, false), Some(MediaCmd::Toggle));
        assert_eq!(media_key(space, true, false, false, false), None);
    }

    #[test]
    fn end_holds_the_last_frame_and_play_starts_over() {
        let dur = 3 * HNS_PER_SEC;
        assert_eq!(end_choice(0, dur), EndChoice::Play { pos: 0 });
        assert_eq!(end_choice(dur / 2, dur), EndChoice::Play { pos: dur / 2 });
        assert_eq!(end_choice(dur - END_GUARD - 1, dur), EndChoice::Play { pos: dur - END_GUARD - 1 });
        assert_eq!(end_choice(dur - END_GUARD, dur), EndChoice::Hold { player: dur - END_GUARD, shown: dur });
        assert_eq!(end_choice(dur, dur), EndChoice::Hold { player: dur - END_GUARD, shown: dur });
        assert_eq!(after_end(dur), EndChoice::Hold { player: dur - END_GUARD, shown: dur });
        // Shorter than the guard: the only picture is the first frame, and the clock stays at the end.
        assert_eq!(after_end(END_GUARD / 2), EndChoice::Hold { player: 0, shown: END_GUARD / 2 });
        assert_eq!(after_end(0), EndChoice::Hold { player: 0, shown: 0 });
        // A user pause near the start is not the end, so it must not hold.
        assert_eq!(end_choice(0, dur), EndChoice::Play { pos: 0 });
    }

    #[test]
    fn video_rect_stays_below_the_header() {
        let pane = RECT { left: 0, top: 0, right: 320, bottom: 800 };
        let placed = below_header(72, pane);
        assert!(placed.top >= 72, "video covers the header at {}", placed.top);
        assert_eq!((placed.left, placed.right, placed.bottom), (0, 320, 800));
        let under = RECT { left: 4, top: 90, right: 300, bottom: 700 };
        assert_eq!(below_header(72, under), under);
        // A header taller than the area pins the child to the bottom instead of a negative height.
        let pushed = below_header(900, pane);
        assert_eq!(pushed.top, pushed.bottom);
        assert!(pushed.top >= 72);
    }

    #[test]
    fn picture_shows_before_header_facts_and_tracks_the_pane() {
        let pane = RECT { left: 0, top: 0, right: 360, bottom: 700 };
        // One line of the file name. Type, size, and dimensions have not arrived.
        // Waiting for them used to keep the child hidden, so this rect stayed the pane fill.
        let early = video_dest(pane, 28, 96);
        assert!(reveal_video(early), "hidden child leaves a solid black video area");
        assert!(early.top >= 28, "picture covers the name at {}", early.top);
        assert_eq!(early.bottom, 700 - transport_height(96));
        assert_eq!((early.left, early.right), (0, 360));
        assert!(early.bottom - early.top > 200);

        // Facts grow the header. The child moves down and the transport stays put.
        let later = video_dest(pane, 96, 96);
        assert!(reveal_video(later));
        assert!(later.top >= 96);
        assert!(later.top > early.top);
        assert_eq!(later.bottom, early.bottom);
        assert!(later.bottom > later.top);

        // Resizing the pane resizes the child. MFPlay letterboxes inside that rect.
        let wide = video_dest(RECT { right: 800, bottom: 900, ..pane }, 96, 96);
        assert_eq!(wide.right - wide.left, 800);
        assert!(wide.bottom - wide.top > later.bottom - later.top);
        assert_eq!(wide.bottom, 900 - transport_height(96));

        // Quick Look passes a stage that is already under its header.
        let stage = RECT { left: 40, top: 48, right: 600, bottom: 500 };
        let f3 = video_dest(stage, stage.top, 96);
        assert_eq!(f3.top, stage.top);
        assert_eq!(f3.bottom, stage.bottom - transport_height(96));
        assert!(reveal_video(f3));

        // No room under the header: do not show a 1×1 child over the transport.
        let crushed = video_dest(RECT { left: 0, top: 0, right: 200, bottom: 36 }, 40, 96);
        assert!(!reveal_video(crushed));
        assert!(crushed.bottom <= crushed.top);
    }

    #[test]
    fn video_child_is_on_screen_before_playback() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            let sample = r"C:\WORK\CARGO_TARGET\itemdlg-design\f3-media\samples\video-portrait.mp4";
            assert!(std::path::Path::new(sample).is_file(), "missing {sample}");
            let probe = test_host();
            let Ok(module) = GetModuleHandleW(None) else { return };
            let hinst: HINSTANCE = module.into();
            let host = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("MaguroMediaTest"),
                w!(""),
                WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                40,
                40,
                400,
                300,
                None,
                None,
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            assert!(!host.0.is_null() && !probe.0.is_null(), "host window");
            let client = RECT { left: 0, top: 0, right: 400, bottom: 300 };
            // Header is only the file name. The preview job has not published facts.
            let dest = video_dest(client, 32, 96);
            assert!(reveal_video(dest));
            open_in(host, sample, Some(dest));
            let (shown, placed, video, failed) = {
                let map = slots().borrow();
                let s = map.get(&key_of(host)).expect("player slot");
                (s.shown, s.placed, s.video, s.failed)
            };
            assert!(!failed, "player failed before the first frame");
            assert!(shown, "child stays hidden, so the pane fill is all that shows");
            assert_eq!(placed, dest);
            assert!(IsWindowVisible(video).as_bool(), "video HWND is hidden");
            let mut wr = RECT::default();
            let _ = GetClientRect(video, &mut wr);
            assert_eq!((wr.right - wr.left, wr.bottom - wr.top), (dest.right - dest.left, dest.bottom - dest.top));
            close(host);
            assert!(!covers(host), "close releases the player");
            let _ = DestroyWindow(host);
            let _ = DestroyWindow(probe);
        }
    }

    #[test]
    fn audio_card_stage_clears_the_transport() {
        let row_h = 24;
        let rows = 8; // title, artist, album, length, bitrate, size, created, folder
        let transport = 40;
        let inset = 12;
        let stage = audio_stage_height(rows, row_h, 160, transport, inset);
        let room = stage - transport - inset;
        let stack = row_h * rows + row_h * 2;
        assert!(room >= stack, "card room {room} is shorter than the rows {stack}");
        assert!(stage >= 160 + inset + transport);
    }

    #[test]
    fn player_opens_wav_and_releases() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            let host = test_host();
            assert!(!host.0.is_null(), "message window");
            let dir = std::env::temp_dir();
            let wav = dir.join("maguro-media-tone.wav");
            let empty = dir.join("maguro-media-zero.wav");
            std::fs::write(&wav, wav_silence()).unwrap();
            std::fs::write(&empty, b"").unwrap();
            open(host, empty.to_str().unwrap());
            assert!(!covers(host), "empty file must not start a player");
            open(host, wav.to_str().unwrap());
            assert!(covers(host), "a real wav should start the player");
            pump(400);
            close(host);
            assert!(!covers(host), "close releases the player");
            let _ = DestroyWindow(host);
            let _ = std::fs::remove_file(&wav);
            let _ = std::fs::remove_file(&empty);
        }
    }
}
