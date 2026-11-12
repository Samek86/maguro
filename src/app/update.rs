//! Startup update check against the latest GitHub release, and the self-replace that installs it.
//! No token and no Actions: one unauthenticated GET to the public releases API, then the raw
//! `maguro.exe` asset and its `maguro.exe.sha256`. The exe is replaced only when the
//! SHA-256 matches. A missing checksum, a mismatch, or a zip-only release is reported and not installed.
//!
//! The running exe is not renamed in this process. The UI and the `--crash-watch` monitor both
//! map it, and that rename fails with access denied. After the download is verified, this process
//! starts `maguro.new.exe --apply-update`, closes every Maguro window so it can exit, and the
//! helper swaps the files only once those processes are gone.

use crate::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Duration;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

/// Test builds can point at a local mock or pretend to be older; release builds use the real values.
const API: &str = match option_env!("MAGURO_UPDATE_API") {
    Some(u) => u,
    None => "https://api.github.com/repos/Samek86/maguro/releases/latest",
};
pub(crate) const APP_VERSION: &str = match option_env!("MAGURO_FAKE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// Largest maguro.exe the updater will download.
const EXE_MAX: usize = 64 << 20;
/// Largest maguro.exe.sha256 the updater will read. A sha256sum line is under 100 bytes.
const SHA_MAX: usize = 4096;
/// GitHub release asset the updater downloads. Releases upload this raw exe, not a zip.
const EXE_ASSET: &str = "maguro.exe";
/// sha256sum checksum published next to the exe. `<64 hex>  maguro.exe`.
const SHA_ASSET: &str = "maguro.exe.sha256";
/// Downloaded build, next to the running exe, until the swap moves it into place.
const NEW_EXE: &str = "maguro.new.exe";
const OLD_EXE: &str = "maguro.old.exe";
/// One-line reason the restarted exe shows. Next to the exe, so portable mode and %APPDATA% stay as they are.
const FAIL_FILE: &str = "maguro.update-fail.txt";
/// Helper mode. The downloaded exe swaps the real one after this process and the crash monitor exit.
const APPLY_UPDATE: &str = "--apply-update";
/// Restarted exe should show the failure dialog. The reason text is in `FAIL_FILE`.
pub(crate) const UPDATE_FAILED: &str = "--update-failed";
/// How long the helper waits for the UI and the monitor to exit.
const EXIT_WAIT: Duration = Duration::from_secs(30);
/// AV and indexers hold the exe for a moment after the last process exits.
const SWAP_TRIES: u32 = 8;
const SWAP_PAUSE: Duration = Duration::from_millis(250);
/// `GetExitCodeProcess` while the process is still alive.
const STILL_ACTIVE: u32 = 259;
/// `OpenProcess` when that pid is already gone.
const ERROR_INVALID_PARAMETER: i32 = 87;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Release {
    /// Raw `maguro.exe` plus a usable `maguro.exe.sha256`.
    Exe { tag: String, url: String, size: u64, sha_url: String, sha_size: u64 },
    /// `maguro.exe` was published without a usable checksum file. Shown, never installed.
    MissingSha { tag: String },
    /// Newer release that published a zip and no `maguro.exe`. Shown, never extracted.
    ZipOnly { tag: String },
}

impl Release {
    fn tag(&self) -> &str {
        match self {
            Release::Exe { tag, .. } | Release::MissingSha { tag } | Release::ZipOnly { tag } => tag,
        }
    }
}

/// Why `install` left the running exe in place.
enum InstallFail {
    /// Checksum asset missing, or the checksum body could not be downloaded.
    MissingSha,
    /// Checksum text was not a sha256sum line for maguro.exe, or the digest differed.
    Mismatch,
    /// `reason` is one line: step, OS message, and code. `rename running exe: Access is denied (5)`.
    Other(String),
}

/// One rename attempt. A lock is retried. A missing path is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenameOutcome {
    Moved,
    Locked,
    Failed,
}

/// What the retry loop does after this attempt (`attempt` is 0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttemptNext {
    Done,
    SleepAndRetry,
    GiveUp,
}

/// Where a two-step swap finished. The running exe is put back when the new one never lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwapEnd {
    Swapped,
    Kept,
    RolledBack,
    RestoreFailed,
}

/// Args for `maguro.new.exe --apply-update <target exe> <ui pid> <monitor pid>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApplyJob {
    pub target: PathBuf,
    pub ui_pid: u32,
    pub monitor_pid: u32,
}

static FOUND: Mutex<Option<Release>> = Mutex::new(None);
static INSTALLING: AtomicBool = AtomicBool::new(false);
static CHECKING: AtomicBool = AtomicBool::new(false);
/// Stage 2 (`InstallFail::Other`). The worker stores it; the dialog reads it.
static FAIL_REASON: Mutex<String> = Mutex::new(String::new());

/// True while a manual check or an install is in flight. The settings button stays disabled.
pub(crate) fn update_busy() -> bool {
    CHECKING.load(Ordering::SeqCst) || INSTALLING.load(Ordering::SeqCst)
}

fn version_parts(v: &str) -> Vec<u64> {
    let core = v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next().unwrap_or("");
    core.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
}

/// True when `tag` (v1.2.3) is a higher version than `current`. Missing parts count as 0.
pub(crate) fn is_newer(tag: &str, current: &str) -> bool {
    let (a, b) = (version_parts(tag), version_parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn asset_file_name(asset: &serde_json::Value) -> Option<&str> {
    let name = asset.get("name")?.as_str()?;
    Some(name.rsplit(['/', '\\']).next().unwrap_or(name))
}

/// The tag plus the `maguro.exe` and `maguro.exe.sha256` assets.
/// The exe wins over a zip. An exe with no usable checksum is `MissingSha`, never a download.
/// A zip is never returned as something to download.
pub(crate) fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    let assets = v.get("assets")?.as_array()?;
    if let Some(asset) = assets.iter().find(|a| asset_file_name(a).is_some_and(|n| n.eq_ignore_ascii_case(EXE_ASSET))) {
        let url = asset.get("browser_download_url")?.as_str()?.to_string();
        let size = asset.get("size")?.as_u64()?;
        if let Some((sha_url, sha_size)) = sha_asset(assets) {
            return Some(Release::Exe { tag, url, size, sha_url, sha_size });
        }
        return Some(Release::MissingSha { tag });
    }
    let zip_only = assets.iter().any(|a| asset_file_name(a).is_some_and(|n| n.to_ascii_lowercase().ends_with(".zip")));
    zip_only.then_some(Release::ZipOnly { tag })
}

/// `maguro.exe.sha256` download URL and size, when the asset is small enough to be a sha256sum file.
fn sha_asset(assets: &[serde_json::Value]) -> Option<(String, u64)> {
    let sha = assets.iter().find(|a| asset_file_name(a).is_some_and(|n| n.eq_ignore_ascii_case(SHA_ASSET)))?;
    let sha_url = sha.get("browser_download_url")?.as_str()?.to_string();
    let sha_size = sha.get("size")?.as_u64()?;
    (1..=SHA_MAX as u64).contains(&sha_size).then_some((sha_url, sha_size))
}

/// First sha256sum line naming `maguro.exe`: `<64 hex>  maguro.exe`.
/// Binary mode (`hash *maguro.exe`) and a directory prefix on the name are accepted.
/// A bare 64-hex line (no filename) is also accepted so a mis-uploaded checksum asset still verifies;
/// a named `maguro.exe` line wins when both appear.
/// The hex is returned as published so the compare can be case-insensitive.
pub(crate) fn parse_sha256sum(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut bare: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        match parts.next() {
            None => {
                if bare.is_none() {
                    bare = Some(hash.to_string());
                }
            }
            Some(name) => {
                if parts.next().is_some() {
                    continue;
                }
                let name = name.trim_start_matches('*');
                let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
                if name.eq_ignore_ascii_case(EXE_ASSET) {
                    return Some(hash.to_string());
                }
            }
        }
    }
    bare
}

