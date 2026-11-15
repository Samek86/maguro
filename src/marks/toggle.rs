//! Toggle rule for important marks. A folder mark is the folder itself.
//! Children are not marked. Callers pass a temp directory in tests.

use super::store::{Store, StoreError};

const ARCHIVE_EXT: &[&str] = &["zip", "7z", "rar", "cab", "iso", "tar", "gz", "tgz", "bz2", "xz"];

/// One selected row, after `SIGDN_FILESYSPATH` (empty when that name did not resolve).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) path: String,
    pub(crate) folder: bool,
    pub(crate) library: bool,
    pub(crate) phone: bool,
    pub(crate) in_archive: bool,
    pub(crate) unmounted: bool,
}

impl Target {
    pub(crate) fn file(path: &str) -> Self {
        Self { path: path.to_string(), folder: false, library: false, phone: false, in_archive: false, unmounted: false }
    }

    pub(crate) fn folder(path: &str) -> Self {
        Self { folder: true, ..Self::file(path) }
    }
}

/// What the one context-menu slot says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuVerb {
    Mark,
    Unmark,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Marked(usize),
    Unmarked(usize),
    Blocked,
    /// [`super::store::CAP`] was hit. `added` is how many new rows fit before the refusal.
    Full { added: usize },
    Empty,
}

pub(crate) fn menu_key(verb: MenuVerb) -> crate::Key {
    match verb {
        MenuVerb::Unmark => crate::Key::RemoveImportantMark,
        MenuVerb::Mark | MenuVerb::Disabled => crate::Key::MarkAsImportant,
    }
}

