//! Per-archive extract cache.
//!
//! `%LOCALAPPDATA%\Maguro\ArchiveCache\<id>\files` holds one archive. `id` is
//! the first 16 bytes of SHA-256 over the normalized path, the size, and the
//! mtime, so a changed archive does not reuse a stale extract. `meta.json`
//! beside `files` remembers the archive path. The pane browses `files`. The
//! address bar shows the archive path instead.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::extract::{self, ExtractError};
use super::kind::browse_kind;

/// One archive may extract at most this many bytes.
pub(crate) const MAX_EXTRACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The cache as a whole stays under this.
pub(crate) const CACHE_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Entries not touched for this long are deleted at startup.
pub(crate) const MAX_AGE_SECS: u64 = 14 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub(crate) enum Plan {
    Normal,
    /// Browse this real directory. It is the cache for a virtual archive path.
    Browse(String),
    Extract { archive: String, inner: String },
    /// The archive file is gone. Open this folder instead.
    Missing(String),
}

#[derive(Clone, Debug)]
pub(crate) struct CacheHit {
    pub id: String,
    pub files: PathBuf,
    pub archive: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Meta {
    archive: String,
    size: u64,
    mtime: u64,
    used: u64,
    bytes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct CacheItem {
    pub id: String,
    pub bytes: u64,
    pub used: u64,
}

pub(crate) fn cache_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Maguro").join("ArchiveCache")
}

pub(crate) fn norm_path(path: &str) -> String {
    let mut s = path.trim().replace('/', "\\");
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s.to_ascii_lowercase()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn entry_id(path: &str, size: u64, mtime: u64) -> String {
    let mut h = Sha256::new();
    h.update(norm_path(path).as_bytes());
    h.update([0]);
    h.update(size.to_le_bytes());
    h.update(mtime.to_le_bytes());
    let dig = h.finalize();
    dig[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((meta.len(), mtime))
}

fn meta_cache() -> &'static Mutex<HashMap<String, Meta>> {
    static M: std::sync::LazyLock<Mutex<HashMap<String, Meta>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &M
}

fn remember(id: &str, meta: Meta) {
    meta_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), meta);
}

fn load_meta(root: &Path, id: &str) -> Option<Meta> {
    if let Some(hit) = meta_cache().lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned() {
        return Some(hit);
    }
    let text = std::fs::read_to_string(root.join(id).join("meta.json")).ok()?;
    let meta: Meta = serde_json::from_str(&text).ok()?;
    remember(id, meta.clone());
    Some(meta)
}

#[derive(Clone, Debug)]
struct Located {
    id: String,
    inner: String,
    archive: String,
    files: PathBuf,
}

/// `path` is `root/<id>/files` or a child of it.
fn locate(path: &str, root: &Path) -> Option<Located> {
    let root_n = norm_path(&root.display().to_string());
    let root_n = root_n.trim_end_matches('\\');
    let path_n = norm_path(path);
    let rest = path_n.strip_prefix(root_n)?.trim_start_matches('\\');
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.split('\\');
    let id = parts.next().unwrap_or("");
    if !is_id(id) || parts.next() != Some("files") {
        return None;
    }
    let inner = parts.filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\\");
    let meta = load_meta(root, id)?;
    Some(Located { id: id.to_string(), inner, archive: meta.archive, files: root.join(id).join("files") })
}