/// Case-insensitive match of two SHA-256 hex digests. A value that is not 64 hex digits does not match.
pub(crate) fn hashes_match(expected: &str, actual: &str) -> bool {
    fn hex64(s: &str) -> bool {
        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
    }
    hex64(expected) && hex64(actual) && expected.eq_ignore_ascii_case(actual)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn sha256_bytes(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_lower(&Sha256::digest(data))
}

/// SHA-256 of `path`, lowercase hex. Streams the file so a 64 MB exe is not hashed from a second copy.
fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 << 10];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex_lower(&hasher.finalize()))
}

/// Closes a WinHTTP handle when dropped.
struct Hnd(*mut core::ffi::c_void);
impl Drop for Hnd {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { _ = WinHttpCloseHandle(self.0); }
        }
    }
}

/// One GET over WinHTTP (no cache layer; redirects followed). Some(body) only on HTTP 200
/// and when the body is at most `max` bytes. Blocking: callers run it on the update worker thread.
fn http_get(url: &str, max: usize) -> Option<Vec<u8>> {
    let wide: Vec<u16> = url.encode_utf16().collect();
    let mut host = [0u16; 256];
    let mut path = vec![0u16; 4096];
    let mut uc = URL_COMPONENTS {
        dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
        lpszHostName: PWSTR(host.as_mut_ptr()),
        dwHostNameLength: host.len() as u32,
        lpszUrlPath: PWSTR(path.as_mut_ptr()),
        dwUrlPathLength: path.len() as u32,
        dwSchemeLength: 1,
        dwExtraInfoLength: 1,
        ..Default::default()
    };
    unsafe {
        WinHttpCrackUrl(&wide, 0, &mut uc).ok()?;
        // Path and query are cracked separately; the request needs both.
        let mut object: Vec<u16> = path[..uc.dwUrlPathLength as usize].to_vec();
        if !uc.lpszExtraInfo.is_null() {
            object.extend_from_slice(std::slice::from_raw_parts(uc.lpszExtraInfo.0, uc.dwExtraInfoLength as usize));
        }
        object.push(0);
        host[uc.dwHostNameLength as usize] = 0;
        let secure = uc.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS;
        let ua = HSTRING::from(format!("Maguro/{APP_VERSION}"));
        let ses = Hnd(WinHttpOpen(&ua, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if ses.0.is_null() {
            return None;
        }
        _ = WinHttpSetTimeouts(ses.0, 10_000, 15_000, 30_000, 60_000);
        let con = Hnd(WinHttpConnect(ses.0, PCWSTR(host.as_ptr()), uc.nPort, 0));
        if con.0.is_null() {
            return None;
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let req = Hnd(WinHttpOpenRequest(con.0, w!("GET"), PCWSTR(object.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
        if req.0.is_null() {
            return None;
        }
        let hdr: Vec<u16> = "Cache-Control: no-cache\r\nPragma: no-cache\r\n".encode_utf16().collect();
        WinHttpSendRequest(req.0, Some(&hdr), None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).ok()?;
        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .ok()?;
        if status != 200 {
            return None;
        }
        let mut body = Vec::new();
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let mut got = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut got).ok()?;
            if got == 0 {
                break;
            }
            body.extend_from_slice(&buf[..got as usize]);
            if body.len() > max {
                return None;
            }
        }
        Some(body)
    }
}

/// `step: message (code)` when Windows returned a code. The message's trailing period is dropped.
pub(crate) fn format_fail_reason(step: &str, message: &str, code: Option<i32>) -> String {
    let message = message.trim().trim_end_matches(['.', '。']).trim();
    if message.is_empty() {
        return match code {
            Some(code) => format!("{step} ({code})"),
            None => step.to_string(),
        };
    }
    match code {
        Some(code) => format!("{step}: {message} ({code})"),
        None => format!("{step}: {message}"),
    }
}

/// Rust's `io::Error` text is `Access is denied. (os error 5)`. Keep the words, not the suffix.
pub(crate) fn io_message(text: &str, code: i32) -> String {
    let suffix = format!(" (os error {code})");
    text.trim().strip_suffix(&suffix).unwrap_or(text.trim()).trim().to_string()
}

fn io_reason(step: &str, err: &std::io::Error) -> String {
    let code = err.raw_os_error();
    let message = match code {
        Some(c) => io_message(&err.to_string(), c),
        None => err.to_string(),
    };
    format_fail_reason(step, &message, code)
}

/// Win32 facility HRESULTs (`0x8007xxxx`) become the original code. A plain code is unchanged.
pub(crate) fn win32_code(hr: i32) -> i32 {
    let u = hr as u32;
    if u & 0xFFFF_0000 == 0x8007_0000 { (u & 0xFFFF) as i32 } else { hr }
}

/// One short line for the dialog, the result file, and the trace. Control characters become spaces.
pub(crate) fn one_line(reason: &str) -> String {
    let flat: String = reason.chars().take(300).map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn rename_outcome(code: Option<i32>) -> RenameOutcome {
    match code {
        // Not found, bad path, or a name that is not a file. Retrying will not create it.
        Some(2 | 3 | 161 | 206 | 267) => RenameOutcome::Failed,
        Some(_) => RenameOutcome::Locked,
        None => RenameOutcome::Failed,
    }
}

pub(crate) fn next_attempt(outcome: RenameOutcome, attempt: u32, tries: u32) -> AttemptNext {
    match outcome {
        RenameOutcome::Moved => AttemptNext::Done,
        RenameOutcome::Failed => AttemptNext::GiveUp,
        RenameOutcome::Locked if attempt + 1 < tries => AttemptNext::SleepAndRetry,
        RenameOutcome::Locked => AttemptNext::GiveUp,
    }
}

/// `running_moved` is exe -> maguro.old.exe. `new_in_place` is the new bytes at the exe path
/// (rename or copy). `restored` is maguro.old.exe put back after the new bytes failed to land.
pub(crate) fn swap_status(running_moved: bool, new_in_place: bool, restored: bool) -> SwapEnd {
    if !running_moved {
        SwapEnd::Kept
    } else if new_in_place {
        SwapEnd::Swapped
    } else if restored {
        SwapEnd::RolledBack
    } else {
        SwapEnd::RestoreFailed
    }
}

pub(crate) fn parse_apply_args(args: &[String]) -> Option<ApplyJob> {
    if args.first().map(String::as_str) != Some(APPLY_UPDATE) || args.len() != 4 {
        return None;
    }
    let target = args.get(1)?;
    if target.is_empty() || target.starts_with("--") {
        return None;
    }
    let ui_pid = args.get(2)?.parse::<u32>().ok()?;
    if ui_pid == 0 {
        return None;
    }
    let monitor_pid = args.get(3)?.parse::<u32>().ok()?;
    Some(ApplyJob { target: PathBuf::from(target), ui_pid, monitor_pid })
}

/// Top-level windows this process owns. Child classes (`MaguroPane`) are not in `EnumWindows`.
pub(crate) fn is_maguro_class(name: &str) -> bool {
    name.starts_with("Maguro")
}

pub(crate) fn install_fail_notice(reason: &str) -> String {
    let base = tr(crate::Key::UpdateCouldNotBeInstalledMaguroWas).to_string();
    let reason = one_line(reason);
    if reason.is_empty() {
        return base;
    }
    let line = tr(crate::Key::UpdateFailReason).replace("{reason}", &reason);
    format!("{base}\n\n{line}")
}

fn download(url: &str, dest: &Path) -> std::result::Result<(), String> {
    if let Err(e) = std::fs::remove_file(dest) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(io_reason("remove previous download", &e));
        }
    }
    let Some(body) = http_get(url, EXE_MAX) else {
        return Err(format_fail_reason("download", "could not download maguro.exe", None));
    };
    std::fs::write(dest, &body).map_err(|e| io_reason("write download", &e))
}

fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// What a release check found. Startup stays quiet for `Current` and `Failed`. A manual check reports both.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CheckHit {
    Newer(Release),
    Current,
    Failed,
}

/// `body` is the releases API response. A release that is not newer is current, even when it is not installable.
pub(crate) fn classify_check(body: &str, current: &str) -> CheckHit {
    match parse_release(body) {
        Some(rel) if is_newer(rel.tag(), current) => CheckHit::Newer(rel),
        Some(_) => CheckHit::Current,
        None => CheckHit::Failed,
    }
}

fn release_stage(rel: &Release) -> usize {
    match rel {
        Release::Exe { .. } => 0,
        Release::ZipOnly { .. } => 3,
        Release::MissingSha { .. } => 4,
    }
}

/// Startup posts only when a newer release exists.
pub(crate) fn startup_stage(hit: &CheckHit) -> Option<usize> {
    match hit {
        CheckHit::Newer(rel) => Some(release_stage(rel)),
        CheckHit::Current | CheckHit::Failed => None,
    }
}

/// Manual check always posts: 0 exe, 3 zip only, 4 checksum missing, 6 current, 7 could not check.
pub(crate) fn manual_stage(hit: &CheckHit) -> usize {
    match hit {
        CheckHit::Newer(rel) => release_stage(rel),
        CheckHit::Current => 6,
        CheckHit::Failed => 7,
    }
}

pub(crate) fn up_to_date_notice(current: &str) -> String {
    tr(crate::Key::MaguroCurIsUpToDate).replace("{cur}", current)
}

pub(crate) fn check_failed_notice() -> String {
    tr(crate::Key::CouldNotCheckForUpdates).to_string()
}

fn fetch_check() -> CheckHit {
    let text = http_get(API, 1 << 20).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    classify_check(&text, APP_VERSION)
}

fn store_newer(hit: &CheckHit) {
    if let CheckHit::Newer(rel) = hit {
        *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(rel.clone());
    }
}

fn poke_update_button() {
    unsafe { crate::settings::refresh_update_button() };
}

/// Settings → About. Ignores the startup-check toggle. The main window shows the result.
pub(crate) fn check_now(main: HWND) {
    if update_busy() {
        return;
    }
    CHECKING.store(true, Ordering::SeqCst);
    let h = main.0 as isize;
    let spawned = std::thread::Builder::new().name("maguro-update".into()).spawn(move || {
        let hit = fetch_check();
        let stage = manual_stage(&hit);
        store_newer(&hit);
        let posted = unsafe { PostMessageW(Some(HWND(h as _)), WM_UPDATE, WPARAM(stage), LPARAM(0)).is_ok() };
        if !posted {
            CHECKING.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        CHECKING.store(false, Ordering::SeqCst);
    }
}

/// Startup: remove the exe a previous update left behind, then ask GitHub when the setting is on.
/// `drop_new` is set for `--updated`: the apply helper has exited, so its `maguro.new.exe` can go too.
/// A normal launch leaves that file alone. The helper may still be running from it.
pub(crate) fn startup(main: HWND, check: bool, drop_new: bool) {
    let h = main.0 as isize;
    _ = std::thread::Builder::new().name("maguro-update".into()).spawn(move || {
        if let Some(dir) = exe_path().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
            // The previous process may still be closing for a moment.
            let mut names = vec![OLD_EXE];
            if drop_new {
                names.push(NEW_EXE);
            }
            for name in names {
                let path = dir.join(name);
                for _ in 0..20 {
                    if !path.exists() || std::fs::remove_file(&path).is_ok() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        if !check {
            return;
        }
        let hit = fetch_check();
        let Some(stage) = startup_stage(&hit) else { return };
        store_newer(&hit);
        unsafe { _ = PostMessageW(Some(HWND(h as _)), WM_UPDATE, WPARAM(stage), LPARAM(0)); }
    });
}

fn other(reason: String) -> InstallFail {
    InstallFail::Other(one_line(&reason))
}

/// Download the raw exe and check its size, MZ header, and SHA-256.
/// The swap happens in `--apply-update`, after this process and the crash monitor have exited.
/// On a missing checksum, a mismatch, or any other failure, `maguro.new.exe` is deleted.
fn install(rel: &Release) -> std::result::Result<(), InstallFail> {
    let Release::Exe { url, size, sha_url, sha_size, .. } = rel else { return Err(InstallFail::MissingSha) };
    let exe = exe_path().ok_or_else(|| other(format_fail_reason("exe path", "could not locate maguro.exe", None)))?;
    let dir = exe.parent().map(|p| p.to_path_buf()).ok_or_else(|| other(format_fail_reason("exe path", "could not locate the install folder", None)))?;
    let new = dir.join(NEW_EXE);
    let res = (|| {
        if *size == 0 || *size > EXE_MAX as u64 {
            return Err(other(format_fail_reason("download size", &format!("release asset is {size} bytes"), None)));
        }
        download(url, &new).map_err(other)?;
        let got = std::fs::metadata(&new).map(|m| m.len()).map_err(|e| other(io_reason("download size", &e)))?;
        if got != *size {
            return Err(other(format_fail_reason("download size", &format!("expected {size} bytes, got {got}"), None)));
        }
        let mut magic = [0u8; 2];
        {
            let mut f = std::fs::File::open(&new).map_err(|e| other(io_reason("read header", &e)))?;
            let n = std::io::Read::read(&mut f, &mut magic).map_err(|e| other(io_reason("read header", &e)))?;
            if n != 2 || &magic != b"MZ" {
                return Err(other(format_fail_reason("read header", "not a Windows executable", None)));
            }
        }
        let sha_body = http_get(sha_url, SHA_MAX).ok_or(InstallFail::MissingSha)?;
        if sha_body.len() as u64 != *sha_size {
            return Err(InstallFail::MissingSha);
        }
        let expected = parse_sha256sum(&String::from_utf8_lossy(&sha_body)).ok_or(InstallFail::Mismatch)?;
        let actual = sha256_file(&new).ok_or_else(|| other(format_fail_reason("sha256", "could not hash maguro.new.exe", None)))?;
        if !hashes_match(&expected, &actual) {
            return Err(InstallFail::Mismatch);
        }
        Ok(())
    })();
    if res.is_err() {
        _ = std::fs::remove_file(&new);
    }
    res
}

fn move_retry(from: &Path, to: &Path, step: &str) -> std::result::Result<(), String> {
    let mut last = String::new();
    for attempt in 0..SWAP_TRIES {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = io_reason(step, &e);
                if next_attempt(rename_outcome(e.raw_os_error()), attempt, SWAP_TRIES) != AttemptNext::SleepAndRetry {
                    return Err(last);
                }
                std::thread::sleep(SWAP_PAUSE);
            }
        }
    }
    Err(last)
}

fn remove_retry(path: &Path, step: &str) -> std::result::Result<(), String> {
    let mut last = String::new();
    for attempt in 0..SWAP_TRIES {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = io_reason(step, &e);
                if next_attempt(rename_outcome(e.raw_os_error()), attempt, SWAP_TRIES) != AttemptNext::SleepAndRetry {
                    return Err(last);
                }
                std::thread::sleep(SWAP_PAUSE);
            }
        }
    }
    Err(last)
}

/// The helper is running `maguro.new.exe`, so renaming that file can be denied even after the UI
/// and the monitor have exited. A copy leaves the helper's file for the new process to delete.
fn copy_retry(from: &Path, to: &Path) -> std::result::Result<(), String> {
    let mut last = String::new();
    for attempt in 0..SWAP_TRIES {
        match std::fs::copy(from, to) {
            Ok(n) => {
                let expect = std::fs::metadata(from).map(|m| m.len()).unwrap_or(n);
                if n == expect && n > 0 {
                    return Ok(());
                }
                _ = std::fs::remove_file(to);
                last = format_fail_reason("copy new exe", &format!("copied {n} bytes, expected {expect}"), None);
                if n != 0 || next_attempt(RenameOutcome::Locked, attempt, SWAP_TRIES) != AttemptNext::SleepAndRetry {
                    return Err(last);
                }
                std::thread::sleep(SWAP_PAUSE);
            }
            Err(e) => {
                last = io_reason("copy new exe", &e);
                if next_attempt(rename_outcome(e.raw_os_error()), attempt, SWAP_TRIES) != AttemptNext::SleepAndRetry {
                    return Err(last);
                }
                std::thread::sleep(SWAP_PAUSE);
            }
        }
    }
    Err(last)
}

fn place_new(new: &Path, exe: &Path) -> std::result::Result<(), String> {
    match move_retry(new, exe, "rename new exe") {
        Ok(()) => Ok(()),
        Err(rename_err) => match copy_retry(new, exe) {
            Ok(()) => Ok(()),
            Err(copy_err) => Err(format!("{rename_err}; {copy_err}")),
        },
    }
}

/// Running exe -> maguro.old.exe, then the new bytes onto the exe. Puts the old one back if that fails.
fn swap(exe: &Path, new: &Path, old: &Path) -> std::result::Result<(), String> {
    if old.exists() {
        remove_retry(old, "remove previous maguro.old.exe")?;
    }
    if let Err(e) = move_retry(exe, old, "rename running exe") {
        return finish_swap(false, false, false, Some(e));
    }
    if let Err(e) = place_new(new, exe) {
        return match rollback(exe) {
            Ok(()) => finish_swap(true, false, true, Some(e)),
            Err(restore) => finish_swap(true, false, false, Some(format!("{e}; {restore}"))),
        };
    }
    finish_swap(true, true, false, None)
}

fn finish_swap(running_moved: bool, new_in_place: bool, restored: bool, err: Option<String>) -> std::result::Result<(), String> {
    match swap_status(running_moved, new_in_place, restored) {
        SwapEnd::Swapped => Ok(()),
        SwapEnd::Kept | SwapEnd::RolledBack | SwapEnd::RestoreFailed => {
            Err(err.unwrap_or_else(|| format_fail_reason("swap", "update was not installed", None)))
        }
    }
}

fn rollback(exe: &Path) -> std::result::Result<(), String> {
    let old = exe.with_file_name(OLD_EXE);
    if !old.exists() {
        return Err(format_fail_reason("restore previous exe", "maguro.old.exe is missing", None));
    }
    if exe.exists() {
        remove_retry(exe, "remove failed update")?;
    }
    move_retry(&old, exe, "restore previous exe")
}

/// WM_UPDATE on the main window.
pub(crate) unsafe fn on_update(h: HWND, stage: usize) {
    CHECKING.store(false, Ordering::SeqCst);
    poke_update_button();
    match stage {
        0 => {
            let Some(rel) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
            let Release::Exe { tag, .. } = &rel else { return };
            let text = tr(crate::Key::MaguroNewIsAvailableYouHaveCur)
            .replace("{new}", tag)
            .replace("{cur}", &format!("v{APP_VERSION}"));
            let title = tr(crate::Key::MaguroUpdate);
            if !ask_confirm(h, Confirm::ok(title, text).default_cancel()) {
                return;
            }
            if INSTALLING.swap(true, Ordering::SeqCst) {
                return;
            }
            poke_update_button();
            let hv = h.0 as isize;
            let spawned = std::thread::Builder::new().name("maguro-update-install".into()).spawn(move || {
                let stage = match install(&rel) {
                    Ok(()) => 1,
                    Err(InstallFail::MissingSha) => {
                        crate::crash::log_kept("update install: checksum missing");
                        4
                    }
                    Err(InstallFail::Mismatch) => {
                        crate::crash::log_kept("update install: checksum mismatch");
                        5
                    }
                    Err(InstallFail::Other(reason)) => {
                        store_fail_reason(&reason);
                        2
                    }
                };
                _ = PostMessageW(Some(HWND(hv as _)), WM_UPDATE, WPARAM(stage), LPARAM(0));
            });
            if spawned.is_err() {
                INSTALLING.store(false, Ordering::SeqCst);
                poke_update_button();
            }
        }
        3 => {
            let Some(Release::ZipOnly { tag }) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
            zip_only(h, &tag);
        }
        4 => {
            INSTALLING.store(false, Ordering::SeqCst);
            poke_update_button();
            let Some(tag) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.tag().to_string()) else { return };
            sha_missing(h, &tag);
        }
        5 => {
            INSTALLING.store(false, Ordering::SeqCst);
            poke_update_button();
            sha_mismatch(h);
        }
        6 => up_to_date(h),
        7 => could_not_check(h),
        1 => match launch_apply_helper() {
            // The helper waits for this process. WM_CLOSE is queued, so the save in the main
            // window still runs, then the process exits and the monitor follows without a crash report.
            Ok(()) => close_all_windows(),
            Err(reason) => {
                store_fail_reason(&reason);
                if let Some(exe) = exe_path() {
                    _ = std::fs::remove_file(exe.with_file_name(NEW_EXE));
                }
                INSTALLING.store(false, Ordering::SeqCst);
                poke_update_button();
                failed(h);
            }
        },
        _ => {
            INSTALLING.store(false, Ordering::SeqCst);
            poke_update_button();
            failed(h);
        }
    }
}

/// Zip-only notice. Versions only: no address and no site name.
pub(crate) fn zip_notice(tag: &str, current: &str) -> String {
    tr(crate::Key::MaguroNewIsAvailableYouHaveCur2).replace("{new}", tag).replace("{cur}", current)
}

/// A newer release published only a zip. Tell the user; do not download or extract it.
unsafe fn zip_only(h: HWND, tag: &str) {
    let text = zip_notice(tag, &format!("v{APP_VERSION}"));
    let title = tr(crate::Key::MaguroUpdate);
    MessageBoxW(Some(h), &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION);
}

/// The release has no `maguro.exe.sha256`, or that file could not be downloaded. Nothing was installed.
unsafe fn sha_missing(h: HWND, tag: &str) {
    let text = tr(crate::Key::MaguroNewIsAvailableYouHaveCur3)
    .replace("{new}", tag)
    .replace("{cur}", &format!("v{APP_VERSION}"));
    let title = tr(crate::Key::MaguroUpdate);
    MessageBoxW(Some(h), &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONWARNING);
}

/// The downloaded exe does not match `maguro.exe.sha256`. The temp file is already deleted.
unsafe fn sha_mismatch(h: HWND) {
    let text = tr(crate::Key::DownloadedMaguroExeDoesNotMatchPanegri);
    let title = tr(crate::Key::MaguroUpdate);
    MessageBoxW(Some(h), &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONWARNING);
}

unsafe fn up_to_date(h: HWND) {
    let text = up_to_date_notice(&format!("v{APP_VERSION}"));
    let title = tr(crate::Key::MaguroUpdate);
    MessageBoxW(Some(h), &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION);
}

unsafe fn could_not_check(h: HWND) {
    let text = check_failed_notice();
    let title = tr(crate::Key::MaguroUpdate);
    MessageBoxW(Some(h), &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONWARNING);
}

fn store_fail_reason(reason: &str) {
    let reason = one_line(reason);
    *FAIL_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason.clone();
    crate::crash::log_kept(&format!("update install: {reason}"));
}

fn take_fail_reason() -> String {
    std::mem::take(&mut *FAIL_REASON.lock().unwrap_or_else(|e| e.into_inner()))
}

unsafe fn failed(h: HWND) {
    show_fail_dialog(h, &take_fail_reason());
}

/// The restarted exe, after a failed apply. The reason was logged when the dialog is shown.
pub(crate) unsafe fn show_update_failed(h: HWND, reason: &str) {
    let reason = one_line(reason);
    if !reason.is_empty() {
        crate::crash::log_kept(&format!("update install: {reason}"));
    }
    show_fail_dialog(h, &reason);
}

unsafe fn show_fail_dialog(h: HWND, reason: &str) {
    let text = install_fail_notice(reason);
    let title = tr(crate::Key::MaguroUpdate);
    let owner = if h.0.is_null() { None } else { Some(h) };
    MessageBoxW(owner, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONWARNING);
}

/// Every top-level Maguro window of this process gets WM_CLOSE.
/// The last main window saves, then `WM_DESTROY` posts quit, so Settings and the other
/// Maguro windows do not keep the process (and the mapped exe) alive.
unsafe fn close_all_windows() {
    unsafe extern "system" fn each(h: HWND, _a1: LPARAM) -> BOOL {
        crate::crash::catch_bool(std::panic::AssertUnwindSafe(|| unsafe { each_inner(h) }))
    }
    unsafe fn each_inner(h: HWND) -> BOOL {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != std::process::id() {
            return TRUE;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, &mut cls) as usize;
        if is_maguro_class(&String::from_utf16_lossy(&cls[..n])) {
            _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        TRUE
    }
    _ = EnumWindows(Some(each), LPARAM(0));
}

/// Theme restart. The new process waits out this one, then opens the saved tabs instead of the startup layout.
pub(crate) const KEEP_TABS: &str = "--keep-tabs";

/// Drop `--keep-tabs` from the launch arguments. True when it was present.
pub(crate) fn take_keep_tabs(args: &mut Vec<String>) -> bool {
    let keep = args.iter().any(|a| a == KEEP_TABS);
    args.retain(|a| a != KEEP_TABS);
    keep
}

/// Start this exe again and close every window of this process. The child waits until we exit, then reads the session just saved.
pub(crate) unsafe fn relaunch_same_session() -> bool {
    let Some(exe) = exe_path() else { return false };
    let started = std::process::Command::new(&exe)
        .arg("--updated")
        .arg(std::process::id().to_string())
        .arg(KEEP_TABS)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn()
        .is_ok();
    if started {
        close_all_windows();
    }
    started
}

/// `--updated <pid>` from the old process: wait for it to save and exit before reading settings,
/// and before this process registers the global hotkey. True when the flag was present, which
/// also means a leftover `maguro.new.exe` (the apply helper) can be deleted.
pub(crate) fn wait_for_previous(args: &mut Vec<String>) -> bool {
    if args.first().map(String::as_str) != Some("--updated") {
        return false;
    }
    let pid = args.get(1).and_then(|p| p.parse::<u32>().ok());
    args.drain(..args.len().min(2));
    if let Some(pid) = pid {
        unsafe {
            if let Ok(p) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                _ = WaitForSingleObject(p, 20_000);
                _ = CloseHandle(p);
            }
        }
    }
    true
}

/// Drop `--update-failed` so it is not opened as a folder. True when it was present.
pub(crate) fn take_update_failed_flag(args: &mut Vec<String>) -> bool {
    let had = args.iter().any(|a| a == UPDATE_FAILED);
    args.retain(|a| a != UPDATE_FAILED);
    had
}

fn fail_path(exe: &Path) -> PathBuf {
    exe.with_file_name(FAIL_FILE)
}

fn write_fail_file(exe: &Path, reason: &str) -> std::result::Result<(), String> {
    let path = fail_path(exe);
    std::fs::write(&path, one_line(reason).as_bytes()).map_err(|e| io_reason("write update result", &e))
}

fn consume_fail_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    _ = std::fs::remove_file(path);
    Some(one_line(&text))
}