/// A drive letter with nothing after it (`C:\` or `C:`).
pub(crate) fn drive_root(path: &str) -> bool {
    let t = path.trim().trim_end_matches(['\\', '/']);
    let b = t.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

pub(crate) fn library_file(path: &str) -> bool {
    path.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase().ends_with(".library-ms")
}

pub(crate) fn archive_named(path: &str) -> bool {
    let name = path.trim().trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("");
    let lower = name.to_ascii_lowercase();
    ARCHIVE_EXT.iter().any(|ext| {
        let dot = format!(".{ext}");
        lower.len() > dot.len() && lower.ends_with(&dot)
    })
}

/// Parent of `path`. `C:\foo` yields `C:\`. A root yields `None`.
pub(crate) fn parent_path(path: &str) -> Option<&str> {
    let path = path.trim().trim_end_matches(['\\', '/']);
    let b = path.as_bytes();
    if b.len() <= 3 && b.get(1) == Some(&b':') {
        return None;
    }
    let i = path.rfind(['\\', '/'])?;
    if i == 2 && b.get(1) == Some(&b':') {
        return path.get(..=2);
    }
    if i == 0 {
        return None;
    }
    path.get(..i)
}

/// `path` sits under an ancestor whose name looks like an archive file.
/// The archive itself is not a child.
pub(crate) fn archive_ancestor(path: &str) -> bool {
    let mut cur = path.trim();
    while let Some(parent) = parent_path(cur) {
        if archive_named(parent) {
            return true;
        }
        cur = parent;
    }
    false
}

/// Real files and folders only. Drive roots, libraries, phones, archive members,
/// unmounted remotes, and rows without a filesystem path are not targets.
pub(crate) fn eligible(t: &Target) -> bool {
    let path = t.path.trim();
    if path.is_empty() || !crate::is_fs(path) {
        return false;
    }
    if t.library || t.phone || t.in_archive || t.unmounted || drive_root(path) || library_file(path) {
        return false;
    }
    true
}

/// Any unmarked eligible row marks the whole eligible selection.
/// Every eligible row already marked unmarks them. Nothing eligible stays disabled.
pub(crate) fn menu_verb(targets: &[Target], marked: impl Fn(&str) -> bool) -> MenuVerb {
    let ok: Vec<&Target> = targets.iter().filter(|t| eligible(t)).collect();
    if ok.is_empty() {
        MenuVerb::Disabled
    } else if ok.iter().all(|t| marked(&t.path)) {
        MenuVerb::Unmark
    } else {
        MenuVerb::Mark
    }
}

/// Marks whose parent is `folder` and whose state is still `ok`.
pub(crate) fn count_ok(store: &Store, folder: &str) -> usize {
    store.in_folder(folder).into_iter().filter(|m| m.state == "ok").count()
}

/// Apply the toggle and write `marks.json` when the store changed.
/// A blocked store returns before any write.
pub(crate) fn apply(store: &mut Store, targets: &[Target]) -> Outcome {
    if store.is_blocked() {
        return Outcome::Blocked;
    }
    let eligible: Vec<&Target> = targets.iter().filter(|t| eligible(t)).collect();
    if eligible.is_empty() {
        return Outcome::Empty;
    }
    let outcome = if eligible.iter().any(|t| store.get(&t.path).is_none()) {
        let mut added = 0usize;
        let mut full = false;
        for t in &eligible {
            if store.get(&t.path).is_some() {
                continue;
            }
            match store.add(&t.path) {
                Ok(()) => added += 1,
                Err(StoreError::Full) => {
                    full = true;
                    break;
                }
                Err(StoreError::Blocked) => return Outcome::Blocked,
                Err(_) => {}
            }
        }
        if full { Outcome::Full { added } } else { Outcome::Marked(added) }
    } else {
        let mut n = 0usize;
        for t in &eligible {
            match store.remove(&t.path) {
                Ok(true) => n += 1,
                Err(StoreError::Blocked) => return Outcome::Blocked,
                Ok(false) | Err(_) => {}
            }
        }
        Outcome::Unmarked(n)
    };
    match store.save() {
        Err(StoreError::Blocked) => Outcome::Blocked,
        _ => outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::store::{FileIdentity, MarksLock, FILE_NAME, CAP};
    use std::sync::{Arc, Mutex};

    fn scratch(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("maguro-mark-toggle-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn store_in(dir: &std::path::Path) -> Store {
        Store::load_in(dir, MarksLock::Shared(Arc::new(Mutex::new(()))))
    }

    fn file_in(dir: &std::path::Path, name: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, b"mark").unwrap();
        path.to_string_lossy().replace('/', "\\")
    }

    struct KeysReset;
    impl Drop for KeysReset {
        fn drop(&mut self) {
            crate::keys::reset_all();
        }
    }

    #[test]
    fn one_file_marks_then_the_same_target_unmarks() {
        let dir = scratch("one");
        let path = file_in(&dir, "note.txt");
        let mut store = store_in(&dir);
        let t = [Target::file(&path)];
        assert_eq!(menu_verb(&t, |_| false), MenuVerb::Mark);
        assert_eq!(apply(&mut store, &t), Outcome::Marked(1));
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("note.txt"), "{text}");
        assert!(store.get(&path).is_some());
        assert_eq!(menu_verb(&t, |p| store.get(p).is_some()), MenuVerb::Unmark);
        assert_eq!(apply(&mut store, &t), Outcome::Unmarked(1));
        assert!(store.get(&path).is_none());
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!text.contains("note.txt"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mixed_selection_marks_all_then_unmarks_all() {
        let dir = scratch("mix");
        let a = file_in(&dir, "a.txt");
        let b = file_in(&dir, "b.txt");
        let c = file_in(&dir, "c.txt");
        let mut store = store_in(&dir);
        assert_eq!(apply(&mut store, &[Target::file(&a)]), Outcome::Marked(1));
        let three = [Target::file(&a), Target::file(&b), Target::file(&c)];
        assert_eq!(menu_verb(&three, |p| store.get(p).is_some()), MenuVerb::Mark);
        assert_eq!(apply(&mut store, &three), Outcome::Marked(2));
        assert!(store.get(&a).is_some() && store.get(&b).is_some() && store.get(&c).is_some());
        assert_eq!(menu_verb(&three, |p| store.get(p).is_some()), MenuVerb::Unmark);
        assert_eq!(apply(&mut store, &three), Outcome::Unmarked(3));
        assert!(store.get(&a).is_none() && store.get(&b).is_none() && store.get(&c).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_mark_does_not_mark_its_children() {
        let dir = scratch("folder");
        let folder = dir.join("marked-dir");
        std::fs::create_dir(&folder).unwrap();
        let child = folder.join("child-note.txt");
        std::fs::write(&child, b"inside").unwrap();
        let folder_s = folder.to_string_lossy().replace('/', "\\");
        let child_s = child.to_string_lossy().replace('/', "\\");
        let mut store = store_in(&dir);
        assert_eq!(apply(&mut store, &[Target::folder(&folder_s)]), Outcome::Marked(1));
        let mark = store.get(&folder_s).unwrap();
        assert!(mark.dir);
        assert!(store.get(&child_s).is_none());
        assert_eq!(count_ok(&store, &folder_s), 0);
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("marked-dir"), "{text}");
        assert!(!text.contains("child-note.txt"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_filesystem_path_is_not_marked() {
        let dir = scratch("nopath");
        let before = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        let mut store = store_in(&dir);
        let ghost = Target::file("");
        assert!(!eligible(&ghost));
        assert_eq!(menu_verb(&[ghost.clone()], |_| false), MenuVerb::Disabled);
        assert_eq!(apply(&mut store, &[ghost]), Outcome::Empty);
        assert_eq!(store.len(), 0);
        assert!(!dir.join(FILE_NAME).exists());
        assert_eq!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0), before);

        let mut library = Target::file(r"C:\Users\a\Documents.library-ms");
        library.library = true;
        let mut phone = Target::file(r"C:\phone\dcim");
        phone.phone = true;
        let mut archived = Target::file(r"C:\work\pack.zip\inner.txt");
        archived.in_archive = true;
        let mut remote = Target::file(r"Z:\not-mounted\a.txt");
        remote.unmounted = true;
        for t in [Target::file(r"C:\"), Target::folder(r"D:\"), library, phone, archived, remote, Target::file(r"C:\Users\a\Documents.library-ms")] {
            assert!(!eligible(&t), "{t:?}");
        }
        assert!(archive_ancestor(r"C:\work\pack.zip\nest\inner.txt"));
        assert!(!archive_ancestor(r"C:\work\pack.zip"));
        assert!(archive_ancestor(r"C:\a\one.7z\b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blocked_store_does_not_write() {
        let dir = scratch("blocked");
        let path = dir.join(FILE_NAME);
        let raw = r#"{"version":99,"marks":[{"path":"D:\\keep.txt","state":"ok"}]}"#;
        std::fs::write(&path, raw).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut store = store_in(&dir);
        assert!(store.is_blocked());
        let file = file_in(&dir, "new.txt");
        assert_eq!(apply(&mut store, &[Target::file(&file)]), Outcome::Blocked);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_20001st_add_is_refused() {
        let dir = scratch("cap");
        let mut store = store_in(&dir);
        let meta = FileIdentity::default();
        for i in 0..CAP {
            store.add_with(&format!(r"C:\m\{i}"), meta.clone(), "2026-11-15T00:00:00Z").unwrap();
        }
        store.save().unwrap();
        let before = std::fs::read(dir.join(FILE_NAME)).unwrap();
        let out = apply(&mut store, &[Target::file(r"C:\m\overflow")]);
        assert_eq!(out, Outcome::Full { added: 0 });
        assert_eq!(store.len(), CAP);
        assert!(store.get(r"C:\m\overflow").is_none());
        assert!(store.get(r"C:\m\0").is_some());
        assert_eq!(std::fs::read(dir.join(FILE_NAME)).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_shortcut_is_ctrl_alt_s_and_a_rebind_is_reported() {
        let _reset = KeysReset;
        crate::keys::reset_all();
        let i = crate::keys::index("file.mark").unwrap();
        assert_eq!(crate::keys::id_of(i), "file.mark");
        assert_eq!(crate::keys::chord_text(i), "Ctrl+Alt+S");
        assert_eq!(crate::keys::lookup(b'S' as u16, true, false, true, false), Some(i));
        assert_eq!(crate::keys::lookup(b'S' as u16, true, false, false, false), crate::keys::index("sess.save"));
        let chord = crate::keys::parse_chord("Ctrl+Shift+F9").unwrap();
        crate::keys::rebind(i, chord);
        assert_eq!(crate::keys::chord_text(i), "Ctrl+Shift+F9");
        assert_eq!(crate::keys::lookup(0x78, true, true, false, false), Some(i));
    }
}