pub(crate) fn join_inner(files: &Path, inner: &str) -> PathBuf {
    let mut path = files.to_path_buf();
    for part in inner.split(['\\', '/']) {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

fn virtual_of(archive: &str, inner: &str) -> String {
    let archive = archive.trim_end_matches(['\\', '/']);
    if inner.is_empty() {
        archive.to_string()
    } else {
        format!(r"{archive}\{inner}")
    }
}

fn parent_dir(path: &str) -> Option<String> {
    let path = path.trim().trim_end_matches(['\\', '/']);
    let bytes = path.as_bytes();
    if bytes.len() <= 3 && bytes.get(1) == Some(&b':') {
        return None;
    }
    let i = path.rfind(['\\', '/'])?;
    if i == 2 && bytes.get(1) == Some(&b':') {
        return path.get(..=2).map(|s| s.to_string());
    }
    if i == 0 {
        return None;
    }
    Some(path[..i].to_string())
}

fn file_name(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// Outermost archive file inside `path`, plus the rest of the path.
/// A directory whose name ends in `.7z` is a folder, so `is_directory` keeps it out.
pub(crate) fn split_virtual(path: &str, is_directory: impl Fn(&str) -> bool) -> Option<(String, String)> {
    let path = path.trim().trim_end_matches(['\\', '/']);
    if path.len() < 3 {
        return None;
    }
    let (head, tail) = if path.starts_with(r"\\") {
        let rest = &path[2..];
        let mut it = rest.splitn(3, '\\');
        let server = it.next().unwrap_or("");
        let share = it.next().unwrap_or("");
        if server.is_empty() || share.is_empty() {
            return None;
        }
        let head = format!(r"\\{server}\{share}");
        let tail = it.next().unwrap_or("");
        (head, tail)
    } else if path.as_bytes().get(1) == Some(&b':') {
        let head = path.get(..3)?.to_string();
        if !head.ends_with('\\') && !head.ends_with('/') {
            return None;
        }
        (head, path.get(3..).unwrap_or(""))
    } else {
        return None;
    };
    let mut cursor = head.trim_end_matches(['\\', '/']).to_string();
    if path.as_bytes().get(1) == Some(&b':') {
        cursor = path.get(..2)?.to_string(); // `D:` then we add `\` with the first part
    }
    let mut saw = false;
    for part in tail.split(['\\', '/']) {
        if part.is_empty() {
            continue;
        }
        saw = true;
        if cursor.ends_with('\\') {
            cursor.push_str(part);
        } else {
            cursor.push('\\');
            cursor.push_str(part);
        }
        if browse_kind(&cursor).is_some() && !is_directory(&cursor) {
            let inner = if cursor.len() >= path.len() { "" } else { path.get(cursor.len() + 1..).unwrap_or("") };
            return Some((cursor, inner.replace('/', "\\")));
        }
    }
    let _ = saw;
    None
}

pub(crate) fn display_of(path: &str, root: &Path) -> String {
    match locate(path, root) {
        Some(loc) => virtual_of(&loc.archive, &loc.inner),
        None => path.to_string(),
    }
}

pub(crate) fn display_path(path: &str) -> String {
    if !norm_path(path).contains(r"\maguro\archivecache\") {
        return path.to_string();
    }
    display_of(path, &cache_root())
}

pub(crate) fn history_path(path: &str) -> String {
    let shown = display_path(path);
    if norm_path(&shown).contains(r"\maguro\archivecache\") {
        String::new()
    } else {
        shown
    }
}

pub(crate) fn session_store_path(path: &str) -> String {
    if !norm_path(path).contains(r"\maguro\archivecache\") {
        return path.to_string();
    }
    display_of(path, &cache_root())
}

pub(crate) fn session_load_path(path: &str) -> String {
    if norm_path(path).contains(r"\maguro\archivecache\") {
        if let Some(loc) = locate(path, &cache_root()) {
            if Path::new(&loc.archive).is_file() {
                return virtual_of(&loc.archive, &loc.inner);
            }
            return parent_dir(&loc.archive).unwrap_or_default();
        }
        return String::new();
    }
    path.to_string()
}

/// What a navigation to `path` should do. `open_enabled` is the settings switch.
/// Cache paths and already-extracted virtual paths still resolve when it is off,
/// so a restored pane does not open a hash folder. A fresh extract does not start
/// when the switch is off.
pub(crate) fn classify(path: &str, root: &Path, open_enabled: bool, is_directory: impl Fn(&str) -> bool) -> Plan {
    if locate(path, root).is_some() {
        return Plan::Normal;
    }
    if let Some((archive, inner)) = split_virtual(path, &is_directory) {
        if let Some((size, mtime)) = stamp(Path::new(&archive)) {
            let id = entry_id(&archive, size, mtime);
            let files = root.join(&id).join("files");
            let meta_ok = root.join(&id).join("meta.json").is_file();
            if meta_ok && files.is_dir() {
                let dest = join_inner(&files, &inner);
                if inner.is_empty() || dest.is_dir() {
                    return Plan::Browse(dest.display().to_string());
                }
            }
        }
        if !open_enabled {
            return Plan::Normal;
        }
        if Path::new(&archive).is_file() {
            return Plan::Extract { archive, inner };
        }
        if let Some(parent) = parent_dir(&archive) {
            return Plan::Missing(parent);
        }
    }
    Plan::Normal
}

pub(crate) fn plan(path: &str) -> Plan {
    let enabled = crate::archive::enabled();
    let root = cache_root();
    classify(path, &root, enabled, |p| Path::new(p).is_dir())
}

pub(crate) struct Up {
    pub parent: String,
    pub select: String,
}

pub(crate) fn up_from_root(path: &str, root: &Path) -> Option<Up> {
    let loc = locate(path, root)?;
    if !loc.inner.is_empty() {
        return None;
    }
    Some(Up { parent: parent_dir(&loc.archive)?, select: file_name(&loc.archive) })
}

pub(crate) fn up_from(path: &str) -> Option<Up> {
    up_from_root(path, &cache_root())
}

pub(crate) fn parent_of_archive(path: &str) -> Option<String> {
    parent_dir(path)
}

pub(crate) fn touch_path(path: &str) {
    let Some(loc) = locate(path, &cache_root()) else { return };
    let Some(mut meta) = load_meta(&cache_root(), &loc.id) else { return };
    meta.used = now_secs();
    let _ = std::fs::write(cache_root().join(&loc.id).join("meta.json"), serde_json::to_string(&meta).unwrap_or_default());
    remember(&loc.id, meta);
}

/// Ids of cache folders among `paths`.
pub(crate) fn pinned_from(paths: &[String], root: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    for path in paths {
        if let Some(loc) = locate(path, root) {
            ids.insert(loc.id);
        }
    }
    ids
}

/// Which cache ids to delete.
/// Aged entries go first. Then the oldest unpinned entries until `cap` holds.
/// Pinned ids (a pane is showing them) stay even when the total is over the cap.
/// `drop_unpinned` deletes every unpinned id, which is the normal-exit pass.
pub(crate) fn victims(items: &[CacheItem], cap: u64, max_age: u64, now: u64, pinned: &HashSet<String>, drop_unpinned: bool) -> Vec<String> {
    let mut drop = Vec::new();
    let mut kept: Vec<&CacheItem> = Vec::new();
    for item in items {
        if pinned.contains(&item.id) {
            kept.push(item);
            continue;
        }
        if drop_unpinned || (max_age > 0 && now.saturating_sub(item.used) > max_age) {
            drop.push(item.id.clone());
        } else {
            kept.push(item);
        }
    }
    if drop_unpinned {
        return drop;
    }
    let mut total: u64 = kept.iter().map(|i| i.bytes).sum();
    let mut order: Vec<&CacheItem> = kept.iter().copied().filter(|i| !pinned.contains(&i.id)).collect();
    order.sort_by_key(|i| i.used);
    for item in order {
        if total <= cap {
            break;
        }
        drop.push(item.id.clone());
        total = total.saturating_sub(item.bytes);
    }
    drop
}

fn dir_bytes(root: &Path) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(meta) = path.metadata() {
                n = n.saturating_add(meta.len());
            }
        }
    }
    n
}

