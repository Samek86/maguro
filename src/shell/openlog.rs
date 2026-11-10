//! Dated file-open timing logs, next to the settings file.
//!
//! `Log/maguro-open-timing-YYYY-MM-DD.log` under the Maguro config directory
//! (`%APPDATA%\Maguro\Log` when Maguro is not portable). Files whose date is more
//! than 14 days before today are deleted once, when a window opens. Nothing else
//! in that folder is removed.
//!
//! The switch defaults off. `MAGURO_OPEN_TIMING=1` (or `true`) forces logging.
//! The existing right-click timing setting turns it on as well. `MAGURO_OPEN_TIMING_LOG`
//! is an explicit file path. When both switches are off, a call is one atomic load
//! and does not format a line, open a file, or touch the debugger.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::imports::*;

const STEM: &str = "maguro-open-timing";

static SETTING: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<()> = Mutex::new(());

pub(crate) fn set_enabled(on: bool) {
    SETTING.store(on, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    env_on() || SETTING.load(Ordering::Relaxed)
}

fn env_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("MAGURO_OPEN_TIMING") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    })
}

fn env_log_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let Ok(p) = std::env::var("MAGURO_OPEN_TIMING_LOG") else {
            return None;
        };
        let p = p.trim();
        if p.is_empty() {
            None
        } else {
            Some(PathBuf::from(p))
        }
    })
    .clone()
}

pub(crate) fn log_name(y: i32, m: u32, d: u32) -> String {
    super::ctxlog::file_name_stem(STEM, y, m, d)
}

pub(crate) fn sweep_startup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = crate::settings::store::config_dir().join("Log");
        let _ = super::ctxlog::sweep_stem(&dir, STEM, super::ctxlog::today_ymd());
    });
}

fn today_file() -> Option<PathBuf> {
    if let Some(path) = env_log_path() {
        return Some(path);
    }
    let dir = crate::settings::store::config_dir().join("Log");
    std::fs::create_dir_all(&dir).ok()?;
    let (y, m, d) = super::ctxlog::today_ymd();
    Some(dir.join(log_name(y, m, d)))
}

fn stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// Format `detail` only when logging is on.
pub(crate) fn trace(phase: &str, detail: impl FnOnce() -> String) {
    if enabled() {
        write(phase, &detail());
    }
}

pub(crate) fn write(phase: &str, detail: &str) {
    if !enabled() {
        return;
    }
    let line = format!("{} [open] {} {}", stamp(), phase, detail.trim());
    let _guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let dbg = format!("{line}\0");
    let wide: Vec<u16> = dbg.encode_utf16().collect();
    unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };
    let Some(path) = today_file() else { return };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_name_is_dated_and_distinct_from_the_menu_log() {
        assert_eq!(log_name(2026, 11, 10), "maguro-open-timing-2026-11-10.log");
        assert_ne!(
            log_name(2026, 11, 10),
            super::super::ctxlog::file_name(2026, 11, 10)
        );
    }
}
