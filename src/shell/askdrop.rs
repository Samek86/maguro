// "Always ask" (dropm == 2) shows Copy / Move / Cancel for a file drop inside a pane,
// on the folder tree, and on the middle of a favourite folder. Other drop settings stay
// with the shell (or, for favourites, with the link they already insert).
//
// The copy or move itself is `start_copy` / `start_drop_move` on a worker thread.
// A deferred MoveItem must not report DROPEFFECT_MOVE from Drop. Some shell drag
// sources delete the originals as soon as they see MOVE, before our worker has
// settled and called MoveItem — the file then vanishes. Report COPY instead so the
// source keeps the files; MoveItem removes them after Drop returns. Copy does not
// need that wait. `mark_move_performed` stays for callers that finish the move
// before Drop returns.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Ole::ReleaseStgMedium;
use windows::Win32::UI::Controls::{TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOGCONFIG_1, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION};

use crate::imports::*;
use crate::pwstr;
use crate::app::*;
use crate::i18n::*;
use crate::pane::*;
use crate::session::*;
use super::fileop::{paste_mode, start_copy, start_drop_move};

/// What Drop returns to the source. A move is reported as a copy when the source was not
/// told that the move already happened.
pub(crate) fn reported_effect(mv: bool, performed: bool) -> u32 {
    if mv && performed { DROPEFFECT_MOVE.0 } else { DROPEFFECT_COPY.0 }
}

/// Where a pane drop lands. `child` is set only when the row under the cursor is a local
/// directory (a share is not stat'd, so it stays `None` and the current folder is used).
pub(crate) fn drop_folder(current: &str, child: Option<&str>) -> Option<String> {
    if let Some(path) = child.filter(|p| is_fs(p)) {
        return Some((*path).to_string());
    }
    is_fs(current).then(|| current.to_string())
}

/// `C:` or `\\server\share`, folded for comparing. `None` for anything else.
pub(crate) fn volume_root(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Some(path[..2].to_ascii_uppercase());
    }
    let rest = path.strip_prefix(r"\\")?;
    if rest.starts_with(['?', '.']) {
        return None;
    }
    let mut parts = rest.split(['\\', '/']).filter(|p| !p.is_empty());
    let server = parts.next()?;
    let share = parts.next()?;
    Some(format!(r"\\{}\{}", server.to_lowercase(), share.to_lowercase()))
}

/// Explorer moves on the same drive and copies across drives.
pub(crate) fn same_volume(srcs: &[String], dst: &str) -> bool {
    let Some(d) = volume_root(dst) else { return false };
    !srcs.is_empty() && srcs.iter().all(|s| volume_root(s).as_deref() == Some(d.as_str()))
}

/// A folder dropped on itself or into its own subfolder. Explorer refuses that drop.
pub(crate) fn drops_into_itself(srcs: &[String], dst: &str) -> bool {
    let d = dst.trim_end_matches(['\\', '/']).to_lowercase();
    srcs.iter().any(|s| {
        let s = s.trim_end_matches(['\\', '/']).to_lowercase();
        !s.is_empty() && (d == s || d.starts_with(&format!("{s}\\")))
    })
}

/// Copy (`Some(false)`) or move (`Some(true)`) for a plain left-button file drop that Maguro runs
/// on its worker. `None` leaves the drop with the shell (a link, or nothing the source allows).
/// Ctrl copies and Shift moves. Otherwise Always copy (dropm 1) copies and Windows rules move on
/// the same drive. `shell_link` is the shell's own default, a shortcut, for a plain drop.
pub(crate) fn drop_choice(dropm: u8, ctrl: bool, shift: bool, alt: bool, allowed: u32, same_volume: bool, shell_link: bool) -> Option<bool> {
    if alt || (ctrl && shift) {
        return None;
    }
    if shell_link && !ctrl && !shift {
        return None;
    }
    let want_move = if shift {
        true
    } else if ctrl || dropm == 1 {
        false
    } else {
        same_volume
    };
    let can_copy = allowed & DROPEFFECT_COPY.0 != 0;
    let can_move = allowed & DROPEFFECT_MOVE.0 != 0;
    match (want_move, can_move, can_copy) {
        (true, true, _) => Some(true),
        (true, false, true) if !shift => Some(false),
        (false, _, true) => Some(false),
        (false, true, false) if !ctrl => Some(true),
        _ => None,
    }
}

/// What a non-asked file drop does once the copy or move is chosen.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum DropPlan {
    /// Run on the worker.
    Run,
    /// Nothing to do: a move into the folder the files already sit in, or a folder into itself.
    Refuse,
    /// Mixed sources: the shell.
    Shell,
}

pub(crate) fn drop_plan(srcs: &[String], dst: &str, mv: bool) -> DropPlan {
    if drops_into_itself(srcs, dst) {
        return DropPlan::Refuse;
    }
    match paste_mode(srcs, dst) {
        Some(true) if mv => DropPlan::Refuse,
        Some(_) => DropPlan::Run,
        None => DropPlan::Shell,
    }
}

