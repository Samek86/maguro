//! Dated right-click timing logs, next to the settings file.
//!
//! `Log/maguro-ctx-timing-YYYY-MM-DD.log`. Files whose date is more than 14 days
//! before today are deleted once, when a window opens. Nothing else in that folder
//! is removed. The switch defaults off. `MAGURO_CTX_TIMING=1` still forces logging.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::System::SystemInformation::GetLocalTime;

pub(crate) const KEEP_DAYS: i64 = 14;
const STEM: &str = "maguro-ctx-timing";

static ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn file_name(y: i32, m: u32, d: u32) -> String {
    format!("{STEM}-{y:04}-{m:02}-{d:02}.log")
}

/// `maguro-ctx-timing-YYYY-MM-DD.log` only. Anything else is left alone.
pub(crate) fn parse_name(name: &str) -> Option<(i32, u32, u32)> {
    let rest = name.strip_prefix(STEM)?.strip_prefix('-')?;
    let rest = rest.strip_suffix(".log")?;
    let mut parts = rest.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !valid_ymd(y, m, d) {
        return None;
    }
    Some((y, m, d))
}

pub(crate) fn older_than_keep(file: (i32, u32, u32), today: (i32, u32, u32)) -> bool {
    let Some(a) = civil_days(file.0, file.1, file.2) else { return false };
    let Some(b) = civil_days(today.0, today.1, today.2) else { return false };
    b - a > KEEP_DAYS
}

/// Delete expired timing logs in `dir`. Missing directory is zero. Other files stay.
pub(crate) fn sweep(dir: &Path, today: (i32, u32, u32)) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for entry in rd.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let Some(day) = parse_name(&name) else { continue };
        if older_than_keep(day, today) && std::fs::remove_file(entry.path()).is_ok() {
            n += 1;
        }
    }
    n
}

pub(crate) fn sweep_startup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = crate::settings::store::config_dir().join("Log");
        let _ = sweep(&dir, today_ymd());
    });
}

pub(crate) fn today_file() -> Option<PathBuf> {
    let dir = crate::settings::store::config_dir().join("Log");
    std::fs::create_dir_all(&dir).ok()?;
    let (y, m, d) = today_ymd();
    Some(dir.join(file_name(y, m, d)))
}

fn today_ymd() -> (i32, u32, u32) {
    let t = unsafe { GetLocalTime() };
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32)
}

fn valid_ymd(y: i32, m: u32, d: u32) -> bool {
    (1970..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= month_len(y, m)
}

fn month_len(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

fn leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days since 1970-01-01. Invalid dates are `None`.
fn civil_days(y: i32, m: u32, d: u32) -> Option<i64> {
    if !valid_ymd(y, m, d) {
        return None;
    }
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    Some(era * 146097 + doe as i64 - 719468)
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProbeRec {
    #[serde(default)]
    pub(crate) verb: String,
    #[serde(default)]
    pub(crate) folded: String,
    #[serde(default)]
    pub(crate) label: String,
}

/// One JSON object. Keys are `id/targets`. Bad keys and over-long strings are dropped.
pub(crate) fn encode_probe(rows: &[(String, u8, Vec<ProbeRec>)]) -> String {
    let mut map = std::collections::BTreeMap::new();
    for (id, targets, items) in rows {
        if id.is_empty() || *targets == 0 {
            continue;
        }
        let kept: Vec<_> = items.iter().take(24).filter_map(clip_rec).collect();
        map.insert(format!("{id}/{targets}"), kept);
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn decode_probe(text: &str) -> Vec<(String, u8, Vec<ProbeRec>)> {
    let Ok(raw) = serde_json::from_str::<std::collections::BTreeMap<String, Vec<ProbeRec>>>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, items) in raw {
        let Some((id, targets)) = split_key(&key) else { continue };
        let kept = items.into_iter().take(24).filter_map(|row| clip_rec(&row)).collect();
        out.push((id, targets, kept));
    }
    out
}

fn clip_rec(row: &ProbeRec) -> Option<ProbeRec> {
    let verb = clip(&row.verb);
    let folded = clip(&row.folded);
    let label = clip(&row.label);
    if verb.is_empty() && folded.is_empty() && label.is_empty() {
        return None;
    }
    Some(ProbeRec { verb, folded, label })
}

fn clip(s: &str) -> String {
    s.chars().take(160).collect()
}

fn split_key(key: &str) -> Option<(String, u8)> {
    let (id, n) = key.rsplit_once('/')?;
    let targets: u8 = n.parse().ok()?;
    if id.is_empty() || targets == 0 {
        return None;
    }
    Some((id.to_string(), targets))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_reject_other_files() {
        assert_eq!(file_name(2026, 11, 9), "maguro-ctx-timing-2026-11-09.log");
        assert_eq!(parse_name("maguro-ctx-timing-2026-11-09.log"), Some((2026, 11, 9)));
        assert_eq!(parse_name("maguro-ctx-timing-2026-10-26.log"), Some((2026, 10, 26)));
        assert!(parse_name("maguro-ctx-timing.log").is_none());
        assert!(parse_name("maguro-ctx-timing-2026-11-09.log.bak").is_none());
        assert!(parse_name("maguro-ctx-timing-2026-13-01.log").is_none());
        assert!(parse_name("notes.txt").is_none());
    }

    #[test]
    fn fourteen_days_stays_and_the_next_day_goes() {
        let today = (2026, 11, 9);
        assert!(!older_than_keep((2026, 11, 9), today));
        assert!(!older_than_keep((2026, 10, 26), today));
        assert!(older_than_keep((2026, 10, 25), today));
        let span = civil_days(2026, 11, 9).unwrap() - civil_days(2026, 10, 26).unwrap();
        assert_eq!(span, 14);
    }

    #[test]
    fn sweep_removes_only_expired_timing_logs() {
        let dir = std::env::temp_dir().join(format!("maguro-ctxlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "maguro-ctx-timing-2026-10-01.log",
            "maguro-ctx-timing-2026-10-26.log",
            "maguro-ctx-timing-2026-11-09.log",
            "maguro-ctx-timing.log",
            "notes.txt",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        assert_eq!(sweep(&dir, (2026, 11, 9)), 1);
        assert!(!dir.join("maguro-ctx-timing-2026-10-01.log").exists());
        assert!(dir.join("maguro-ctx-timing-2026-10-26.log").exists());
        assert!(dir.join("maguro-ctx-timing-2026-11-09.log").exists());
        assert!(dir.join("maguro-ctx-timing.log").exists());
        assert!(dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_file_keeps_marks_and_drops_junk() {
        let rows = vec![(
            "clsid:abc".into(),
            1u8,
            vec![ProbeRec { verb: "compare".into(), folded: "compare".into(), label: "Compare".into() }],
        )];
        let text = encode_probe(&rows);
        let back = decode_probe(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, "clsid:abc");
        assert_eq!(back[0].1, 1);
        assert_eq!(back[0].2[0].label, "Compare");
        assert!(decode_probe("not json").is_empty());
        assert!(decode_probe(r#"{"":[],"bad":[]}"#).is_empty());
    }
}