/// Whether startup should show the failure dialog.
/// A successful `--updated` launch drops a stale note. The failure restart sets the flag.
/// A note with no flag still shows, so a helper that died after writing the file is not silent.
pub(crate) fn fail_note_to_show(flagged: bool, updated: bool, note: Option<String>) -> Option<String> {
    if updated && !flagged {
        return None;
    }
    match (flagged, note) {
        (false, None) => None,
        (_, Some(text)) => Some(text),
        (true, None) => Some(String::new()),
    }
}

/// `--update-failed`, or a result file the helper wrote beside the exe. The file is removed.
/// Empty means the flag was set and the file was already gone: the dialog has no extra line.
pub(crate) fn take_update_failed(args: &mut Vec<String>) -> Option<String> {
    let flagged = take_update_failed_flag(args);
    let updated = args.first().map(String::as_str) == Some("--updated");
    let from_file = exe_path().and_then(|exe| consume_fail_file(&fail_path(&exe)));
    fail_note_to_show(flagged, updated, from_file)
}

/// `--apply-update` never installs hooks or opens a window. It swaps the exe and starts the new one.
pub(crate) fn apply_helper(args: &[String]) -> bool {
    if args.first().map(String::as_str) != Some(APPLY_UPDATE) {
        return false;
    }
    let code = match parse_apply_args(args) {
        Some(job) => apply_update(&job),
        None => {
            let reason = format_fail_reason("apply update", "expected the exe path, the window pid, and the monitor pid", None);
            unsafe { show_fail_dialog(HWND::default(), &reason) };
            2
        }
    };
    std::process::exit(code);
}