/// Middle of a favourite folder row, and only while the drop action is Always ask.
/// Edges stay an insert. Groups, files, and shell places are not copy targets.
pub(crate) fn fav_into_folder(dropm: u8, group: bool, file: bool, path: &str, y: i32, rh: i32) -> bool {
    dropm == 2 && !group && !file && is_fs(path) && rh > 0 && y >= rh / 4 && y < rh * 3 / 4
}

/// Some(true) is Move, Some(false) is Copy, None is Cancel.
pub(crate) unsafe fn ask_copy_or_move(owner: HWND) -> Option<bool> {
    let title = HSTRING::from("Maguro");
    let head = HSTRING::from(tr(crate::Key::CopyOrMoveTheseItems));
    let copy = HSTRING::from(tr(crate::Key::Copy));
    let mv = HSTRING::from(tr(crate::Key::Move));
    let cancel = HSTRING::from(tr(crate::Key::Cancel));
    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: 100, pszButtonText: PCWSTR(copy.as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: 101, pszButtonText: PCWSTR(mv.as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: 2, pszButtonText: PCWSTR(cancel.as_ptr()) },
    ];
    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        hInstance: HINSTANCE::default(),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: Default::default(),
        pszWindowTitle: PCWSTR(title.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: PCWSTR::null() },
        pszMainInstruction: PCWSTR(head.as_ptr()),
        pszContent: PCWSTR::null(),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 100,
        cRadioButtons: 0,
        pRadioButtons: std::ptr::null(),
        nDefaultRadioButton: 0,
        pszVerificationText: PCWSTR::null(),
        pszExpandedInformation: PCWSTR::null(),
        pszExpandedControlText: PCWSTR::null(),
        pszCollapsedControlText: PCWSTR::null(),
        Anonymous2: TASKDIALOGCONFIG_1 { hFooterIcon: HICON::default() },
        pszFooter: PCWSTR::null(),
        pfCallback: None,
        lpCallbackData: 0,
        cxWidth: 0,
    };
    let mut pressed = 0i32;
    if TaskDialogIndirect(&cfg, Some(&mut pressed), None, None).is_err() {
        return None;
    }
    match pressed {
        100 => Some(false),
        101 => Some(true),
        _ => None,
    }
}

/// Tell the source the move already happened, so it does not delete the originals.
#[allow(dead_code)] // kept for a sync move that finishes before Drop returns
pub(crate) unsafe fn mark_move_performed(obj: &IDataObject) -> bool {
    set_drop_dword(obj, w!("Performed DropEffect"), DROPEFFECT_MOVE.0)
}

/// Store one DWORD clipboard format ("Performed DropEffect", "Paste Succeeded") on a data object.
pub(crate) unsafe fn set_drop_dword(obj: &IDataObject, name: PCWSTR, value: u32) -> bool {
    let fmt = RegisterClipboardFormatW(name);
    if fmt == 0 {
        return false;
    }
    let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, 4) else { return false };
    let p = GlobalLock(mem) as *mut u32;
    if p.is_null() {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    *p = value;
    let _ = GlobalUnlock(mem);
    let etc = FORMATETC {
        cfFormat: fmt as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: mem },
        pUnkForRelease: std::mem::ManuallyDrop::new(None),
    };
    if obj.SetData(&etc, &medium, true).is_ok() {
        true
    } else {
        let _ = GlobalFree(Some(mem));
        false
    }
}

/// Filesystem paths in the drop. Empty when the drop is missing, mixed, or not files:
/// the caller then leaves the drop with the shell.
pub(crate) unsafe fn drop_fs_paths(d: &Ref<IDataObject>) -> Vec<String> {
    let paths = hdrop_paths(d);
    let paths = if !paths.is_empty() { paths } else { shell_paths(d) };
    if paths.is_empty() || paths.iter().any(|p| !is_fs(p)) {
        Vec::new()
    } else {
        paths
    }
}

unsafe fn hdrop_paths(d: &Ref<IDataObject>) -> Vec<String> {
    let Some(obj) = d.as_ref() else { return Vec::new() };
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = obj.GetData(&fmt) else { return Vec::new() };
    let drop = HDROP(medium.u.hGlobal.0);
    let mut paths = Vec::new();
    for k in 0..DragQueryFileW(drop, u32::MAX, None) {
        let need = DragQueryFileW(drop, k, None) as usize;
        let mut buf = vec![0u16; need + 1];
        let got = DragQueryFileW(drop, k, Some(&mut buf)) as usize;
        let path = String::from_utf16_lossy(buf.get(..got).unwrap_or(&[]));
        if !path.is_empty() {
            paths.push(path);
        }
    }
    ReleaseStgMedium(&mut medium);
    paths
}

