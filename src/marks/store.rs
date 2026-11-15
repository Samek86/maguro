//! marks.json: path plus the file id captured when a mark was added.
//!
//! A file that cannot be parsed is left in place unless a valid marks.json.bak can replace it.
//! A higher `version` is loaded for reads and then blocked so this build never rewrites it.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::settings::store::write_atomic;

pub(crate) const FILE_NAME: &str = "marks.json";
pub(crate) const BAK_NAME: &str = "marks.json.bak";
/// New marks are refused at this size. Existing rows are never dropped to make room.
pub(crate) const CAP: usize = 20_000;
pub(crate) const VERSION: u32 = 1;
/// Lost rows older than this many days are deleted. `ok` and `recycled` stay.
const LOST_KEEP_DAYS: i64 = 180;
const DAY_SECS: i64 = 86_400;
/// One writer at a time across processes that have single-instance mode off.
const MUTEX_NAME: &str = r"Local\Maguro.marks";
/// 100-ns ticks from 1601-01-01 to 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 11_644_473_600_000_0000;

pub(crate) const STATE_OK: &str = "ok";
pub(crate) const STATE_LOST: &str = "lost";
pub(crate) const STATE_RECYCLED: &str = "recycled";

#[derive(Clone, Debug)]
enum Mutation {
    Add(Mark),
    Remove(String),
    UpdatePath { from: String, path: String },
    /// `lost_since` / identity fields: `None` leaves the value, `Some` writes it (including clearing with `Some(None)`).
    Patch {
        key: String,
        state: Option<String>,
        lost_since: Option<Option<String>>,
        vol: Option<Option<String>>,
        fid: Option<Option<String>>,
        size: Option<Option<u64>>,
        mtime: Option<Option<u64>>,
        dir: Option<bool>,
    },
}

/// vol/fid come from `GetFileInformationByHandleEx(FileIdInfo)`.
/// `vol` is the 64-bit serial as 16 hex digits. `fid` is the 16 id bytes, in memory order, as 32 hex digits.
/// Both are `None` when the volume cannot provide an id (network, FAT, or a failed open).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) vol: Option<String>,
    pub(crate) fid: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) mtime: Option<u64>,
    pub(crate) dir: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mark {
    pub(crate) path: String,
    key: String,
    parent: String,
    name: String,
    pub(crate) dir: bool,
    pub(crate) vol: Option<String>,
    pub(crate) fid: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) mtime: Option<u64>,
    pub(crate) added: String,
    pub(crate) state: String,
    pub(crate) lost_since: Option<String>,
    pub(crate) extra: Map<String, Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// The file on disk could not be trusted. Nothing was written.
    Blocked,
    /// The store already holds [`CAP`] marks.
    Full,
    NotFound,
    BadPath,
    /// `update_path` would land on a different mark.
    Conflict,
    Io(String),
}

/// Why a store refuses to write. A newer file stays readable. An unreadable file stays in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Newer(u32),
    Unreadable,
}

/// How many rows an import kept. Rows past [`CAP`] are not stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImportReport {
    pub(crate) kept: usize,
    pub(crate) skipped_cap: usize,
}

/// How `save` excludes other Maguro processes. Tests share a [`MarksLock::Shared`] mutex
/// so two handles can merge without taking the real `Local\Maguro.marks` mutex.
#[derive(Clone)]
pub(crate) enum MarksLock {
    Os,
    Shared(Arc<Mutex<()>>),
}

impl MarksLock {
    fn with_lock<R>(&self, f: impl FnOnce() -> R) -> std::io::Result<R> {
        match self {
            MarksLock::Os => os_with_lock(f),
            MarksLock::Shared(inner) => {
                let _guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                Ok(f())
            }
        }
    }
}