fn apply_update(job: &ApplyJob) -> i32 {
    let target = &job.target;
    let new = target.with_file_name(NEW_EXE);
    let old = target.with_file_name(OLD_EXE);
    if let Err(reason) = wait_pids(job) {
        return report_failure(job, &reason);
    }
    if let Err(reason) = swap(target, &new, &old) {
        return report_failure(job, &reason);
    }
    _ = std::fs::remove_file(fail_path(target));
    match spawn_updated(target) {
        Ok(()) => 0,
        Err(reason) => {
            let reason = match rollback(target) {
                Ok(()) => reason,
                Err(restore) => format!("{reason}; {restore}"),
            };
            report_failure(job, &reason)
        }
    }
}

fn wait_pids(job: &ApplyJob) -> std::result::Result<(), String> {
    wait_pid(job.ui_pid, "wait for ui process", EXIT_WAIT)?;
    if job.monitor_pid != 0 {
        wait_pid(job.monitor_pid, "wait for crash monitor", EXIT_WAIT)?;
    }
    Ok(())
}

fn wait_pid(pid: u32, step: &str, timeout: Duration) -> std::result::Result<(), String> {
    if pid == 0 {
        return Err(format_fail_reason(step, "missing process id", None));
    }
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let wait = WaitForSingleObject(h, ms);
                _ = CloseHandle(h);
                if wait == WAIT_OBJECT_0 {
                    return Ok(());
                }
                if wait == WAIT_TIMEOUT {
                    return Err(format_fail_reason(step, &format!("pid {pid} still running after {}s", timeout.as_secs()), Some(WAIT_TIMEOUT.0 as i32)));
                }
                Err(format_fail_reason(step, &format!("pid {pid} wait failed"), Some(wait.0 as i32)))
            }
            Err(e) => {
                let code = win32_code(e.code().0);
                if code == ERROR_INVALID_PARAMETER {
                    return Ok(());
                }
                Err(format_fail_reason(step, &e.to_string(), Some(code)))
            }
        }
    }
}