unsafe fn shell_paths(d: &Ref<IDataObject>) -> Vec<String> {
    let Some(arr) = drop_items(d) else { return Vec::new() };
    let n = arr.GetCount().unwrap_or(0).min(10_000);
    let mut out = Vec::new();
    for k in 0..n {
        let Ok(it) = arr.GetItemAt(k) else { return Vec::new() };
        let Ok(name) = it.GetDisplayName(SIGDN_FILESYSPATH) else { return Vec::new() };
        let path = pwstr(name);
        if path.is_empty() {
            return Vec::new();
        }
        out.push(path);
    }
    out
}

/// Start the file operation and the effect the source should see.
/// Copy and move both go through `start_transfer`, which calls `decide_fast`.
pub(crate) unsafe fn run_asked(obj: &IDataObject, srcs: Vec<String>, dst: String, mv: bool) -> u32 {
    let _ = obj; // deferred move must not stamp Performed DropEffect from Drop
    let started = if mv {
        // MoveItem inside Drop cannot see the item the drag source still holds.
        start_drop_move(srcs, dst)
    } else {
        let rename = paste_mode(&srcs, &dst) == Some(true);
        start_copy(srcs, dst, rename, false)
    };
    if !started {
        return DROPEFFECT_NONE.0;
    }
    // Deferred MoveItem: always COPY back to the source. Reporting MOVE here made
    // the shell delete before settle+MoveItem, so the file disappeared.
    reported_effect(false, false)
}

/// Windows the shell registers a drop target on. OLE delivers the drop to the deepest one,
/// which is the list (or the items-view surface), not the DefView window `GetWindow` returns.
pub(crate) fn shell_drop_class(class: &str) -> bool {
    matches!(class, "SHELLDLL_DefView" | "SysListView32" | "DirectUIHWND" | "SysTreeView32")
}

enum AskWhere {
    Pane(Arc<AtomicUsize>),
    Tree,
}

impl Clone for AskWhere {
    fn clone(&self) -> Self {
        match self {
            AskWhere::Pane(slot) => AskWhere::Pane(slot.clone()),
            AskWhere::Tree => AskWhere::Tree,
        }
    }
}

const DROP_SUB: usize = 8;

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The drop target OLE currently has on `hwnd` is ours when the mark matches that pointer.
/// The pointer is only compared, never called.
unsafe fn already_ours(hwnd: HWND) -> bool {
    let cur = GetPropW(hwnd, w!("OleDropTargetInterface")).0;
    !cur.is_null() && cur == GetPropW(hwnd, w!("MaguroAskDrop")).0
}

unsafe fn wrap_window(hwnd: HWND, fallback: &IDropTarget, where_: AskWhere) {
    if hwnd.0.is_null() || already_ours(hwnd) {
        return;
    }
    // Drop the shell's registration. Highlight and non-ask drops go to the view's own
    // target, which is `fallback` (the shell view), not whatever pointer the property held.
    let _ = RevokeDragDrop(hwnd);
    let dt: IDropTarget = AskDrop {
        hwnd,
        inner: fallback.clone(),
        where_,
        srcs: RefCell::new(Vec::new()),
        basket: Cell::new(false),
        allowed: Cell::new(0),
        shell_effect: Cell::new(0),
        right: Cell::new(false),
    }
    .into();
    if RegisterDragDrop(hwnd, &dt).is_err() {
        let _ = RegisterDragDrop(hwnd, fallback);
        return;
    }
    let now = GetPropW(hwnd, w!("OleDropTargetInterface"));
    if !now.0.is_null() {
        let _ = SetPropW(hwnd, w!("MaguroAskDrop"), Some(now));
    }
}

unsafe fn wrap_family(root: HWND, fallback: &IDropTarget, where_: &AskWhere) {
    if root.0.is_null() {
        return;
    }
    let mut wins = vec![root];
    extern "system" fn collect(h: HWND, out: LPARAM) -> BOOL {
        crate::crash::catch_bool(std::panic::AssertUnwindSafe(|| unsafe {
            let want = shell_drop_class(&class_name(h)) || !GetPropW(h, w!("OleDropTargetInterface")).0.is_null();
            if want {
                (*(out.0 as *mut Vec<HWND>)).push(h);
            }
            true.into()
        }))
    }
    let _ = EnumChildWindows(Some(root), Some(collect), LPARAM(&mut wins as *mut Vec<HWND> as isize));
    for hwnd in wins {
        wrap_window(hwnd, fallback, where_.clone());
    }
}

unsafe fn pane_from(mut hwnd: HWND) -> Option<usize> {
    for _ in 0..8 {
        if let Some(i) = pane_of(hwnd) {
            return Some(i);
        }
        hwnd = GetParent(hwnd).unwrap_or_default();
        if hwnd.0.is_null() {
            return None;
        }
    }
    None
}

