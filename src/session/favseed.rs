// First-run favourites. The favourites store is maguro.json in the Maguro config
// directory. A missing file is the only signal that Maguro has never saved
// favourites. An existing file is left alone, even when its tree is empty, so a
// group or item the user deleted is not put back on the next launch. Settings
// calls `restore_missing_defaults` for that, and only for places that are absent.
// grids.ini and any other application's config are not read or written.

use std::collections::HashSet;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

use super::favtree::{tidy_path, FavNode};
use super::model::Session;
use crate::imports::*;
use crate::{tr, Key, THIS_PC};

/// Shell namespace parsing name of the Recycle Bin. Favourites already store and
/// open `::{CLSID}` places (This PC). The same form navigates to the Recycle Bin.
pub(crate) const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

/// `GetDriveTypeW`: removable media.
const DRIVE_REMOVABLE: u32 = 2;
/// `GetDriveTypeW`: fixed disk.
const DRIVE_FIXED: u32 = 3;

/// One fixed or removable drive letter. `removable` selects the display name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeedDrive {
    pub(crate) letter: char,
    pub(crate) removable: bool,
}

/// Places resolved on this machine. Tests pass their own; nothing here is a user profile path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeedSources {
    /// Filesystem path of the Desktop known folder for the current user. Empty omits the row.
    pub(crate) desktop: String,
    pub(crate) drives: Vec<SeedDrive>,
}

/// `Some(true)` is removable, `Some(false)` is fixed. Network, optical, and missing media stay out.
pub(crate) fn classify_drive(kind: u32) -> Option<bool> {
    match kind {
        DRIVE_REMOVABLE => Some(true),
        DRIVE_FIXED => Some(false),
        _ => None,
    }
}

/// Fixed and removable letters from a drive-type probe, in letter order. A repeated letter is kept once.
pub(crate) fn drives_from_kinds(entries: &[(char, u32)]) -> Vec<SeedDrive> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for &(letter, kind) in entries {
        let Some(removable) = classify_drive(kind) else {
            continue;
        };
        let letter = letter.to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() || !seen.insert(letter) {
            continue;
        }
        out.push(SeedDrive { letter, removable });
    }
    out.sort_by_key(|d| d.letter);
    out
}

fn drive_root(letter: char) -> Option<String> {
    let letter = letter.to_ascii_uppercase();
    letter.is_ascii_alphabetic().then(|| format!("{letter}:\\"))
}

fn drive_caption(removable: bool, letter: char) -> String {
    let kind = if removable {
        tr(Key::RemovableDrive)
    } else {
        tr(Key::LocalDisk)
    };
    format!("{kind} ({letter}:)")
}

/// Identity of a favourite path. Drive roots and trailing slashes collapse, and case does not count.
fn path_key(path: &str) -> String {
    tidy_path(path).to_ascii_lowercase()
}

fn push_place(children: &mut Vec<FavNode>, seen: &mut HashSet<String>, path: &str, name: &str) {
    let path = tidy_path(path);
    let key = path_key(&path);
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    children.push(FavNode::leaf(&path, name, false));
}

fn collect_path_keys(nodes: &[FavNode], seen: &mut HashSet<String>) {
    for n in nodes {
        let key = path_key(&n.path);
        if !key.is_empty() {
            seen.insert(key);
        }
        if !n.children.is_empty() {
            collect_path_keys(&n.children, seen);
        }
    }
}

