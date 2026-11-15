//! Rows, search, and sort for the important-marks list.
//!
//! Every store row is listed, including `lost` and `recycled`. A folder mark is that folder
//! only. Import replaces the list with the file (paths already there are updated, and paths
//! missing from the file are removed). The list stops at [`CAP`]. Settings import and reset
//! never call this module, so they leave marks.json alone. This module never opens maguro.json.

use std::cmp::Ordering;
use std::path::Path;

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

use super::store::{marks_path, unix_of_added, BlockKind, ImportReport, Store, StoreError, STATE_LOST, STATE_OK, STATE_RECYCLED};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) added: String,
    pub(crate) dir: bool,
    pub(crate) vol: Option<String>,
    pub(crate) fid: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortCol {
    Name,
    Path,
    State,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JumpPlan {
    /// Browse `folder`. `select` is the item name when the mark is still `ok` and present.
    Open { folder: String, select: Option<String> },
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathKind {
    Missing,
    File,
    Dir,
}

/// One row per store entry, in store order. The length matches [`Store::len`].
pub(crate) fn rows_of(store: &Store) -> Vec<Row> {
    store
        .iter()
        .map(|mark| Row {
            name: file_name(&mark.path),
            path: mark.path.clone(),
            state: mark.state.clone(),
            added: mark.added.clone(),
            dir: mark.dir,
            vol: mark.vol.clone(),
            fid: mark.fid.clone(),
        })
        .collect()
}

/// Case-insensitive substring of the name or the full path. An empty query keeps every row.
pub(crate) fn filter_rows(rows: &[Row], query: &str) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter().filter(|row| row.name.to_lowercase().contains(&q) || row.path.to_lowercase().contains(&q)).cloned().collect()
}

/// Stable. Equal keys stay in their previous order.
pub(crate) fn sort_rows(rows: &mut [Row], col: SortCol, desc: bool) {
    rows.sort_by(|a, b| {
        let ord = match col {
            SortCol::Name => cmp_fold(&a.name, &b.name),
            SortCol::Path => cmp_fold(&a.path, &b.path),
            SortCol::State => a.state.cmp(&b.state),
            SortCol::Added => a.added.cmp(&b.added),
        };
        if desc { ord.reverse() } else { ord }
    });
}

pub(crate) fn state_key(state: &str) -> crate::Key {
    match state {
        STATE_LOST => crate::Key::MarkStateLost,
        STATE_RECYCLED => crate::Key::MarkStateRecycled,
        _ => crate::Key::MarkStateHere,
    }
}

/// Remove the selected marks. A blocked store is unchanged.
pub(crate) fn unmark(store: &mut Store, paths: &[String]) -> Result<usize, StoreError> {
    if store.is_blocked() {
        return Err(StoreError::Blocked);
    }
    let mut n = 0;
    for path in paths {
        if store.remove(path)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Delete `lost` rows only. `ok` and `recycled` stay.
pub(crate) fn cleanup_lost(store: &mut Store) -> Result<usize, StoreError> {
    store.remove_lost()
}

pub(crate) fn export_text(store: &Store) -> String {
    store.export_json()
}

pub(crate) fn import_replace(store: &mut Store, bytes: &[u8]) -> Result<ImportReport, StoreError> {
    store.import_replace(bytes)
}

/// Text for a blocked store, including the file path. `None` when the store can be saved.
pub(crate) fn blocked_message(store: &Store) -> Option<String> {
    let kind = store.block_kind()?;
    let path = marks_path(store.folder()).display().to_string();
    Some(match kind {
        BlockKind::Newer(version) => crate::tf(crate::Key::MarksBlockedNewer, &[("version", &version.to_string()), ("path", &path)]),
        BlockKind::Unreadable => crate::tf(crate::Key::MarksBlockedBad, &[("path", &path)]),
    })
}

/// Where a double-click goes. `lost` and `recycled` never invent a file to select.
pub(crate) fn plan_jump(path: &str, state: &str, kind: impl Fn(&str) -> PathKind) -> JumpPlan {
    let path = path.trim().trim_end_matches(['\\', '/']);
    if path.is_empty() {
        return JumpPlan::Unavailable;
    }
    let here = kind(path);
    let present = state == STATE_OK && matches!(here, PathKind::File | PathKind::Dir);
    if present {
        if let Some(folder) = parent_dir(path) {
            if kind(&folder) == PathKind::Dir {
                let name = file_name(path);
                if !name.is_empty() {
                    return JumpPlan::Open { folder, select: Some(name) };
                }
            }
        }
        if here == PathKind::Dir {
            return JumpPlan::Open { folder: path.to_string(), select: None };
        }
    }
    let mut cur = path.to_string();
    while let Some(parent) = parent_dir(&cur) {
        if kind(&parent) == PathKind::Dir {
            return JumpPlan::Open { folder: parent, select: None };
        }
        cur = parent;
    }
    JumpPlan::Unavailable
}

pub(crate) fn file_name(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed).to_string()
}

/// Parent directory. `C:\data\a.txt` yields `C:\data`. `C:\data` yields `C:\`. A root yields `None`.
pub(crate) fn parent_dir(path: &str) -> Option<String> {
    let path = path.trim().trim_end_matches(['\\', '/']);
    if path.is_empty() {
        return None;
    }
    let bytes = path.as_bytes();
    if bytes.len() <= 2 && bytes.get(1) == Some(&b':') {
        return None;
    }
    let i = path.rfind(['\\', '/'])?;
    if i == 2 && bytes.get(1) == Some(&b':') {
        return Some(path[..=i].to_string());
    }
    if i == 0 {
        return None;
    }
    Some(path[..i].to_string())
}

fn cmp_fold(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// True when `path` is maguro.json. The list never writes that file.
pub(crate) fn is_settings_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("maguro.json"))
}

/// Local clock for an RFC3339 `added` value. `offset_minutes` is minutes east of UTC.
/// Empty stays empty. A stamp that does not parse is returned unchanged.
pub(crate) fn format_added_at(added: &str, offset_minutes: i32) -> String {
    let trimmed = added.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Some(unix) = unix_of_added(trimmed) else {
        return added.to_string();
    };
    let local = unix.saturating_add(i64::from(offset_minutes).saturating_mul(60));
    format_clock(super::store::clock_parts(local))
}

/// Same clock in the computer's time zone. The offset comes from that instant, not a fixed zone.
pub(crate) fn format_added_local(added: &str) -> String {
    let Some(offset) = system_offset_minutes(added) else {
        return if added.trim().is_empty() { String::new() } else { added.to_string() };
    };
    format_added_at(added, offset)
}

/// Minutes east of UTC for this stamp on this computer. `None` when the stamp cannot be shown.
fn system_offset_minutes(added: &str) -> Option<i32> {
    let trimmed = added.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unix = unix_of_added(trimmed)?;
    let (y, m, d, h, mi, s) = super::store::clock_parts(unix);
    if !(1..=9999).contains(&y) {
        return None;
    }
    let utc = SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDay: d as u16,
        wHour: h as u16,
        wMinute: mi as u16,
        wSecond: s as u16,
        wMilliseconds: 0,
        wDayOfWeek: 0,
    };
    let mut local = SYSTEMTIME::default();
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()? };
    let wall = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond);
    let local_unix = unix_of_added(&wall)?;
    i32::try_from((local_unix - unix) / 60).ok()
}