pub(crate) struct Store {
    dir: PathBuf,
    lock: MarksLock,
    version: u32,
    entries: Vec<Mark>,
    /// Parent directory (lowercase) → file names (lowercase) in that folder.
    by_folder: HashMap<String, HashSet<String>>,
    /// (vol, fid) → index in `entries`. Rows with a null id are omitted.
    by_id: HashMap<(String, String), usize>,
    extra: Map<String, Value>,
    mutations: Vec<Mutation>,
    dirty: bool,
    blocked: bool,
    /// The next save writes `entries` as the whole file. A normal save merges mutations instead.
    replace_pending: bool,
    now_unix: i64,
    notice: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct FileDoc {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    marks: Vec<EntryJson>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct EntryJson {
    path: String,
    #[serde(default)]
    dir: bool,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    fid: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mtime: Option<u64>,
    #[serde(default)]
    added: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    lost_since: Option<String>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

fn one() -> u32 {
    1
}

enum FileState {
    Missing,
    Bad,
    Good(Parsed),
}

struct Parsed {
    version: u32,
    entries: Vec<Mark>,
    extra: Map<String, Value>,
}

pub(crate) fn marks_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Open `path` and read the id, size, and last-write time. A missing or unsupported file
/// comes back with `vol` and `fid` unset rather than an error.
pub(crate) fn read_file_identity(path: &Path) -> FileIdentity {
    let text = path.to_string_lossy();
    let opened = crate::winpath::win_long_path(&text);
    unsafe { read_identity_handle(&opened) }.unwrap_or_default()
}

unsafe fn read_identity_handle(path: &str) -> Option<FileIdentity> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileBasicInfo, FileIdInfo, FileStandardInfo, OPEN_EXISTING,
    };
    use windows::core::HSTRING;

    let handle = CreateFileW(
        &HSTRING::from(path),
        FILE_READ_ATTRIBUTES.0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )
    .ok()?;
    struct Opened(windows::Win32::Foundation::HANDLE);
    impl Drop for Opened {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let opened = Opened(handle);
    let mut id = FILE_ID_INFO::default();
    let id_ok = GetFileInformationByHandleEx(
        opened.0,
        FileIdInfo,
        &mut id as *mut FILE_ID_INFO as *mut _,
        std::mem::size_of::<FILE_ID_INFO>() as u32,
    )
    .is_ok();
    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = GetFileInformationByHandleEx(
        opened.0,
        FileStandardInfo,
        &mut standard as *mut FILE_STANDARD_INFO as *mut _,
        std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
    )
    .is_ok();
    let mut basic = FILE_BASIC_INFO::default();
    let basic_ok = GetFileInformationByHandleEx(
        opened.0,
        FileBasicInfo,
        &mut basic as *mut FILE_BASIC_INFO as *mut _,
        std::mem::size_of::<FILE_BASIC_INFO>() as u32,
    )
    .is_ok();
    let mut out = FileIdentity::default();
    if id_ok {
        out.vol = Some(format!("{:016X}", id.VolumeSerialNumber));
        out.fid = Some(fid_hex(&id.FileId.Identifier));
    }
    if standard_ok {
        out.dir = standard.Directory;
        if standard.EndOfFile >= 0 {
            out.size = Some(standard.EndOfFile as u64);
        }
    }
    if basic_ok && basic.LastWriteTime > 0 {
        out.mtime = Some(basic.LastWriteTime as u64);
    }
    Some(out)
}

fn fid_hex(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn clean_id(value: Option<String>) -> Option<String> {
    let text = value?.trim().to_string();
    if text.is_empty() { None } else { Some(text.to_ascii_uppercase()) }
}

/// Same shape as a folder-views key: case-folded, slash-normalized, `\\?\` stripped.
/// The stored `path` keeps the casing the caller passed.
fn path_key(path: &str) -> Option<String> {
    let mut s = path.trim().replace('/', "\\");
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = strip_prefix_ci(&s, "\\\\?\\UNC\\") {
        s = format!("\\\\{rest}");
    } else if let Some(rest) = strip_prefix_ci(&s, "\\\\?\\") {
        s = rest.to_string();
    }
    let unc = s.starts_with("\\\\");
    let body = if unc { s.get(2..).unwrap_or("") } else { s.as_str() };
    let parts: Vec<&str> = body.split('\\').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() {
        return None;
    }
    let mut out = if unc { format!("\\\\{}", parts.join("\\")) } else { parts.join("\\") };
    out = out.to_lowercase();
    if out.len() == 2 && out.as_bytes()[1] == b':' {
        out.push('\\');
    }
    if !(out.len() == 3 && out.as_bytes()[1] == b':' && out.as_bytes()[2] == b'\\') {
        out = out.trim_end_matches('\\').to_string();
    }
    crate::is_fs(&out).then_some(out)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

fn parent_name(key: &str) -> Option<(String, String)> {
    let i = key.rfind('\\')?;
    let name = key.get(i + 1..).unwrap_or("");
    if name.is_empty() {
        return None;
    }
    let parent = if !key.starts_with("\\\\") && i == 2 && key.as_bytes().get(1) == Some(&b':') {
        key[..=i].to_string()
    } else {
        key[..i].to_string()
    };
    Some((parent, name.to_string()))
}

fn locate(path: &str) -> Option<(String, String, String)> {
    let key = path_key(path)?;
    let (parent, name) = parent_name(&key)?;
    Some((key, parent, name))
}

fn current_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `lost_since` is due once it is 180 days old, or older. 179 days stays.
fn lost_expired(lost_since: &str, now_unix: i64) -> bool {
    let Some(then) = unix_of_rfc3339(lost_since) else {
        return false;
    };
    now_unix.saturating_sub(then) >= LOST_KEEP_DAYS * DAY_SECS
}

fn unix_of_rfc3339(text: &str) -> Option<i64> {
    let s = text.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let y: i32 = s.get(0..4)?.parse().ok()?;
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let sep = b.get(10).copied()?;
    if sep != b'T' && sep != b't' && sep != b' ' {
        return None;
    }
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let min: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    if b.get(13) != Some(&b':') || b.get(16) != Some(&b':') {
        return None;
    }
    if month < 1 || month > 12 || day < 1 || day > month_len(y, month)? || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let rest = s.get(i..)?;
    let offset = parse_offset(rest)?;
    let days = civil_days(y, month, day)?;
    let local = days * DAY_SECS + i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec);
    Some(local - i64::from(offset))
}

fn parse_offset(rest: &str) -> Option<i32> {
    if rest == "Z" || rest == "z" {
        return Some(0);
    }
    let b = rest.as_bytes();
    let sign = match b.first().copied()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    if rest.len() == 6 && b.get(3) == Some(&b':') {
        let oh: i32 = rest.get(1..3)?.parse().ok()?;
        let om: i32 = rest.get(4..6)?.parse().ok()?;
        if oh > 23 || om > 59 {
            return None;
        }
        return Some(sign * (oh * 3600 + om * 60));
    }
    if rest.len() == 5 && b.get(3) != Some(&b':') {
        let oh: i32 = rest.get(1..3)?.parse().ok()?;
        let om: i32 = rest.get(3..5)?.parse().ok()?;
        if oh > 23 || om > 59 {
            return None;
        }
        return Some(sign * (oh * 3600 + om * 60));
    }
    None
}

fn month_len(y: i32, m: u32) -> Option<u32> {
    Some(match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(y) => 29,
        2 => 28,
        _ => return None,
    })
}

fn leap(y: i32) -> bool {
    let y = y as i64;
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days since 1970-01-01. Howard Hinnant's civil-from-days inverse.
fn civil_days(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) {
        return None;
    }
    let y = i64::from(y);
    let m = i64::from(m);
    let d = i64::from(d);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    Some(era * 146097 + doe as i64 - 719468)
}

fn ymd_from_unix_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub(crate) fn stamp_unix(unix: i64) -> String {
    format_unix_z(unix)
}

fn format_unix_z(unix: i64) -> String {
    let days = unix.div_euclid(DAY_SECS);
    let secs = unix.rem_euclid(DAY_SECS);
    let (y, m, d) = ymd_from_unix_days(days);
    let h = secs / 3600;
    let mi = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn now_rfc3339() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    let off = local_offset_minutes();
    let sign = if off >= 0 { '+' } else { '-' };
    let abs = off.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, abs / 60, abs % 60
    )
}

fn local_offset_minutes() -> i32 {
    use windows::Win32::System::SystemServices::{TIME_ZONE_ID_DAYLIGHT, TIME_ZONE_ID_STANDARD};
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tzi = TIME_ZONE_INFORMATION::default();
    let kind = unsafe { GetTimeZoneInformation(&mut tzi) };
    let extra = if kind == TIME_ZONE_ID_DAYLIGHT {
        tzi.DaylightBias
    } else if kind == TIME_ZONE_ID_STANDARD {
        tzi.StandardBias
    } else {
        0
    };
    -(tzi.Bias + extra)
}

fn local_stamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn local_today() -> (u16, u16, u16) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay)
}

fn file_local_ymd(path: &Path) -> Option<(u16, u16, u16)> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let dur = modified.duration_since(UNIX_EPOCH).ok()?;
    let ticks = dur.as_secs() * 10_000_000 + u64::from(dur.subsec_nanos()) / 100 + FILETIME_UNIX_EPOCH;
    let utc = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut local = FILETIME::default();
    unsafe { FileTimeToLocalFileTime(&utc, &mut local).ok()? };
    let mut st = SYSTEMTIME::default();
    unsafe { FileTimeToSystemTime(&local, &mut st).ok()? };
    Some((st.wYear, st.wMonth, st.wDay))
}

fn backup_due(bak_ymd: Option<(u16, u16, u16)>, today: (u16, u16, u16)) -> bool {
    bak_ymd != Some(today)
}

fn os_with_lock<R>(f: impl FnOnce() -> R) -> std::io::Result<R> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject, INFINITE};
    use windows::core::HSTRING;

    let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(MUTEX_NAME)) }.map_err(|e| std::io::Error::other(e.to_string()))?;
    let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
    if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(std::io::Error::other("could not lock marks.json"));
    }
    struct Hold(windows::Win32::Foundation::HANDLE);
    impl Drop for Hold {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseMutex(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }
    let _hold = Hold(handle);
    Ok(f())
}