fn process_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) else { return false };
        let mut code = STILL_ACTIVE;
        let running = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE;
        _ = CloseHandle(h);
        running
    }
}

fn report_failure(job: &ApplyJob, reason: &str) -> i32 {
    let reason = one_line(reason);
    crate::crash::log_kept(&format!("update install: {reason}"));
    // The UI is still up. A second maguro.exe would hand off to it and the dialog would never show.
    if process_running(job.ui_pid) || !job.target.is_file() {
        unsafe { show_fail_dialog(HWND::default(), &reason) };
        return 1;
    }
    if let Err(e) = write_fail_file(&job.target, &reason) {
        unsafe { show_fail_dialog(HWND::default(), &format!("{reason}; {e}")) };
        return 1;
    }
    match spawn_failed(&job.target) {
        Ok(()) => 1,
        Err(e) => {
            unsafe { show_fail_dialog(HWND::default(), &format!("{reason}; {e}")) };
            1
        }
    }
}

fn launch_apply_helper() -> std::result::Result<(), String> {
    let exe = exe_path().ok_or_else(|| format_fail_reason("exe path", "could not locate maguro.exe", None))?;
    let new = exe.with_file_name(NEW_EXE);
    if !new.is_file() {
        return Err(format_fail_reason("start apply helper", "maguro.new.exe is missing", None));
    }
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let args = vec![
        APPLY_UPDATE.to_string(),
        exe.to_string_lossy().into_owned(),
        std::process::id().to_string(),
        crate::crash::watcher_pid().to_string(),
    ];
    spawn_detached(&new, &args, &dir, "start apply helper", true)
}

