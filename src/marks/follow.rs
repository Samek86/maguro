//! Keep a mark on the same file when its path changes.
//!
//! Path and id decisions live here as plain functions so tests never touch a real
//! `marks.json`. The shell sink and the notify handler only translate events into those calls.
//! File-id lookup uses `OpenFileById` on a directory handle (`C:\`), never a raw volume.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::store::{
    read_file_identity, stamp_unix, FileIdentity, Store, StoreError, STATE_LOST, STATE_OK, STATE_RECYCLED,
};

/// How long a shell delete stays a candidate before it becomes `lost`.
pub(crate) const FOLLOW_WINDOW_MS: u64 = 2_000;
/// One background pass, about ten seconds after startup, looks up at most this many rows.
pub(crate) const SWEEP_LIMIT: usize = 500;
const SWEEP_DELAY_SECS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnKind {
    Rename,
    Move,
    Delete,
    Copy,
}

/// One finished `IFileOperation` item. `succeeded` is false on failure and on cancel.
pub(crate) struct OwnOp {
    pub(crate) kind: OwnKind,
    pub(crate) succeeded: bool,
    pub(crate) src: String,
    pub(crate) dest: String,
    pub(crate) recycled: bool,
    pub(crate) new_id: Option<FileIdentity>,
    pub(crate) child_ids: Vec<(String, FileIdentity)>,
}