unsafe extern "system" fn drop_watch_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, a1: usize, a2: usize) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { drop_watch_inner(h, msg, wp, lp, a1, a2) }))
}

/// A new list child means Explorer is about to register its own target on it. Wrap again
/// once that create has returned.
unsafe fn drop_watch_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _a1: usize, _a2: usize) -> LRESULT {
    let r = DefSubclassProc(h, msg, wp, lp);
    if msg == WM_PARENTNOTIFY && wp.0 & 0xFFFF == WM_CREATE as usize {
        if let Some(i) = pane_from(h) {
            schedule_pane_drop(i);
        }
    }
    r
}

#[implement(IDropTarget)]
pub(crate) struct AskDrop {
    hwnd: HWND,
    inner: IDropTarget,
    where_: AskWhere,
    /// Filesystem paths seen on DragEnter. Kept so Drop does not have to read the data
    /// object a second time, and so DragOver can allow the drop.
    srcs: RefCell<Vec<String>>,
    /// This drag started in the file basket. Copy by default, on our worker, not the shell's.
    basket: Cell<bool>,
    /// Effects the source allows, from the last DragEnter or DragOver.
    allowed: Cell<u32>,
    /// The shell target's own answer to the last DragEnter or DragOver.
    shell_effect: Cell<u32>,
    /// A right-button drag: the shell shows its Copy / Move / Shortcut menu.
    right: Cell<bool>,
}

impl AskDrop {
    unsafe fn forward_enter(&self, d: &Ref<IDataObject>, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) {
        if let Some(obj) = d.as_ref() {
            let _ = self.inner.DragEnter(obj, key, *pt, effect);
        }
    }

    /// Remember what the source allows (`effect` on the way in) before the shell answers.
    unsafe fn note_allowed(&self, effect: *mut DROPEFFECT) {
        if !effect.is_null() {
            self.allowed.set((*effect).0);
        }
    }

    unsafe fn note_shell(&self, effect: *mut DROPEFFECT) {
        if !effect.is_null() {
            self.shell_effect.set((*effect).0);
        }
    }

    /// Always copy: the cursor says copy for a plain drop the worker will copy.
    unsafe fn offer_always_copy(&self, key: MODIFIERKEYS_FLAGS, effect: *mut DROPEFFECT) {
        if effect.is_null() || self.right.get() || self.srcs.borrow().is_empty() || app(|a| a.s.dropm) != 1 {
            return;
        }
        let (ctrl, shift, alt) = crate::basket::drag_modifiers(key);
        if shift || alt || ctrl {
            return;
        }
        let shell = (*effect).0;
        if shell & (DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0) != 0 && self.allowed.get() & DROPEFFECT_COPY.0 != 0 {
            *effect = DROPEFFECT_COPY;
        }
    }

    /// The folder a plain file drop lands in when Maguro can run it. `None` is the shell's drop.
    unsafe fn worker_target(&self, pt: &POINTL) -> Option<String> {
        bind_hwnd(self.hwnd);
        match &self.where_ {
            AskWhere::Pane(slot) => {
                let i = slot.load(Ordering::SeqCst);
                let cur = app(|a| a.s.paths.get(i).cloned()).unwrap_or_default();
                if !is_fs(&cur) || crate::pane::recent::path_is_recent(&cur) {
                    return None;
                }
                match crate::pane::drop_row_at(i, POINT { x: pt.x, y: pt.y }) {
                    crate::pane::DropRow::Here => Some(cur),
                    crate::pane::DropRow::Folder(p) => Some(p),
                    crate::pane::DropRow::Shell => None,
                }
            }
            AskWhere::Tree => tree_folder(pt),
        }
    }

    /// A plain file drop under Windows rules or Always copy, on the worker. `None` when the
    /// shell keeps it (virtual items, a link, a right-button drag, a ZIP or program row).
    unsafe fn worker_drop(&self, d: &Ref<IDataObject>, srcs: Vec<String>, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Option<u32> {
        if self.right.get() || srcs.is_empty() {
            return None;
        }
        let obj = d.as_ref()?;
        let dst = self.worker_target(pt)?;
        let allowed = if effect.is_null() { self.allowed.get() } else { (*effect).0 };
        let (ctrl, shift, alt) = crate::basket::drag_modifiers(key);
        let link = self.shell_effect.get() == DROPEFFECT_LINK.0;
        let mv = drop_choice(app(|a| a.s.dropm), ctrl, shift, alt, allowed, same_volume(&srcs, &dst), link)?;
        match drop_plan(&srcs, &dst, mv) {
            DropPlan::Shell => None,
            DropPlan::Refuse => {
                let _ = self.inner.DragLeave();
                Some(DROPEFFECT_NONE.0)
            }
            DropPlan::Run => {
                let _ = self.inner.DragLeave();
                Some(run_asked(obj, srcs, dst, mv))
            }
        }
    }

    /// The tree's own target often reports no effect for a file. Without one, OLE never calls
    /// Drop, so Always ask would not run. Offer copy or move while the pointer is on a folder.
    unsafe fn keep_tree_drop(&self, pt: &POINTL, effect: *mut DROPEFFECT) {
        if effect.is_null() || !matches!(self.where_, AskWhere::Tree) || self.srcs.borrow().is_empty() || app(|a| a.s.dropm) != 2 {
            return;
        }
        bind_hwnd(self.hwnd);
        if tree_destination(pt).is_none() {
            return;
        }
        if (*effect).0 & (DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0) == 0 {
            *effect = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0);
        }
    }