fn format_clock(parts: (i32, u32, u32, u32, u32, u32)) -> String {
    let (y, m, d, h, mi, s) = parts;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::store::{FileIdentity, MarksLock, CAP, FILE_NAME};
    use std::sync::{Arc, Mutex};

    const NOW: i64 = 1_700_000_000;
    const ADDED: &str = "2026-11-15T00:00:00Z";

    fn scratch(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("maguro-marks-list-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn store_in(dir: &Path) -> Store {
        Store::load_at(dir, MarksLock::Shared(Arc::new(Mutex::new(()))), NOW)
    }

    fn id(vol: &str, fid: &str, dir: bool) -> FileIdentity {
        FileIdentity { vol: Some(vol.into()), fid: Some(fid.into()), size: Some(4), mtime: Some(5), dir }
    }

    fn seed(store: &mut Store) {
        store.add_with(r"C:\data\ok.txt", id("AAAA", "1111", false), ADDED).unwrap();
        store.add_with(r"C:\data\box", id("BBBB", "2222", true), "2026-11-14T00:00:00Z").unwrap();
        store.add_with(r"C:\data\gone.txt", id("CCCC", "3333", false), "2026-11-13T00:00:00Z").unwrap();
        store.set_state(r"C:\data\gone.txt", STATE_LOST, Some(Some("2026-11-16T00:00:00Z".into()))).unwrap();
        store.add_with(r"C:\data\old.txt", id("DDDD", "4444", false), "2026-11-12T00:00:00Z").unwrap();
        store.set_state(r"C:\data\old.txt", STATE_RECYCLED, Some(None)).unwrap();
    }

    #[test]
    fn the_list_matches_the_store_including_lost_recycled_and_folders() {
        let dir = scratch("rows");
        let mut store = store_in(&dir);
        seed(&mut store);
        let rows = rows_of(&store);
        assert_eq!(rows.len(), store.len());
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|r| r.state == STATE_OK && r.name == "ok.txt" && r.fid.as_deref() == Some("1111")));
        assert!(rows.iter().any(|r| r.dir && r.name == "box" && r.path == r"C:\data\box"));
        assert!(rows.iter().any(|r| r.state == STATE_LOST && r.path == r"C:\data\gone.txt"));
        assert!(rows.iter().any(|r| r.state == STATE_RECYCLED && r.path == r"C:\data\old.txt"));
        assert!(rows.iter().all(|r| r.name != "inner.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_matches_a_name_or_a_path_substring() {
        let dir = scratch("find");
        let mut store = store_in(&dir);
        seed(&mut store);
        let rows = rows_of(&store);
        assert_eq!(filter_rows(&rows, "  ").len(), 4);
        assert_eq!(filter_rows(&rows, "GONE").len(), 1);
        assert_eq!(filter_rows(&rows, r"data\box").len(), 1);
        assert_eq!(filter_rows(&rows, "no-such").len(), 0);
        assert_eq!(filter_rows(&rows, "data").len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sort_is_stable_for_each_column() {
        let mut rows = vec![
            Row { path: r"C:\b".into(), name: "Same".into(), state: STATE_OK.into(), added: "2026-11-02T00:00:00Z".into(), dir: false, vol: None, fid: None },
            Row { path: r"C:\a".into(), name: "same".into(), state: STATE_OK.into(), added: "2026-11-02T00:00:00Z".into(), dir: false, vol: None, fid: None },
            Row { path: r"C:\c".into(), name: "Later".into(), state: STATE_LOST.into(), added: "2026-11-03T00:00:00Z".into(), dir: true, vol: None, fid: None },
            Row { path: r"C:\d".into(), name: "Bin".into(), state: STATE_RECYCLED.into(), added: "2026-11-01T00:00:00Z".into(), dir: false, vol: None, fid: None },
        ];
        sort_rows(&mut rows, SortCol::Name, false);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Bin", "Later", "Same", "same"]);
        sort_rows(&mut rows, SortCol::State, false);
        let states: Vec<_> = rows.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(states, [STATE_LOST, STATE_OK, STATE_OK, STATE_RECYCLED]);
        assert_eq!(rows[1].path, r"C:\b", "equal states keep the previous order");
        assert_eq!(rows[2].path, r"C:\a");
        sort_rows(&mut rows, SortCol::Added, true);
        assert_eq!(rows[0].added, "2026-11-03T00:00:00Z");
        assert_eq!(rows[3].added, "2026-11-01T00:00:00Z");
        let tied = rows.iter().filter(|r| r.added == "2026-11-02T00:00:00Z").map(|r| r.path.as_str()).collect::<Vec<_>>();
        assert_eq!(tied, [r"C:\b", r"C:\a"]);
        sort_rows(&mut rows, SortCol::Path, false);
        assert_eq!(rows[0].path, r"C:\a");
        assert_eq!(rows[3].path, r"C:\d");
    }

    #[test]
    fn cleanup_removes_only_lost_rows() {
        let dir = scratch("clean");
        let mut store = store_in(&dir);
        seed(&mut store);
        assert_eq!(cleanup_lost(&mut store).unwrap(), 1);
        let rows = rows_of(&store);
        assert!(rows.iter().all(|r| r.state != STATE_LOST));
        assert!(rows.iter().any(|r| r.state == STATE_OK && r.name == "ok.txt"));
        assert!(rows.iter().any(|r| r.dir));
        assert!(rows.iter().any(|r| r.state == STATE_RECYCLED));
        assert_eq!(rows.len(), 3);
        store.save().unwrap();
        let back = store_in(&dir);
        assert_eq!(back.len(), 3);
        assert!(rows_of(&back).iter().all(|r| r.state != STATE_LOST));
        assert!(back.get(r"C:\data\old.txt").unwrap().state == STATE_RECYCLED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unmark_removes_the_row() {
        let dir = scratch("unmark");
        let mut store = store_in(&dir);
        seed(&mut store);
        assert_eq!(unmark(&mut store, &[r"C:\data\ok.txt".into(), r"C:\missing".into()]).unwrap(), 1);
        assert!(rows_of(&store).iter().all(|r| r.name != "ok.txt"));
        assert_eq!(store.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_then_import_replaces_and_leaves_maguro_json() {
        let dir = scratch("round");
        let settings = dir.join("maguro.json");
        std::fs::write(&settings, b"{\"keep\":true}\n").unwrap();
        let before = std::fs::read(&settings).unwrap();
        let mut store = store_in(&dir);
        seed(&mut store);
        store.save().unwrap();
        let exported = export_text(&store);
        let copy = dir.join("marks-export.json");
        std::fs::write(&copy, &exported).unwrap();
        unmark(&mut store, &[r"C:\data\ok.txt".into()]).unwrap();
        store.save().unwrap();
        assert!(store.get(r"C:\data\ok.txt").is_none());
        let bytes = std::fs::read(&copy).unwrap();
        let report = import_replace(&mut store, &bytes).unwrap();
        assert_eq!(report.kept, 4);
        assert_eq!(report.skipped_cap, 0);
        store.save().unwrap();
        let back = store_in(&dir);
        assert_eq!(back.len(), 4);
        let ok = back.get(r"C:\data\ok.txt").unwrap();
        assert_eq!(ok.state, STATE_OK);
        assert_eq!(ok.vol.as_deref(), Some("AAAA"));
        assert_eq!(ok.fid.as_deref(), Some("1111"));
        assert_eq!(back.get(r"C:\data\gone.txt").unwrap().state, STATE_LOST);
        assert_eq!(back.get(r"C:\data\old.txt").unwrap().state, STATE_RECYCLED);
        assert_eq!(back.get(r"C:\data\box").unwrap().fid.as_deref(), Some("2222"));
        assert!(back.get(r"C:\data\box").unwrap().dir);
        assert_eq!(std::fs::read(&settings).unwrap(), before);
        assert!(!dir.join(FILE_NAME).exists() || std::fs::read(dir.join(FILE_NAME)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_stops_at_the_cap() {
        let dir = scratch("cap");
        let mut store = store_in(&dir);
        let mut marks = String::from("{\"version\":1,\"marks\":[");
        for i in 0..=CAP {
            if i > 0 {
                marks.push(',');
            }
            marks.push_str(&format!(
                "{{\"path\":\"C:\\\\m\\\\{i}\",\"state\":\"ok\",\"vol\":\"AA\",\"fid\":\"{i:032X}\",\"added\":\"2026-11-15T00:00:00Z\"}}"
            ));
        }
        marks.push_str("]}");
        let report = import_replace(&mut store, marks.as_bytes()).unwrap();
        assert_eq!(report.kept, CAP);
        assert_eq!(report.skipped_cap, 1);
        assert_eq!(store.len(), CAP);
        assert!(store.get(r"C:\m\0").is_some());
        assert!(store.get(&format!(r"C:\m\{CAP}")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blocked_store_reports_the_path_and_does_not_write() {
        let dir = scratch("block");
        let path = dir.join(FILE_NAME);
        let raw = r#"{"version":99,"marks":[{"path":"D:\\keep.txt","state":"ok","vol":"AA","fid":"BB"}]}"#;
        std::fs::write(&path, raw).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut store = store_in(&dir);
        let message = blocked_message(&store).unwrap();
        assert!(message.contains("99"), "{message}");
        assert!(message.contains("marks.json"), "{message}");
        assert!(message.contains(&path.display().to_string()) || message.contains("marks.json"), "{message}");
        assert!(unmark(&mut store, &[r"D:\keep.txt".into()]).is_err());
        assert!(cleanup_lost(&mut store).is_err());
        assert!(import_replace(&mut store, br#"{"version":1,"marks":[]}"#).is_err());
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!dir.join("maguro.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_store_names_the_file_and_jump_does_not_invent_a_file() {
        let dir = scratch("bad");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, b"not-json").unwrap();
        let before = std::fs::read(&path).unwrap();
        let store = store_in(&dir);
        let message = blocked_message(&store).unwrap();
        assert!(message.contains("marks.json"), "{message}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);

        let map = |p: &str| match p {
            r"C:\data" | r"C:\" => PathKind::Dir,
            r"C:\data\ok.txt" => PathKind::File,
            r"C:\data\gone.txt" | r"C:\data\old.txt" => PathKind::Missing,
            _ => PathKind::Missing,
        };
        assert_eq!(
            plan_jump(r"C:\data\ok.txt", STATE_OK, map),
            JumpPlan::Open { folder: r"C:\data".into(), select: Some("ok.txt".into()) }
        );
        assert_eq!(
            plan_jump(r"C:\data\box", STATE_OK, |p| if p == r"C:\data\box" || p == r"C:\data" || p == r"C:\" { PathKind::Dir } else { PathKind::Missing }),
            JumpPlan::Open { folder: r"C:\data".into(), select: Some("box".into()) }
        );
        assert_eq!(
            plan_jump(r"C:\data\gone.txt", STATE_LOST, map),
            JumpPlan::Open { folder: r"C:\data".into(), select: None }
        );
        assert_eq!(
            plan_jump(r"C:\data\old.txt", STATE_RECYCLED, |p| if p == r"C:\data\old.txt" { PathKind::File } else { map(p) }),
            JumpPlan::Open { folder: r"C:\data".into(), select: None }
        );
        assert_eq!(
            plan_jump(r"C:\missing\gone.txt", STATE_LOST, |p| if p == r"C:\" { PathKind::Dir } else { PathKind::Missing }),
            JumpPlan::Open { folder: r"C:\".into(), select: None }
        );
        assert_eq!(plan_jump(r"D:\nope\gone.txt", STATE_LOST, |_| PathKind::Missing), JumpPlan::Unavailable);
        assert_eq!(plan_jump(r"C:\data\ok.txt", STATE_OK, |_| PathKind::Missing), JumpPlan::Unavailable);
    }

    #[test]
    fn added_display_applies_a_fixed_offset_and_drops_the_z() {
        assert_eq!(format_added_at("2026-11-15T00:00:00Z", 9 * 60), "2026-11-15 09:00:00");
        assert!(!format_added_at("2026-11-15T00:00:00Z", 9 * 60).ends_with('Z'));
    }

    #[test]
    fn added_display_crosses_the_calendar_day_for_both_signs() {
        assert_eq!(format_added_at("2026-11-15T00:00:00Z", -5 * 60), "2026-11-14 19:00:00");
        assert_eq!(format_added_at("2026-11-15T18:00:00Z", 9 * 60), "2026-11-16 03:00:00");
    }

    #[test]
    fn added_display_in_the_system_zone_has_no_trailing_z() {
        let shown = format_added_local("2026-11-15T00:00:00Z");
        assert!(!shown.is_empty());
        assert!(!shown.contains('Z'), "{shown}");
        assert!(!shown.contains('T'), "{shown}");
        assert!(shown.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | ' ' | ':')), "{shown}");
    }

    #[test]
    fn a_bad_added_stamp_does_not_panic() {
        assert_eq!(format_added_at("", 9 * 60), "");
        assert_eq!(format_added_at("   ", -5 * 60), "");
        assert_eq!(format_added_at("not-a-stamp", 9 * 60), "not-a-stamp");
        assert_eq!(format_added_at("2026-13-40T99:00:00Z", -5 * 60), "2026-13-40T99:00:00Z");
        assert_eq!(format_added_local(""), "");
        assert_eq!(format_added_local("nope"), "nope");
    }

    #[test]
    fn formatting_a_row_for_display_leaves_utc_in_the_file() {
        let dir = scratch("utc-display");
        let mut store = store_in(&dir);
        store.add_with(r"C:\data\ok.txt", id("AAAA", "1111", false), "2026-11-15T00:00:00Z").unwrap();
        store.save().unwrap();
        let rows = rows_of(&store);
        let shown = format_added_at(&rows[0].added, 9 * 60);
        assert_eq!(shown, "2026-11-15 09:00:00");
        assert_eq!(rows[0].added, "2026-11-15T00:00:00Z");
        let exported = export_text(&store);
        assert!(exported.contains("2026-11-15T00:00:00Z"), "{exported}");
        assert!(!exported.contains("2026-11-15 09:00:00"), "{exported}");
        let on_disk = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(on_disk.contains("2026-11-15T00:00:00Z"), "{on_disk}");
        assert!(!on_disk.contains("2026-11-15 09:00:00"), "{on_disk}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