fn spawn_updated(exe: &Path) -> std::result::Result<(), String> {
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    // The new process waits for this helper before it registers Alt+Z, then deletes the leftovers.
    let args = vec!["--updated".to_string(), std::process::id().to_string()];
    spawn_detached(exe, &args, &dir, "start updated maguro", false)
}

fn spawn_failed(exe: &Path) -> std::result::Result<(), String> {
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    // `--updated` waits out this helper so it can delete maguro.new.exe. The flag shows the dialog.
    let args = vec!["--updated".to_string(), std::process::id().to_string(), UPDATE_FAILED.to_string()];
    spawn_detached(exe, &args, &dir, "restart maguro", false)
}

fn spawn_detached(exe: &Path, args: &[String], dir: &Path, step: &str, no_window: bool) -> std::result::Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).current_dir(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // The helper is the same Windows subsystem binary. No console. The restarted app must still show its window.
    if no_window {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
    }
    cmd.spawn().map(|_| ()).map_err(|e| io_reason(step, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_tabs_flag_is_removed_from_the_launch_args() {
        let mut args = vec!["--keep-tabs".into(), r"C:\Work".into()];
        assert!(take_keep_tabs(&mut args));
        assert_eq!(args, vec![r"C:\Work".to_string()]);
        assert!(!take_keep_tabs(&mut args));
        assert_eq!(args, vec![r"C:\Work".to_string()]);
    }

    #[test]
    fn zip_notice_has_versions_and_no_address() {
        let text = zip_notice("v0.2.0", "v0.1.77");
        assert!(text.contains("v0.2.0"), "{text}");
        assert!(text.contains("v0.1.77"), "{text}");
        let low = text.to_ascii_lowercase();
        assert!(!low.contains("github"), "{text}");
        assert!(!text.contains("http"), "{text}");
        assert!(!text.contains("{url}"), "{text}");
        assert!(!text.contains('{'), "{text}");
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn release_json_picks_exe_and_sha_assets() {
        let j = r#"{"tag_name":"v0.2.0","assets":[{"name":"notes.txt","size":3,"browser_download_url":"x"},
            {"name":"maguro-v0.2.0-x64.zip","size":99,"browser_download_url":"https://e/p.zip"},
            {"name":"maguro.exe.SHA256","size":80,"browser_download_url":"https://e/maguro.exe.sha256"},
            {"name":"Maguro.EXE","size":1234,"browser_download_url":"https://e/maguro.exe"}]}"#;
        assert_eq!(
            parse_release(j),
            Some(Release::Exe {
                tag: "v0.2.0".into(),
                url: "https://e/maguro.exe".into(),
                size: 1234,
                sha_url: "https://e/maguro.exe.sha256".into(),
                sha_size: 80,
            })
        );
        assert_eq!(parse_release(r#"{"tag_name":"v1","assets":[]}"#), None);
        assert_eq!(parse_release("not json"), None);
        // A checksum with no exe is not an update.
        assert_eq!(
            parse_release(r#"{"tag_name":"v9","assets":[{"name":"maguro.exe.sha256","size":80,"browser_download_url":"https://e/s"}]}"#),
            None
        );
    }

    #[test]
    fn release_json_exe_without_sha_is_not_installed() {
        let j = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"maguro.exe","size":1234,"browser_download_url":"https://e/maguro.exe"},
            {"name":"maguro-v0.2.0-x64.zip","size":99,"browser_download_url":"https://e/p.zip"}]}"#;
        assert_eq!(parse_release(j), Some(Release::MissingSha { tag: "v0.2.0".into() }));
        // An empty or oversized checksum file cannot be used.
        let empty = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"maguro.exe","size":10,"browser_download_url":"https://e/maguro.exe"},
            {"name":"maguro.exe.sha256","size":0,"browser_download_url":"https://e/s"}]}"#;
        assert_eq!(parse_release(empty), Some(Release::MissingSha { tag: "v0.2.0".into() }));
        let big = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"maguro.exe","size":10,"browser_download_url":"https://e/maguro.exe"},
            {"name":"maguro.exe.sha256","size":4097,"browser_download_url":"https://e/s"}]}"#;
        assert_eq!(parse_release(big), Some(Release::MissingSha { tag: "v0.2.0".into() }));
    }

    #[test]
    fn sha256sum_parses_and_compares_case_insensitively() {
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        // sha256sum text mode: 64 hex, two spaces, maguro.exe.
        assert_eq!(parse_sha256sum(&format!("{hash}  maguro.exe\n")).as_deref(), Some(hash));
        assert_eq!(parse_sha256sum(&format!("{hash}  maguro.exe\r\n")).as_deref(), Some(hash));
        let upper = hash.to_ascii_uppercase();
        let parsed = parse_sha256sum(&format!("\u{feff}{upper}  maguro.exe")).unwrap();
        assert!(hashes_match(&parsed, hash));
        assert!(hashes_match(hash, &upper));
        assert!(!hashes_match(&parsed, &hash.replace('b', "c")));
        // Binary mode, a directory prefix, and a case-different file name.
        assert_eq!(parse_sha256sum(&format!("{hash} *maguro.exe")).as_deref(), Some(hash));
        assert_eq!(parse_sha256sum(&format!("{hash}  build/maguro.exe")).as_deref(), Some(hash));
        assert_eq!(parse_sha256sum(&format!("{hash}  MAGURO.EXE")).as_deref(), Some(hash));
        assert_eq!(parse_sha256sum(&format!("# comment\n\n{hash}  maguro.exe")).as_deref(), Some(hash));
        assert!(parse_sha256sum("deadbeef  maguro.exe").is_none());
        assert!(parse_sha256sum(&format!("{hash}  other.exe")).is_none());
        assert!(parse_sha256sum(&format!("{hash}  maguro.exe extra")).is_none());
        // Bare hash (no filename) — tolerant of a mis-uploaded sha256 asset.
        assert_eq!(parse_sha256sum(&format!("{hash}")).as_deref(), Some(hash));
        assert_eq!(parse_sha256sum(&format!("{hash}\n")).as_deref(), Some(hash));
        // Named line wins over a bare hash earlier in the file.
        assert_eq!(parse_sha256sum(&format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n{hash}  maguro.exe")).as_deref(), Some(hash));
        assert!(parse_sha256sum("").is_none());
        assert!(parse_sha256sum("SHA256 (maguro.exe) = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").is_none());
        assert!(!hashes_match("gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg", hash));
        assert!(!hashes_match(hash, &hash[..63]));
        assert!(!hashes_match("", ""));
    }

    #[test]
    fn sha256_of_abc_matches_the_published_digest() {
        // echo -n abc | sha256sum
        let digest = sha256_bytes(b"abc");
        assert_eq!(digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let line = format!("{}  maguro.exe", digest.to_ascii_uppercase());
        assert!(hashes_match(parse_sha256sum(&line).as_deref().unwrap(), &digest));
        let path = std::env::temp_dir().join(format!("pg-sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(sha256_file(&path).as_deref(), Some(digest.as_str()));
        _ = std::fs::remove_file(&path);
    }

    #[test]
    fn release_json_zip_only_is_not_an_exe() {
        let j = r#"{"tag_name":"v0.1.4","assets":[
            {"name":"maguro-v0.1.4-x64.zip","size":1252190,"browser_download_url":"https://e/p.zip"}]}"#;
        assert_eq!(parse_release(j), Some(Release::ZipOnly { tag: "v0.1.4".into() }));
        // Some other exe is not the update asset. A zip beside it is still zip-only.
        let other = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"setup.exe","size":10,"browser_download_url":"https://e/setup.exe"},
            {"name":"maguro-v0.2.0-x64.zip","size":99,"browser_download_url":"https://e/p.zip"}]}"#;
        assert_eq!(parse_release(other), Some(Release::ZipOnly { tag: "v0.2.0".into() }));
        assert_eq!(
            parse_release(r#"{"tag_name":"v9","assets":[{"name":"readme.txt","size":1,"browser_download_url":"x"}]}"#),
            None
        );
    }

    #[test]
    fn swap_moves_and_rolls_back() {
        let d = std::env::temp_dir().join(format!("pg-swap-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (exe, new, old) = (d.join("maguro.exe"), d.join("maguro.new.exe"), d.join(OLD_EXE));
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        assert!(swap(&exe, &new, &old).is_ok());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "old");
        assert!(rollback(&exe).is_ok());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert!(!old.exists());
        // A missing new file is put back, and the error names the step.
        let err = swap(&exe, &new, &old).unwrap_err();
        assert!(err.contains("rename new exe") || err.contains("copy new exe"), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(swap_status(false, false, false), SwapEnd::Kept);
        assert_eq!(swap_status(true, false, true), SwapEnd::RolledBack);
        _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fail_reason_names_the_step_and_the_os_code() {
        assert_eq!(
            format_fail_reason("rename running exe", &io_message("Access is denied. (os error 5)", 5), Some(5)),
            "rename running exe: Access is denied (5)"
        );
        assert_eq!(format_fail_reason("download", "could not download maguro.exe", None), "download: could not download maguro.exe");
        assert_eq!(format_fail_reason("rename running exe", "Access is denied.\r\n", Some(5)), "rename running exe: Access is denied (5)");
        assert_eq!(format_fail_reason("wait for ui process", "", Some(258)), "wait for ui process (258)");
        let err = std::io::Error::from_raw_os_error(5);
        let reason = io_reason("rename running exe", &err);
        assert!(reason.starts_with("rename running exe: "), "{reason}");
        assert!(reason.ends_with(" (5)"), "{reason}");
        assert!(!reason.contains("os error"), "{reason}");
        assert_eq!(win32_code(0x8007_0005u32 as i32), 5);
        assert_eq!(win32_code(5), 5);
        assert_eq!(one_line("rename running exe: Access is denied (5)\r\nextra"), "rename running exe: Access is denied (5) extra");
    }

    #[test]
    fn retry_gives_up_and_rollback_follows_the_second_failure() {
        assert_eq!(rename_outcome(Some(5)), RenameOutcome::Locked);
        assert_eq!(rename_outcome(Some(32)), RenameOutcome::Locked);
        assert_eq!(rename_outcome(Some(33)), RenameOutcome::Locked);
        assert_eq!(rename_outcome(Some(2)), RenameOutcome::Failed);
        assert_eq!(rename_outcome(Some(3)), RenameOutcome::Failed);
        assert_eq!(rename_outcome(None), RenameOutcome::Failed);
        assert_eq!(next_attempt(RenameOutcome::Locked, 0, SWAP_TRIES), AttemptNext::SleepAndRetry);
        assert_eq!(next_attempt(RenameOutcome::Locked, SWAP_TRIES - 1, SWAP_TRIES), AttemptNext::GiveUp);
        assert_eq!(next_attempt(RenameOutcome::Moved, 3, SWAP_TRIES), AttemptNext::Done);
        assert_eq!(next_attempt(RenameOutcome::Failed, 0, SWAP_TRIES), AttemptNext::GiveUp);
        // A lock, then success: the new exe is in place. No rollback.
        assert_eq!(scripted_swap(&[RenameOutcome::Locked, RenameOutcome::Moved], &[RenameOutcome::Moved], &[RenameOutcome::Failed]), SwapEnd::Swapped);
        // The running exe never moves. It stays where it is.
        assert_eq!(scripted_swap(&[RenameOutcome::Locked, RenameOutcome::Locked, RenameOutcome::Locked], &[], &[]), SwapEnd::Kept);
        // The new exe never lands, and the old one is put back.
        assert_eq!(
            scripted_swap(&[RenameOutcome::Moved], &[RenameOutcome::Locked, RenameOutcome::Failed], &[RenameOutcome::Moved]),
            SwapEnd::RolledBack
        );
        // The new exe never lands, and putting the old one back fails too.
        assert_eq!(
            scripted_swap(&[RenameOutcome::Moved], &[RenameOutcome::Failed], &[RenameOutcome::Locked, RenameOutcome::Locked, RenameOutcome::Locked]),
            SwapEnd::RestoreFailed
        );
    }

    /// Replay rename attempts the way `move_retry` does. `tries` is 3 so the scripts stay short.
    fn scripted_swap(running: &[RenameOutcome], new_exe: &[RenameOutcome], restore: &[RenameOutcome]) -> SwapEnd {
        const TRIES: u32 = 3;
        let running_moved = phase_moved(running, TRIES);
        if !running_moved {
            return swap_status(false, false, false);
        }
        if phase_moved(new_exe, TRIES) {
            return swap_status(true, true, false);
        }
        swap_status(true, false, phase_moved(restore, TRIES))
    }

    fn phase_moved(outcomes: &[RenameOutcome], tries: u32) -> bool {
        for (i, outcome) in outcomes.iter().copied().enumerate() {
            match next_attempt(outcome, i as u32, tries) {
                AttemptNext::Done => return true,
                AttemptNext::GiveUp => return false,
                AttemptNext::SleepAndRetry => {}
            }
        }
        false
    }

    #[test]
    fn apply_update_args_round_trip() {
        let args = vec![
            "--apply-update".into(),
            r"D:\Tools\Maguro\maguro.exe".into(),
            "4242".into(),
            "4243".into(),
        ];
        let job = parse_apply_args(&args).unwrap();
        assert_eq!(job.target, PathBuf::from(r"D:\Tools\Maguro\maguro.exe"));
        assert_eq!((job.ui_pid, job.monitor_pid), (4242, 4243));
        let spaced = vec!["--apply-update".into(), r"C:\Program Files\Maguro\maguro.exe".into(), "9".into(), "0".into()];
        let no_mon = parse_apply_args(&spaced).unwrap();
        assert_eq!(no_mon.monitor_pid, 0);
        assert_eq!(no_mon.target, PathBuf::from(r"C:\Program Files\Maguro\maguro.exe"));
        assert!(parse_apply_args(&["--apply-update".into()]).is_none());
        assert!(parse_apply_args(&["--apply-update".into(), r"C:\maguro.exe".into(), "nope".into(), "1".into()]).is_none());
        assert!(parse_apply_args(&["--apply-update".into(), r"C:\maguro.exe".into(), "0".into(), "1".into()]).is_none());
        assert!(parse_apply_args(&["--updated".into(), "1".into()]).is_none());
        assert!(parse_apply_args(&["--apply-update".into(), "--updated".into(), "1".into(), "2".into()]).is_none());
        let note = Some("rename running exe: Access is denied (5)".to_string());
        assert_eq!(fail_note_to_show(true, true, note.clone()), note);
        assert_eq!(fail_note_to_show(false, true, note.clone()), None);
        assert_eq!(fail_note_to_show(false, false, note.clone()), note);
        assert_eq!(fail_note_to_show(true, false, None), Some(String::new()));
        assert_eq!(fail_note_to_show(false, false, None), None);
    }

    #[test]
    fn update_failed_flag_is_removed_and_the_note_is_one_line() {
        let mut args = vec!["--updated".into(), "12".into(), "--update-failed".into(), r"C:\Work".into()];
        assert!(take_update_failed_flag(&mut args));
        assert_eq!(args, vec!["--updated".to_string(), "12".to_string(), r"C:\Work".to_string()]);
        assert!(!take_update_failed_flag(&mut args));
        let dir = std::env::temp_dir().join(format!("maguro-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("maguro.exe");
        std::fs::write(&exe, b"x").unwrap();
        write_fail_file(&exe, "rename running exe: Access is denied (5)\r\nsecond").unwrap();
        let got = consume_fail_file(&fail_path(&exe)).unwrap();
        assert_eq!(got, "rename running exe: Access is denied (5) second");
        assert!(!fail_path(&exe).exists());
        assert!(consume_fail_file(&fail_path(&exe)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_dialog_adds_a_translated_reason_line() {
        let reason = "rename running exe: Access is denied (5)";
        let prev = crate::UI.load(Ordering::Relaxed);
        crate::UI.store(0, Ordering::Relaxed);
        let en = install_fail_notice(reason);
        assert!(en.contains("The update could not be installed. Maguro was left as it was."), "{en}");
        assert!(en.contains("Reason: rename running exe: Access is denied (5)"), "{en}");
        assert!(!en.contains("{reason}"), "{en}");
        assert_eq!(install_fail_notice("  \n"), "The update could not be installed. Maguro was left as it was.");
        crate::UI.store(1, Ordering::Relaxed);
        let ko = install_fail_notice(reason);
        assert!(ko.contains("이유:"), "{ko}");
        assert!(ko.contains(reason), "{ko}");
        crate::UI.store(2, Ordering::Relaxed);
        let ja = install_fail_notice(reason);
        assert!(ja.contains("理由:"), "{ja}");
        assert!(ja.contains("更新をインストールできませんでした"), "{ja}");
        crate::UI.store(prev, Ordering::Relaxed);
        assert!(is_maguro_class("MaguroMain"));
        assert!(is_maguro_class("MaguroSettings"));
        assert!(is_maguro_class("MaguroAbout"));
        assert!(!is_maguro_class("Notepad"));
        assert!(!is_maguro_class(""));
    }

    fn exe_json(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","assets":[
            {{"name":"maguro.exe","size":10,"browser_download_url":"https://e/maguro.exe"}},
            {{"name":"maguro.exe.sha256","size":80,"browser_download_url":"https://e/s"}}]}}"#
        )
    }

    #[test]
    fn manual_check_classifies_the_release_body() {
        let newer = classify_check(&exe_json("v0.2.0"), "0.1.0");
        assert!(matches!(newer, CheckHit::Newer(Release::Exe { .. })), "{newer:?}");
        assert_eq!(manual_stage(&newer), 0);
        assert_eq!(startup_stage(&newer), Some(0));

        let same = classify_check(&exe_json("v0.1.0"), "0.1.0");
        assert_eq!(same, CheckHit::Current);
        assert_eq!(manual_stage(&same), 6);
        assert_eq!(startup_stage(&same), None);

        let older = classify_check(&exe_json("v0.0.9"), "0.1.0");
        assert_eq!(older, CheckHit::Current);
        assert_eq!(startup_stage(&older), None);

        let zip = classify_check(
            r#"{"tag_name":"v0.2.0","assets":[{"name":"maguro-v0.2.0-x64.zip","size":99,"browser_download_url":"https://e/p.zip"}]}"#,
            "0.1.0",
        );
        assert!(matches!(zip, CheckHit::Newer(Release::ZipOnly { .. })), "{zip:?}");
        assert_eq!(manual_stage(&zip), 3);
        assert_eq!(startup_stage(&zip), Some(3));

        let old_zip = classify_check(
            r#"{"tag_name":"v0.0.1","assets":[{"name":"maguro.zip","size":4,"browser_download_url":"https://e/p.zip"}]}"#,
            "0.1.0",
        );
        assert_eq!(old_zip, CheckHit::Current);

        let missing = classify_check(
            r#"{"tag_name":"v0.2.0","assets":[{"name":"maguro.exe","size":10,"browser_download_url":"https://e/maguro.exe"}]}"#,
            "0.1.0",
        );
        assert!(matches!(missing, CheckHit::Newer(Release::MissingSha { .. })), "{missing:?}");
        assert_eq!(manual_stage(&missing), 4);

        for body in ["", "not json", r#"{"tag_name":"v9","assets":[]}"#] {
            let failed = classify_check(body, "0.1.0");
            assert_eq!(failed, CheckHit::Failed, "{body}");
            assert_eq!(manual_stage(&failed), 7);
            assert_eq!(startup_stage(&failed), None);
        }
    }

    #[test]
    fn manual_check_notices_name_the_version_and_no_site() {
        let current = up_to_date_notice("v0.1.0");
        assert!(current.contains("v0.1.0"), "{current}");
        assert!(!current.contains('{'), "{current}");
        let failed = check_failed_notice();
        assert!(!failed.is_empty());
        for text in [current.as_str(), failed.as_str()] {
            let low = text.to_ascii_lowercase();
            assert!(!low.contains("github"), "{text}");
            assert!(!text.contains("http"), "{text}");
        }
    }
}