impl Store {
    /// Load `dir`. A missing file is an empty store. Tests pass a temp directory, never the real config folder.
    pub(crate) fn load_at(dir: &Path, lock: MarksLock, now_unix: i64) -> Store {
        match read_store_file(&marks_path(dir)) {
            FileState::Missing => Store::blank(dir, lock, now_unix),
            FileState::Good(parsed) => Store::from_good(dir, lock, now_unix, parsed),
            FileState::Bad => Store::recover(dir, lock, now_unix),
        }
    }

    pub(crate) fn load_in(dir: &Path, lock: MarksLock) -> Store {
        Store::load_at(dir, lock, current_unix())
    }

    fn blank(dir: &Path, lock: MarksLock, now_unix: i64) -> Store {
        Store {
            dir: dir.to_path_buf(),
            lock,
            version: VERSION,
            entries: Vec::new(),
            by_folder: HashMap::new(),
            by_id: HashMap::new(),
            extra: Map::new(),
            mutations: Vec::new(),
            dirty: false,
            blocked: false,
            replace_pending: false,
            now_unix,
            notice: None,
        }
    }

    fn blocked_blank(dir: &Path, lock: MarksLock, now_unix: i64) -> Store {
        let mut store = Store::blank(dir, lock, now_unix);
        store.blocked = true;
        store
    }

    fn from_good(dir: &Path, lock: MarksLock, now_unix: i64, parsed: Parsed) -> Store {
        let blocked = parsed.version > VERSION;
        let mut store = Store::from_parsed(dir, lock, now_unix, parsed, blocked, None);
        if !blocked {
            store.purge_expired_lost(now_unix);
        }
        store
    }

    fn from_parsed(dir: &Path, lock: MarksLock, now_unix: i64, parsed: Parsed, blocked: bool, notice: Option<String>) -> Store {
        let mut store = Store::blank(dir, lock, now_unix);
        store.version = parsed.version.max(1);
        store.entries = parsed.entries;
        store.extra = parsed.extra;
        store.blocked = blocked;
        store.notice = notice;
        store.reindex();
        store
    }

    /// Broken marks.json plus a usable backup: rename the broken file aside and write the backup
    /// back as marks.json. A missing backup, or a backup this build must not rewrite, blocks.
    fn recover(dir: &Path, lock: MarksLock, now_unix: i64) -> Store {
        match lock.with_lock(|| repair_locked(dir, lock.clone(), now_unix)) {
            Ok(Ok(store)) => store,
            Ok(Err(_)) | Err(_) => Store::blocked_blank(dir, lock, now_unix),
        }
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.blocked
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Config folder that holds this store's marks.json. Never the settings file.
    pub(crate) fn folder(&self) -> &Path {
        &self.dir
    }

    /// Why this build will not write the file. `None` when the store can be saved.
    pub(crate) fn block_kind(&self) -> Option<BlockKind> {
        if !self.blocked {
            return None;
        }
        if self.version > VERSION {
            Some(BlockKind::Newer(self.version))
        } else {
            Some(BlockKind::Unreadable)
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn version(&self) -> u32 {
        self.version
    }

    /// One line after a backup was loaded. Empty when this load did not recover anything.
    pub(crate) fn recovery_notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(crate) fn extra(&self, key: &str) -> Option<&Value> {
        self.extra.get(key)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Mark> {
        self.entries.iter()
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Mark> {
        let key = path_key(path)?;
        self.entries.iter().find(|e| e.key == key)
    }

    pub(crate) fn in_folder(&self, folder: &str) -> Vec<&Mark> {
        let key = match path_key(folder) {
            Some(key) => key,
            None => return Vec::new(),
        };
        let Some(names) = self.by_folder.get(&key) else {
            return Vec::new();
        };
        self.entries.iter().filter(|e| e.parent == key && names.contains(&e.name)).collect()
    }

    pub(crate) fn by_file_id(&self, vol: &str, fid: &str) -> Option<&Mark> {
        let vol = vol.trim().to_ascii_uppercase();
        let fid = fid.trim().to_ascii_uppercase();
        if vol.is_empty() || fid.is_empty() {
            return None;
        }
        let index = *self.by_id.get(&(vol, fid))?;
        self.entries.get(index)
    }

    pub(crate) fn add(&mut self, path: &str) -> Result<(), StoreError> {
        let meta = read_file_identity(Path::new(path));
        let added = now_rfc3339();
        self.add_with(path, meta, &added)
    }

    /// `added` is the RFC3339 timestamp stored on a new row. An empty string means "now".
    pub(crate) fn add_with(&mut self, path: &str, meta: FileIdentity, added: &str) -> Result<(), StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some((key, parent, name)) = locate(path) else {
            return Err(StoreError::BadPath);
        };
        if self.entries.iter().any(|e| e.key == key) {
            return Ok(());
        }
        if self.entries.len() >= CAP {
            return Err(StoreError::Full);
        }
        let mark = Mark {
            path: path.trim().to_string(),
            key,
            parent,
            name,
            dir: meta.dir,
            vol: clean_id(meta.vol),
            fid: clean_id(meta.fid),
            size: meta.size,
            mtime: meta.mtime,
            added: if added.is_empty() { now_rfc3339() } else { added.to_string() },
            state: STATE_OK.to_string(),
            lost_since: None,
            extra: Map::new(),
        };
        let index = self.entries.len();
        self.mutations.push(Mutation::Add(mark.clone()));
        self.entries.push(mark);
        self.index_insert(index);
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn remove(&mut self, path: &str) -> Result<bool, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some((key, _, _)) = locate(path) else {
            return Err(StoreError::BadPath);
        };
        let Some(pos) = self.entries.iter().position(|e| e.key == key) else {
            return Ok(false);
        };
        self.entries.remove(pos);
        self.reindex();
        self.mutations.push(Mutation::Remove(key));
        self.dirty = true;
        Ok(true)
    }

    pub(crate) fn update_path(&mut self, from: &str, to: &str) -> Result<(), StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some((from_key, _, _)) = locate(from) else {
            return Err(StoreError::BadPath);
        };
        let Some((to_key, to_parent, to_name)) = locate(to) else {
            return Err(StoreError::BadPath);
        };
        let Some(pos) = self.entries.iter().position(|e| e.key == from_key) else {
            return Err(StoreError::NotFound);
        };
        if from_key != to_key && self.entries.iter().any(|e| e.key == to_key) {
            return Err(StoreError::Conflict);
        }
        let mark = &mut self.entries[pos];
        if mark.path == to.trim() && mark.key == to_key {
            return Ok(());
        }
        let old_parent = mark.parent.clone();
        let old_name = mark.name.clone();
        mark.path = to.trim().to_string();
        mark.key = to_key;
        mark.parent = to_parent.clone();
        mark.name = to_name.clone();
        self.unindex_folder(&old_parent, &old_name);
        self.by_folder.entry(to_parent).or_default().insert(to_name);
        self.mutations.push(Mutation::UpdatePath { from: from_key, path: to.trim().to_string() });
        self.dirty = true;
        Ok(())
    }

    /// Drop every `lost` row. `ok` and `recycled` stay. A blocked store is not changed.
    pub(crate) fn remove_lost(&mut self) -> Result<usize, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let keys: Vec<String> = self.entries.iter().filter(|e| e.state == STATE_LOST).map(|e| e.key.clone()).collect();
        let n = keys.len();
        if n == 0 {
            return Ok(0);
        }
        self.entries.retain(|e| e.state != STATE_LOST);
        self.reindex();
        for key in keys {
            self.mutations.push(Mutation::Remove(key));
        }
        self.dirty = true;
        Ok(n)
    }

    /// The on-disk document, including rows this build will not rewrite.
    /// Does not write marks.json.
    pub(crate) fn export_json(&self) -> String {
        self.to_json()
    }

    /// Replace every row with the file. Same shape as marks.json.
    /// A blocked store, a newer `version`, or a file that is not marks JSON is refused and nothing is written.
    /// Rows after the first [`CAP`] are skipped. `ok`, `lost`, and `recycled` all come across.
    pub(crate) fn import_replace(&mut self, bytes: &[u8]) -> Result<ImportReport, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some(parsed) = parse_doc(bytes) else {
            return Err(StoreError::Io("not a marks file".into()));
        };
        if parsed.version > VERSION {
            return Err(StoreError::Io("newer version".into()));
        }
        let total = parsed.entries.len();
        let skipped_cap = total.saturating_sub(CAP);
        let mut entries = parsed.entries;
        entries.truncate(CAP);
        self.entries = entries;
        self.extra = parsed.extra;
        self.version = parsed.version.max(1);
        self.reindex();
        self.mutations.clear();
        self.replace_pending = true;
        self.dirty = true;
        self.purge_expired_lost(self.now_unix);
        Ok(ImportReport { kept: self.entries.len(), skipped_cap })
    }

    /// Drop `lost` rows whose `lost_since` is 180 days old or older. Returns how many were removed.
    pub(crate) fn purge_expired_lost(&mut self, now_unix: i64) -> usize {
        if self.blocked {
            return 0;
        }
        let before = self.entries.len();
        self.entries.retain(|e| e.state != STATE_LOST || e.lost_since.as_deref().is_none_or(|ts| !lost_expired(ts, now_unix)));
        let removed = before - self.entries.len();
        if removed > 0 {
            self.reindex();
            self.dirty = true;
        }
        removed
    }

    /// Overlay this process's adds, removes, and path updates onto `into` (a fresh read).
    /// Pending edits win. Expired lost rows are purged with this store's clock.
    pub(crate) fn merge_apply(&self, into: &mut Store) {
        if into.blocked || into.version > VERSION {
            return;
        }
        apply_mutations(&mut into.entries, &self.mutations);
        into.reindex();
        into.purge_expired_lost(self.now_unix);
    }

    pub(crate) fn save(&mut self) -> Result<(), StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        if !self.dirty {
            return Ok(());
        }
        let lock = self.lock.clone();
        match lock.with_lock(|| self.save_locked()) {
            Err(e) | Ok(Err(e)) => Err(StoreError::Io(e.to_string())),
            Ok(Ok(result)) => result,
        }
    }