    unsafe fn destination(&self, pt: &POINTL) -> Option<String> {
        bind_hwnd(self.hwnd);
        match &self.where_ {
            AskWhere::Pane(slot) => pane_destination(slot),
            AskWhere::Tree => tree_destination(pt),
        }
    }

    /// Enough to show a cursor. The precise folder, including a row under the pointer,
    /// is resolved on Drop so DragOver does not walk accessibility on every move.
    unsafe fn can_land(&self, pt: &POINTL) -> bool {
        match &self.where_ {
            AskWhere::Pane(slot) => {
                let i = slot.load(Ordering::SeqCst);
                app(|a| a.s.paths.get(i).is_some_and(|p| is_fs(p)))
            }
            AskWhere::Tree => tree_destination(pt).is_some(),
        }
    }

    unsafe fn offer_basket(&self, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) {
        if effect.is_null() {
            return;
        }
        let (ctrl, shift, alt) = crate::basket::drag_modifiers(key);
        let want = crate::basket::drag_effect(ctrl, shift, alt);
        let ok = want != 0 && !self.srcs.borrow().is_empty() && self.can_land(pt);
        *effect = if ok { DROPEFFECT(want) } else { DROPEFFECT_NONE };
    }
}

unsafe fn pane_destination(slot: &AtomicUsize) -> Option<String> {
    let i = slot.load(Ordering::SeqCst);
    let cur = app(|a| a.s.paths.get(i).cloned()).unwrap_or_default();
    let child = acc_name_at_cursor().and_then(|name| join_child(&cur, &name)).filter(|p| local_directory(p));
    drop_folder(&cur, child.as_deref())
}

/// Folder under a drop on the left tree. `pt` is the screen point from `IDropTarget`.
/// `HitTest` wants that point in the control's client area; a screen point misses the node,
/// and the drop then ends with no prompt.
unsafe fn tree_destination(pt: &POINTL) -> Option<String> {
    let (tree, hwnd) = app(|a| a.tree.clone().map(|t| (t, a.tree_hwnd)))?;
    if hwnd.0.is_null() {
        return None;
    }
    let mut at = POINT { x: pt.x, y: pt.y };
    if !ScreenToClient(hwnd, &mut at).as_bool() {
        return None;
    }
    let item = tree.HitTest(&at).ok()?;
    shell_fs_path(&item)
}

/// A plain file-system folder under a tree drop. A ZIP, a library or a drive the shell owns is `None`.
unsafe fn tree_folder(pt: &POINTL) -> Option<String> {
    let (tree, hwnd) = app(|a| a.tree.clone().map(|t| (t, a.tree_hwnd)))?;
    if hwnd.0.is_null() {
        return None;
    }
    let mut at = POINT { x: pt.x, y: pt.y };
    if !ScreenToClient(hwnd, &mut at).as_bool() {
        return None;
    }
    let item = tree.HitTest(&at).ok()?;
    let attr = item.GetAttributes(SFGAO_FOLDER | SFGAO_STREAM | SFGAO_FILESYSTEM).map(|f| f.0).unwrap_or(0);
    if !real_folder(attr) || attr & SFGAO_FILESYSTEM.0 == 0 {
        return None;
    }
    let path = pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?);
    is_fs(&path).then_some(path)
}

unsafe fn shell_fs_path(item: &IShellItem) -> Option<String> {
    for kind in [SIGDN_FILESYSPATH, SIGDN_DESKTOPABSOLUTEPARSING] {
        let Ok(name) = item.GetDisplayName(kind) else { continue };
        let path = pwstr(name);
        if is_fs(&path) {
            return Some(path);
        }
    }
    None
}

/// Replace the shell view's drop target so Always ask can run first. The shell target
/// still handles the highlight and every drop that is not Always ask. The wrapper goes on
/// the list (and any other child that already has a target): a wrapper on the parent
/// never sees a drop the child window has registered for.
pub(crate) unsafe fn install_pane_drop(i: usize) {
    let Some(sv) = view_of::<IShellView>(i) else { return };
    let Ok(hwnd) = sv.GetWindow() else { return };
    let Ok(inner) = sv.cast::<IDropTarget>() else { return };
    let Some(slot) = app(|a| a.panes.get(i).and_then(|p| p.as_ref()).map(|p| p.slot.clone())) else { return };
    let _ = SetWindowSubclass(hwnd, Some(drop_watch_proc), DROP_SUB, 0);
    wrap_family(hwnd, &inner, &AskWhere::Pane(slot));
}

