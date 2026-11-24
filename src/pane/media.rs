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
    MFPCreateMediaPlayer, MFStartup, IMFPMediaPlayer, IMFPMediaPlayerCallback, IMFPMediaPlayerCallback_Impl, MFP_EVENT_HEADER, MFP_EVENT_TYPE_ERROR,
    MFP_EVENT_TYPE_MEDIAITEM_SET, MFP_EVENT_TYPE_PAUSE, MFP_EVENT_TYPE_PLAY, MFP_EVENT_TYPE_PLAYBACK_ENDED, MFP_MEDIAPLAYER_STATE_PLAYING, MFP_OPTION_NONE,
    MFP_POSITIONTYPE_100NS, MFSTARTUP_FULL, MFVideoARMode_PreservePicture, MF_VERSION,
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

struct Slot {
    gen: u64,
    path: String,
    kind: MediaKind,
    player: Option<IMFPMediaPlayer>,
    video: HWND,
    failed: bool,
    ready: bool,
    playing: bool,
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
pub(crate) unsafe fn open(host: HWND, path: &str) {
    let ext = super::preview::ext_of(path);
    let Some(kind) = media_kind(&ext) else {
        close(host);
        return;
    };
    let key = key_of(host);
    let same = slots().borrow().get(&key).is_some_and(|s| s.path == path && (s.player.is_some() || s.failed));
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
    let video = if kind == MediaKind::Video { create_video(host) } else { HWND::default() };
    if kind == MediaKind::Video && video.0.is_null() {
        return;
    }
    slots().borrow_mut().insert(
        key,
        Slot {
            gen,
            path: path.to_string(),
            kind,
            player: None,
            video,
            failed: false,
            ready: false,
            playing: false,
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
        },
    );
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
    invalidate_bar(host);
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
                if slot.playing {
                    slot.playing = false;
                    Act::Pause
                } else {
                    slot.playing = true;
                    Act::Play
                }
            }
            MediaCmd::SeekBack => {
                let pos = seek_step(slot.pos, slot.dur, -SEEK_STEP);
                slot.pos = pos;
                Act::Seek(pos)
            }
            MediaCmd::SeekFwd => {
                let pos = seek_step(slot.pos, slot.dur, SEEK_STEP);
                slot.pos = pos;
                Act::Seek(pos)
            }
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
        if rect_eq(slot.placed, rect) {
            return;
        }
        slot.placed = rect;
        (slot.video, rect, slot.player.clone())
    };
    if video.0.is_null() || !IsWindow(Some(video)).as_bool() {
        return;
    }
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    let _ = SetWindowPos(video, None, rect.left, rect.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_SHOWWINDOW);
    if let Some(p) = player {
        let _ = p.UpdateVideo();
    }
    let _ = InvalidateRect(Some(video), None, false);
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
    Volume(f32),
    Mute(bool),
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
            Part::Seek => {
                let pos = seek_target(slot.dur, ratio_along(slot.geom.seek, x));
                slot.pos = pos;
                Act::Seek(pos)
            }
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

unsafe fn refresh(host: HWND) {
    let key = key_of(host);
    let player = slots().borrow().get(&key).and_then(|s| s.player.clone());
    let Some(player) = player else { return };
    let pos = read_hns(&player, false);
    let dur = read_hns(&player, true);
    let playing = player.GetState().ok() == Some(MFP_MEDIAPLAYER_STATE_PLAYING);
    if let Some(slot) = slots().borrow_mut().get_mut(&key) {
        if let Some(pos) = pos {
            slot.pos = pos;
        }
        if let Some(dur) = dur {
            slot.dur = dur;
        }
        slot.playing = playing;
    }
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
            let vol = slots().borrow().get(&key).map(|s| s.volume).unwrap_or(1.0);
            let muted = slots().borrow().get(&key).map(|s| s.muted).unwrap_or(false);
            let _ = p.SetVolume(vol);
            let _ = p.SetMute(muted);
            let _ = p.UpdateVideo();
        }
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.ready = true;
        }
        refresh(host);
        let _ = InvalidateRect(Some(host), None, false);
        return;
    }
    if ev.kind == MFP_EVENT_TYPE_PLAY.0 {
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.playing = true;
        }
    } else if ev.kind == MFP_EVENT_TYPE_PAUSE.0 || ev.kind == MFP_EVENT_TYPE_PLAYBACK_ENDED.0 {
        if let Some(slot) = slots().borrow_mut().get_mut(&key) {
            slot.playing = false;
            if ev.kind == MFP_EVENT_TYPE_PLAYBACK_ENDED.0 && slot.dur > 0 {
                slot.pos = slot.dur;
            }
        }
    }
    invalidate_bar(host);
}

unsafe fn fail(host: HWND) {
    let key = key_of(host);
    let _ = KillTimer(Some(host), TIMER_MEDIA);
    let (player, video) = {
        let mut map = slots().borrow_mut();
        let Some(slot) = map.get_mut(&key) else { return };
        slot.failed = true;
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
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: w!("MaguroMediaVid"),
            ..Default::default()
        });
    });
}

unsafe fn create_video(parent: HWND) -> HWND {
    ensure_class();
    let Ok(module) = GetModuleHandleW(None) else { return HWND::default() };
    let hinst: HINSTANCE = module.into();
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("MaguroMediaVid"),
        w!(""),
        WS_CHILD | WS_CLIPSIBLINGS,
        0,
        0,
        8,
        8,
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