    fn save_locked(&mut self) -> std::io::Result<Result<(), StoreError>> {
        if self.replace_pending {
            return self.save_replace();
        }
        let path = marks_path(&self.dir);
        match read_store_file(&path) {
            FileState::Missing => {
                self.purge_expired_lost(self.now_unix);
            }
            FileState::Bad => {
                self.blocked = true;
                self.dirty = false;
                return Ok(Err(StoreError::Blocked));
            }
            FileState::Good(parsed) => {
                if parsed.version > VERSION {
                    let notice = self.notice.clone();
                    *self = Store::from_parsed(&self.dir, self.lock.clone(), self.now_unix, parsed, true, notice);
                    return Ok(Err(StoreError::Blocked));
                }
                let bytes = std::fs::read(&path)?;
                let mut merged = Store::from_parsed(&self.dir, self.lock.clone(), self.now_unix, parsed, false, self.notice.clone());
                self.merge_apply(&mut merged);
                self.entries = merged.entries;
                self.extra = merged.extra;
                self.version = merged.version.max(1);
                self.by_folder = merged.by_folder;
                self.by_id = merged.by_id;
                self.maybe_backup(&bytes)?;
            }
        }
        let json = self.to_json();
        write_atomic(&path, json.as_bytes())?;
        self.mutations.clear();
        self.dirty = false;
        Ok(Ok(()))
    }

    /// Write `entries` as the whole file. A bad file or a newer version is left unchanged.
    fn save_replace(&mut self) -> std::io::Result<Result<(), StoreError>> {
        let path = marks_path(&self.dir);
        match read_store_file(&path) {
            FileState::Bad => {
                self.blocked = true;
                self.dirty = false;
                self.replace_pending = false;
                return Ok(Err(StoreError::Blocked));
            }
            FileState::Good(parsed) if parsed.version > VERSION => {
                let notice = self.notice.clone();
                *self = Store::from_parsed(&self.dir, self.lock.clone(), self.now_unix, parsed, true, notice);
                return Ok(Err(StoreError::Blocked));
            }
            FileState::Good(_) => {
                let bytes = std::fs::read(&path)?;
                self.maybe_backup(&bytes)?;
            }
            FileState::Missing => {}
        }
        self.purge_expired_lost(self.now_unix);
        let json = self.to_json();
        write_atomic(&path, json.as_bytes())?;
        self.mutations.clear();
        self.dirty = false;
        self.replace_pending = false;
        Ok(Ok(()))
    }

    /// First successful save of the local calendar day copies the previous file to marks.json.bak.
    fn maybe_backup(&self, previous: &[u8]) -> std::io::Result<()> {
        if previous.is_empty() || parse_doc(previous).is_none() {
            return Ok(());
        }
        let bak = self.dir.join(BAK_NAME);
        let ymd = if bak.is_file() { file_local_ymd(&bak) } else { None };
        if !backup_due(ymd, local_today()) {
            return Ok(());
        }
        write_atomic(&bak, previous)
    }

