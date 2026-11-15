//! Important-file marks, separate from maguro.json.
//!
//! The file is marks.json in the config folder (roaming Maguro, or the exe folder when
//! portable.txt is present). This module never opens maguro.json.

pub(crate) mod follow;
mod list;
mod list_win;
mod resolve;
mod store;
mod toggle;

pub(crate) use list_win::{alive as list_open, captures as captures_list, close as close_list, on_key as on_list_key, open as open_list, refresh as refresh_list, restyle as restyle_list};

#[allow(unused_imports)]
pub(crate) use store::{read_file_identity, FileIdentity, MarksLock, Store, StoreError, CAP, FILE_NAME};
#[allow(unused_imports)]
pub(crate) use toggle::{apply, count_ok, eligible, menu_key, menu_verb, MenuVerb, Outcome, Target};

use std::sync::Mutex;
use std::time::Duration;

use crate::imports::*;

static LIVE: Mutex<Option<Store>> = Mutex::new(None);

fn live() -> &'static Mutex<Option<Store>> {
    &LIVE
}

/// Load marks.json from the settings folder. A missing file is an empty store.
pub(crate) fn load_live() {
    let dir = crate::settings::store::config_dir();
    let store = Store::load_in(&dir, MarksLock::Os);
    *live().lock().unwrap_or_else(|e| e.into_inner()) = Some(store);
    follow::arm_sweep();
}

/// Run `f` against the live store and save when it reports a change.
/// The overlay refresh is posted to the UI thread; `f` must not call into the UI.
pub(crate) fn commit_edit(f: impl FnOnce(&mut Store) -> bool) {
    let changed = {
        let mut g = live().lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = g.as_mut() else { return };
        let changed = f(store);
        if changed {
            let _ = store.save();
        }
        changed
    };
    if changed {
        follow::poke_ui();
    }
}

pub(crate) fn count_in_folder(folder: &str) -> usize {
    let g = live().lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map(|s| count_ok(s, folder)).unwrap_or(0)
}

/// Read the live store. The lock is not held across `f`'s further calls.
pub(crate) fn with_store<R>(f: impl FnOnce(&Store) -> R) -> Option<R> {
    let g = live().lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map(f)
}

unsafe fn resolve_items(items: &[(String, bool)]) -> Vec<Target> {
    items.iter().map(|(p, file)| resolve::from_parsing(p, !*file)).collect()
}

fn verb_of(targets: &[Target]) -> MenuVerb {
    let g = live().lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some(s) => menu_verb(targets, |p| s.get(p).is_some()),
        None => menu_verb(targets, |_| false),
    }
}

/// Label and whether the command can run. The chord is the current binding.
pub(crate) unsafe fn menu_parts(items: &[(String, bool)]) -> (String, bool) {
    let targets = resolve_items(items);
    let verb = verb_of(&targets);
    let name = crate::tr(menu_key(verb));
    let chord = crate::keys::index("file.mark").map(crate::keys::chord_text).unwrap_or_default();
    let label = if chord.is_empty() { name.to_string() } else { format!("{name}\t{chord}") };
    (label, !matches!(verb, MenuVerb::Disabled))
}

pub(crate) unsafe fn toggle_items(items: &[(String, bool)]) {
    let targets = resolve_items(items);
    let out = {
        let mut g = live().lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = g.as_mut() else { return };
        apply(store, &targets)
    };
    after_toggle(out);
}

pub(crate) unsafe fn toggle_pane(pane: usize) {
    toggle_items(&crate::selected_items(pane));
}

fn after_toggle(out: Outcome) {
    match out {
        Outcome::Full { .. } => {
            crate::app::app(|a| {
                a.note = Some((crate::tr(crate::Key::ImportantMarksFull).to_string(), Instant::now() + Duration::from_secs(5)));
                a.status_key.clear();
            });
            unsafe { let _ = MessageBeep(MB_ICONWARNING); }
        }
        Outcome::Blocked | Outcome::Empty => {
            unsafe { let _ = MessageBeep(MB_ICONWARNING); }
        }
        Outcome::Marked(_) | Outcome::Unmarked(_) => {
            crate::app::app(|a| a.status_key.clear());
        }
    }
    unsafe {
        crate::update_status();
        crate::pane::marks_overlay::on_marks_changed();
    }
}