pub(crate) fn maintain(root: &Path, pinned: &HashSet<String>, drop_unpinned: bool) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut items = Vec::new();
    let mut partials = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if name.ends_with(".partial") {
            partials.push(ent.path());
            continue;
        }
        if !is_id(&name) {
            continue;
        }
        let dir = ent.path();
        let Some(meta) = load_meta(root, &name) else {
            if !pinned.contains(&name) {
                partials.push(dir);
            }
            continue;
        };
        let bytes = if meta.bytes > 0 { meta.bytes } else { dir_bytes(&dir.join("files")) };
        items.push(CacheItem { id: name, bytes, used: meta.used });
    }
    let gone = victims(&items, CACHE_CAP_BYTES, MAX_AGE_SECS, now_secs(), pinned, drop_unpinned);
    for id in gone {
        let _ = std::fs::remove_dir_all(root.join(&id));
        meta_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }
    for path in partials {
        let _ = std::fs::remove_dir_all(path);
    }
}

pub(crate) fn startup(paths: &[String]) {
    let root = cache_root();
    let _ = std::fs::create_dir_all(&root);
    let pinned = pinned_from(paths, &root);
    maintain(&root, &pinned, false);
}

pub(crate) fn shutdown(paths: &[String]) {
    let root = cache_root();
    let pinned = pinned_from(paths, &root);
    maintain(&root, &pinned, true);
}