    fn to_json(&self) -> String {
        let marks = self.entries.iter().map(EntryJson::from).collect();
        let doc = FileDoc { version: self.version.max(1), marks, extra: self.extra.clone() };
        let mut text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{\"version\":1}".into());
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    fn reindex(&mut self) {
        self.by_folder.clear();
        self.by_id.clear();
        for i in 0..self.entries.len() {
            self.index_insert(i);
        }
    }

    fn index_insert(&mut self, index: usize) {
        let mark = &self.entries[index];
        self.by_folder.entry(mark.parent.clone()).or_default().insert(mark.name.clone());
        if let (Some(vol), Some(fid)) = (&mark.vol, &mark.fid) {
            if !vol.is_empty() && !fid.is_empty() {
                self.by_id.insert((vol.clone(), fid.clone()), index);
            }
        }
    }

    fn unindex_folder(&mut self, parent: &str, name: &str) {
        let Some(set) = self.by_folder.get_mut(parent) else { return };
        set.remove(name);
        if set.is_empty() {
            self.by_folder.remove(parent);
        }
    }

    /// The path itself when it is marked, plus every marked path under it.
    pub(crate) fn affected_paths(&self, path: &str) -> Vec<String> {
        let Some(key) = path_key(path) else { return Vec::new() };
        let prefix = child_prefix(&key);
        self.entries.iter().filter(|e| e.key == key || e.key.starts_with(&prefix)).map(|e| e.path.clone()).collect()
    }

    /// Move `from` and every marked path under it onto `to`. Nothing is written when a destination
    /// is already a different mark. `identity`, when set, replaces the root row's id (cross-volume).
    pub(crate) fn retarget_tree(&mut self, from: &str, to: &str, identity: Option<&FileIdentity>) -> Result<Vec<String>, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some(from_key) = path_key(from) else { return Err(StoreError::BadPath) };
        if path_key(to).is_none() {
            return Err(StoreError::BadPath);
        }
        let prefix = child_prefix(&from_key);
        let rows: Vec<(String, String)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let dest = if e.key == from_key {
                    Some(to.trim().to_string())
                } else if e.key.starts_with(&prefix) {
                    super::follow::rewrite_prefix(&e.path, from, to)
                } else {
                    None
                };
                dest.map(|d| (e.path.clone(), d))
            })
            .collect();
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let moving: HashSet<String> = rows.iter().filter_map(|(src, _)| path_key(src)).collect();
        let mut dest_keys = HashSet::new();
        for (_, dest) in &rows {
            let Some(dest_key) = path_key(dest) else { return Err(StoreError::BadPath) };
            if !dest_keys.insert(dest_key.clone()) {
                return Ok(Vec::new());
            }
            if self.entries.iter().any(|e| e.key == dest_key && !moving.contains(&e.key)) {
                return Ok(Vec::new());
            }
        }
        let mut out = Vec::with_capacity(rows.len());
        let root_moved = rows.iter().any(|(src, _)| path_key(src).as_deref() == Some(from_key.as_str()));
        for (src, dest) in &rows {
            self.update_path(src, dest)?;
            out.push(dest.clone());
        }
        if root_moved {
            if let Some(id) = identity {
                self.patch_identity(to, id)?;
            }
        }
        Ok(out)
    }

    /// `lost_since`: `None` keeps it, `Some(None)` clears it, `Some(Some(s))` sets it.
    /// Returns whether the row changed.
    pub(crate) fn set_state(&mut self, path: &str, state: &str, lost_since: Option<Option<String>>) -> Result<bool, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some((key, _, _)) = locate(path) else { return Err(StoreError::BadPath) };
        let Some(pos) = self.entries.iter().position(|e| e.key == key) else { return Ok(false) };
        let next_lost = match &lost_since {
            None => self.entries[pos].lost_since.clone(),
            Some(value) => value.clone(),
        };
        if self.entries[pos].state == state && self.entries[pos].lost_since == next_lost {
            return Ok(false);
        }
        self.entries[pos].state = state.to_string();
        self.entries[pos].lost_since = next_lost;
        self.mutations.push(Mutation::Patch {
            key,
            state: Some(state.to_string()),
            lost_since,
            vol: None,
            fid: None,
            size: None,
            mtime: None,
            dir: None,
        });
        self.dirty = true;
        Ok(true)
    }

    /// Write vol/fid/size/mtime/dir. A file id that already belongs to another row is left alone.
    pub(crate) fn patch_identity(&mut self, path: &str, id: &FileIdentity) -> Result<bool, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let Some((key, _, _)) = locate(path) else { return Err(StoreError::BadPath) };
        let Some(pos) = self.entries.iter().position(|e| e.key == key) else { return Ok(false) };
        let vol = clean_id(id.vol.clone());
        let fid = clean_id(id.fid.clone());
        let taken = match (&vol, &fid) {
            (Some(v), Some(f)) => self.by_id.get(&(v.clone(), f.clone())).is_some_and(|&other| other != pos),
            _ => false,
        };
        let (vol, fid) = if taken { (self.entries[pos].vol.clone(), self.entries[pos].fid.clone()) } else { (vol, fid) };
        let mark = &self.entries[pos];
        if mark.vol == vol && mark.fid == fid && mark.size == id.size && mark.mtime == id.mtime && mark.dir == id.dir {
            return Ok(false);
        }
        let mark = &mut self.entries[pos];
        mark.vol = vol.clone();
        mark.fid = fid.clone();
        mark.size = id.size;
        mark.mtime = id.mtime;
        mark.dir = id.dir;
        self.rebind_id(pos);
        self.mutations.push(Mutation::Patch {
            key,
            state: None,
            lost_since: None,
            vol: Some(vol),
            fid: Some(fid),
            size: Some(id.size),
            mtime: Some(id.mtime),
            dir: Some(id.dir),
        });
        self.dirty = true;
        Ok(true)
    }

    pub(crate) fn forget_tree(&mut self, path: &str) -> Result<usize, StoreError> {
        if self.blocked {
            return Err(StoreError::Blocked);
        }
        let rows = self.affected_paths(path);
        let n = rows.len();
        for row in rows {
            self.remove(&row)?;
        }
        Ok(n)
    }

    fn rebind_id(&mut self, index: usize) {
        self.by_id.retain(|_, slot| *slot != index);
        let mark = &self.entries[index];
        if let (Some(vol), Some(fid)) = (&mark.vol, &mark.fid) {
            if !vol.is_empty() && !fid.is_empty() {
                self.by_id.insert((vol.clone(), fid.clone()), index);
            }
        }
    }
}

fn child_prefix(key: &str) -> String {
    if key.ends_with('\\') { key.to_string() } else { format!("{key}\\") }
}

fn apply_mutations(entries: &mut Vec<Mark>, mutations: &[Mutation]) {
    for mutation in mutations {
        match mutation {
            Mutation::Add(mark) => {
                if let Some(slot) = entries.iter_mut().find(|e| e.key == mark.key) {
                    *slot = mark.clone();
                } else if entries.len() < CAP {
                    entries.push(mark.clone());
                }
            }
            Mutation::Remove(key) => entries.retain(|e| &e.key != key),
            Mutation::UpdatePath { from, path } => {
                let Some((to_key, to_parent, to_name)) = locate(path) else { continue };
                if entries.iter().any(|e| e.key == to_key && e.key != *from) {
                    continue;
                }
                let Some(slot) = entries.iter_mut().find(|e| e.key == *from) else { continue };
                slot.path = path.clone();
                slot.key = to_key;
                slot.parent = to_parent;
                slot.name = to_name;
            }
            Mutation::Patch { key, state, lost_since, vol, fid, size, mtime, dir } => {
                let Some(slot) = entries.iter_mut().find(|e| &e.key == key) else { continue };
                if let Some(state) = state {
                    slot.state = state.clone();
                }
                if let Some(lost_since) = lost_since {
                    slot.lost_since = lost_since.clone();
                }
                if let Some(vol) = vol {
                    slot.vol = vol.clone();
                }
                if let Some(fid) = fid {
                    slot.fid = fid.clone();
                }
                if let Some(size) = size {
                    slot.size = *size;
                }
                if let Some(mtime) = mtime {
                    slot.mtime = *mtime;
                }
                if let Some(dir) = dir {
                    slot.dir = *dir;
                }
            }
        }
    }
}