impl OwnOp {
    fn new(kind: OwnKind, src: &str, dest: &str) -> Self {
        Self {
            kind,
            succeeded: true,
            src: src.to_string(),
            dest: dest.to_string(),
            recycled: false,
            new_id: None,
            child_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IdHit {
    /// `OpenFileById` returned this path.
    Found(String),
    /// The volume is here and the id did not open.
    Gone,
    /// The volume or share is not reachable. Not the same as deleted.
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LostCand {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) vol: Option<String>,
    pub(crate) fid: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) mtime: Option<u64>,
    pub(crate) at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Arrival {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) vol: Option<String>,
    pub(crate) fid: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) mtime: Option<u64>,
    pub(crate) at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VolumeRoot {
    Letter(char),
    /// No mounted letter has this serial, or more than one does and the old letter is gone.
    Unplugged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SweepClass {
    Lost,
    Recycled,
    Missing,
}

pub(crate) struct SweepItem {
    pub(crate) index: usize,
    pub(crate) class: SweepClass,
    pub(crate) has_id: bool,
    pub(crate) unreachable: bool,
}

fn slashes(path: &str) -> String {
    path.trim().replace('/', "\\")
}

fn drive_root_text(path: &str) -> bool {
    let t = slashes(path);
    let t = t.trim_end_matches('\\');
    let b = t.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn norm_cmp(path: &str) -> String {
    let s = slashes(path);
    if drive_root_text(&s) {
        return format!("{}\\", s.trim_end_matches('\\')).to_ascii_lowercase();
    }
    s.trim_end_matches('\\').to_ascii_lowercase()
}

pub(crate) fn paths_equal(a: &str, b: &str) -> bool {
    norm_cmp(a) == norm_cmp(b)
}

fn base_name(path: &str) -> String {
    let s = slashes(path);
    let s = s.trim_end_matches('\\');
    s.rsplit('\\').next().unwrap_or("").to_ascii_lowercase()
}

fn drive_root(path: &str) -> String {
    let s = slashes(path);
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return format!("{}:\\", (b[0] as char).to_ascii_uppercase());
    }
    String::new()
}

pub(crate) fn is_unc(path: &str) -> bool {
    let s = slashes(path);
    let lower = s.to_ascii_lowercase();
    lower.starts_with("\\\\?\\unc\\") || (s.starts_with("\\\\") && !s.starts_with("\\\\?\\"))
}

pub(crate) fn in_recycle_bin(path: &str) -> bool {
    let s = slashes(path).to_ascii_lowercase();
    s.contains("\\$recycle.bin\\") || s.ends_with("\\$recycle.bin")
}

/// Replace `from` with `to` on `path` and on every path under it. A longer sibling name is not a child.
pub(crate) fn rewrite_prefix(path: &str, from: &str, to: &str) -> Option<String> {
    let path_s = slashes(path);
    let from_s = if drive_root_text(from) {
        format!("{}\\", slashes(from).trim_end_matches('\\'))
    } else {
        slashes(from).trim_end_matches('\\').to_string()
    };
    let to_s = if drive_root_text(to) {
        format!("{}\\", slashes(to).trim_end_matches('\\'))
    } else {
        slashes(to).trim_end_matches('\\').to_string()
    };
    let path_l = path_s.to_ascii_lowercase();
    let from_l = from_s.to_ascii_lowercase();
    if path_l == from_l.trim_end_matches('\\') || path_l == from_l {
        return Some(to_s);
    }
    let boundary = if from_l.ends_with('\\') { from_l } else { format!("{from_l}\\") };
    if !path_l.starts_with(&boundary) || boundary.len() > path_s.len() {
        return None;
    }
    let rest = &path_s[boundary.len()..];
    let head = to_s.trim_end_matches('\\');
    Some(format!("{head}\\{rest}"))
}

/// Apply one Maguro file operation. Failure, cancel, and copy leave the store as it was.
pub(crate) fn apply_own(store: &mut Store, op: &OwnOp) -> Result<bool, StoreError> {
    if !op.succeeded {
        return Ok(false);
    }
    match op.kind {
        OwnKind::Copy => Ok(false),
        OwnKind::Rename => {
            let moved = store.retarget_tree(&op.src, &op.dest, None)?;
            Ok(!moved.is_empty())
        }
        OwnKind::Move => {
            let recycle = in_recycle_bin(&op.dest);
            let id = if recycle { None } else { op.new_id.as_ref() };
            let moved = store.retarget_tree(&op.src, &op.dest, id)?;
            if moved.is_empty() {
                return Ok(false);
            }
            let state = if recycle { STATE_RECYCLED } else { STATE_OK };
            for path in &moved {
                store.set_state(path, state, Some(None))?;
            }
            if !recycle {
                for (path, id) in &op.child_ids {
                    store.patch_identity(path, id)?;
                }
            }
            Ok(true)
        }
        OwnKind::Delete if op.recycled => apply_recycle(store, &op.src),
        OwnKind::Delete => store.forget_tree(&op.src).map(|n| n > 0),
    }
}

pub(crate) fn apply_recycle(store: &mut Store, path: &str) -> Result<bool, StoreError> {
    let rows = store.affected_paths(path);
    if rows.is_empty() {
        return Ok(false);
    }
    let mut any = false;
    for row in rows {
        any |= store.set_state(&row, STATE_RECYCLED, Some(None))?;
    }
    Ok(any)
}

/// `lost_since` is kept once a row is already `lost`, so the 180-day clock does not restart.
pub(crate) fn apply_lost(store: &mut Store, path: &str, stamp: &str) -> Result<bool, StoreError> {
    if is_unc(path) {
        return Ok(false);
    }
    let rows = store.affected_paths(path);
    if rows.is_empty() {
        return Ok(false);
    }
    let mut any = false;
    for row in &rows {
        let stamp = match store.get(row) {
            Some(mark) if mark.state == STATE_LOST => mark.lost_since.clone().unwrap_or_else(|| stamp.to_string()),
            _ => stamp.to_string(),
        };
        any |= store.set_state(row, STATE_LOST, Some(Some(stamp)))?;
    }
    Ok(any)
}

/// A file-id probe. A UNC path is never marked `lost`: a disconnect is not a delete.
pub(crate) fn apply_id_hit(store: &mut Store, path: &str, hit: IdHit, stamp: &str) -> Result<bool, StoreError> {
    if is_unc(path) && !matches!(hit, IdHit::Found(_)) {
        return Ok(false);
    }
    match hit {
        IdHit::Unreachable => Ok(false),
        IdHit::Gone => apply_lost(store, path, stamp),
        IdHit::Found(new_path) => {
            if paths_equal(&new_path, path) {
                if in_recycle_bin(&new_path) {
                    return apply_recycle(store, path);
                }
                return Ok(false);
            }
            let recycle = in_recycle_bin(&new_path);
            let moved = store.retarget_tree(path, &new_path, None)?;
            if moved.is_empty() {
                return Ok(false);
            }
            let state = if recycle { STATE_RECYCLED } else { STATE_OK };
            for row in &moved {
                store.set_state(row, state, Some(None))?;
            }
            Ok(true)
        }
    }
}

/// The path is still the marked path, but the file id is a different file.
/// The new file is not marked. Volumes without an id keep the mark.
pub(crate) fn note_identity(store: &mut Store, path: &str, seen: &FileIdentity, stamp: &str) -> Result<bool, StoreError> {
    let Some(mark) = store.get(path) else { return Ok(false) };
    let state = mark.state.clone();
    let had = mark.fid.clone();
    match (had, seen.fid.clone()) {
        // Same id at this path: a restore, or the file is still the one we marked.
        (Some(had), Some(got)) if had.eq_ignore_ascii_case(&got) => {
            let mut changed = false;
            if state != STATE_OK {
                changed |= store.set_state(path, STATE_OK, Some(None))?;
            }
            changed |= store.patch_identity(path, seen)?;
            Ok(changed)
        }
        // A new file reused the path. Only an `ok` row becomes `lost`; a recycled id stays put.
        (Some(_), Some(_)) if state == STATE_OK => store.set_state(path, STATE_LOST, Some(Some(stamp.to_string()))),
        _ => Ok(false),
    }
}

/// The letter to open for `stored_vol`. The path's own letter wins when it still has that serial.
pub(crate) fn pick_volume_root(stored_vol: &str, stored_path: &str, mounted: &[(char, &str)]) -> VolumeRoot {
    let want = stored_vol.trim().to_ascii_uppercase();
    if want.is_empty() {
        return VolumeRoot::Unplugged;
    }
    let current = drive_root(stored_path).chars().next();
    if let Some(letter) = current {
        if mounted.iter().any(|(got, vol)| *got == letter && vol.eq_ignore_ascii_case(&want)) {
            return VolumeRoot::Letter(letter);
        }
    }
    let hits: Vec<char> = mounted.iter().filter(|(_, vol)| vol.eq_ignore_ascii_case(&want)).map(|(letter, _)| *letter).collect();
    match hits.as_slice() {
        [one] => VolumeRoot::Letter(*one),
        _ => VolumeRoot::Unplugged,
    }
}

pub(crate) fn sweep_plan(items: &[SweepItem], limit: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for class in [SweepClass::Lost, SweepClass::Recycled, SweepClass::Missing] {
        for item in items.iter().filter(|item| item.class == class && item.has_id && !item.unreachable) {
            if out.len() >= limit {
                return out;
            }
            out.push(item.index);
        }
    }
    out
}

fn same_token(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

fn same_meas(a: Option<u64>, b: Option<u64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn in_window(parked: u64, at: u64) -> bool {
    at >= parked && at - parked <= FOLLOW_WINDOW_MS
}

/// Follow a create only for one candidate. Two name/size/mtime hits is a guess, so neither wins.
/// The same file id is not a guess. A create at the parked path is a replacement, not a move.
pub(crate) fn match_arrival(pending: &[LostCand], arrival: &Arrival) -> Option<usize> {
    let mut fid_hits = Vec::new();
    let mut name_hits = Vec::new();
    for (index, cand) in pending.iter().enumerate() {
        if !in_window(cand.at_ms, arrival.at_ms) || paths_equal(&cand.path, &arrival.path) {
            continue;
        }
        if same_token(&cand.fid, &arrival.fid) && (cand.vol.is_none() || arrival.vol.is_none() || same_token(&cand.vol, &arrival.vol)) {
            fid_hits.push(index);
            continue;
        }
        if cand.name.eq_ignore_ascii_case(&arrival.name) && same_meas(cand.size, arrival.size) && same_meas(cand.mtime, arrival.mtime) {
            name_hits.push(index);
        }
    }
    match fid_hits.as_slice() {
        [one] => Some(*one),
        [] => match name_hits.as_slice() {
            [one] => Some(*one),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn apply_follow(store: &mut Store, cand: &LostCand, arrival: &Arrival) -> Result<bool, StoreError> {
    let recycle = in_recycle_bin(&arrival.path);
    let same_fid = same_token(&cand.fid, &arrival.fid);
    let id = if recycle || same_fid {
        None
    } else {
        Some(FileIdentity {
            vol: arrival.vol.clone(),
            fid: arrival.fid.clone(),
            size: arrival.size,
            mtime: arrival.mtime,
            dir: cand.dir,
        })
    };
    let moved = store.retarget_tree(&cand.path, &arrival.path, id.as_ref())?;
    if moved.is_empty() {
        return Ok(false);
    }
    let state = if recycle { STATE_RECYCLED } else { STATE_OK };
    for path in &moved {
        store.set_state(path, state, Some(None))?;
    }
    Ok(true)
}

fn now_stamp() -> String {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    stamp_unix(unix)
}

fn mono_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn pending() -> &'static Mutex<Vec<LostCand>> {
    static PENDING: Mutex<Vec<LostCand>> = Mutex::new(Vec::new());
    &PENDING
}

fn lock_pending() -> std::sync::MutexGuard<'static, Vec<LostCand>> {
    pending().lock().unwrap_or_else(|e| e.into_inner())
}

fn covers(cand: &LostCand, path: &str) -> bool {
    if paths_equal(&cand.path, path) {
        return true;
    }
    if !cand.dir {
        return false;
    }
    let root = norm_cmp(&cand.path);
    let child = norm_cmp(path);
    child.starts_with(&format!("{root}\\"))
}

fn is_parked(path: &str) -> bool {
    lock_pending().iter().any(|cand| covers(cand, path))
}

fn clear_pending_under(path: &str) {
    let root = norm_cmp(path);
    let prefix = format!("{root}\\");
    let mut rows = lock_pending();
    rows.retain(|cand| {
        let got = norm_cmp(&cand.path);
        got != root && !got.starts_with(&prefix)
    });
}

fn park(path: &str, dir: bool, vol: Option<String>, fid: Option<String>, size: Option<u64>, mtime: Option<u64>) {
    let mut rows = lock_pending();
    rows.retain(|cand| !paths_equal(&cand.path, path));
    if rows.len() >= 256 {
        rows.remove(0);
    }
    rows.push(LostCand {
        name: base_name(path),
        path: path.to_string(),
        dir,
        vol,
        fid,
        size,
        mtime,
        at_ms: mono_ms(),
    });
}

fn take_due(now: u64) -> Vec<LostCand> {
    let mut rows = lock_pending();
    let mut keep = Vec::new();
    let mut due = Vec::new();
    for cand in rows.drain(..) {
        if now.saturating_sub(cand.at_ms) > FOLLOW_WINDOW_MS {
            due.push(cand);
        } else {
            keep.push(cand);
        }
    }
    *rows = keep;
    due
}

/// Milliseconds until the oldest parked delete must become `lost`. `None` when nothing is parked.
pub(crate) fn wait_ms() -> Option<u64> {
    let now = mono_ms();
    lock_pending().iter().map(|cand| FOLLOW_WINDOW_MS.saturating_sub(now.saturating_sub(cand.at_ms)).max(1)).min()
}

pub(crate) fn expire_due() {
    let due = take_due(mono_ms());
    if due.is_empty() {
        return;
    }
    let stamp = now_stamp();
    super::commit_edit(|store| {
        let mut any = false;
        for cand in &due {
            any |= apply_lost(store, &cand.path, &stamp).unwrap_or(false);
        }
        any
    });
}

struct RowSnap {
    path: String,
    dir: bool,
    vol: Option<String>,
    fid: Option<String>,
    size: Option<u64>,
    mtime: Option<u64>,
}

fn snap_affected(path: &str) -> Vec<RowSnap> {
    super::with_store(|store| {
        store
            .affected_paths(path)
            .into_iter()
            .map(|row| {
                let mark = store.get(&row);
                RowSnap {
                    path: row,
                    dir: mark.map(|m| m.dir).unwrap_or(false),
                    vol: mark.and_then(|m| m.vol.clone()),
                    fid: mark.and_then(|m| m.fid.clone()),
                    size: mark.and_then(|m| m.size),
                    mtime: mark.and_then(|m| m.mtime),
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Shell notification already decoded to paths. `new_path` is set for a rename.
pub(crate) fn on_shell_event(event: u32, old_path: &str, new_path: &str) {
    use windows::Win32::UI::Shell::{SHCNE_CREATE, SHCNE_DELETE, SHCNE_MKDIR, SHCNE_RENAMEFOLDER, SHCNE_RENAMEITEM, SHCNE_RMDIR};
    if event == SHCNE_RENAMEITEM.0 || event == SHCNE_RENAMEFOLDER.0 {
        on_renamed(old_path, new_path);
    } else if event == SHCNE_DELETE.0 || event == SHCNE_RMDIR.0 {
        on_deleted(old_path, event == SHCNE_RMDIR.0);
    } else if event == SHCNE_CREATE.0 || event == SHCNE_MKDIR.0 {
        on_created(old_path);
    }
}

fn on_renamed(from: &str, to: &str) {
    if from.is_empty() || to.is_empty() || is_unc(from) && is_unc(to) && paths_equal(from, to) {
        return;
    }
    let recycle = in_recycle_bin(to);
    super::commit_edit(|store| {
        let moved = store.retarget_tree(from, to, None).unwrap_or_default();
        if moved.is_empty() {
            return false;
        }
        let state = if recycle { STATE_RECYCLED } else { STATE_OK };
        for path in &moved {
            let _ = store.set_state(path, state, Some(None));
        }
        true
    });
    clear_pending_under(from);
}

fn on_deleted(path: &str, rmdir: bool) {
    if path.is_empty() || is_unc(path) {
        return;
    }
    let rows = snap_affected(path);
    if rows.is_empty() {
        return;
    }
    let tree = rmdir || rows.iter().any(|row| row.dir) || rows.len() > 1;
    if let Some(root) = rows.iter().find(|row| paths_equal(&row.path, path)) {
        if let Some(fid) = root.fid.as_deref() {
            match lookup_fid(&drive_root(path), fid) {
                IdHit::Found(new_path) if !paths_equal(&new_path, path) => {
                    let _ = apply_found_commit(path, &new_path);
                    clear_pending_under(path);
                    return;
                }
                IdHit::Found(_) => return,
                IdHit::Unreachable => return,
                IdHit::Gone => {}
            }
        }
    } else if tree {
        let mut followed = false;
        for row in rows.iter().take(32) {
            let Some(fid) = row.fid.as_deref() else { continue };
            if let IdHit::Found(new_path) = lookup_fid(&drive_root(&row.path), fid) {
                if !paths_equal(&new_path, &row.path) {
                    let _ = apply_found_commit(&row.path, &new_path);
                    followed = true;
                }
            }
        }
        if followed {
            return;
        }
    }
    let (vol, fid, size, mtime) = rows
        .iter()
        .find(|row| paths_equal(&row.path, path))
        .map(|row| (row.vol.clone(), row.fid.clone(), row.size, row.mtime))
        .unwrap_or((None, None, None, None));
    park(path, tree, vol, fid, size, mtime);
}

fn apply_found_commit(path: &str, new_path: &str) -> bool {
    let hit = IdHit::Found(new_path.to_string());
    let stamp = now_stamp();
    let mut changed = false;
    super::commit_edit(|store| {
        changed = apply_id_hit(store, path, hit.clone(), &stamp).unwrap_or(false);
        changed
    });
    changed
}

fn on_created(path: &str) {
    if path.is_empty() {
        return;
    }
    let id = read_file_identity(Path::new(path));
    let stamp = now_stamp();
    super::commit_edit(|store| note_identity(store, path, &id, &stamp).unwrap_or(false));
    let arrival = Arrival {
        path: path.to_string(),
        name: base_name(path),
        vol: id.vol.clone(),
        fid: id.fid.clone(),
        size: id.size,
        mtime: id.mtime,
        at_ms: mono_ms(),
    };
    let pending = lock_pending().clone();
    let Some(index) = match_arrival(&pending, &arrival) else { return };
    let cand = pending[index].clone();
    super::commit_edit(|store| {
        let ok = apply_follow(store, &cand, &arrival).unwrap_or(false);
        if ok && cand.dir && !same_token(&cand.fid, &arrival.fid) && !in_recycle_bin(path) {
            refresh_child_ids(store, path);
        }
        ok
    });
    clear_pending_under(&cand.path);
}

fn refresh_child_ids(store: &mut Store, root: &str) {
    let kids = store.affected_paths(root);
    for kid in kids {
        if paths_equal(&kid, root) {
            continue;
        }
        let id = read_file_identity(Path::new(&kid));
        if id.fid.is_some() {
            let _ = store.patch_identity(&kid, &id);
        }
    }
}

/// Names are the folder's children, already case-folded. Missing marked names are opened by file id.
pub(crate) fn on_listed(folder: &str, present_lower: &[String]) {
    if folder.is_empty() || is_unc(folder) {
        return;
    }
    let rows = super::with_store(|store| {
        store
            .in_folder(folder)
            .into_iter()
            .map(|mark| RowSnap {
                path: mark.path.clone(),
                dir: mark.dir,
                vol: mark.vol.clone(),
                fid: mark.fid.clone(),
                size: mark.size,
                mtime: mark.mtime,
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    if rows.is_empty() {
        return;
    }
    let stamp = now_stamp();
    let mut lookups = Vec::new();
    let mut seen = Vec::new();
    for row in rows.into_iter().take(64) {
        let name = base_name(&row.path);
        if !present_lower.iter().any(|got| got == &name) {
            if row.fid.is_none() || is_parked(&row.path) {
                continue;
            }
            let Some(fid) = row.fid.clone() else { continue };
            lookups.push((row.path, lookup_fid(&drive_root(folder), &fid)));
        } else if row.fid.is_some() {
            let id = read_file_identity(Path::new(&row.path));
            seen.push((row.path, id));
        }
    }
    if lookups.is_empty() && seen.is_empty() {
        return;
    }
    super::commit_edit(|store| {
        let mut any = false;
        for (path, hit) in &lookups {
            any |= apply_id_hit(store, path, hit.clone(), &stamp).unwrap_or(false);
        }
        for (path, id) in &seen {
            any |= note_identity(store, path, id, &stamp).unwrap_or(false);
        }
        any
    });
}

fn skip_listed_folder(folder: &str) -> bool {
    if folder.is_empty() || is_unc(folder) {
        return true;
    }
    // Drive-button cache: 1 is no root, 4 is a remote share. `GetDriveTypeW` can block, so it is not called here.
    let Some(letter) = drive_root(folder).chars().next() else { return true };
    let letter = letter as u8;
    crate::app::app_try(|app| app.drives.iter().any(|drive| drive.0 == letter && (drive.1 == 1 || drive.1 == 4))).unwrap_or(false)
}

pub(crate) fn on_folder_shown(folder: &str) {
    if skip_listed_folder(folder) {
        return;
    }
    let opened = crate::winpath::win_long_path(folder);
    let Ok(rd) = std::fs::read_dir(opened) else { return };
    let mut names = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.is_empty() && name != "." && name != ".." {
            names.push(name);
        }
    }
    on_listed(folder, &names);
}

pub(crate) fn on_own_rename(from: &str, to: &str) {
    if from.is_empty() || to.is_empty() {
        return;
    }
    super::commit_edit(|store| apply_own(store, &OwnOp::new(OwnKind::Rename, from, to)).unwrap_or(false));
    clear_pending_under(from);
}

pub(crate) fn on_own_move(from: &str, to: &str) {
    if from.is_empty() || to.is_empty() {
        return;
    }
    let fresh = read_file_identity(Path::new(to));
    super::commit_edit(|store| {
        let old_vol = store.get(from).and_then(|mark| mark.vol.clone());
        let cross = old_vol.as_ref().zip(fresh.vol.as_ref()).is_some_and(|(had, got)| !had.eq_ignore_ascii_case(got));
        let mut op = OwnOp::new(OwnKind::Move, from, to);
        if cross {
            op.new_id = Some(fresh);
        }
        let changed = apply_own(store, &op).unwrap_or(false);
        if changed && cross && !in_recycle_bin(to) {
            refresh_child_ids(store, to);
        }
        changed
    });
    clear_pending_under(from);
}

/// Copy does not add a mark. The call exists so a successful copy is an explicit no-op.
pub(crate) fn on_own_copy(src: &str, dest: &str) {
    super::commit_edit(|store| apply_own(store, &OwnOp::new(OwnKind::Copy, src, dest)).unwrap_or(false));
}

pub(crate) fn on_own_delete(path: &str, recycled: bool) {
    if path.is_empty() {
        return;
    }
    super::commit_edit(|store| {
        let mut op = OwnOp::new(OwnKind::Delete, path, "");
        op.recycled = recycled;
        apply_own(store, &op).unwrap_or(false)
    });
    clear_pending_under(path);
}

/// Robocopy has already copied then deleted. The destination id is new even on the same volume.
/// A copy-only fast transfer must not call this.
pub(crate) fn on_fast_moves(pairs: &[(String, String)]) {
    if pairs.is_empty() {
        return;
    }
    let ids: Vec<Option<FileIdentity>> = pairs
        .iter()
        .map(|(_, dest)| {
            let id = read_file_identity(Path::new(dest));
            (id.fid.is_some() || id.vol.is_some()).then_some(id)
        })
        .collect();
    super::commit_edit(|store| {
        let mut any = false;
        for ((src, dest), id) in pairs.iter().zip(ids.iter()) {
            let mut op = OwnOp::new(OwnKind::Move, src, dest);
            op.new_id = id.clone();
            if apply_own(store, &op).unwrap_or(false) {
                any = true;
                refresh_child_ids(store, dest);
            }
        }
        any
    });
    for (src, _) in pairs {
        clear_pending_under(src);
    }
}

pub(crate) fn arm_sweep() {
    std::thread::Builder::new()
        .name("marks-sweep".into())
        .spawn(|| {
            set_low_priority();
            std::thread::sleep(std::time::Duration::from_secs(SWEEP_DELAY_SECS));
            sweep_once();
        })
        .ok();
}

fn set_low_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

struct SweepRow {
    path: String,
    vol: String,
    fid: String,
    class: SweepClass,
}

fn sweep_once() {
    let rows = super::with_store(|store| {
        let mut out = Vec::new();
        for mark in store.iter() {
            if is_unc(&mark.path) {
                continue;
            }
            let (Some(vol), Some(fid)) = (mark.vol.clone(), mark.fid.clone()) else { continue };
            let class = if mark.state == STATE_LOST {
                SweepClass::Lost
            } else if mark.state == STATE_RECYCLED {
                SweepClass::Recycled
            } else if mark.state == STATE_OK {
                SweepClass::Missing
            } else {
                continue;
            };
            out.push(SweepRow { path: mark.path.clone(), vol, fid, class });
        }
        out
    })
    .unwrap_or_default();
    let plan_items: Vec<SweepItem> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| SweepItem {
            index,
            class: row.class,
            has_id: true,
            unreachable: is_parked(&row.path) || (row.class == SweepClass::Missing && !path_missing(&row.path)),
        })
        .collect();
    let picked: Vec<&SweepRow> = sweep_plan(&plan_items, SWEEP_LIMIT).into_iter().filter_map(|index| rows.get(index)).collect();
    if picked.is_empty() {
        return;
    }
    let mounted = mounted_volumes();
    let mounted_ref: Vec<(char, &str)> = mounted.iter().map(|(letter, vol)| (*letter, vol.as_str())).collect();
    let stamp = now_stamp();
    let mut hits = Vec::new();
    for row in picked {
        let root = match pick_volume_root(&row.vol, &row.path, &mounted_ref) {
            VolumeRoot::Letter(letter) => format!("{letter}:\\"),
            VolumeRoot::Unplugged => {
                hits.push((row.path.clone(), IdHit::Unreachable));
                continue;
            }
        };
        hits.push((row.path.clone(), lookup_fid(&root, &row.fid)));
    }
    super::commit_edit(|store| {
        let mut any = false;
        for (path, hit) in &hits {
            any |= apply_id_hit(store, path, hit.clone(), &stamp).unwrap_or(false);
        }
        any
    });
}

fn path_missing(path: &str) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => false,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn mounted_volumes() -> Vec<(char, String)> {
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;
    use windows::core::HSTRING;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_RAMDISK: u32 = 6;
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for index in 0..26u32 {
        if mask >> index & 1 == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let root = format!("{letter}:\\");
        let kind = unsafe { drive_type(&HSTRING::from(&root)) };
        if kind != DRIVE_REMOVABLE && kind != DRIVE_FIXED && kind != DRIVE_RAMDISK {
            continue;
        }
        if let Some(vol) = read_file_identity(Path::new(&root)).vol {
            out.push((letter, vol));
        }
    }
    out
}

unsafe fn drive_type(root: &windows::core::HSTRING) -> u32 {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    GetDriveTypeW(root)
}

/// Open `root` (a directory such as `C:\`) and look up `fid` with `OpenFileById`.
pub(crate) fn lookup_fid(root: &str, fid_hex: &str) -> IdHit {
    if root.is_empty() || fid_hex.trim().is_empty() {
        return IdHit::Unreachable;
    }
    let Some(fid) = parse_fid(fid_hex) else { return IdHit::Unreachable };
    unsafe { lookup_fid_at(root, fid) }
}

fn parse_fid(hex: &str) -> Option<[u8; 16]> {
    let hex = hex.trim();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for index in 0..16 {
        out[index] = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

unsafe fn lookup_fid_at(root: &str, fid: [u8; 16]) -> IdHit {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFinalPathNameByHandleW, OpenFileById, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_128, FILE_ID_DESCRIPTOR, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_DOS, ExtendedFileIdType,
    };
    use windows::core::HSTRING;

    let opened = crate::winpath::win_long_path(root);
    let Ok(hint) = CreateFileW(
        &HSTRING::from(&opened),
        FILE_READ_ATTRIBUTES.0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    ) else {
        return IdHit::Unreachable;
    };
    struct Opened(windows::Win32::Foundation::HANDLE);
    impl Drop for Opened {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let hint = Opened(hint);
    let mut desc = FILE_ID_DESCRIPTOR::default();
    desc.dwSize = std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32;
    desc.Type = ExtendedFileIdType;
    desc.Anonymous.ExtendedFileId = FILE_ID_128 { Identifier: fid };
    let file = match OpenFileById(hint.0, &desc, FILE_READ_ATTRIBUTES.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, FILE_FLAG_BACKUP_SEMANTICS) {
        Ok(handle) if handle.is_invalid() => return IdHit::Gone,
        Ok(handle) => Opened(handle),
        Err(err) => return id_error(&err),
    };
    let mut buf = [0u16; 1024];
    let n = GetFinalPathNameByHandleW(file.0, &mut buf, VOLUME_NAME_DOS) as usize;
    if n == 0 || n >= buf.len() {
        return IdHit::Gone;
    }
    let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
    let wide = String::from_utf16_lossy(&buf[..end]);
    let path = dos_from_final(&wide);
    if path.len() >= 2 && (path.as_bytes()[1] == b':' || path.starts_with(r"\\")) {
        IdHit::Found(path)
    } else {
        IdHit::Gone
    }
}

fn id_error(err: &windows_core::Error) -> IdHit {
    // Win32 facility HRESULTs keep the code in the low 16 bits.
    let code = (err.code().0 as u32) & 0xFFFF;
    if code == 2 || code == 3 || code == 87 || code == 1168 {
        IdHit::Gone
    } else {
        IdHit::Unreachable
    }
}

fn dos_from_final(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// Ask the main window to redraw stars. Safe from the file-operation worker.
pub(crate) fn poke_ui() {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW};
    struct Acc {
        pid: u32,
        hwnds: Vec<isize>,
    }
    unsafe extern "system" fn enum_win(hwnd: HWND, lp: LPARAM) -> windows_core::BOOL {
        let acc = &mut *(lp.0 as *mut Acc);
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == acc.pid {
            let mut buf = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut buf);
            if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == "MaguroMain" {
                acc.hwnds.push(hwnd.0 as isize);
            }
        }
        true.into()
    }
    let mut acc = Acc { pid: std::process::id(), hwnds: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(enum_win), LPARAM(&mut acc as *mut Acc as isize));
        for hwnd in acc.hwnds {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), crate::WM_MARKS, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::store::{MarksLock, FILE_NAME};
    use std::sync::{Arc, Mutex as StdMutex};

    const ADDED: &str = "2026-11-15T00:00:00Z";
    const STAMP: &str = "2026-11-16T00:00:00Z";
    const NOW: i64 = 1_700_000_000;

    fn scratch(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("maguro-follow-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn store_at(dir: &std::path::Path) -> Store {
        Store::load_at(dir, MarksLock::Shared(Arc::new(StdMutex::new(()))), NOW)
    }

    fn ident(vol: &str, fid: &str, dir: bool) -> FileIdentity {
        FileIdentity { vol: Some(vol.into()), fid: Some(fid.into()), size: Some(10), mtime: Some(20), dir }
    }

    fn add(store: &mut Store, path: &str, id: FileIdentity) {
        store.add_with(path, id, ADDED).unwrap();
    }

    fn op(kind: OwnKind, src: &str, dest: &str) -> OwnOp {
        OwnOp::new(kind, src, dest)
    }

    #[test]
    fn a_longer_sibling_is_not_under_the_renamed_folder() {
        assert_eq!(rewrite_prefix(r"D:\Work\Photos\a.txt", r"D:\Work\Photos", r"D:\Work\Pics"), Some(r"D:\Work\Pics\a.txt".into()));
        assert_eq!(rewrite_prefix(r"D:\Work\Photos", r"D:\Work\Photos", r"D:\Work\Pics"), Some(r"D:\Work\Pics".into()));
        assert!(rewrite_prefix(r"D:\Work\Photos2\a.txt", r"D:\Work\Photos", r"D:\Work\Pics").is_none());
    }

    #[test]
    fn rename_keeps_the_file_id_and_a_folder_rename_rewrites_children() {
        let dir = scratch("rename");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\Work\note.txt", ident("VOL", "FID1", false));
        add(&mut store, r"D:\Work\Photos", ident("VOL", "DIR1", true));
        add(&mut store, r"D:\Work\Photos\kid\a.txt", ident("VOL", "KID1", false));
        add(&mut store, r"D:\Work\Photos2\leave.txt", ident("VOL", "SIB", false));
        apply_own(&mut store, &op(OwnKind::Rename, r"D:\Work\note.txt", r"D:\Work\renamed.txt")).unwrap();
        let mark = store.get(r"D:\Work\renamed.txt").unwrap();
        assert_eq!(mark.fid.as_deref(), Some("FID1"));
        assert_eq!(mark.vol.as_deref(), Some("VOL"));
        assert_eq!(mark.state, STATE_OK);
        assert!(store.get(r"D:\Work\note.txt").is_none());

        apply_own(&mut store, &op(OwnKind::Rename, r"D:\Work\Photos", r"D:\Work\Pics")).unwrap();
        assert!(store.get(r"D:\Work\Photos").is_none());
        assert_eq!(store.get(r"D:\Work\Pics").unwrap().fid.as_deref(), Some("DIR1"));
        assert_eq!(store.get(r"D:\Work\Pics").unwrap().state, STATE_OK);
        assert_eq!(store.get(r"D:\Work\Pics\kid\a.txt").unwrap().fid.as_deref(), Some("KID1"));
        assert_eq!(store.get(r"D:\Work\Photos2\leave.txt").unwrap().fid.as_deref(), Some("SIB"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_volume_move_keeps_the_id_and_cross_volume_replaces_it() {
        let dir = scratch("move");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("AAAA", "1111", false));
        add(&mut store, r"D:\box\tree", ident("AAAA", "FOLD", true));
        add(&mut store, r"D:\box\tree\c.txt", ident("AAAA", "CHILD", false));
        apply_own(&mut store, &op(OwnKind::Move, r"D:\box\a.txt", r"D:\other\a.txt")).unwrap();
        let mark = store.get(r"D:\other\a.txt").unwrap();
        assert_eq!(mark.fid.as_deref(), Some("1111"));
        assert_eq!(mark.state, STATE_OK);
        assert!(store.get(r"D:\box\a.txt").is_none());

        let mut cross = op(OwnKind::Move, r"D:\box\tree", r"E:\box\tree");
        cross.new_id = Some(ident("BBBB", "FOLD2", true));
        cross.child_ids.push((r"E:\box\tree\c.txt".into(), ident("BBBB", "CHILD2", false)));
        apply_own(&mut store, &cross).unwrap();
        assert_eq!(store.get(r"E:\box\tree").unwrap().fid.as_deref(), Some("FOLD2"));
        assert_eq!(store.get(r"E:\box\tree").unwrap().vol.as_deref(), Some("BBBB"));
        assert_eq!(store.get(r"E:\box\tree\c.txt").unwrap().fid.as_deref(), Some("CHILD2"));
        assert_eq!(store.get(r"E:\box\tree\c.txt").unwrap().state, STATE_OK);
        assert!(store.get(r"D:\box\tree").is_none());
        assert!(store.get(r"D:\box\tree\c.txt").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recycle_keeps_the_id_and_a_later_lookup_restores_ok() {
        let dir = scratch("recycle");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("VOL", "FIDR", false));
        add(&mut store, r"D:\box\dir", ident("VOL", "DIRR", true));
        add(&mut store, r"D:\box\dir\c.txt", ident("VOL", "CR", false));
        let mut del = op(OwnKind::Delete, r"D:\box\a.txt", "");
        del.recycled = true;
        apply_own(&mut store, &del).unwrap();
        let mark = store.get(r"D:\box\a.txt").unwrap();
        assert_eq!(mark.state, STATE_RECYCLED);
        assert_eq!(mark.fid.as_deref(), Some("FIDR"));
        apply_id_hit(&mut store, r"D:\box\a.txt", IdHit::Found(r"D:\back\a.txt".into()), STAMP).unwrap();
        let mark = store.get(r"D:\back\a.txt").unwrap();
        assert_eq!(mark.state, STATE_OK);
        assert_eq!(mark.fid.as_deref(), Some("FIDR"));
        assert!(mark.lost_since.is_none());
        assert!(store.get(r"D:\box\a.txt").is_none());

        let mut folder = op(OwnKind::Delete, r"D:\box\dir", "");
        folder.recycled = true;
        apply_own(&mut store, &folder).unwrap();
        assert_eq!(store.get(r"D:\box\dir").unwrap().state, STATE_RECYCLED);
        assert_eq!(store.get(r"D:\box\dir\c.txt").unwrap().state, STATE_RECYCLED);
        assert_eq!(store.get(r"D:\box\dir\c.txt").unwrap().fid.as_deref(), Some("CR"));
        note_identity(&mut store, r"D:\box\dir\c.txt", &ident("VOL", "CR", false), STAMP).unwrap();
        assert_eq!(store.get(r"D:\box\dir\c.txt").unwrap().state, STATE_OK);
        assert_eq!(store.get(r"D:\box\dir\c.txt").unwrap().fid.as_deref(), Some("CR"));
        assert!(store.get(r"D:\box\dir\c.txt").unwrap().lost_since.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recycle_bin_path_is_recycled_and_a_missing_id_is_lost() {
        let dir = scratch("lookup");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("VOL", "FIDX", false));
        add(&mut store, r"D:\box\b.txt", ident("VOL", "FIDY", false));
        apply_id_hit(&mut store, r"D:\box\a.txt", IdHit::Found(r"D:\$Recycle.Bin\S-1-5\a.txt".into()), STAMP).unwrap();
        let mark = store.get(r"D:\$Recycle.Bin\S-1-5\a.txt").unwrap();
        assert_eq!(mark.state, STATE_RECYCLED);
        assert_eq!(mark.fid.as_deref(), Some("FIDX"));
        apply_id_hit(&mut store, r"D:\box\b.txt", IdHit::Gone, STAMP).unwrap();
        let mark = store.get(r"D:\box\b.txt").unwrap();
        assert_eq!(mark.state, STATE_LOST);
        assert_eq!(mark.lost_since.as_deref(), Some(STAMP));
        assert_eq!(mark.fid.as_deref(), Some("FIDY"));
        apply_id_hit(&mut store, r"D:\box\b.txt", IdHit::Gone, "2099-01-01T00:00:00Z").unwrap();
        assert_eq!(store.get(r"D:\box\b.txt").unwrap().lost_since.as_deref(), Some(STAMP));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn permanent_delete_removes_the_mark_and_children_and_copy_adds_nothing() {
        let dir = scratch("delete-copy");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\dir", ident("VOL", "D", true));
        add(&mut store, r"D:\box\dir\c.txt", ident("VOL", "C", false));
        add(&mut store, r"D:\box\keep.txt", ident("VOL", "K", false));
        apply_own(&mut store, &op(OwnKind::Delete, r"D:\box\dir", "")).unwrap();
        assert!(store.get(r"D:\box\dir").is_none());
        assert!(store.get(r"D:\box\dir\c.txt").is_none());
        assert!(store.get(r"D:\box\keep.txt").is_some());
        apply_own(&mut store, &op(OwnKind::Copy, r"D:\box\keep.txt", r"D:\box\keep-copy.txt")).unwrap();
        assert!(store.get(r"D:\box\keep.txt").is_some());
        assert!(store.get(r"D:\box\keep-copy.txt").is_none());
        assert_eq!(store.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_and_cancel_leave_the_store_unchanged() {
        let dir = scratch("cancel");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("VOL", "FID", false));
        for kind in [OwnKind::Rename, OwnKind::Move, OwnKind::Delete, OwnKind::Copy] {
            let mut item = op(kind, r"D:\box\a.txt", r"D:\box\b.txt");
            item.succeeded = false;
            item.recycled = true;
            item.new_id = Some(ident("NEW", "NEW", false));
            assert!(!apply_own(&mut store, &item).unwrap());
        }
        let mark = store.get(r"D:\box\a.txt").unwrap();
        assert_eq!(mark.state, STATE_OK);
        assert_eq!(mark.fid.as_deref(), Some("FID"));
        assert!(store.get(r"D:\box\b.txt").is_none());
        assert_eq!(store.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cross_drive_follows_only_a_single_name_size_and_mtime_match() {
        let cand = LostCand {
            path: r"D:\box\a.txt".into(),
            name: "a.txt".into(),
            dir: false,
            vol: Some("AAAA".into()),
            fid: Some("1111".into()),
            size: Some(10),
            mtime: Some(20),
            at_ms: 1_000,
        };
        let arrive = |name: &str, size: Option<u64>, mtime: Option<u64>, at: u64, fid: &str| Arrival {
            path: format!(r"E:\dst\{name}"),
            name: name.into(),
            vol: Some("BBBB".into()),
            fid: Some(fid.into()),
            size,
            mtime,
            at_ms: at,
        };
        assert_eq!(match_arrival(&[cand.clone()], &arrive("a.txt", Some(10), Some(20), 3_000, "9999")), Some(0));
        assert_eq!(match_arrival(&[cand.clone()], &arrive("a.txt", Some(10), Some(20), 1_000, "9999")), Some(0));
        assert!(match_arrival(&[cand.clone()], &arrive("a.txt", Some(11), Some(20), 3_000, "9999")).is_none());
        assert!(match_arrival(&[cand.clone()], &arrive("a.txt", Some(10), Some(21), 3_000, "9999")).is_none());
        assert!(match_arrival(&[cand.clone()], &arrive("a.txt", None, Some(20), 3_000, "9999")).is_none());
        assert!(match_arrival(&[cand.clone()], &arrive("b.txt", Some(10), Some(20), 3_000, "9999")).is_none());
        assert!(match_arrival(&[cand.clone()], &arrive("a.txt", Some(10), Some(20), 3_001, "9999")).is_none());
        let twin = LostCand { path: r"D:\other\a.txt".into(), ..cand.clone() };
        assert!(match_arrival(&[cand.clone(), twin], &arrive("a.txt", Some(10), Some(20), 2_000, "9999")).is_none());
        let mut same = arrive("other.bin", Some(1), Some(2), 2_000, "1111");
        same.vol = Some("AAAA".into());
        assert_eq!(match_arrival(&[cand.clone()], &same), Some(0));

        let dir = scratch("follow");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("AAAA", "1111", false));
        let hit = arrive("a.txt", Some(10), Some(20), 1_500, "9999");
        apply_follow(&mut store, &cand, &hit).unwrap();
        let mark = store.get(r"E:\dst\a.txt").unwrap();
        assert_eq!(mark.state, STATE_OK);
        assert_eq!(mark.fid.as_deref(), Some("9999"));
        assert_eq!(mark.vol.as_deref(), Some("BBBB"));
        assert!(store.get(r"D:\box\a.txt").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_same_path_replacement_with_a_different_id_is_lost_and_not_marked() {
        let dir = scratch("replace");
        let mut store = store_at(&dir);
        add(&mut store, r"D:\box\a.txt", ident("VOL", "OLD1", false));
        note_identity(&mut store, r"D:\box\a.txt", &ident("VOL", "NEW1", false), STAMP).unwrap();
        assert_eq!(store.len(), 1);
        let mark = store.get(r"D:\box\a.txt").unwrap();
        assert_eq!(mark.state, STATE_LOST);
        assert_eq!(mark.fid.as_deref(), Some("OLD1"));
        assert_eq!(mark.lost_since.as_deref(), Some(STAMP));
        let mut plain = ident("VOL", "NEW1", false);
        plain.fid = None;
        plain.vol = None;
        let mut fat = store_at(&dir);
        add(&mut fat, r"D:\box\b.txt", FileIdentity { size: Some(4), ..FileIdentity::default() });
        assert!(!note_identity(&mut fat, r"D:\box\b.txt", &plain, STAMP).unwrap());
        assert_eq!(fat.get(r"D:\box\b.txt").unwrap().state, STATE_OK);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disconnected_unc_path_does_not_become_lost() {
        let dir = scratch("unc");
        let mut store = store_at(&dir);
        add(&mut store, r"\\server\share\a.txt", FileIdentity::default());
        assert!(!apply_id_hit(&mut store, r"\\server\share\a.txt", IdHit::Unreachable, STAMP).unwrap());
        assert!(!apply_id_hit(&mut store, r"\\server\share\a.txt", IdHit::Gone, STAMP).unwrap());
        assert!(!apply_lost(&mut store, r"\\server\share\a.txt", STAMP).unwrap());
        let mark = store.get(r"\\server\share\a.txt").unwrap();
        assert_eq!(mark.state, STATE_OK);
        assert!(mark.lost_since.is_none());
        assert!(mark.fid.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removable_letter_change_picks_the_single_matching_volume() {
        let mounted = [('E', "AAAA"), ('F', "BBBB")];
        assert_eq!(pick_volume_root("AAAA", r"E:\box\a.txt", &mounted), VolumeRoot::Letter('E'));
        assert_eq!(pick_volume_root("BBBB", r"E:\box\a.txt", &mounted), VolumeRoot::Letter('F'));
        assert_eq!(pick_volume_root("BBBB", r"E:\box\a.txt", &[('F', "BBBB"), ('G', "BBBB")]), VolumeRoot::Unplugged);
        assert_eq!(pick_volume_root("AAAA", r"E:\box\a.txt", &[]), VolumeRoot::Unplugged);
        let dir = scratch("letter");
        let mut store = store_at(&dir);
        add(&mut store, r"E:\box\a.txt", ident("AAAA", "FID", false));
        apply_id_hit(&mut store, r"E:\box\a.txt", IdHit::Unreachable, STAMP).unwrap();
        assert_eq!(store.get(r"E:\box\a.txt").unwrap().state, STATE_OK);
        apply_id_hit(&mut store, r"E:\box\a.txt", IdHit::Found(r"F:\box\a.txt".into()), STAMP).unwrap();
        assert_eq!(store.get(r"F:\box\a.txt").unwrap().state, STATE_OK);
        assert_eq!(store.get(r"F:\box\a.txt").unwrap().fid.as_deref(), Some("FID"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_on_load_still_drops_only_lost_rows_180_days_old() {
        let dir = scratch("purge");
        let day = 86_400i64;
        let doc = serde_json::json!({
            "version": 1,
            "marks": [
                {"path": "D:\\keep-ok.txt", "state": "ok", "lost_since": stamp_unix(NOW - 181 * day), "added": ADDED},
                {"path": "D:\\keep-recycled.txt", "state": "recycled", "lost_since": stamp_unix(NOW - 181 * day), "added": ADDED},
                {"path": "D:\\keep-179.txt", "state": "lost", "lost_since": stamp_unix(NOW - 179 * day), "added": ADDED},
                {"path": "D:\\drop-180.txt", "state": "lost", "lost_since": stamp_unix(NOW - 180 * day), "added": ADDED}
            ]
        });
        std::fs::write(dir.join(FILE_NAME), serde_json::to_string(&doc).unwrap()).unwrap();
        let store = Store::load_at(&dir, MarksLock::Shared(Arc::new(StdMutex::new(()))), NOW);
        assert!(store.get(r"D:\keep-ok.txt").is_some());
        assert_eq!(store.get(r"D:\keep-recycled.txt").unwrap().state, STATE_RECYCLED);
        assert!(store.get(r"D:\keep-179.txt").is_some());
        assert!(store.get(r"D:\drop-180.txt").is_none());
        let plan = sweep_plan(
            &[
                SweepItem { index: 0, class: SweepClass::Missing, has_id: true, unreachable: false },
                SweepItem { index: 1, class: SweepClass::Lost, has_id: true, unreachable: false },
                SweepItem { index: 2, class: SweepClass::Recycled, has_id: false, unreachable: false },
                SweepItem { index: 3, class: SweepClass::Lost, has_id: true, unreachable: true },
            ],
            500,
        );
        assert_eq!(plan, vec![1, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_handles_merge_a_path_update_a_recycle_and_a_delete() {
        let dir = scratch("merge");
        let lock = MarksLock::Shared(Arc::new(StdMutex::new(())));
        let mut seed = Store::load_at(&dir, lock.clone(), NOW);
        seed.add_with(r"D:\Work\a.txt", ident("AAAA", "01", false), ADDED).unwrap();
        seed.add_with(r"D:\Work\b.txt", ident("BBBB", "02", false), ADDED).unwrap();
        seed.add_with(r"D:\Work\c.txt", ident("CCCC", "03", false), ADDED).unwrap();
        seed.save().unwrap();
        let mut left = Store::load_at(&dir, lock.clone(), NOW);
        let mut right = Store::load_at(&dir, lock.clone(), NOW);
        let mut moved = op(OwnKind::Move, r"D:\Work\a.txt", r"E:\Work\a.txt");
        moved.new_id = Some(ident("EEEE", "99", false));
        apply_own(&mut left, &moved).unwrap();
        let mut recycled = op(OwnKind::Delete, r"D:\Work\c.txt", "");
        recycled.recycled = true;
        apply_own(&mut left, &recycled).unwrap();
        apply_own(&mut right, &op(OwnKind::Delete, r"D:\Work\b.txt", "")).unwrap();
        left.save().unwrap();
        right.save().unwrap();
        let merged = Store::load_at(&dir, lock, NOW);
        let mark = merged.get(r"E:\Work\a.txt").unwrap();
        assert_eq!(mark.fid.as_deref(), Some("99"));
        assert_eq!(mark.vol.as_deref(), Some("EEEE"));
        assert_eq!(mark.state, STATE_OK);
        assert!(merged.get(r"D:\Work\a.txt").is_none());
        assert!(merged.get(r"D:\Work\b.txt").is_none());
        assert_eq!(merged.get(r"D:\Work\c.txt").unwrap().state, STATE_RECYCLED);
        assert_eq!(merged.get(r"D:\Work\c.txt").unwrap().fid.as_deref(), Some("03"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