/// Install now, then a few times more. The view often registers its target after
/// `OnViewCreated` returns, or again when the list is rebuilt.
pub(crate) unsafe fn schedule_pane_drop(i: usize) {
    install_pane_drop(i);
    let Some(h) = app(|a| a.panes.get(i).and_then(|p| p.as_ref()).map(|p| p.hwnd)).filter(|h| !h.0.is_null()) else { return };
    let _ = SetPropW(h, w!("MaguroDropTries"), Some(HANDLE(4 as *mut core::ffi::c_void)));
    let _ = SetTimer(Some(h), TIMER_DROP, 50, None);
}

pub(crate) unsafe fn on_pane_drop_timer(h: HWND) {
    let left = GetPropW(h, w!("MaguroDropTries")).0 as usize;
    if let Some(i) = pane_of(h) {
        install_pane_drop(i);
    }
    let next = left.saturating_sub(1);
    if next == 0 {
        let _ = KillTimer(Some(h), TIMER_DROP);
        let _ = RemovePropW(h, w!("MaguroDropTries"));
    } else {
        let _ = SetPropW(h, w!("MaguroDropTries"), Some(HANDLE(next as *mut core::ffi::c_void)));
        let _ = SetTimer(Some(h), TIMER_DROP, 200, None);
    }
}

/// `h` is a pane or a window inside one. Re-wrap without stretching the retry timer.
pub(crate) unsafe fn install_drop_at(h: HWND) {
    if let Some(i) = pane_from(h) {
        install_pane_drop(i);
    }
}

/// `h` is a pane or a window inside one. Re-wrap, and keep checking while the view settles.
pub(crate) unsafe fn schedule_drop_at(h: HWND) {
    if let Some(i) = pane_from(h) {
        schedule_pane_drop(i);
    }
}

pub(crate) unsafe fn install_tree_drop(tree: &INameSpaceTreeControl, hwnd: HWND) {
    let Ok(inner) = tree.cast::<IDropTarget>() else { return };
    let _ = SetWindowSubclass(hwnd, Some(tree_drop_watch), DROP_SUB, 0);
    wrap_family(hwnd, &inner, &AskWhere::Tree);
}

unsafe extern "system" fn tree_drop_watch(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, a1: usize, a2: usize) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { tree_drop_watch_inner(h, msg, wp, lp, a1, a2) }))
}

unsafe fn tree_drop_watch_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _a1: usize, _a2: usize) -> LRESULT {
    let r = DefSubclassProc(h, msg, wp, lp);
    if msg == WM_PARENTNOTIFY && wp.0 & 0xFFFF == WM_CREATE as usize {
        let _ = h;
        schedule_tree_drop();
    }
    r
}

pub(crate) unsafe fn schedule_tree_drop() {
    let Some((tree, hwnd, main)) = app(|a| Some((a.tree.clone()?, a.tree_hwnd, a.main))) else { return };
    if hwnd.0.is_null() || main.0.is_null() {
        return;
    }
    install_tree_drop(&tree, hwnd);
    let _ = SetPropW(main, w!("MaguroTreeDrop"), Some(HANDLE(4 as *mut core::ffi::c_void)));
    let _ = SetTimer(Some(main), TIMER_DROP, 50, None);
}

pub(crate) unsafe fn on_tree_drop_timer(h: HWND) {
    let left = GetPropW(h, w!("MaguroTreeDrop")).0 as usize;
    if let Some((tree, hwnd)) = app(|a| Some((a.tree.clone()?, a.tree_hwnd))) {
        if !hwnd.0.is_null() {
            install_tree_drop(&tree, hwnd);
        }
    }
    let next = left.saturating_sub(1);
    if next == 0 {
        let _ = KillTimer(Some(h), TIMER_DROP);
        let _ = RemovePropW(h, w!("MaguroTreeDrop"));
    } else {
        let _ = SetPropW(h, w!("MaguroTreeDrop"), Some(HANDLE(next as *mut core::ffi::c_void)));
        let _ = SetTimer(Some(h), TIMER_DROP, 200, None);
    }
}