fn repair_locked(dir: &Path, lock: MarksLock, now_unix: i64) -> std::io::Result<Store> {
    let path = marks_path(dir);
    match read_store_file(&path) {
        FileState::Missing => return Ok(Store::blank(dir, lock, now_unix)),
        FileState::Good(parsed) => return Ok(Store::from_good(dir, lock, now_unix, parsed)),
        FileState::Bad => {}
    }
    let bak = match read_store_file(&dir.join(BAK_NAME)) {
        FileState::Good(parsed) => parsed,
        _ => return Ok(Store::blocked_blank(dir, lock, now_unix)),
    };
    if bak.version > VERSION {
        return Ok(Store::from_parsed(dir, lock, now_unix, bak, true, None));
    }
    let Some(broken_name) = quarantine(&path) else {
        return Ok(Store::blocked_blank(dir, lock, now_unix));
    };
    let notice = format!("recovered marks.json from marks.json.bak; kept the unreadable file as {broken_name}");
    crate::crash::log(&notice);
    let mut store = Store::from_parsed(dir, lock.clone(), now_unix, bak, false, Some(notice.clone()));
    store.purge_expired_lost(now_unix);
    let json = store.to_json();
    if let Err(err) = write_atomic(&path, json.as_bytes()) {
        let broken = dir.join(&broken_name);
        if !path.exists() {
            let _ = std::fs::rename(&broken, &path);
        }
        if path.exists() && !dir.join(&broken_name).exists() {
            return Ok(Store::blocked_blank(dir, lock, now_unix));
        }
        store.dirty = true;
        store.notice = Some(notice);
        return Err(err);
    }
    store.dirty = false;
    store.mutations.clear();
    Ok(store)
}

/// Move a broken marks.json aside. The name uses local time. A name that is already taken gets `-2`, `-3`, …
fn quarantine(path: &Path) -> Option<String> {
    let stem = format!("marks.json.broken-{}", local_stamp());
    let parent = path.parent().unwrap_or(Path::new("."));
    for n in 0..100 {
        let name = if n == 0 { stem.clone() } else { format!("{stem}-{n}") };
        let dest = parent.join(&name);
        if dest.exists() {
            continue;
        }
        if std::fs::rename(path, &dest).is_ok() {
            return Some(name);
        }
    }
    None
}

fn read_store_file(path: &Path) -> FileState {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Missing,
        Err(_) => FileState::Bad,
        Ok(bytes) => match parse_doc(&bytes) {
            Some(parsed) => FileState::Good(parsed),
            None => FileState::Bad,
        },
    }
}

fn parse_doc(bytes: &[u8]) -> Option<Parsed> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim().trim_start_matches('\u{feff}');
    let doc: FileDoc = serde_json::from_str(text).ok()?;
    let mut entries = Vec::new();
    for entry in doc.marks {
        let Some(mark) = mark_from_json(entry) else { continue };
        if let Some(pos) = entries.iter().position(|e: &Mark| e.key == mark.key) {
            entries[pos] = mark;
        } else {
            entries.push(mark);
        }
    }
    Some(Parsed { version: doc.version.max(1), entries, extra: doc.extra })
}

fn mark_from_json(entry: EntryJson) -> Option<Mark> {
    let (key, parent, name) = locate(&entry.path)?;
    let state = entry.state.unwrap_or_else(|| STATE_OK.to_string());
    let state = if state.is_empty() { STATE_OK.to_string() } else { state };
    Some(Mark {
        path: entry.path,
        key,
        parent,
        name,
        dir: entry.dir,
        vol: clean_id(entry.vol),
        fid: clean_id(entry.fid),
        size: entry.size,
        mtime: entry.mtime,
        added: entry.added,
        state,
        lost_since: entry.lost_since.filter(|s| !s.is_empty()),
        extra: entry.extra,
    })
}

