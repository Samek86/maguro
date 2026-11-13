//! Filesystem folders and drives can open in Maguro. Win+E, the taskbar and
//! virtual folders stay with Explorer: only the Directory and Drive classes are
//! registered, under HKCU\Software\Classes, and a shell: or CLSID path that still
//! arrives is handed back to explorer.exe.

mod apply;
mod handoff;
mod plan;

pub(crate) use apply::{open_in_explorer, reload_shown, repair_moved_exe, set_registered, shown_on, take_launch_args};
#[cfg(test)]
pub(crate) use apply::set_shown_for_test;
pub(crate) use handoff::{intent_elapsed_ms, note_activate, noted_pane, parent_pid};
#[allow(unused_imports)]
pub(crate) use plan::{
    handoff_action, handoff_text, link_file, list_activation, list_open, menu_open, menu_open_item, navigable, open_verb, parse_handoff, tree_activate, ActKind, Handoff, ListAct, OpenIn,
    OpenWhere,
};