impl IDropTarget_Impl for AskDrop_Impl {
    fn DragEnter(&self, d: Ref<IDataObject>, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            super::note_shell_drag();
            let basket = d.as_ref().is_some_and(|obj| crate::basket::is_basket_data(obj));
            self.basket.set(basket);
            self.right.set(key.0 & MK_RBUTTON.0 != 0);
            self.note_allowed(effect);
            self.forward_enter(&d, key, pt, effect);
            self.note_shell(effect);
            // Remember the files now. A second read on Drop fails for some objects, and
            // DragOver has to know this is a file drop before it can offer an effect.
            // Every file drop keeps them: Drop runs a plain copy or move on the worker.
            *self.srcs.borrow_mut() = drop_fs_paths(&d);
            if basket {
                self.offer_basket(key, pt, effect);
            } else {
                self.keep_tree_drop(pt, effect);
                self.offer_always_copy(key, effect);
            }
        }
        Ok(())
    }
    fn DragOver(&self, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            self.note_allowed(effect);
            let _ = self.inner.DragOver(key, *pt, effect);
            self.note_shell(effect);
            if self.basket.get() {
                self.offer_basket(key, pt, effect);
            } else {
                self.keep_tree_drop(pt, effect);
                self.offer_always_copy(key, effect);
            }
        }
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        unsafe {
            self.basket.set(false);
            self.srcs.borrow_mut().clear();
            let _ = self.inner.DragLeave();
            super::end_shell_drag_target();
        }
        Ok(())
    }
    fn Drop(&self, d: Ref<IDataObject>, key: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            super::end_shell_drag_target();
            bind_hwnd(self.hwnd);
            let basket = self.basket.get();
            self.basket.set(false);
            let stored = std::mem::take(&mut *self.srcs.borrow_mut());
            // A basket drop copies unless Shift is held. The shell target would move on
            // the same drive, and it would do the work on this thread. Leave it out.
            if basket {
                let _ = self.inner.DragLeave();
                let srcs = if stored.is_empty() { drop_fs_paths(&d) } else { stored };
                let (ctrl, shift, alt) = crate::basket::drag_modifiers(key);
                let eff = match self.destination(pt) {
                    Some(dst) => crate::basket::drop_transfer(srcs, dst, ctrl, shift, alt),
                    None => DROPEFFECT_NONE.0,
                };
                if !effect.is_null() {
                    *effect = DROPEFFECT(eff);
                }
                return Ok(());
            }
            // Always ask owns every filesystem drop, same drive included. Falling through
            // to the shell here is what moved a same-drive file with no prompt.
            let srcs = if stored.is_empty() { drop_fs_paths(&d) } else { stored };
            if app(|a| a.s.dropm) == 2 {
                if !srcs.is_empty() {
                    let _ = self.inner.DragLeave();
                    let dst = self.destination(pt);
                    if let (Some(obj), Some(dst)) = (d.as_ref(), dst) {
                        let owner = app(|a| a.main);
                        if let Some(mv) = ask_copy_or_move(owner) {
                            if !effect.is_null() {
                                *effect = DROPEFFECT(run_asked(obj, srcs, dst, mv));
                            }
                        } else if !effect.is_null() {
                            *effect = DROPEFFECT_NONE;
                        }
                    } else if !effect.is_null() {
                        *effect = DROPEFFECT_NONE;
                    }
                    return Ok(());
                }
            }
            // Windows rules and Always copy: a plain file copy or move runs on the worker.
            // The shell target would run IFileOperation right here, on the UI thread, and
            // Maguro would freeze until the copy finished.
            if let Some(eff) = self.worker_drop(&d, srcs, key, pt, effect) {
                if !effect.is_null() {
                    *effect = DROPEFFECT(eff);
                }
                return Ok(());
            }
            if let Some(obj) = d.as_ref() {
                let _ = self.inner.Drop(obj, key, *pt, effect);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{drop_choice, drop_folder, drop_plan, drops_into_itself, fav_into_folder, reported_effect, same_volume, shell_drop_class, volume_root, DropPlan};
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};

    const ANY: u32 = DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0;

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn windows_rules_move_on_one_drive_and_copy_across() {
        assert_eq!(drop_choice(0, false, false, false, ANY, true, false), Some(true));
        assert_eq!(drop_choice(0, false, false, false, ANY, false, false), Some(false));
        assert_eq!(drop_choice(0, true, false, false, ANY, true, false), Some(false), "Ctrl copies");
        assert_eq!(drop_choice(0, false, true, false, ANY, false, false), Some(true), "Shift moves");
    }

    #[test]
    fn always_copy_copies_unless_shift() {
        assert_eq!(drop_choice(1, false, false, false, ANY, true, false), Some(false));
        assert_eq!(drop_choice(1, false, true, false, ANY, true, false), Some(true));
    }

    #[test]
    fn links_and_forbidden_effects_stay_with_the_shell() {
        assert_eq!(drop_choice(0, false, false, true, ANY, true, false), None, "Alt links");
        assert_eq!(drop_choice(0, true, true, false, ANY, true, false), None, "Ctrl+Shift links");
        assert_eq!(drop_choice(0, false, false, false, ANY, true, true), None, "the shell's default is a shortcut");
        assert_eq!(drop_choice(0, true, false, false, ANY, true, true), Some(false), "Ctrl still copies");
        // The source decides what it allows.
        assert_eq!(drop_choice(0, false, false, false, DROPEFFECT_COPY.0, true, false), Some(false));
        assert_eq!(drop_choice(0, false, true, false, DROPEFFECT_COPY.0, true, false), None);
        assert_eq!(drop_choice(0, false, false, false, DROPEFFECT_MOVE.0, false, false), Some(true));
        assert_eq!(drop_choice(0, true, false, false, DROPEFFECT_MOVE.0, false, false), None);
        assert_eq!(drop_choice(0, false, false, false, 0, false, false), None);
    }

    #[test]
    fn volumes_compare_by_drive_or_share() {
        assert_eq!(volume_root(r"c:\x").as_deref(), Some("C:"));
        assert_eq!(volume_root(r"\\NAS\Share\d").as_deref(), Some(r"\\nas\share"));
        assert_eq!(volume_root(r"\\?\C:\x"), None);
        assert!(same_volume(&v(&[r"C:\a\x", r"c:\b\y"]), r"C:\z"));
        assert!(!same_volume(&v(&[r"C:\a\x", r"D:\b\y"]), r"C:\z"));
        assert!(same_volume(&v(&[r"\\nas\s\a"]), r"\\NAS\S\b"));
        assert!(!same_volume(&v(&[r"\\nas\s\a"]), r"\\nas\t\b"));
        assert!(!same_volume(&[], r"C:\z"));
    }

    #[test]
    fn drop_plan_refuses_self_and_same_folder_moves() {
        assert!(drops_into_itself(&v(&[r"C:\a"]), r"C:\a"));
        assert!(drops_into_itself(&v(&[r"C:\a"]), r"C:\A\sub"));
        assert!(!drops_into_itself(&v(&[r"C:\a"]), r"C:\ab"));
        assert_eq!(drop_plan(&v(&[r"C:\a"]), r"C:\a\sub", false), DropPlan::Refuse);
        assert_eq!(drop_plan(&v(&[r"C:\a\x.txt"]), r"C:\a", true), DropPlan::Refuse);
        assert_eq!(drop_plan(&v(&[r"C:\a\x.txt"]), r"C:\a", false), DropPlan::Run, "Ctrl copy in place makes '- Copy'");
        assert_eq!(drop_plan(&v(&[r"C:\a\x.txt"]), r"D:\b", true), DropPlan::Run);
        assert_eq!(drop_plan(&v(&[r"C:\a\x.txt", r"C:\b\y.txt"]), r"C:\a", false), DropPlan::Shell);
    }

    #[test]
    fn asked_move_is_a_copy_until_the_source_is_told() {
        assert_eq!(reported_effect(false, false), DROPEFFECT_COPY.0);
        assert_eq!(reported_effect(false, true), DROPEFFECT_COPY.0);
        assert_eq!(reported_effect(true, true), DROPEFFECT_MOVE.0);
        assert_eq!(reported_effect(true, false), DROPEFFECT_COPY.0);
    }


    #[test]
    fn deferred_ask_move_reports_copy_so_the_source_keeps_the_file() {
        // Same effect run_asked returns after start_drop_move: never MOVE from Drop.
        assert_eq!(reported_effect(false, false), DROPEFFECT_COPY.0);
    }

    #[test]
    fn pane_drop_lands_in_the_folder_under_the_cursor() {
        assert_eq!(drop_folder(r"C:\pics", Some(r"C:\pics\holiday")), Some(r"C:\pics\holiday".into()));
        assert_eq!(drop_folder(r"C:\pics", None), Some(r"C:\pics".into()));
        assert_eq!(drop_folder("::{thispc}", None), None);
        assert_eq!(drop_folder("::{thispc}", Some(r"D:\work")), Some(r"D:\work".into()));
    }

    #[test]
    fn shell_drop_goes_to_the_list_not_the_header() {
        assert!(shell_drop_class("SysListView32"));
        assert!(shell_drop_class("DirectUIHWND"));
        assert!(shell_drop_class("SHELLDLL_DefView"));
        assert!(shell_drop_class("SysTreeView32"));
        assert!(!shell_drop_class("SysHeader32"));
        assert!(!shell_drop_class("MaguroPane"));
    }

    #[test]
    fn favourite_folder_middle_is_a_copy_target_only_when_asking() {
        assert!(fav_into_folder(2, false, false, r"C:\pics", 10, 20));
        assert!(!fav_into_folder(2, false, false, r"C:\pics", 1, 20));
        assert!(!fav_into_folder(2, false, false, r"C:\pics", 16, 20));
        assert!(!fav_into_folder(0, false, false, r"C:\pics", 10, 20));
        assert!(!fav_into_folder(1, false, false, r"C:\pics", 10, 20));
        assert!(!fav_into_folder(2, true, false, r"C:\pics", 10, 20));
        assert!(!fav_into_folder(2, false, true, r"C:\pics", 10, 20));
        assert!(!fav_into_folder(2, false, false, "::{thispc}", 10, 20));
    }
}