impl From<&Mark> for EntryJson {
    fn from(mark: &Mark) -> Self {
        EntryJson {
            path: mark.path.clone(),
            dir: mark.dir,
            vol: mark.vol.clone(),
            fid: mark.fid.clone(),
            size: mark.size,
            mtime: mark.mtime,
            added: mark.added.clone(),
            state: Some(mark.state.clone()),
            lost_since: mark.lost_since.clone(),
            extra: mark.extra.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("maguro-marks-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn shared() -> MarksLock {
        MarksLock::Shared(Arc::new(Mutex::new(())))
    }

    fn meta(vol: Option<&str>, fid: Option<&str>) -> FileIdentity {
        FileIdentity {
            vol: vol.map(str::to_string),
            fid: fid.map(str::to_string),
            size: Some(48213),
            mtime: Some(134_077_356_000_000_000),
            dir: false,
        }
    }

    fn empty_meta() -> FileIdentity {
        FileIdentity::default()
    }

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn rfc3339_round_trip_and_offset() {
        assert_eq!(unix_of_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_of_rfc3339("2000-01-01T00:00:00Z"), Some(946_684_800));
        assert_eq!(format_unix_z(946_684_800), "2000-01-01T00:00:00Z");
        assert_eq!(unix_of_rfc3339("2026-11-15T19:30:00+09:00"), Some(1_794_738_600));
        assert_eq!(unix_of_rfc3339("2026-11-15T10:30:00Z"), Some(1_794_738_600));
        assert_eq!(unix_of_rfc3339(&format_unix_z(1_794_700_800)), Some(1_794_700_800));
    }

    #[test]
    fn lost_at_180_days_is_expired_and_179_is_kept() {
        let day = DAY_SECS;
        assert!(!lost_expired(&format_unix_z(NOW - 179 * day), NOW));
        assert!(lost_expired(&format_unix_z(NOW - 180 * day), NOW));
        assert!(lost_expired(&format_unix_z(NOW - 181 * day), NOW));
        assert!(!lost_expired(&format_unix_z(NOW - 180 * day + 1), NOW));
        assert!(!lost_expired("not-a-time", NOW));
        assert!(!backup_due(Some((2026, 11, 15)), (2026, 11, 15)));
        assert!(backup_due(Some((2026, 11, 14)), (2026, 11, 15)));
        assert!(backup_due(None, (2026, 11, 15)));
    }

    #[test]
    fn round_trip_keeps_casing_and_matches_case_insensitively() {
        let dir = scratch("round");
        let lock = shared();
        let mut store = Store::load_at(&dir, lock.clone(), NOW);
        store
            .add_with(r"D:\Work\Report\Final.docx", meta(Some("8C3A1F20D4E9B7A1"), Some("00000000000000000003000000012A4F")), "2026-11-15T19:30:00+09:00")
            .unwrap();
        store.save().unwrap();
        let back = Store::load_at(&dir, lock, NOW);
        let mark = back.get(r"d:/work/report/final.DOCX").unwrap();
        assert_eq!(mark.path, r"D:\Work\Report\Final.docx");
        assert_eq!(mark.vol.as_deref(), Some("8C3A1F20D4E9B7A1"));
        assert_eq!(mark.added, "2026-11-15T19:30:00+09:00");
        assert_eq!(mark.state, "ok");
        assert_eq!(mark.lost_since, None);
        assert_eq!(back.in_folder(r"D:\work\REPORT").len(), 1);
        assert!(back.in_folder(r"D:\work").is_empty());
        assert!(back.by_file_id("8c3a1f20d4e9b7a1", "00000000000000000003000000012a4f").is_some());
        assert_eq!(back.iter().count(), 1);
        assert!(!back.is_blocked());
        assert!(!dir.join("maguro.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_fields_on_the_root_and_the_entry_round_trip() {
        let dir = scratch("extra");
        let raw = r#"{
            "version": 1,
            "future": 7,
            "marks": [{
                "path": "D:\\Work\\Report\\Final.docx",
                "dir": false,
                "vol": null,
                "fid": null,
                "size": 4,
                "mtime": 5,
                "added": "2026-11-15T19:30:00Z",
                "state": "ok",
                "lost_since": null,
                "note": "keep"
            }]
        }"#;
        std::fs::write(marks_path(&dir), raw).unwrap();
        let mut store = Store::load_at(&dir, shared(), NOW);
        assert_eq!(store.extra("future"), Some(&Value::from(7)));
        assert_eq!(store.get(r"D:\Work\Report\Final.docx").unwrap().extra.get("note"), Some(&Value::from("keep")));
        store.add_with(r"D:\Work\Other.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        let text = std::fs::read_to_string(marks_path(&dir)).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["future"], 7);
        assert_eq!(value["marks"][0]["note"], "keep");
        assert_eq!(value["marks"][0]["path"], r"D:\Work\Report\Final.docx");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_higher_version_is_readable_and_never_rewritten() {
        let dir = scratch("ver");
        let raw = b"{\"version\":2,\"future\":1,\"marks\":[{\"path\":\"D:\\\\Work\\\\a.txt\",\"note\":\"x\"}]}\n";
        let path = marks_path(&dir);
        std::fs::write(&path, raw).unwrap();
        let mut store = Store::load_at(&dir, shared(), NOW);
        assert!(store.is_blocked());
        assert_eq!(store.version(), 2);
        assert!(store.get(r"d:\work\a.txt").is_some());
        assert_eq!(store.extra("future"), Some(&Value::from(1)));
        assert_eq!(store.add_with(r"D:\Work\b.txt", empty_meta(), "2026-11-15T00:00:00Z"), Err(StoreError::Blocked));
        assert_eq!(store.remove(r"D:\Work\a.txt"), Err(StoreError::Blocked));
        assert_eq!(store.save(), Err(StoreError::Blocked));
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert!(!dir.join(BAK_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_with_a_good_backup_is_renamed_and_later_saves_work() {
        let dir = scratch("bak-ok");
        let bad = b"{not json";
        let bak = r#"{"version":1,"marks":[{"path":"D:\\Work\\from-bak.txt","vol":null,"fid":null,"added":"2026-11-15T00:00:00Z","state":"ok"}]}"#;
        std::fs::write(marks_path(&dir), bad).unwrap();
        std::fs::write(dir.join(BAK_NAME), bak).unwrap();
        let mut store = Store::load_at(&dir, shared(), NOW);
        assert!(!store.is_blocked());
        let notice = store.recovery_notice().unwrap();
        assert!(notice.contains("marks.json.bak"), "{notice}");
        assert!(notice.contains("marks.json.broken-"), "{notice}");
        assert!(store.get(r"D:\Work\from-bak.txt").is_some());
        let broken = broken_files(&dir);
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert_eq!(std::fs::read(&broken[0]).unwrap(), bad);
        assert!(marks_path(&dir).is_file());
        store.add_with(r"D:\Work\after.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        let again = Store::load_at(&dir, shared(), NOW);
        assert!(again.get(r"D:\Work\from-bak.txt").is_some());
        assert!(again.get(r"D:\Work\after.txt").is_some());
        assert_eq!(std::fs::read(&broken[0]).unwrap(), bad);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_without_a_usable_backup_is_left_untouched() {
        let dir = scratch("bak-bad");
        let bad = b"{not json";
        let bad_bak = b"{also bad";
        let path = marks_path(&dir);
        let bak = dir.join(BAK_NAME);
        std::fs::write(&path, bad).unwrap();
        std::fs::write(&bak, bad_bak).unwrap();
        let mut store = Store::load_at(&dir, shared(), NOW);
        assert!(store.is_blocked());
        assert!(store.recovery_notice().is_none());
        assert_eq!(store.add(r"D:\Work\x.txt"), Err(StoreError::Blocked));
        assert_eq!(store.remove(r"D:\Work\x.txt"), Err(StoreError::Blocked));
        assert_eq!(store.update_path(r"D:\Work\x.txt", r"D:\Work\y.txt"), Err(StoreError::Blocked));
        assert_eq!(store.save(), Err(StoreError::Blocked));
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        assert_eq!(std::fs::read(&bak).unwrap(), bad_bak);
        assert!(broken_files(&dir).is_empty());

        let dir2 = scratch("bak-missing");
        let path2 = marks_path(&dir2);
        std::fs::write(&path2, bad).unwrap();
        let mut store = Store::load_at(&dir2, shared(), NOW);
        assert!(store.is_blocked());
        assert_eq!(store.save(), Err(StoreError::Blocked));
        assert_eq!(std::fs::read(&path2).unwrap(), bad);
        assert!(!dir2.join(BAK_NAME).exists());
        assert!(broken_files(&dir2).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn two_handles_merge_adds_removes_and_path_updates() {
        let dir = scratch("merge");
        let lock = shared();
        let mut a = Store::load_at(&dir, lock.clone(), NOW);
        let mut b = Store::load_at(&dir, lock.clone(), NOW);
        a.add_with(r"D:\Work\one.txt", meta(Some("AA"), Some("01")), "2026-11-15T00:00:00Z").unwrap();
        b.add_with(r"D:\Work\two.txt", meta(Some("BB"), Some("02")), "2026-11-15T00:00:00Z").unwrap();
        a.save().unwrap();
        b.save().unwrap();
        let both = Store::load_at(&dir, lock.clone(), NOW);
        assert!(both.get(r"D:\Work\one.txt").is_some());
        assert!(both.get(r"D:\Work\two.txt").is_some());

        let mut seeded = Store::load_at(&dir, lock.clone(), NOW);
        seeded.save().ok();
        let mut left = Store::load_at(&dir, lock.clone(), NOW);
        let mut right = Store::load_at(&dir, lock.clone(), NOW);
        left.update_path(r"D:\Work\one.txt", r"D:\Work\moved.txt").unwrap();
        right.remove(r"D:\Work\two.txt").unwrap();
        right.add_with(r"D:\Work\three.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        left.save().unwrap();
        right.save().unwrap();
        let merged = Store::load_at(&dir, lock, NOW);
        assert!(merged.get(r"D:\Work\one.txt").is_none());
        assert_eq!(merged.get(r"D:\Work\moved.txt").unwrap().vol.as_deref(), Some("AA"));
        assert!(merged.get(r"D:\Work\two.txt").is_none());
        assert!(merged.get(r"D:\Work\three.txt").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_20001st_mark_is_refused_and_nothing_is_evicted() {
        let dir = scratch("cap");
        let mut store = Store::load_at(&dir, shared(), NOW);
        for i in 0..CAP {
            store.add_with(&format!(r"C:\m\{i}"), empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        }
        assert_eq!(store.len(), CAP);
        assert_eq!(store.add_with(r"C:\m\overflow", empty_meta(), "2026-11-15T00:00:00Z"), Err(StoreError::Full));
        assert_eq!(store.len(), CAP);
        assert!(store.get(r"C:\m\0").is_some());
        assert!(store.get(r"C:\m\19999").is_some());
        assert!(store.get(r"C:\m\overflow").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lost_rows_older_than_180_days_are_purged_on_load() {
        let dir = scratch("purge");
        let day = DAY_SECS;
        let doc = serde_json::json!({
            "version": 1,
            "marks": [
                {"path": "D:\\keep-ok.txt", "state": "ok", "lost_since": format_unix_z(NOW - 181 * day), "added": "2026-01-01T00:00:00Z"},
                {"path": "D:\\keep-recycled.txt", "state": "recycled", "lost_since": format_unix_z(NOW - 181 * day), "added": "2026-01-01T00:00:00Z"},
                {"path": "D:\\keep-179.txt", "state": "lost", "lost_since": format_unix_z(NOW - 179 * day), "added": "2026-01-01T00:00:00Z"},
                {"path": "D:\\drop-180.txt", "state": "lost", "lost_since": format_unix_z(NOW - 180 * day), "added": "2026-01-01T00:00:00Z"},
                {"path": "D:\\drop-181.txt", "state": "lost", "lost_since": format_unix_z(NOW - 181 * day), "added": "2026-01-01T00:00:00Z"},
                {"path": "D:\\keep-null.txt", "state": "lost", "lost_since": null, "added": "2026-01-01T00:00:00Z"}
            ]
        });
        std::fs::write(marks_path(&dir), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        let mut store = Store::load_at(&dir, shared(), NOW);
        assert!(store.get(r"D:\keep-ok.txt").is_some());
        assert_eq!(store.get(r"D:\keep-recycled.txt").unwrap().state, STATE_RECYCLED);
        assert!(store.get(r"D:\keep-179.txt").is_some());
        assert!(store.get(r"D:\keep-null.txt").is_some());
        assert!(store.get(r"D:\drop-180.txt").is_none());
        assert!(store.get(r"D:\drop-181.txt").is_none());
        store.save().unwrap();
        let again = Store::load_at(&dir, shared(), NOW);
        assert!(again.get(r"D:\drop-180.txt").is_none());
        assert!(again.get(r"D:\keep-179.txt").is_some());
        assert_eq!(again.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn null_file_ids_are_stored_and_skipped_by_the_id_index() {
        let dir = scratch("null-id");
        let mut store = Store::load_at(&dir, shared(), NOW);
        store.add_with(r"D:\Work\no-id.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        assert!(store.get(r"D:\Work\no-id.txt").unwrap().vol.is_none());
        assert!(store.get(r"D:\Work\no-id.txt").unwrap().fid.is_none());
        assert!(store.by_file_id("AA", "BB").is_none());
        store.save().unwrap();
        let text = std::fs::read_to_string(marks_path(&dir)).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["marks"][0]["vol"].is_null());
        assert!(value["marks"][0]["fid"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_temp_file_on_ntfs_has_a_volume_and_file_id() {
        let dir = scratch("identity");
        let file = dir.join("note.txt");
        std::fs::write(&file, b"hello-marks").unwrap();
        let id = read_file_identity(&file);
        assert!(id.vol.as_ref().is_some_and(|v| v.len() == 16), "{id:?}");
        assert!(id.fid.as_ref().is_some_and(|v| v.len() == 32), "{id:?}");
        assert_eq!(id.size, Some(11));
        assert!(id.mtime.is_some());
        assert!(!id.dir);
        let mut store = Store::load_at(&dir, shared(), NOW);
        store.add(&file.to_string_lossy()).unwrap();
        let mark = store.get(&file.to_string_lossy()).unwrap();
        assert_eq!(mark.vol, id.vol);
        assert_eq!(mark.fid, id.fid);
        assert_eq!(mark.size, Some(11));
        let missing = read_file_identity(Path::new(r"C:\maguro-marks-no-such\missing.txt"));
        assert!(missing.vol.is_none());
        assert!(missing.fid.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_writes_only_inside_the_injected_directory() {
        let root = scratch("sandbox");
        let inside = root.join("cfg");
        let outside = root.join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("marks.json"), b"sentinel-marks").unwrap();
        std::fs::write(outside.join("maguro.json"), b"sentinel-settings").unwrap();
        let mut store = Store::load_at(&inside, shared(), NOW);
        store.add_with(r"D:\Work\inside.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        store.add_with(r"D:\Work\inside-2.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        assert_eq!(std::fs::read(outside.join("marks.json")).unwrap(), b"sentinel-marks");
        assert_eq!(std::fs::read(outside.join("maguro.json")).unwrap(), b"sentinel-settings");
        assert!(!inside.join("maguro.json").exists());
        assert!(marks_path(&inside).is_file());
        for entry in std::fs::read_dir(&inside).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                name == FILE_NAME || name == BAK_NAME || name.starts_with("marks.json.broken-") || name.contains(".tmp"),
                "unexpected file in the config dir: {name}"
            );
        }
        let root_names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(root_names.iter().all(|n| n != "marks.json" && n != "maguro.json" && n != "marks.json.bak"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_first_save_of_the_day_keeps_one_backup() {
        let dir = scratch("daily");
        let lock = shared();
        let mut store = Store::load_at(&dir, lock.clone(), NOW);
        store.add_with(r"D:\Work\a.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        assert!(!dir.join(BAK_NAME).exists(), "nothing to back up on the first write");

        let today_bak = r#"{"version":1,"marks":[{"path":"D:\\sentinel.txt","added":"2026-11-15T00:00:00Z","state":"ok"}]}"#;
        std::fs::write(dir.join(BAK_NAME), today_bak).unwrap();
        store.add_with(r"D:\Work\b.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        assert_eq!(std::fs::read(dir.join(BAK_NAME)).unwrap(), today_bak.as_bytes());

        let yesterday = SystemTime::now() - std::time::Duration::from_secs(36 * 3600);
        let file = std::fs::OpenOptions::new().write(true).open(dir.join(BAK_NAME)).unwrap();
        file.set_modified(yesterday).unwrap();
        drop(file);
        let before = std::fs::read(marks_path(&dir)).unwrap();
        store.add_with(r"D:\Work\c.txt", empty_meta(), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        assert_eq!(std::fs::read(dir.join(BAK_NAME)).unwrap(), before);
        assert_ne!(std::fs::read(marks_path(&dir)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_named_mutex_can_be_taken_and_released() {
        let value = MarksLock::Os.with_lock(|| 7).unwrap();
        assert_eq!(value, 7);
    }

    #[test]
    fn a_missing_file_starts_empty_and_save_does_not_create_it_until_there_is_a_change() {
        let dir = scratch("empty");
        let mut store = Store::load_in(&dir, shared());
        assert!(!store.is_blocked());
        assert_eq!(store.len(), 0);
        store.save().unwrap();
        assert!(!marks_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn broken_files(dir: &Path) -> Vec<PathBuf> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("marks.json.broken-")))
            .collect();
        names.sort();
        names
    }
}