/// Index path of the first group named `name`. A group on this level wins over one nested deeper.
fn group_path(nodes: &[FavNode], name: &str) -> Option<Vec<usize>> {
    fn walk(nodes: &[FavNode], name: &str, path: &mut Vec<usize>) -> bool {
        for (i, n) in nodes.iter().enumerate() {
            if n.is_group() && n.name == name {
                path.push(i);
                return true;
            }
        }
        for (i, n) in nodes.iter().enumerate() {
            if !n.is_group() {
                continue;
            }
            path.push(i);
            if walk(&n.children, name, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    walk(nodes, name, &mut path).then_some(path)
}

/// Open `path` and append `missing` to that group. Existing children stay in front.
fn append_to_group(nodes: &mut [FavNode], path: &[usize], missing: Vec<FavNode>) {
    let Some((&i, rest)) = path.split_first() else {
        return;
    };
    let Some(node) = nodes.get_mut(i) else {
        return;
    };
    node.open = true;
    if rest.is_empty() {
        node.children.extend(missing);
    } else {
        append_to_group(&mut node.children, rest, missing);
    }
}

/// The Default group: This PC, Desktop, fixed and removable drives, then the Recycle Bin.
/// Names come from the active UI language. Desktop is the path in `sources`, never a baked-in profile path.
pub(crate) fn default_favorite_group(sources: &SeedSources) -> FavNode {
    let mut group = FavNode::group(tr(Key::Default));
    let mut seen = HashSet::new();
    push_place(&mut group.children, &mut seen, THIS_PC, tr(Key::ThisPc));
    let desktop = sources.desktop.trim();
    if !desktop.is_empty() {
        push_place(&mut group.children, &mut seen, desktop, tr(Key::Desktop));
    }
    let mut drives = sources.drives.clone();
    drives.sort_by_key(|d| d.letter.to_ascii_uppercase());
    for drive in &drives {
        let Some(root) = drive_root(drive.letter) else {
            continue;
        };
        let caption = drive_caption(drive.removable, drive.letter.to_ascii_uppercase());
        push_place(&mut group.children, &mut seen, &root, &caption);
    }
    push_place(
        &mut group.children,
        &mut seen,
        RECYCLE_BIN,
        tr(Key::RecycleBinFolder),
    );
    group
}

/// True when this directory has no maguro.json yet. Other files, including grids.ini, do not count.
pub(crate) fn favorites_store_missing(dir: &Path) -> bool {
    !dir.join(crate::store::SETTINGS_FILE).exists()
}

/// Add the Default group on a first start. `settings_present` means maguro.json already existed.
/// A present file is not changed, including one whose favourites the user deleted.
/// A second call on a session that already has favourites does not add another group.
/// Returns whether the group was added. The Settings button does not call this.
pub(crate) fn seed_favorites_if_absent(
    settings_present: bool,
    session: &mut Session,
    sources: &SeedSources,
) -> bool {
    if settings_present || !session.tree.is_empty() || !session.favs.is_empty() {
        return false;
    }
    restore_missing_defaults(session, sources) > 0
}

/// Put back default places that are not already in the tree.
///
/// The rows come from `default_favorite_group`, the same list a first launch builds.
/// A path already stored anywhere (any case, with or without a trailing slash) is left
/// where it is, including its name. Nothing is removed. Existing nodes stay in order.
/// Missing rows are appended, in seed order, to the group that wears the current
/// language's default name, or to a new group at the end when that name is absent.
/// That group and its parents are opened so the side panel can show the new rows.
/// Returns how many leaves were added. An existing tree is not rewritten when
/// nothing is missing. A flat `favs` list with an empty tree is lifted first,
/// the same migration the favourites page already does.
pub(crate) fn restore_missing_defaults(session: &mut Session, sources: &SeedSources) -> usize {
    session.ensure_fav_tree();
    let seed = default_favorite_group(sources);
    let mut have = HashSet::new();
    collect_path_keys(&session.tree, &mut have);
    let missing: Vec<FavNode> = seed
        .children
        .into_iter()
        .filter(|n| {
            let key = path_key(&n.path);
            !key.is_empty() && !have.contains(&key)
        })
        .collect();
    let added = missing.len();
    if added == 0 {
        return 0;
    }
    if let Some(path) = group_path(&session.tree, &seed.name) {
        append_to_group(&mut session.tree, &path, missing);
    } else {
        let mut group = FavNode::group(&seed.name);
        group.children = missing;
        session.tree.push(group);
    }
    session.sync_favs();
    added
}

/// Same as `restore_missing_defaults`, using this machine's desktop and drives.
pub(crate) fn restore_default_favorites(session: &mut Session) -> usize {
    restore_missing_defaults(session, &machine_sources())
}

/// Desktop known folder for this user, plus fixed and removable drives that are present now.
fn machine_sources() -> SeedSources {
    SeedSources {
        desktop: desktop_known_folder(),
        drives: probed_drives(),
    }
}

/// Seed the in-memory session. The caller has already checked that maguro.json is absent.
pub(crate) fn seed_first_run_favorites(session: &mut Session) {
    let _ = seed_favorites_if_absent(false, session, &machine_sources());
}

/// Filesystem path of `FOLDERID_Desktop`. Empty when the shell cannot name it.
fn desktop_known_folder() -> String {
    unsafe {
        let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) else {
            return String::new();
        };
        let path = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        path
    }
}

fn probed_drives() -> Vec<SeedDrive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut entries = Vec::new();
    for i in 0..26u32 {
        if mask >> i & 1 == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = HSTRING::from(format!("{letter}:\\"));
        let kind = unsafe { GetDriveTypeW(&root) };
        entries.push((letter, kind));
    }
    drives_from_kinds(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::UI;
    use std::sync::atomic::Ordering;

    fn sources() -> SeedSources {
        SeedSources {
            desktop: r"D:\Desk".into(),
            drives: vec![
                SeedDrive {
                    letter: 'd',
                    removable: true,
                },
                SeedDrive {
                    letter: 'C',
                    removable: false,
                },
                SeedDrive {
                    letter: 'C',
                    removable: false,
                },
                SeedDrive {
                    letter: 'E',
                    removable: true,
                },
            ],
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("maguro-favseed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn with_ui(slot: u8, f: impl FnOnce()) {
        let prev = UI.load(Ordering::Relaxed);
        UI.store(slot, Ordering::Relaxed);
        f();
        UI.store(prev, Ordering::Relaxed);
    }

    #[test]
    fn drive_probe_keeps_fixed_and_removable_in_letter_order() {
        let drives = drives_from_kinds(&[
            ('Z', 0),
            ('d', DRIVE_REMOVABLE),
            ('C', DRIVE_FIXED),
            ('E', 4),
            ('F', 5),
            ('B', 1),
            ('G', 6),
            ('c', DRIVE_FIXED),
        ]);
        assert_eq!(
            drives,
            vec![
                SeedDrive {
                    letter: 'C',
                    removable: false
                },
                SeedDrive {
                    letter: 'D',
                    removable: true
                },
            ]
        );
    }

    #[test]
    fn first_run_seeds_once_with_localized_names() {
        with_ui(0, || {
            let src = sources();
            let mut session = Session::default();
            assert!(seed_favorites_if_absent(false, &mut session, &src));
            assert!(
                !seed_favorites_if_absent(false, &mut session, &src),
                "a second seed must not add another group"
            );
            assert_eq!(session.tree.len(), 1);
            let group = &session.tree[0];
            assert!(group.is_group() && group.open);
            assert_eq!(group.name, "Default");
            let paths: Vec<&str> = group.children.iter().map(|n| n.path.as_str()).collect();
            assert_eq!(
                paths,
                vec![THIS_PC, r"D:\Desk", r"C:\", r"D:\", r"E:\", RECYCLE_BIN]
            );
            let names: Vec<&str> = group.children.iter().map(|n| n.name.as_str()).collect();
            assert_eq!(
                names,
                vec![
                    "This PC",
                    "Desktop",
                    "Local disk (C:)",
                    "Removable drive (D:)",
                    "Removable drive (E:)",
                    "Recycle Bin",
                ]
            );
            assert!(group.children.iter().all(|n| !n.file));
            assert!(session.favs.iter().any(|f| f.contains(THIS_PC)));
            assert!(session.favs.iter().any(|f| f.contains(RECYCLE_BIN)));
        });

        with_ui(1, || {
            let group = default_favorite_group(&sources());
            assert_eq!(group.name, "기본");
            assert_eq!(group.children[0].name, "내 PC");
            assert_eq!(group.children[1].name, "바탕 화면");
            assert_eq!(group.children[2].name, "로컬 디스크 (C:)");
            assert_eq!(group.children[3].name, "이동식 드라이브 (D:)");
            assert_eq!(group.children[5].name, "휴지통");
        });

        with_ui(2, || {
            let group = default_favorite_group(&sources());
            assert_eq!(group.name, "既定");
            assert_eq!(group.children[0].name, "PC");
            assert_eq!(group.children[1].name, "デスクトップ");
            assert_eq!(group.children[2].name, "ローカル ディスク (C:)");
            assert_eq!(group.children[3].name, "リムーバブル ドライブ (D:)");
            assert_eq!(group.children[5].name, "ごみ箱");
        });
    }

    #[test]
    fn blank_desktop_is_omitted_and_the_path_is_the_one_given() {
        let src = SeedSources {
            desktop: "  ".into(),
            drives: vec![SeedDrive {
                letter: 'C',
                removable: false,
            }],
        };
        let group = default_favorite_group(&src);
        let paths: Vec<&str> = group.children.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, vec![THIS_PC, r"C:\", RECYCLE_BIN]);
        let src = SeedSources {
            desktop: r"E:\Work\Desk".into(),
            drives: Vec::new(),
        };
        let group = default_favorite_group(&src);
        assert_eq!(group.children[1].path, r"E:\Work\Desk");
        assert!(!group.children.iter().any(|n| n.path.contains("Users")));
    }

    #[test]
    fn first_run_file_seeds_once_and_a_reload_does_not_add_another_group() {
        let dir = scratch("once");
        assert!(favorites_store_missing(&dir));
        std::fs::write(dir.join("grids.ini"), "fav=Z:\\other-app\n").unwrap();
        let grids = std::fs::read(dir.join("grids.ini")).unwrap();
        let src = sources();
        let mut session = Session::default();
        assert!(seed_favorites_if_absent(
            !favorites_store_missing(&dir),
            &mut session,
            &src
        ));
        assert_eq!(std::fs::read(dir.join("grids.ini")).unwrap(), grids);

        let body = serde_json::json!({
            "version": 1,
            "favorites_tree": crate::tree_to_values(&session.tree),
        });
        let text = serde_json::to_string_pretty(&body).unwrap();
        std::fs::write(dir.join(crate::store::SETTINGS_FILE), &text).unwrap();
        assert!(!favorites_store_missing(&dir));

        let mut again = crate::store::try_load_at(&dir).unwrap();
        assert!(!seed_favorites_if_absent(true, &mut again, &src));
        assert_eq!(again.tree.len(), 1);
        assert_eq!(again.tree[0].name, session.tree[0].name);
        assert_eq!(
            again.tree[0]
                .children
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>(),
            session.tree[0]
                .children
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(std::fs::read(dir.join("grids.ini")).unwrap(), grids);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_config_is_not_seeded_and_the_file_stays() {
        let dir = scratch("existing");
        let text = "{\n  \"version\": 1,\n  \"ctx_win11_pack_migrated\": true,\n  \"favorites_tree\": [{\"name\": \"Work\", \"children\": [{\"path\": \"D:\\\\Photos\", \"name\": \"Photos\"}]}],\n  \"lang\": \"en\"\n}\n";
        let path = dir.join(crate::store::SETTINGS_FILE);
        std::fs::write(&path, text).unwrap();
        std::fs::write(dir.join("grids.ini"), "[keep]\n").unwrap();
        let before = std::fs::read(&path).unwrap();
        let grids = std::fs::read(dir.join("grids.ini")).unwrap();
        let mut session = crate::store::try_load_at(&dir).unwrap();
        assert!(!favorites_store_missing(&dir));
        assert!(!seed_favorites_if_absent(true, &mut session, &sources()));
        assert_eq!(session.tree.len(), 1);
        assert_eq!(session.tree[0].name, "Work");
        assert_eq!(session.tree[0].children.len(), 1);
        assert_eq!(session.tree[0].children[0].path, r"D:\Photos");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(dir.join("grids.ini")).unwrap(), grids);

        // An older file that never had a favourites key is still an existing store.
        let older = scratch("older");
        let older_text = "{\"version\":1,\"lang\":\"ko\",\"ctx_win11_pack_migrated\":true}\n";
        std::fs::write(older.join(crate::store::SETTINGS_FILE), older_text).unwrap();
        let mut older_session = crate::store::try_load_at(&older).unwrap();
        assert!(older_session.tree.is_empty());
        assert!(!seed_favorites_if_absent(
            true,
            &mut older_session,
            &sources()
        ));
        assert!(older_session.tree.is_empty());
        assert_eq!(
            std::fs::read_to_string(older.join(crate::store::SETTINGS_FILE)).unwrap(),
            older_text
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&older);
    }

    #[test]
    fn deleted_group_and_deleted_item_stay_deleted() {
        let src = sources();
        let dir = scratch("deleted-group");
        std::fs::write(
            dir.join(crate::store::SETTINGS_FILE),
            "{\"version\":1,\"ctx_win11_pack_migrated\":true,\"favorites_tree\":[]}\n",
        )
        .unwrap();
        let before = std::fs::read(dir.join(crate::store::SETTINGS_FILE)).unwrap();
        let mut session = crate::store::try_load_at(&dir).unwrap();
        assert!(session.tree.is_empty());
        assert!(!seed_favorites_if_absent(true, &mut session, &src));
        assert!(session.tree.is_empty() && session.favs.is_empty());
        assert_eq!(
            std::fs::read(dir.join(crate::store::SETTINGS_FILE)).unwrap(),
            before
        );

        // The user removed Desktop and the drives and left This PC and the Recycle Bin.
        let partial = scratch("deleted-item");
        let kept = serde_json::json!({
            "version": 1,
            "ctx_win11_pack_migrated": true,
            "favorites_tree": [{
                "name": "Default",
                "children": [
                    {"name": "This PC", "path": THIS_PC},
                    {"name": "Recycle Bin", "path": RECYCLE_BIN}
                ]
            }]
        });
        std::fs::write(
            partial.join(crate::store::SETTINGS_FILE),
            serde_json::to_string_pretty(&kept).unwrap(),
        )
        .unwrap();
        let partial_before = std::fs::read(partial.join(crate::store::SETTINGS_FILE)).unwrap();
        let mut partial_session = crate::store::try_load_at(&partial).unwrap();
        assert!(!seed_favorites_if_absent(true, &mut partial_session, &src));
        let paths: Vec<&str> = partial_session.tree[0]
            .children
            .iter()
            .map(|n| n.path.as_str())
            .collect();
        assert_eq!(paths, vec![THIS_PC, RECYCLE_BIN]);
        assert_eq!(
            std::fs::read(partial.join(crate::store::SETTINGS_FILE)).unwrap(),
            partial_before
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&partial);
    }

    fn child_paths(node: &FavNode) -> Vec<&str> {
        node.children.iter().map(|n| n.path.as_str()).collect()
    }

    #[test]
    fn restore_on_an_empty_tree_matches_the_first_run_group() {
        with_ui(0, || {
            let src = sources();
            let mut session = Session::default();
            let expect = default_favorite_group(&src);
            assert_eq!(restore_missing_defaults(&mut session, &src), expect.children.len());
            assert_eq!(session.tree, vec![expect]);
            let tree = session.tree.clone();
            let favs = session.favs.clone();
            assert_eq!(restore_missing_defaults(&mut session, &src), 0);
            assert_eq!(session.tree, tree);
            assert_eq!(session.favs, favs);
        });
    }

    #[test]
    fn restore_adds_only_missing_and_keeps_existing_order() {
        with_ui(0, || {
            let src = sources();
            // Same places as the seed, one drive written in another case. Nothing is missing.
            let mut present = FavNode::group("Default");
            present.open = false;
            present.children = default_favorite_group(&src).children;
            present.children[2].path = r"c:\".into();
            let mut session = Session::default();
            session.tree = vec![present];
            let before = session.tree.clone();
            assert_eq!(restore_missing_defaults(&mut session, &src), 0);
            assert_eq!(session.tree, before);
            assert!(!session.tree[0].open);
            assert_eq!(session.tree[0].children[2].path, r"c:\");

            let mut work = FavNode::group("Work");
            work.children.push(FavNode::leaf(r"D:\Photos", "Photos", false));
            // Trailing slash still counts as the desktop path. The stored text stays.
            work.children.push(FavNode::leaf(r"D:\Desk\", "Desk copy", false));
            let mut partial = FavNode::group("Default");
            partial.open = false;
            partial.children.push(FavNode::leaf(RECYCLE_BIN, "Bin", false));
            partial.children.push(FavNode::leaf("c:/", "Mine", false));
            partial.children.push(FavNode::leaf(THIS_PC, "PC", false));
            session.tree = vec![work, partial];
            session.favs.clear();
            assert_eq!(restore_missing_defaults(&mut session, &src), 2);

            assert_eq!(session.tree.len(), 2);
            assert_eq!(session.tree[0].name, "Work");
            assert_eq!(child_paths(&session.tree[0]), vec![r"D:\Photos", r"D:\Desk\"]);
            assert_eq!(session.tree[0].children[1].name, "Desk copy");
            let group = &session.tree[1];
            assert!(group.open);
            assert_eq!(
                child_paths(group),
                vec![RECYCLE_BIN, "c:/", THIS_PC, r"D:\", r"E:\"]
            );
            assert_eq!(group.children[0].name, "Bin");
            assert_eq!(group.children[1].name, "Mine");
            assert_eq!(group.children[2].name, "PC");
            assert_eq!(group.children[3].name, "Removable drive (D:)");
            assert_eq!(group.children[4].name, "Removable drive (E:)");
            assert!(session.favs[0].starts_with(r"D:\Photos"));
            let mine = session.favs.iter().position(|f| f.starts_with("c:/")).unwrap();
            let added = session.favs.iter().position(|f| f.contains("Removable drive (D:)")).unwrap();
            assert!(mine < added);
            let tree = session.tree.clone();
            let favs = session.favs.clone();
            assert_eq!(restore_missing_defaults(&mut session, &src), 0);
            assert_eq!(session.tree, tree);
            assert_eq!(session.favs, favs);
        });
    }

    #[test]
    fn restore_appends_a_group_when_the_seed_name_is_absent() {
        with_ui(0, || {
            let src = sources();
            let mut places = FavNode::group("Places");
            places.children.push(FavNode::leaf(THIS_PC, "PC", false));
            places.children.push(FavNode::leaf(r"E:\Mine", "Mine", false));
            let mut session = Session::default();
            session.tree = vec![places];
            let n = restore_missing_defaults(&mut session, &src);
            // This PC is already stored. Desktop, three drives, and the Recycle Bin are not.
            assert_eq!(n, 5);
            assert_eq!(session.tree.len(), 2);
            assert_eq!(session.tree[0].name, "Places");
            assert_eq!(child_paths(&session.tree[0]), vec![THIS_PC, r"E:\Mine"]);
            assert_eq!(session.tree[0].children[0].name, "PC");
            let added = &session.tree[1];
            assert_eq!(added.name, "Default");
            assert!(added.open);
            assert_eq!(
                child_paths(added),
                vec![r"D:\Desk", r"C:\", r"D:\", r"E:\", RECYCLE_BIN]
            );

            // A nested group with the seed name receives the rows. No second group is created.
            let mut inner = FavNode::group("Default");
            inner.open = false;
            inner.children.push(FavNode::leaf(THIS_PC, "PC", false));
            let mut jobs = FavNode::group("Jobs");
            jobs.open = false;
            jobs.children.push(inner);
            session.tree = vec![jobs];
            assert!(restore_missing_defaults(&mut session, &src) > 0);
            assert_eq!(session.tree.len(), 1);
            assert!(session.tree[0].open);
            assert_eq!(session.tree[0].name, "Jobs");
            assert_eq!(session.tree[0].children.len(), 1);
            let inner = &session.tree[0].children[0];
            assert!(inner.open);
            assert_eq!(inner.children[0].path, THIS_PC);
            assert_eq!(inner.children[0].name, "PC");
            assert_eq!(inner.children[1].path, r"D:\Desk");

            // A flat list is lifted into the tree first, then only the missing places are added.
            session.tree.clear();
            session.favs = vec![r"C:\".into(), r"D:\Photos".into()];
            assert_eq!(restore_missing_defaults(&mut session, &src), 5);
            assert_eq!(session.tree[0].path, r"C:\");
            assert_eq!(session.tree[1].path, r"D:\Photos");
            assert!(session.tree[2].is_group());
            assert!(!session.tree[2].children.iter().any(|n| path_key(&n.path) == path_key(r"C:\")));
            assert_eq!(session.tree[2].children[0].path, THIS_PC);
        });
    }
}