pub(crate) fn open_into_cache(archive: &Path, cancel: &AtomicBool, limit: u64) -> Result<CacheHit, ExtractError> {
    let root = cache_root();
    std::fs::create_dir_all(&root).map_err(|e| ExtractError::Io(e.to_string()))?;
    let (size, mtime) = stamp(archive).ok_or_else(|| ExtractError::Io("archive is not a file".into()))?;
    let shown = archive.display().to_string();
    let id = entry_id(&shown, size, mtime);
    let dir = root.join(&id);
    let files = dir.join("files");
    if dir.join("meta.json").is_file() && files.is_dir() {
        touch_known(&root, &id);
        return Ok(CacheHit { id, files, archive: shown });
    }
    let partial = root.join(format!("{id}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    let partial_files = partial.join("files");
    std::fs::create_dir_all(&partial_files).map_err(|e| ExtractError::Io(e.to_string()))?;
    let extracted = extract::extract(archive, &partial_files, limit, cancel);
    if let Err(e) = extracted {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let bytes = extracted.unwrap_or(0);
    let meta = Meta { archive: shown.clone(), size, mtime, used: now_secs(), bytes };
    let text = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".into());
    std::fs::write(partial.join("meta.json"), text).map_err(|e| ExtractError::Io(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&partial, &dir).map_err(|e| ExtractError::Io(e.to_string()))?;
    remember(&id, meta);
    Ok(CacheHit { id, files: dir.join("files"), archive: shown })
}

fn touch_known(root: &Path, id: &str) {
    if let Some(mut meta) = load_meta(root, id) {
        meta.used = now_secs();
        let _ = std::fs::write(root.join(id).join("meta.json"), serde_json::to_string(&meta).unwrap_or_default());
        remember(id, meta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maguro-cache-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn virtual_and_cache_paths_roundtrip() {
        let root = root().join("map");
        let _ = std::fs::remove_dir_all(&root);
        let archive = root.join("work").join("foo.7z");
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(&archive, b"not-a-real-7z-for-the-map").unwrap();
        let (size, mtime) = stamp(&archive).unwrap();
        let id = entry_id(&archive.display().to_string(), size, mtime);
        let files = root.join("cache").join(&id).join("files");
        std::fs::create_dir_all(files.join("sub")).unwrap();
        let meta = Meta { archive: archive.display().to_string(), size, mtime, used: 10, bytes: 4 };
        std::fs::write(root.join("cache").join(&id).join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        // The in-memory meta cache is process-global. Use a fresh id read from disk
        // by clearing the slot first.
        meta_cache().lock().unwrap().remove(&id);

        let cache_sub = files.join("sub").display().to_string();
        let shown = display_of(&cache_sub, &root.join("cache"));
        assert!(shown.ends_with(r"foo.7z\sub"), "{shown}");
        let stored = display_of(&files.display().to_string(), &root.join("cache"));
        assert!(stored.ends_with("foo.7z") && !stored.contains("ArchiveCache"), "{stored}");

        let not_dir = |p: &str| Path::new(p).is_dir();
        match classify(&shown, &root.join("cache"), true, not_dir) {
            Plan::Browse(dest) => assert!(dest.ends_with(r"files\sub") || dest.ends_with(r"files/sub"), "{dest}"),
            other => panic!("expected browse, got {other:?}"),
        }
        match classify(&archive.display().to_string(), &root.join("cache"), true, not_dir) {
            Plan::Browse(dest) => assert!(dest.ends_with("files"), "{dest}"),
            other => panic!("expected the cache root, got {other:?}"),
        }
        // Setting off does not start a new extract.
        let other = root.join("work").join("other.7z");
        std::fs::write(&other, b"x").unwrap();
        match classify(&other.display().to_string(), &root.join("cache"), false, |p| Path::new(p).is_dir()) {
            Plan::Normal => {}
            other => panic!("setting off should leave the file alone, got {other:?}"),
        }
        let gone = r"D:\missing\gone.7z\inner";
        match classify(gone, &root.join("cache"), true, |_| false) {
            Plan::Missing(parent) => assert_eq!(parent, r"D:\missing"),
            other => panic!("expected the parent, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn up_from_the_cache_root_selects_the_archive() {
        let root = root().join("up");
        let _ = std::fs::remove_dir_all(&root);
        let archive = r"D:\work\foo.7z";
        let id = "0123456789abcdef0123456789abcdef";
        std::fs::create_dir_all(root.join(id).join("files").join("sub")).unwrap();
        let meta = Meta { archive: archive.into(), size: 1, mtime: 2, used: 3, bytes: 1 };
        std::fs::write(root.join(id).join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        meta_cache().lock().unwrap().remove(id);
        let files = root.join(id).join("files");
        let up = up_from_root(&files.display().to_string(), &root).unwrap();
        assert_eq!(up.parent, r"D:\work");
        assert_eq!(up.select, "foo.7z");
        assert!(up_from_root(&files.join("sub").display().to_string(), &root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lru_drops_old_and_then_the_oldest_until_the_cap() {
        let items = vec![
            CacheItem { id: "a".into(), bytes: 100, used: 10 },
            CacheItem { id: "b".into(), bytes: 50, used: 20 },
            CacheItem { id: "c".into(), bytes: 80, used: 30 },
            CacheItem { id: "pin".into(), bytes: 500, used: 1 },
        ];
        let mut pinned = HashSet::new();
        pinned.insert("pin".into());
        // Age: now=100, max_age=50, so used=10 (age 90) is old. used=20 is age 80, also old.
        // used=30 is age 70, also old. All unpinned are older than 50. 
        let aged = victims(&items, 1000, 50, 100, &pinned, false);
        assert!(aged.contains(&"a".into()) && aged.contains(&"b".into()) && aged.contains(&"c".into()), "{aged:?}");
        assert!(!aged.contains(&"pin".into()));

        let fresh = vec![
            CacheItem { id: "a".into(), bytes: 40, used: 90 },
            CacheItem { id: "b".into(), bytes: 40, used: 95 },
            CacheItem { id: "c".into(), bytes: 40, used: 99 },
        ];
        let over = victims(&fresh, 80, 1000, 100, &HashSet::new(), false);
        assert_eq!(over, vec!["a".to_string()]);

        let all = victims(&fresh, 0, 0, 100, &pinned, true);
        assert_eq!(all.len(), 3);
        let pinned_only = HashSet::from(["b".into()]);
        let exit = victims(&fresh, 0, 0, 100, &pinned_only, true);
        assert!(exit.contains(&"a".into()) && exit.contains(&"c".into()) && !exit.contains(&"b".into()), "{exit:?}");
    }

    #[test]
    fn session_load_maps_a_cache_path_or_falls_back() {
        let root = root().join("sess");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // session_load_path uses the real cache root, so this test calls the
        // pieces with an injected root instead.
        let id = "abcdefabcdefabcdefabcdefabcdefab";
        let archive = root.join("keep.7z");
        std::fs::write(&archive, b"z").unwrap();
        std::fs::create_dir_all(root.join("cache").join(id).join("files")).unwrap();
        let meta = Meta { archive: archive.display().to_string(), size: 1, mtime: 1, used: 1, bytes: 1 };
        std::fs::write(root.join("cache").join(id).join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        meta_cache().lock().unwrap().remove(id);
        let cache_path = root.join("cache").join(id).join("files").display().to_string();
        let shown = display_of(&cache_path, &root.join("cache"));
        assert!(shown.ends_with("keep.7z"), "{shown}");
        let missing_archive = r"D:\gone\nope.7z";
        let meta2 = Meta { archive: missing_archive.into(), size: 1, mtime: 1, used: 1, bytes: 1 };
        let id2 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        std::fs::create_dir_all(root.join("cache").join(id2).join("files")).unwrap();
        std::fs::write(root.join("cache").join(id2).join("meta.json"), serde_json::to_string(&meta2).unwrap()).unwrap();
        meta_cache().lock().unwrap().remove(id2);
        let loc = locate(&root.join("cache").join(id2).join("files").display().to_string(), &root.join("cache")).unwrap();
        assert_eq!(parent_dir(&loc.archive).unwrap(), r"D:\gone");
        let _ = std::fs::remove_dir_all(&root);
    }
}
