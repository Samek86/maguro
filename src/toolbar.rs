//! Command toolbar. `toolbar.json` sits next to `maguro.json` (roaming Maguro, or the exe
//! folder when `portable.txt` is present). It is never written into `maguro.json`.
//! Settings reset does not open this file. A missing file uses the default layout.
//! A file that cannot be parsed is left on disk and the default layout is used instead.
//!
//! Vertical window tabs draw the bar across the left of the top strip, inside `tab_w`.
//! Horizontal window tabs draw the same buttons down a narrow left column (30px
//! at 100%), where the vertical tab strip would sit. Buttons are icon-only.
//! The pinned Change button opens the customize window.
//! A folder or file item opens that preset path. The old pick-at-click Open command
//! is dropped when `toolbar.json` is read.

use std::path::Path;

use serde_json::{Map, Value};

use std::sync::OnceLock;

use windows::Win32::System::DataExchange::{AddClipboardFormatListener, IsClipboardFormatAvailable, RemoveClipboardFormatListener};
use windows::Win32::UI::Controls::{TTF_ABSOLUTE, TTF_TRACK, TTM_GETBUBBLESIZE, TTM_POP, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION};

use crate::app::*;
use crate::imports::*;
use crate::ui::LineIcon;

pub(crate) const FILE_NAME: &str = "toolbar.json";
const VERSION: u32 = 1;

/// Button-window ceiling. Layout still stops at the pixels that are free; this only
/// bounds how many HWNDs [`create_buttons`] makes. See [`crate::CMD_CAPACITY`].

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    NewFolder,
    NewFile,
    DeleteSelection,
    Paste,
    /// In-place rename (F2 / the shell's own edit). Not batch rename (`file.rename`).
    RenameInPlace,
    ViewMode,
    Properties,
    Mark,
    /// Pinned. Opens the customize window.
    EditToolbar,
    Overflow,
    /// Commands that are not already a pane-header button or a right-hand icon.
    CopyPath,
    BatchRename,
    InvertSelection,
    NewTab,
    CloseTab,
    DuplicateTab,
    ShowHidden,
    Zoom,
    QuickLook,
    PreviewPane,
    SplitRight,
    SplitBelow,
    /// Retired. Old files name `file.open`; load drops that item. Preset targets replaced it.
    Open,
    PaneColor,
    ClosePane,
    Terminal,
    /// Window-tab colour. Distinct from [`Action::PaneColor`], which tints the pane header.
    TabColor,
    /// Open a terminal in the active folder with the runas verb.
    TerminalAdmin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IconChoice {
    Auto,
    Manual { pack: Option<String>, file: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Command { id: String, action: Option<Action>, icon: IconChoice },
    Separator,
    Folder { path: String, icon: IconChoice },
    File { path: String, icon: IconChoice },
    Program { exe: String, args: String, dir: String, icon: IconChoice },
    Raw(Value),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Doc {
    items: Vec<Item>,
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Command {
        action: Action,
        icon: IconChoice,
    },
    Separator,
    /// Open this folder in the active pane.
    Folder {
        path: String,
        icon: IconChoice,
    },
    /// Open this file with the shell.
    File {
        path: String,
        icon: IconChoice,
    },
}

/// What one toolbar button runs. A target is a preset path, not a command id.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Run {
    Command(Action),
    Folder(String),
    File(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Px {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Px {
    fn empty() -> Self {
        Self { left: 0, top: 0, right: 0, bottom: 0 }
    }

    pub(crate) fn live(self) -> bool {
        self.right > self.left && self.bottom > self.top
    }

    fn to_rect(self) -> RECT {
        RECT { left: self.left, top: self.top, right: self.right, bottom: self.bottom }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BarLay {
    pub shown: bool,
    pub commands: Vec<Px>,
    /// Index into the piece list for each visible command button.
    pub command_at: Vec<usize>,
    pub separators: Vec<Px>,
    pub overflow: Option<Px>,
    pub edit: Px,
    /// Piece indexes that did not fit.
    pub hidden: Vec<usize>,
}

impl BarLay {
    fn hidden_bar() -> Self {
        Self { shown: false, commands: Vec::new(), command_at: Vec::new(), separators: Vec::new(), overflow: None, edit: Px::empty(), hidden: Vec::new() }
    }
}

pub(crate) struct LayIn {
    /// Window tabs run down the left. The command bar is then the horizontal top strip.
    pub vertical_tabs: bool,
    /// Right edge of the horizontal command area. Unused for the vertical column.
    pub tab_w: i32,
    /// Horizontal bar: button top. Vertical column: top of the column (below the tab row).
    pub top: i32,
    /// Bottom of the vertical column. Unused for the horizontal bar.
    pub bottom: i32,
    /// Left edge of the vertical column. Unused for the horizontal bar.
    pub left: i32,
    pub dpi: i32,
    pub max_buttons: usize,
}

/// Where the live window wants the bar. Built from the main client, not from a test.
#[derive(Clone, Copy)]
pub(crate) struct Origin {
    pub vertical_tabs: bool,
    pub tab_w: i32,
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub dpi: i32,
}

impl Origin {
    fn lay_in(self) -> LayIn {
        LayIn { vertical_tabs: self.vertical_tabs, tab_w: self.tab_w, top: self.top, bottom: self.bottom, left: self.left, dpi: self.dpi, max_buttons: CMD_CAPACITY as usize }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnableInput {
    pub selection: u32,
    pub folder_can_receive: bool,
    pub clipboard_pasteable: bool,
}

pub(crate) enum Read {
    Missing,
    Loaded(Doc),
    Corrupt,
}

/// What the dedicated toolbar tooltip is showing. Anchor is the button, in screen pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTip {
    pub control: i32,
    pub text: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Whether a hover or a re-layout should talk to the tooltip window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipStep {
    Idle,
    Show,
    Hide,
}

/// Same control, text, and anchor: send nothing. A slot that changed command, a button
/// that moved, or the pointer leaving the bar is a real update.
pub(crate) fn tip_step(live: Option<&LiveTip>, next: Option<&LiveTip>) -> TipStep {
    match (live, next) {
        (None, None) => TipStep::Idle,
        (Some(_), None) => TipStep::Hide,
        (None, Some(_)) => TipStep::Show,
        (Some(have), Some(want)) if have == want => TipStep::Idle,
        (Some(_), Some(_)) => TipStep::Show,
    }
}

/// Horizontal bar (vertical tabs): below the icon. Left column: to the right of it.
pub(crate) fn command_tip_side(vertical_tabs: bool) -> crate::TipSide {
    if vertical_tabs {
        crate::TipSide::Below
    } else {
        crate::TipSide::Right
    }
}

/// Why the tracking tooltip is being reconsidered. Both bars share one bubble.
/// The right-hand icon strip uses the shared subclassed tooltip, not this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipCause {
    /// Pointer moved onto a command button.
    Hover,
    /// Pointer left the command buttons.
    Leave,
    /// Mouse-down, or the instant before a command runs.
    Press,
    /// `WM_ACTIVATE` inactive or `WM_ACTIVATEAPP` false.
    Inactive,
    /// Focus left for a window other than the tooltip itself.
    KillFocus,
    /// The frame is minimizing.
    Minimize,
    /// The buttons were placed again.
    Relayout,
}

/// What to tell the tooltip window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipOrder {
    Hide,
    Show,
}

/// A tracking tip does not get `WM_MOUSELEAVE` after the foreground window changes,
/// so the bubble stays up until the pointer comes back. Hide on press, inactivity,
/// focus loss, minimize, and whenever the buttons are placed again. A hover shows
/// only while the frame is in front, not iconic, and not holding a button press.
/// The next move after a relayout shows it again if the pointer is still there.
pub(crate) fn tip_order(cause: TipCause, active: bool, minimized: bool, over_button: bool, held: bool) -> TipOrder {
    let may_show = active && !minimized && !held && over_button;
    match cause {
        TipCause::Hover if may_show => TipOrder::Show,
        TipCause::Hover | TipCause::Relayout | TipCause::Leave | TipCause::Press | TipCause::Inactive | TipCause::KillFocus | TipCause::Minimize => TipOrder::Hide,
    }
}

/// Focus moved onto the tooltip window. Leave the bubble up in that case: hiding
/// would dismiss the tip the moment it took the keyboard. Every other destination
/// drops it.
pub(crate) fn tip_hides_on_focus(to_tooltip: bool) -> bool {
    !to_tooltip
}

/// `TME_LEAVE` fires once. Arm it again while the pointer is on a command button
/// and the frame is active, so the next leave still hides the bubble.
pub(crate) fn tip_rearm_leave(over_button: bool, active: bool, minimized: bool) -> bool {
    over_button && active && !minimized
}

struct State {
    doc: Doc,
    clip_pasteable: bool,
    /// Last logical enabled flag for each command id. The HWND stays enabled.
    enabled: Vec<(i32, bool)>,
    /// Last button painted in each slot, so a re-layout that swaps commands or targets repaints.
    face: Vec<(i32, Option<(Run, LineIcon)>)>,
    tip: isize,
    tip_added: bool,
    live_tip: Option<LiveTip>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State { doc: Doc::default_layout(), clip_pasteable: false, enabled: Vec::new(), face: Vec::new(), tip: 0, tip_added: false, live_tip: None })).lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn command_id(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::NewFolder => "file.dir",
        Action::NewFile => "file.new",
        Action::DeleteSelection => "file.delete",
        Action::Paste => "file.paste",
        Action::RenameInPlace => "edit.rename",
        Action::ViewMode => "view.mode",
        Action::Properties => "file.properties",
        Action::Mark => "file.mark",
        Action::EditToolbar | Action::Overflow => return None,
        Action::CopyPath => "file.path",
        Action::BatchRename => "file.rename",
        Action::InvertSelection => "file.invert",
        Action::NewTab => "tab.new",
        Action::CloseTab => "tab.close",
        Action::DuplicateTab => "tab.dup",
        Action::ShowHidden => "view.hidden",
        Action::Zoom => "view.zoom",
        Action::QuickLook => "view.peek",
        Action::PreviewPane => "view.preview",
        Action::SplitRight => "pane.split.right",
        Action::SplitBelow => "pane.split.below",
        Action::Open => "file.open",
        Action::PaneColor => "pane.color",
        Action::ClosePane => "pane.close",
        Action::Terminal => "file.terminal",
        Action::TabColor => "tab.color",
        Action::TerminalAdmin => "file.terminal.admin",
    })
}

pub(crate) fn action_of(id: &str) -> Option<Action> {
    Some(match id {
        "file.dir" => Action::NewFolder,
        "file.new" => Action::NewFile,
        "file.delete" => Action::DeleteSelection,
        "file.paste" => Action::Paste,
        "edit.rename" => Action::RenameInPlace,
        "view.mode" => Action::ViewMode,
        "file.properties" => Action::Properties,
        "file.mark" => Action::Mark,
        "file.path" => Action::CopyPath,
        "file.rename" => Action::BatchRename,
        "file.invert" => Action::InvertSelection,
        "tab.new" => Action::NewTab,
        "tab.close" => Action::CloseTab,
        "tab.dup" => Action::DuplicateTab,
        "view.hidden" => Action::ShowHidden,
        "view.zoom" => Action::Zoom,
        "view.peek" => Action::QuickLook,
        "view.preview" => Action::PreviewPane,
        "pane.split.right" => Action::SplitRight,
        "pane.split.below" => Action::SplitBelow,
        "file.open" => Action::Open,
        "pane.color" => Action::PaneColor,
        "pane.close" => Action::ClosePane,
        "file.terminal" => Action::Terminal,
        "tab.color" => Action::TabColor,
        "file.terminal.admin" => Action::TerminalAdmin,
        _ => return None,
    })
}

/// Key-table id, when the command has a rebindable chord. In-place rename has none:
/// `file.rename` is batch rename.
pub(crate) fn key_id(action: Action) -> Option<&'static str> {
    match action {
        Action::NewFolder => Some("file.dir"),
        Action::NewFile => Some("file.new"),
        Action::Mark => Some("file.mark"),
        Action::CopyPath => Some("file.path"),
        Action::BatchRename => Some("file.rename"),
        Action::InvertSelection => Some("file.invert"),
        Action::NewTab => Some("tab.new"),
        Action::CloseTab => Some("tab.close"),
        Action::DuplicateTab => Some("tab.dup"),
        Action::ShowHidden => Some("view.hidden"),
        Action::Zoom => Some("view.zoom"),
        Action::QuickLook => Some("view.peek"),
        Action::PreviewPane => Some("view.preview"),
        _ => None,
    }
}

pub(crate) fn fixed_shortcut(action: Action) -> Option<&'static str> {
    match action {
        Action::DeleteSelection => Some("Del"),
        Action::Paste => Some("Ctrl+V"),
        Action::RenameInPlace => Some("F2"),
        _ => None,
    }
}

pub(crate) fn label_key(action: Action) -> crate::Key {
    match action {
        Action::NewFolder => crate::Key::NewFolder,
        Action::NewFile => crate::Key::NewTextFile,
        Action::DeleteSelection => crate::Key::Delete,
        Action::Paste => crate::Key::CtxPaste,
        Action::RenameInPlace => crate::Key::Rename,
        Action::ViewMode => crate::Key::ToolbarView,
        Action::Properties => crate::Key::Properties,
        Action::Mark => crate::Key::MarkAsImportant,
        Action::EditToolbar => crate::Key::ToolbarChange,
        Action::Overflow => crate::Key::ToolbarOverflow,
        Action::CopyPath => crate::Key::ToolbarCopyPath,
        Action::BatchRename => crate::Key::BatchRename,
        Action::InvertSelection => crate::Key::InvertSelection,
        Action::NewTab => crate::Key::NewTab,
        Action::CloseTab => crate::Key::CloseTab,
        Action::DuplicateTab => crate::Key::DuplicateTab,
        Action::ShowHidden => crate::Key::ShowHiddenFiles,
        Action::Zoom => crate::Key::ToolbarZoom,
        Action::QuickLook => crate::Key::QuickLook,
        Action::PreviewPane => crate::Key::PreviewPane,
        Action::SplitRight => crate::Key::ToolbarSplitRight,
        Action::SplitBelow => crate::Key::ToolbarSplitBelow,
        Action::Open => crate::Key::ToolbarOpen,
        Action::PaneColor => crate::Key::ToolbarPaneColor,
        Action::ClosePane => crate::Key::ClosePane,
        Action::Terminal => crate::Key::ToolbarTerminal,
        Action::TabColor => crate::Key::ToolbarTabColor,
        Action::TerminalAdmin => crate::Key::ToolbarTerminalAdmin,
    }
}

pub(crate) fn tooltip(action: Action) -> String {
    let name = crate::tr(label_key(action));
    if let Some(id) = key_id(action) {
        return crate::tip_with_chord(name, id);
    }
    match fixed_shortcut(action) {
        Some(chord) => format!("{name} ({chord})"),
        None => name.to_string(),
    }
}

pub(crate) fn drawn_icon(action: Action, icon: &IconChoice) -> LineIcon {
    if let IconChoice::Manual { pack: Some(key), .. } = icon {
        if let Some(def) = crate::ui::icon_catalog::icon_by_key(key) {
            return def.kind;
        }
    }
    match action {
        Action::NewFolder => LineIcon::Group,
        Action::NewFile => LineIcon::NewFile,
        Action::DeleteSelection => LineIcon::Del,
        Action::Paste => LineIcon::Paste,
        Action::RenameInPlace => LineIcon::Rename,
        Action::ViewMode => LineIcon::View,
        Action::Properties => LineIcon::Properties,
        Action::Mark => LineIcon::Important,
        Action::EditToolbar => LineIcon::Edit,
        Action::Overflow => LineIcon::Chevrons,
        Action::CopyPath => LineIcon::Copy,
        Action::BatchRename => LineIcon::Rename,
        Action::InvertSelection => LineIcon::Invert,
        Action::NewTab => LineIcon::Add,
        Action::CloseTab => LineIcon::Close,
        Action::DuplicateTab => LineIcon::Duplicate,
        Action::ShowHidden => LineIcon::Hidden,
        Action::Zoom => LineIcon::ZoomIn,
        Action::QuickLook => LineIcon::Preview,
        Action::PreviewPane => LineIcon::SidebarLeft,
        Action::SplitRight => LineIcon::Fwd,
        Action::SplitBelow => LineIcon::Down,
        Action::Open => LineIcon::OpenFolder,
        Action::PaneColor => LineIcon::PaneColor,
        Action::ClosePane => LineIcon::PaneClose,
        Action::Terminal => LineIcon::Terminal,
        Action::TabColor => LineIcon::TabColor,
        Action::TerminalAdmin => LineIcon::TerminalAdmin,
    }
}

/// A folder that can take a new file or a paste. Recent and shell places cannot.
pub(crate) fn folder_can_receive(path: &str, recent: bool) -> bool {
    crate::is_fs(path) && !recent
}

pub(crate) fn clipboard_pasteable(has_files: bool, has_picture: bool) -> bool {
    has_files || has_picture
}

pub(crate) fn enabled(action: Action, input: &EnableInput) -> bool {
    match action {
        Action::NewFolder | Action::NewFile => input.folder_can_receive,
        Action::DeleteSelection | Action::RenameInPlace | Action::Properties | Action::Mark | Action::BatchRename | Action::InvertSelection | Action::QuickLook => input.selection > 0,
        Action::Paste => input.folder_can_receive && input.clipboard_pasteable,
        Action::ViewMode | Action::Overflow | Action::EditToolbar | Action::NewTab | Action::CloseTab | Action::DuplicateTab | Action::ShowHidden | Action::Zoom | Action::PreviewPane | Action::SplitRight | Action::SplitBelow | Action::Open | Action::PaneColor | Action::TabColor | Action::ClosePane | Action::Terminal | Action::TerminalAdmin | Action::CopyPath => true,
    }
}

/// Whether the command icon is drawn in the accent. The button fill stays the
/// resting ground. `show_hidden` is the shell "show all objects" flag.
pub(crate) fn command_toggled(action: Action, show_hidden: bool) -> bool {
    match action {
        Action::ShowHidden => show_hidden,
        _ => false,
    }
}

impl Doc {
    pub(crate) fn default_layout() -> Self {
        const HEAD: [Action; 8] = [Action::NewFolder, Action::NewFile, Action::DeleteSelection, Action::Paste, Action::RenameInPlace, Action::ViewMode, Action::Properties, Action::Mark];
        const TAIL: [Action; 5] = [Action::ShowHidden, Action::SplitRight, Action::SplitBelow, Action::ClosePane, Action::Terminal];
        fn cmd(action: Action) -> Option<Item> {
            let id = command_id(action)?.to_string();
            Some(Item::Command { action: action_of(&id), id, icon: IconChoice::Auto })
        }
        let mut items: Vec<Item> = HEAD.into_iter().filter_map(cmd).collect();
        items.push(Item::Separator);
        items.extend(TAIL.into_iter().filter_map(cmd));
        Self { items, extra: Map::new() }
    }

    pub(crate) fn command_ids(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|it| match it {
                Item::Command { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn pieces(&self) -> Vec<Piece> {
        self.items
            .iter()
            .filter_map(|it| match it {
                // `file.open` is retired. A file that still names it never becomes a button.
                Item::Command { action: Some(action), icon, .. } if *action != Action::Open => Some(Piece::Command { action: *action, icon: icon.clone() }),
                Item::Separator => Some(Piece::Separator),
                Item::Folder { path, icon } => Some(Piece::Folder { path: path.clone(), icon: icon.clone() }),
                Item::File { path, icon } => Some(Piece::File { path: path.clone(), icon: icon.clone() }),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn to_json(&self) -> String {
        let items: Vec<Value> = self.items.iter().map(Item::to_json).collect();
        let mut obj = Map::new();
        obj.insert("version".into(), Value::from(VERSION));
        obj.insert("items".into(), Value::Array(items));
        for (k, v) in &self.extra {
            obj.insert(k.clone(), v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_else(|_| "{\"version\":1,\"items\":[]}".into()) + "\n"
    }

    pub(crate) fn current() -> Self {
        state().doc.clone()
    }

    /// Write `toolbar.json` and use this layout on the bar.
    pub(crate) fn install(self) -> bool {
        if save(&file_path(), &self).is_err() {
            return false;
        }
        state().doc = self;
        true
    }

    pub(crate) fn item_count(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn is_separator(&self, i: usize) -> bool {
        matches!(self.items.get(i), Some(Item::Separator))
    }

    pub(crate) fn label_at(&self, i: usize) -> String {
        match self.items.get(i) {
            Some(Item::Command { action: Some(action), .. }) => crate::tr(label_key(*action)).to_string(),
            Some(Item::Command { id, .. }) => id.clone(),
            Some(Item::Separator) => crate::tr(crate::Key::ToolbarSeparator).to_string(),
            Some(Item::Folder { path, .. }) => file_tail(path),
            Some(Item::File { path, .. }) => file_tail(path),
            Some(Item::Program { exe, .. }) => file_tail(exe),
            Some(Item::Raw(_)) => String::new(),
            None => String::new(),
        }
    }

    pub(crate) fn glyph_at(&self, i: usize) -> Option<LineIcon> {
        match self.items.get(i)? {
            Item::Separator => None,
            Item::Command { action: Some(action), icon, .. } => Some(drawn_icon(*action, icon)),
            Item::Command { .. } => Some(LineIcon::NewFile),
            Item::Folder { icon, .. } => Some(choice_icon(icon, LineIcon::OpenFolder)),
            Item::File { icon, .. } => Some(choice_icon(icon, LineIcon::PinFile)),
            Item::Program { icon, .. } => Some(choice_icon(icon, LineIcon::OpenIn)),
            Item::Raw(_) => Some(LineIcon::NewFile),
        }
    }

    pub(crate) fn insert_action(&mut self, at: usize, action: Action) {
        if action == Action::Open {
            return;
        }
        let Some(id) = command_id(action) else { return };
        let at = at.min(self.items.len());
        self.items.insert(at, Item::Command { id: id.to_string(), action: Some(action), icon: IconChoice::Auto });
    }

    pub(crate) fn insert_separator(&mut self, at: usize) {
        let at = at.min(self.items.len());
        self.items.insert(at, Item::Separator);
    }

    pub(crate) fn remove_at(&mut self, i: usize) -> bool {
        if i >= self.items.len() {
            return false;
        }
        self.items.remove(i);
        true
    }

    /// `to` is the index before the move. Dropping on the same gap is a no-op.
    pub(crate) fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() {
            return false;
        }
        let to = to.min(self.items.len());
        if from == to || from + 1 == to {
            return false;
        }
        let item = self.items.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.items.insert(to, item);
        true
    }

    pub(crate) fn reset_default(&mut self) {
        *self = Self::default_layout();
    }

    pub(crate) fn manual_icon(&self, i: usize) -> bool {
        matches!(self.items.get(i), Some(Item::Command { icon: IconChoice::Manual { .. }, .. } | Item::Folder { icon: IconChoice::Manual { .. }, .. } | Item::File { icon: IconChoice::Manual { .. }, .. } | Item::Program { icon: IconChoice::Manual { .. }, .. }))
    }

    pub(crate) fn has_action(&self, action: Action) -> bool {
        self.items.iter().any(|it| matches!(it, Item::Command { action: Some(found), .. } if *found == action))
    }

    /// `None` restores the command's automatic icon. A key selects a catalog icon.
    pub(crate) fn set_pack(&mut self, i: usize, key: Option<&str>) {
        let icon = match key {
            None => IconChoice::Auto,
            Some(k) => IconChoice::Manual { pack: Some(k.to_string()), file: None },
        };
        let Some(slot) = (match self.items.get_mut(i) {
            Some(Item::Command { icon, .. } | Item::Folder { icon, .. } | Item::File { icon, .. } | Item::Program { icon, .. }) => Some(icon),
            _ => None,
        }) else {
            return;
        };
        *slot = icon;
    }

    pub(crate) fn insert_folder(&mut self, at: usize, path: &str) {
        self.insert_target(at, true, path);
    }

    pub(crate) fn insert_file(&mut self, at: usize, path: &str) {
        self.insert_target(at, false, path);
    }

    fn insert_target(&mut self, at: usize, folder: bool, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        let at = at.min(self.items.len());
        let icon = IconChoice::Auto;
        let item = if folder { Item::Folder { path: path.to_string(), icon } } else { Item::File { path: path.to_string(), icon } };
        self.items.insert(at, item);
    }

    pub(crate) fn is_folder_target(&self, i: usize) -> bool {
        matches!(self.items.get(i), Some(Item::Folder { .. }))
    }

    pub(crate) fn is_file_target(&self, i: usize) -> bool {
        matches!(self.items.get(i), Some(Item::File { .. }))
    }

    pub(crate) fn target_path(&self, i: usize) -> Option<&str> {
        match self.items.get(i) {
            Some(Item::Folder { path, .. } | Item::File { path, .. }) => Some(path.as_str()),
            _ => None,
        }
    }

    /// Replace the path of a folder or file item. A command is left alone.
    pub(crate) fn set_target_path(&mut self, i: usize, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        match self.items.get_mut(i) {
            Some(Item::Folder { path: slot, .. } | Item::File { path: slot, .. }) => {
                *slot = path.to_string();
                true
            }
            _ => false,
        }
    }

    pub(crate) fn target_missing(&self, i: usize) -> bool {
        self.target_path(i).is_some_and(|path| !target_present(path))
    }

    /// Bar and palette tooltip. A target adds the full path under the name.
    pub(crate) fn item_tip(&self, i: usize) -> String {
        match self.items.get(i) {
            Some(Item::Folder { path, .. } | Item::File { path, .. }) => target_tip(&file_tail(path), path),
            _ => self.label_at(i),
        }
    }
}

fn file_tail(path: &str) -> String {
    path.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or(path).to_string()
}

fn choice_icon(icon: &IconChoice, auto: LineIcon) -> LineIcon {
    if let IconChoice::Manual { pack: Some(key), .. } = icon {
        if let Some(def) = crate::ui::icon_catalog::icon_by_key(key) {
            return def.kind;
        }
    }
    auto
}

/// A preset the bar can open. Empty and missing paths are gray and beep instead of running.
///
/// `GetFileAttributes` on a disconnected mapping, an empty removable volume, or a UNC
/// share stalls the UI thread or raises the system critical-error box. The editor used
/// to do that while it was being shown, so the window never appeared. An unmounted
/// letter is missing with no filesystem call. A UNC path or a cached remote drive is
/// left enabled: probing it would bind the share.
pub(crate) fn target_present(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() || path.contains('\0') {
        return false;
    }
    if path.starts_with(r"\\") || path.starts_with("//") {
        return true;
    }
    if let Some(letter) = drive_letter(path) {
        if !drive_mounted(letter) {
            return false;
        }
        match cached_drive_type(letter) {
            Some(1) => return false, // DRIVE_NO_ROOT_DIR
            Some(4) => return true,  // DRIVE_REMOTE
            _ => {}
        }
    }
    exists_quiet(path)
}

fn cached_drive_type(letter: u8) -> Option<u32> {
    crate::app_try(|a| a.drives.iter().find(|d| d.0 == letter).map(|d| d.1)).flatten()
}

fn drive_letter(path: &str) -> Option<u8> {
    match path.as_bytes() {
        [c, b':', ..] if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

fn drive_mounted(letter: u8) -> bool {
    if !(b'A'..=b'Z').contains(&letter) {
        return false;
    }
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    if mask == 0 {
        return true;
    }
    mask >> (letter - b'A') & 1 != 0
}

fn exists_quiet(path: &str) -> bool {
    use windows::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX};
    unsafe {
        let prev = SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX);
        let ok = Path::new(path).try_exists().unwrap_or(false);
        let _ = SetErrorMode(prev);
        ok
    }
}

/// Name, then the full path. One line when the name is already the whole path.
pub(crate) fn target_tip(name: &str, path: &str) -> String {
    if path.is_empty() || name == path {
        name.to_string()
    } else {
        format!("{name}\r\n{path}")
    }
}

fn is_slot(p: &Piece) -> bool {
    matches!(p, Piece::Command { .. } | Piece::Folder { .. } | Piece::File { .. })
}

fn run_of(piece: &Piece) -> Option<Run> {
    match piece {
        Piece::Command { action, .. } => Some(Run::Command(*action)),
        Piece::Folder { path, .. } => Some(Run::Folder(path.clone())),
        Piece::File { path, .. } => Some(Run::File(path.clone())),
        Piece::Separator => None,
    }
}

fn piece_icon(piece: &Piece) -> Option<LineIcon> {
    match piece {
        Piece::Command { action, icon } => Some(drawn_icon(*action, icon)),
        Piece::Folder { icon, .. } => Some(choice_icon(icon, LineIcon::OpenFolder)),
        Piece::File { icon, .. } => Some(choice_icon(icon, LineIcon::PinFile)),
        Piece::Separator => None,
    }
}

fn run_label(run: &Run) -> String {
    match run {
        Run::Command(action) => crate::tr(label_key(*action)).to_string(),
        Run::Folder(path) | Run::File(path) => file_tail(path),
    }
}

fn run_tip(run: &Run) -> String {
    match run {
        Run::Command(action) => tooltip(*action),
        Run::Folder(path) | Run::File(path) => target_tip(&file_tail(path), path),
    }
}

fn run_enabled(run: &Run, input: &EnableInput) -> bool {
    match run {
        Run::Command(action) => enabled(*action, input),
        Run::Folder(path) | Run::File(path) => target_present(path),
    }
}

impl Item {
    fn to_json(&self) -> Value {
        let icon = |icon: &IconChoice| icon_json(icon);
        match self {
            Item::Command { id, icon: choice, .. } => json_obj(&[("type", Value::from("command")), ("id", Value::from(id.clone())), ("icon", icon(choice))]),
            Item::Separator => json_obj(&[("type", Value::from("separator"))]),
            Item::Folder { path, icon: choice } => json_obj(&[("type", Value::from("folder")), ("path", Value::from(path.clone())), ("icon", icon(choice))]),
            Item::File { path, icon: choice } => json_obj(&[("type", Value::from("file")), ("path", Value::from(path.clone())), ("icon", icon(choice))]),
            Item::Program { exe, args, dir, icon: choice } => json_obj(&[("type", Value::from("program")), ("exe", Value::from(exe.clone())), ("args", Value::from(args.clone())), ("dir", Value::from(dir.clone())), ("icon", icon(choice))]),
            Item::Raw(v) => v.clone(),
        }
    }
}

fn json_obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

fn icon_json(icon: &IconChoice) -> Value {
    match icon {
        IconChoice::Auto => json_obj(&[("mode", Value::from("auto"))]),
        IconChoice::Manual { pack, file } => {
            let mut m = Map::new();
            m.insert("mode".into(), Value::from("manual"));
            if let Some(pack) = pack {
                m.insert("pack".into(), Value::from(pack.clone()));
            }
            if let Some(file) = file {
                m.insert("file".into(), Value::from(file.clone()));
            }
            Value::Object(m)
        }
    }
}

fn parse_icon(v: Option<&Value>) -> IconChoice {
    let Some(obj) = v.and_then(|v| v.as_object()) else {
        return IconChoice::Auto;
    };
    let mode = obj.get("mode").and_then(|m| m.as_str()).unwrap_or("auto");
    if mode != "manual" {
        return IconChoice::Auto;
    }
    let text = |key: &str| obj.get(key).and_then(|s| s.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    IconChoice::Manual { pack: text("pack"), file: text("file") }
}

fn parse_item(v: &Value) -> Item {
    let Some(obj) = v.as_object() else {
        return Item::Raw(v.clone());
    };
    let kind = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let text = |key: &str| obj.get(key).and_then(|s| s.as_str()).unwrap_or("").to_string();
    let icon = parse_icon(obj.get("icon"));
    match kind {
        "command" => {
            let id = text("id");
            Item::Command { action: action_of(&id), id, icon }
        }
        "separator" => Item::Separator,
        "folder" => Item::Folder { path: text("path"), icon },
        "file" => Item::File { path: text("path"), icon },
        "program" => Item::Program { exe: text("exe"), args: text("args"), dir: text("dir"), icon },
        _ => Item::Raw(v.clone()),
    }
}

pub(crate) fn parse(bytes: &[u8]) -> std::result::Result<Doc, ()> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim().trim_start_matches('\u{feff}').trim();
    let v: Value = serde_json::from_str(text).map_err(|_| ())?;
    let obj = v.as_object().ok_or(())?;
    let items = obj.get("items").and_then(|x| x.as_array()).ok_or(())?;
    let mut extra = obj.clone();
    extra.remove("version");
    extra.remove("items");
    Ok(Doc { items: items.iter().map(parse_item).filter_map(keep_loaded).collect(), extra })
}

/// `file.open` was the pick-at-click command. Drop it so an old file still loads.
fn keep_loaded(item: Item) -> Option<Item> {
    match item {
        Item::Command { action: Some(Action::Open), .. } => None,
        Item::Command { ref id, .. } if id == "file.open" => None,
        other => Some(other),
    }
}

pub(crate) fn read_file(path: &Path) -> Read {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Corrupt,
        Ok(bytes) => match parse(&bytes) {
            Ok(doc) => Read::Loaded(doc),
            Err(()) => Read::Corrupt,
        },
    }
}

pub(crate) fn or_default(read: Read) -> Doc {
    match read {
        Read::Loaded(doc) => doc,
        Read::Missing | Read::Corrupt => Doc::default_layout(),
    }
}

/// Write `toolbar.json`. The edit window calls this. Settings reset does not.
#[allow(dead_code)]
pub(crate) fn save(path: &Path, doc: &Doc) -> std::io::Result<()> {
    crate::settings::store::write_atomic(path, doc.to_json().as_bytes())
}

pub(crate) fn file_path() -> PathBuf {
    crate::settings::store::config_dir().join(FILE_NAME)
}

/// Read `toolbar.json`. Does not create it, and does not replace a broken file.
pub(crate) fn load() {
    let doc = or_default(read_file(&file_path()));
    // The serializer stays in the binary for the edit window. This launch does not write the file.
    let _linked = (VERSION, doc.command_ids().len(), doc.to_json().len());
    state().doc = doc;
}

fn scale(dpi: i32, v: i32) -> i32 {
    v * dpi.max(1) / 96
}

fn button_metrics(dpi: i32) -> (i32, i32, i32, i32) {
    let dpi = dpi.max(1);
    let (w, h) = crate::tool_icon_px(dpi);
    (w, h, scale(dpi, 4), scale(dpi, 6))
}

pub(crate) fn layout(pieces: &[Piece], input: &LayIn) -> BarLay {
    if input.vertical_tabs {
        layout_row(pieces, input)
    } else {
        layout_column(pieces, input)
    }
}

/// Buttons across the top strip. Change sits at the right edge. Overflow sits just left of it.
fn layout_row(pieces: &[Piece], input: &LayIn) -> BarLay {
    let dpi = input.dpi.max(1);
    let (btn_w, btn_h, _, pad) = button_metrics(dpi);
    // Horizontal neighbours sit 2 DIP apart. The vertical column keeps the 4 DIP gap.
    let gap = scale(dpi, 2);
    let sep_w = scale(dpi, 1).max(1);
    if input.tab_w < btn_w {
        return BarLay::hidden_bar();
    }
    let top = input.top;
    let edit = Px { left: input.tab_w - btn_w, top, right: input.tab_w, bottom: top + btn_h };
    let piece_w = |p: &Piece| if is_slot(p) { btn_w } else { sep_w };
    let take = |room: i32| -> usize {
        let mut n = 0;
        let mut buttons = 0usize;
        let mut used = 0i32;
        for (i, p) in pieces.iter().enumerate() {
            let add = piece_w(p) + if i == 0 { 0 } else { gap };
            if used + add > room {
                break;
            }
            if is_slot(p) {
                if buttons >= input.max_buttons {
                    break;
                }
                buttons += 1;
            }
            used += add;
            n = i + 1;
        }
        while n > 0 && matches!(pieces.get(n - 1), Some(Piece::Separator)) {
            n -= 1;
        }
        n
    };
    let room_full = (edit.left - gap - pad).max(0);
    let all = take(room_full);
    let (n, overflow) = if all == pieces.len() {
        (all, None)
    } else {
        let over_right = edit.left - gap;
        let over_left = over_right - btn_w;
        if over_left < pad {
            (0, None)
        } else {
            let room = (over_left - gap - pad).max(0);
            let over = Px { left: over_left, top, right: over_right, bottom: top + btn_h };
            (take(room), Some(over))
        }
    };
    let mut x = pad;
    let mut commands = Vec::new();
    let mut command_at = Vec::new();
    let mut separators = Vec::new();
    for (i, p) in pieces.iter().enumerate().take(n) {
        let w = piece_w(p);
        let r = Px { left: x, top, right: x + w, bottom: top + btn_h };
        if is_slot(p) {
            commands.push(r);
            command_at.push(i);
        } else {
            separators.push(r);
        }
        x += w + gap;
    }
    BarLay { shown: true, commands, command_at, separators, overflow, edit, hidden: (n..pieces.len()).collect() }
}

/// Buttons down the left column. Change sits at the bottom. Overflow sits just above it.
/// The column is not dragged. A short column keeps Change and folds the rest into ».
fn layout_column(pieces: &[Piece], input: &LayIn) -> BarLay {
    let dpi = input.dpi.max(1);
    let (btn_w, btn_h, gap, _) = button_metrics(dpi);
    let col_w = crate::command_col_px(dpi);
    let sep_h = scale(dpi, 1).max(1);
    let pad_y = (scale(dpi, 32) - btn_h) / 2;
    if col_w < btn_w || input.bottom <= input.top {
        return BarLay::hidden_bar();
    }
    let x = input.left + (col_w - btn_w) / 2;
    let content_top = input.top + pad_y.max(0);
    let edit_bottom = input.bottom - pad_y.max(0);
    let edit_top = edit_bottom - btn_h;
    if edit_top < content_top {
        return BarLay::hidden_bar();
    }
    let edit = Px { left: x, top: edit_top, right: x + btn_w, bottom: edit_bottom };
    let piece_h = |p: &Piece| if is_slot(p) { btn_h } else { sep_h };
    let take = |room: i32| -> usize {
        let mut n = 0;
        let mut buttons = 0usize;
        let mut used = 0i32;
        for (i, p) in pieces.iter().enumerate() {
            let add = piece_h(p) + if i == 0 { 0 } else { gap };
            if used + add > room {
                break;
            }
            if is_slot(p) {
                if buttons >= input.max_buttons {
                    break;
                }
                buttons += 1;
            }
            used += add;
            n = i + 1;
        }
        while n > 0 && matches!(pieces.get(n - 1), Some(Piece::Separator)) {
            n -= 1;
        }
        n
    };
    let room_full = (edit.top - gap - content_top).max(0);
    let all = take(room_full);
    let (n, overflow) = if all == pieces.len() {
        (all, None)
    } else {
        let over_bottom = edit.top - gap;
        let over_top = over_bottom - btn_h;
        if over_top < content_top {
            (0, None)
        } else {
            let room = (over_top - gap - content_top).max(0);
            let over = Px { left: x, top: over_top, right: x + btn_w, bottom: over_bottom };
            (take(room), Some(over))
        }
    };
    let mut y = content_top;
    let mut commands = Vec::new();
    let mut command_at = Vec::new();
    let mut separators = Vec::new();
    for (i, p) in pieces.iter().enumerate().take(n) {
        let h = piece_h(p);
        let r = Px { left: x, top: y, right: x + btn_w, bottom: y + h };
        if is_slot(p) {
            commands.push(r);
            command_at.push(i);
        } else {
            separators.push(r);
        }
        y += h + gap;
    }
    BarLay { shown: true, commands, command_at, separators, overflow, edit, hidden: (n..pieces.len()).collect() }
}

fn live_doc() -> Doc {
    state().doc.clone()
}

fn layout_now(o: &Origin) -> (Doc, BarLay) {
    let doc = live_doc();
    let lay = layout(&doc.pieces(), &o.lay_in());
    (doc, lay)
}

/// Top strip when the tabs are vertical. Left column when the tabs are horizontal.
pub(crate) fn bar_origin(a: &crate::App) -> Origin {
    let g = crate::geo(a);
    let s = |v: i32| v * a.dpi / 96;
    let btn_top = (s(32) - crate::tool_icon_px(a.dpi).1) / 2;
    let drives = crate::drive_slots(a.s.drive_buttons, a.drives.len());
    let icons = crate::icon_slots(g.w, btn_top, a.dpi, drives, a.s.bar);
    Origin { vertical_tabs: a.s.vertical_tabs, tab_w: icons.tab_w, top: if a.s.vertical_tabs { btn_top } else { g.top }, bottom: g.bot, left: crate::vtab_left(g.side, g.gap), dpi: a.dpi }
}

pub(crate) struct SlotMove {
    pub id: i32,
    pub rect: RECT,
    pub show: bool,
}

pub(crate) fn moves(o: &Origin) -> Vec<SlotMove> {
    let (_, lay) = layout_now(o);
    let mut out = Vec::with_capacity(CMD_CAPACITY as usize + 2);
    for i in 0..CMD_CAPACITY {
        let rect = lay.commands.get(i as usize).copied().unwrap_or_else(Px::empty);
        out.push(SlotMove { id: ID_CMD_FIRST + i, rect: rect.to_rect(), show: lay.shown && rect.live() });
    }
    let over = lay.overflow.filter(|_| lay.shown).unwrap_or_else(Px::empty);
    out.push(SlotMove { id: ID_CMD_OVERFLOW, rect: over.to_rect(), show: lay.shown && over.live() });
    let edit = if lay.shown { lay.edit } else { Px::empty() };
    out.push(SlotMove { id: ID_CMD_EDIT, rect: edit.to_rect(), show: edit.live() });
    out
}

pub(crate) fn is_command_control(id: i32) -> bool {
    (ID_CMD_FIRST..=ID_CMD_EDIT).contains(&id)
}

fn command_slot(id: i32) -> Option<usize> {
    (ID_CMD_FIRST..ID_CMD_OVERFLOW).contains(&id).then_some((id - ID_CMD_FIRST) as usize)
}

fn slot_face(id: i32, doc: &Doc, lay: &BarLay) -> Option<(Run, LineIcon)> {
    if !lay.shown {
        return None;
    }
    if id == ID_CMD_EDIT && lay.edit.live() {
        return Some((Run::Command(Action::EditToolbar), LineIcon::Edit));
    }
    if id == ID_CMD_OVERFLOW && lay.overflow.is_some() {
        return Some((Run::Command(Action::Overflow), LineIcon::Chevrons));
    }
    let slot = command_slot(id)?;
    let at = *lay.command_at.get(slot)?;
    let pieces = doc.pieces();
    let piece = pieces.get(at)?;
    Some((run_of(piece)?, piece_icon(piece)?))
}

fn placed_slot(id: i32) -> Option<(Run, LineIcon)> {
    if !is_command_control(id) {
        return None;
    }
    let origin = app(|a| if a.main.0.is_null() { None } else { Some(bar_origin(a)) })?;
    let (doc, lay) = layout_now(&origin);
    slot_face(id, &doc, &lay)
}

/// Icon plus the toggle and enabled flags the button paints. The window itself stays enabled.
pub(crate) struct CommandVisual {
    pub icon: LineIcon,
    pub enabled: bool,
    pub on: bool,
}

pub(crate) unsafe fn command_visual(id: i32) -> Option<CommandVisual> {
    let (run, icon) = placed_slot(id)?;
    let input = enable_input();
    let on = match &run {
        Run::Command(action) => command_toggled(*action, *action == Action::ShowHidden && crate::hidden()),
        Run::Folder(_) | Run::File(_) => false,
    };
    Some(CommandVisual { icon, enabled: run_enabled(&run, &input), on })
}

fn action_for(id: i32, doc: &Doc, lay: &BarLay) -> Option<Run> {
    slot_face(id, doc, lay).map(|(run, _)| run)
}

fn hidden_runs(doc: &Doc, lay: &BarLay) -> Vec<Run> {
    let pieces = doc.pieces();
    lay.hidden.iter().filter_map(|i| pieces.get(*i).and_then(run_of)).collect()
}

const CMD_BTN_SUB: usize = 41;
const CMD_MAIN_SUB: usize = 42;
const TIP_TOOL: usize = 1;

pub(crate) unsafe fn create_buttons(main: HWND) {
    for id in ID_CMD_FIRST..=ID_CMD_EDIT {
        let b = crate::child(main, w!("BUTTON"), w!(""), BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), id, HFONT::default());
        _ = ShowWindow(b, SW_HIDE);
        if !b.0.is_null() {
            _ = SetWindowSubclass(b, Some(cmd_btn_proc), CMD_BTN_SUB, 0);
        }
    }
    _ = SetWindowSubclass(main, Some(cmd_main_proc), CMD_MAIN_SUB, 0);
    let _ = ensure_tip(main);
    refresh_tips();
}

pub(crate) unsafe fn refresh_tips() {
    // Translated name of the bar. Buttons stay icon-only; this keeps the key live for readers.
    let _ = crate::tr(crate::Key::CommandToolbar);
    let main = app(|a| a.main);
    if main.0.is_null() {
        return;
    }
    let _ = ensure_tip(main);
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    if !tip.0.is_null() {
        crate::set_tip_font(tip);
    }
    let origin = app(|a| bar_origin(a));
    let (doc, lay) = layout_now(&origin);
    for id in ID_CMD_FIRST..=ID_CMD_EDIT {
        let Ok(b) = GetDlgItem(Some(main), id) else {
            continue;
        };
        let face = slot_face(id, &doc, &lay);
        if face_changed(id, face.clone()) {
            _ = InvalidateRect(Some(b), None, false);
        }
        let Some((run, _)) = face else { continue };
        let name = run_label(&run);
        if crate::text_of(b) != name {
            _ = SetWindowTextW(b, &HSTRING::from(name));
        }
    }
    // A resize can move the button under the pointer, or give that slot a new command.
    retip_under_cursor(main);
}

fn face_changed(id: i32, action: Option<(Run, LineIcon)>) -> bool {
    let mut st = state();
    if let Some(slot) = st.face.iter_mut().find(|(k, _)| *k == id) {
        if slot.1 == action {
            return false;
        }
        slot.1 = action;
        true
    } else {
        st.face.push((id, action));
        true
    }
}

/// Repaint every command button. `toggle_hidden` calls this so the accent follows
/// the menu and the shortcut, not only the toolbar click.
pub(crate) unsafe fn invalidate_commands() {
    let main = app(|a| a.main);
    if main.0.is_null() {
        return;
    }
    for id in ID_CMD_FIRST..=ID_CMD_EDIT {
        if let Ok(b) = GetDlgItem(Some(main), id) {
            _ = InvalidateRect(Some(b), None, false);
        }
    }
}

fn enable_input() -> EnableInput {
    let (pane, path) = app(|a| {
        let i = a.s.active;
        let path = a.s.paths.get(i).cloned().unwrap_or_default();
        (i, path)
    });
    let recent = crate::pane::recent::locked(pane);
    let sel = unsafe { crate::view_of::<IFolderView2>(pane).and_then(|fv| fv.ItemCount(SVGIO_SELECTION).ok()).unwrap_or(0) as u32 };
    let clip = state().clip_pasteable;
    EnableInput { selection: sel, folder_can_receive: folder_can_receive(&path, recent), clipboard_pasteable: clip }
}

pub(crate) unsafe fn sync_enabled() {
    let main = app(|a| a.main);
    if main.0.is_null() {
        return;
    }
    let origin = app(|a| bar_origin(a));
    let (doc, lay) = layout_now(&origin);
    let input = enable_input();
    let pieces = doc.pieces();
    for i in 0..CMD_CAPACITY {
        let id = ID_CMD_FIRST + i;
        let Ok(b) = GetDlgItem(Some(main), id) else {
            continue;
        };
        let Some(at) = lay.command_at.get(i as usize).copied() else {
            continue;
        };
        let Some(run) = pieces.get(at).and_then(run_of) else {
            continue;
        };
        // The window stays enabled. A disabled HWND never sees the mouse, so its tooltip cannot show.
        if !IsWindowEnabled(b).as_bool() {
            _ = EnableWindow(b, true);
        }
        if logical_enabled_changed(id, run_enabled(&run, &input)) {
            _ = InvalidateRect(Some(b), None, false);
        }
    }
}

fn logical_enabled_changed(id: i32, on: bool) -> bool {
    let mut st = state();
    if let Some(slot) = st.enabled.iter_mut().find(|(k, _)| *k == id) {
        if slot.1 == on {
            return false;
        }
        slot.1 = on;
        true
    } else {
        st.enabled.push((id, on));
        true
    }
}

thread_local! {
    static TIP_BUSY: Cell<bool> = const { Cell::new(false) };
    /// A dismiss arrived while a tip message was already on the stack.
    static TIP_SUPPRESS: Cell<bool> = const { Cell::new(false) };
}

struct TipGuard;
impl Drop for TipGuard {
    fn drop(&mut self) {
        TIP_BUSY.with(|c| c.set(false));
    }
}

/// Puts the keyboard back if showing or hiding the tip moved it onto the tip.
/// A button that is tracking a press keeps capture: moving focus then releases
/// that capture and the click never becomes `BN_CLICKED`.
struct TipFocus {
    main: HWND,
    prior: HWND,
}

impl Drop for TipFocus {
    fn drop(&mut self) {
        unsafe { keep_focus_off_tip(self.main, self.prior) }
    }
}

unsafe fn keep_focus_off_tip(main: HWND, prior: HWND) {
    if !GetCapture().0.is_null() {
        return;
    }
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    let now = GetFocus();
    if tip.0.is_null() || now != tip {
        return;
    }
    let prior_ok = !prior.0.is_null() && prior != tip && IsWindow(Some(prior)).as_bool();
    if prior_ok && !restore_list_after_toolbar(toolbar_seat_of(prior, main)) {
        _ = SetFocus(Some(prior));
        return;
    }
    restore_toolbar_focus(main);
}

fn track_info(owner: HWND, label: &HSTRING) -> TTTOOLINFOW {
    TTTOOLINFOW { cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32, uFlags: TTF_TRACK | TTF_ABSOLUTE, hwnd: owner, uId: TIP_TOOL, lpszText: PWSTR(label.as_ptr() as *mut u16), ..Default::default() }
}

fn pack_xy(x: i32, y: i32) -> LPARAM {
    LPARAM((x as u16 as usize as isize) | ((y as u16 as usize as isize) << 16))
}

/// Toolbar tips use their own tooltip window. The shared one also runs tracking tips
/// for tabs and the address bar, and a TTF_SUBCLASS tool on that window stops showing
/// once a tracking tip has been activated. Disabled buttons never see the mouse, so
/// this tip is placed from the button subclass and from the main window.
unsafe fn ensure_tip(owner: HWND) -> HWND {
    let existing = state().tip;
    if existing != 0 {
        let tip = HWND(existing as *mut std::ffi::c_void);
        if IsWindow(Some(tip)).as_bool() {
            return tip;
        }
    }
    // TTS_ALWAYSTIP | TTS_NOPREFIX | TTS_NOANIMATE | TTS_NOFADE.
    // WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW: a tracking tip is a popup owned by the
    // main frame. Without those styles ShowWindow inside TTM_TRACKACTIVATE can make
    // the tip the foreground window. SetFocus to the command button or the file
    // list then fails, and the keyboard stays off the pane.
    let style = WS_POPUP | WINDOW_STYLE(0x01 | 0x02 | 0x10 | 0x20);
    let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let Ok(tip) = CreateWindowExW(ex, TOOLTIPS_CLASSW, None, style, 0, 0, 0, 0, Some(owner), None, None, None) else {
        return HWND::default();
    };
    crate::set_tip_font(tip);
    let mut st = state();
    st.tip = tip.0 as isize;
    st.tip_added = false;
    st.live_tip = None;
    tip
}

unsafe fn command_button_at_cursor(main: HWND) -> Option<HWND> {
    if main.0.is_null() {
        return None;
    }
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        return None;
    }
    let mut client = pt;
    if !ScreenToClient(main, &mut client).as_bool() {
        return None;
    }
    // Skip hidden slots. Do not skip disabled ones: a gray command must still hit.
    let child = ChildWindowFromPointEx(main, client, CWP_SKIPINVISIBLE);
    if child.0.is_null() || child == main {
        return None;
    }
    let id = GetDlgCtrlID(child);
    if !is_command_control(id) || !IsWindowVisible(child).as_bool() {
        return None;
    }
    Some(child)
}

unsafe fn retip_under_cursor(main: HWND) {
    let over = command_button_at_cursor(main);
    match want_tip(main, TipCause::Relayout, over.is_some()) {
        TipOrder::Show => show_tip_for(over.unwrap_or_default()),
        TipOrder::Hide => dismiss_tip(main),
    }
}

unsafe fn show_tip_for(btn: HWND) {
    if btn.0.is_null() || !IsWindowVisible(btn).as_bool() {
        return;
    }
    let main = GetParent(btn).unwrap_or_default();
    if want_tip(main, TipCause::Hover, true) != TipOrder::Show {
        dismiss_tip(main);
        return;
    }
    // One-shot. Arm on the show as well as the move, including a disabled slot
    // whose move was delivered to the main window.
    arm_leave(btn);
    let id = GetDlgCtrlID(btn);
    let Some((run, _)) = placed_slot(id) else {
        dismiss_tip(main);
        return;
    };
    let mut rc = RECT::default();
    if GetWindowRect(btn, &mut rc).is_err() || rc.right <= rc.left || rc.bottom <= rc.top {
        dismiss_tip(main);
        return;
    }
    apply_tip(main, Some(LiveTip { control: id, text: run_tip(&run), left: rc.left, top: rc.top, right: rc.right, bottom: rc.bottom }));
}

fn tip_window_flags(main: HWND) -> (bool, bool, bool) {
    let cap = unsafe { GetCapture() };
    let held = !cap.0.is_null() && is_command_control(unsafe { GetDlgCtrlID(cap) });
    let active = !main.0.is_null() && unsafe { GetActiveWindow() } == main;
    let minimized = !main.0.is_null() && unsafe { IsIconic(main) }.as_bool();
    (active, minimized, held)
}

fn want_tip(main: HWND, cause: TipCause, over_button: bool) -> TipOrder {
    let (active, minimized, held) = tip_window_flags(main);
    tip_order(cause, active, minimized, over_button, held)
}

/// Hide when [`tip_order`] says this cause drops the bubble. Press, leave,
/// deactivation, and minimize all do. A hover does not come through here.
pub(crate) unsafe fn dismiss_for(main: HWND, cause: TipCause) {
    let over = command_button_at_cursor(main).is_some();
    if want_tip(main, cause, over) == TipOrder::Hide {
        dismiss_tip(main);
    }
}

/// `TTM_TRACKACTIVATE` false and `TTM_POP`, and forget which button was hovered.
/// Safe to call again. A dismiss that lands inside a tip message is applied
/// before that message shows the bubble.
pub(crate) unsafe fn dismiss_tip(main: HWND) {
    if main.0.is_null() {
        return;
    }
    if TIP_BUSY.with(|c| c.replace(true)) {
        TIP_SUPPRESS.with(|c| c.set(true));
        state().live_tip = None;
        return;
    }
    let _guard = TipGuard;
    deactivate_tip(main);
}

unsafe fn deactivate_tip(main: HWND) {
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    state().live_tip = None;
    if !tip.0.is_null() && IsWindow(Some(tip)).as_bool() {
        let label = HSTRING::new();
        let ti = track_info(main, &label);
        let lp = LPARAM(&ti as *const _ as isize);
        SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(lp));
        SendMessageW(tip, TTM_POP, None, None);
    }
    // A dismiss that arrived from inside the pop has been honoured. The next hover may show.
    TIP_SUPPRESS.with(|c| c.set(false));
}

/// Arm `TME_LEAVE` on the command button under the pointer when the frame is
/// in front. Coming back from a terminal does not deliver the leave that was
/// lost while Maguro was inactive.
pub(crate) unsafe fn arm_tip_leave(main: HWND) {
    let over = command_button_at_cursor(main);
    let (active, minimized, _) = tip_window_flags(main);
    if tip_rearm_leave(over.is_some(), active, minimized) {
        arm_leave(over.unwrap_or_default());
    }
}

/// Hide unless focus moved onto the tooltip window itself.
pub(crate) unsafe fn note_focus_left(main: HWND, next: HWND) {
    if main.0.is_null() {
        return;
    }
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    let to_tip = !tip.0.is_null() && next == tip;
    if tip_hides_on_focus(to_tip) {
        dismiss_for(main, TipCause::KillFocus);
    }
}

unsafe fn apply_tip(main: HWND, next: Option<LiveTip>) {
    if main.0.is_null() {
        return;
    }
    if TIP_BUSY.with(|c| c.replace(true)) {
        if next.is_none() {
            TIP_SUPPRESS.with(|c| c.set(true));
        }
        return;
    }
    let _guard = TipGuard;
    // Drop runs while TIP_BUSY is still set, so a focus fix cannot re-enter the tip.
    let prior = GetFocus();
    let _back = TipFocus { main, prior };
    if next.is_some() && TIP_SUPPRESS.with(|c| c.get()) {
        deactivate_tip(main);
        return;
    }
    let live = state().live_tip.clone();
    match tip_step(live.as_ref(), next.as_ref()) {
        TipStep::Idle => {}
        TipStep::Hide => deactivate_tip(main),
        TipStep::Show => {
            let Some(next) = next else { return };
            if TIP_SUPPRESS.with(|c| c.get()) {
                deactivate_tip(main);
                return;
            }
            let tip = ensure_tip(main);
            if tip.0.is_null() {
                return;
            }
            let vertical = app(|a| a.s.vertical_tabs);
            let label = HSTRING::from(next.text.as_str());
            let ti = track_info(main, &label);
            let lp = LPARAM(&ti as *const _ as isize);
            if !state().tip_added {
                SendMessageW(tip, TTM_ADDTOOLW, None, Some(lp));
                state().tip_added = true;
            }
            SendMessageW(tip, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(720)));
            SendMessageW(tip, TTM_UPDATETIPTEXTW, None, Some(lp));
            let size = SendMessageW(tip, TTM_GETBUBBLESIZE, None, Some(lp));
            let (tw, th) = if size.0 != 0 {
                ((size.0 & 0xffff) as i32, ((size.0 >> 16) & 0xffff) as i32)
            } else {
                let chars = next.text.chars().count().max(8) as i32;
                (chars * 8 + 16, 26)
            };
            let item = RECT { left: next.left, top: next.top, right: next.right, bottom: next.bottom };
            let work = crate::tip_work_area(item);
            let (x, y) = crate::tip_place(item, tw, th, command_tip_side(vertical), work);
            SendMessageW(tip, TTM_TRACKPOSITION, None, Some(pack_xy(x, y)));
            // Opening a terminal deactivates this frame from inside the activate call.
            SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(lp));
            if TIP_SUPPRESS.with(|c| c.get()) {
                deactivate_tip(main);
                return;
            }
            state().live_tip = Some(next);
        }
    }
}

unsafe fn arm_leave(h: HWND) {
    let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
    _ = TrackMouseEvent(&mut t);
}

unsafe extern "system" fn cmd_btn_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, data: usize) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { cmd_btn_proc_inner(h, msg, wp, lp, id, data) }))
}

unsafe fn cmd_btn_proc_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            arm_leave(h);
            // A press hides the bubble first. Moves while the button has capture
            // used to show it again, and the command then never got a leave.
            if GetCapture() != h {
                show_tip_for(h);
            }
        }
        WM_MOUSELEAVE => {
            let main = GetParent(h).unwrap_or_default();
            // A disabled neighbour does not receive this move. Hit-test includes it.
            if let Some(btn) = command_button_at_cursor(main) {
                if btn != h {
                    arm_leave(btn);
                    show_tip_for(btn);
                }
            } else {
                dismiss_for(main, TipCause::Leave);
            }
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK => {
            // Before the button takes capture and before the command runs. A move
            // during the press must not put the bubble back: opening a terminal
            // never delivers the leave.
            let main = GetParent(h).unwrap_or_default();
            dismiss_for(main, TipCause::Press);
        }
        WM_KILLFOCUS => {
            let main = GetParent(h).unwrap_or_default();
            note_focus_left(main, HWND(wp.0 as *mut std::ffi::c_void));
        }
        WM_NCDESTROY => {
            _ = RemoveWindowSubclass(h, Some(cmd_btn_proc), CMD_BTN_SUB);
        }
        _ => {}
    }
    let result = DefSubclassProc(h, msg, wp, lp);
    // Mouse-down gives the button focus and capture. Moving focus before the
    // button finishes the click releases that capture, so `BN_CLICKED` never
    // arrives. Restore only after the click, the menu, or the dialog returns.
    if matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_KEYUP) {
        restore_toolbar_focus(GetParent(h).unwrap_or_default());
    }
    result
}

/// Mouse moves over a disabled command button are delivered to the main window.
unsafe extern "system" fn cmd_main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, data: usize) -> LRESULT {
    crate::crash::catch_wnd(std::panic::AssertUnwindSafe(|| unsafe { cmd_main_proc_inner(h, msg, wp, lp, id, data) }))
}

unsafe fn cmd_main_proc_inner(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_MOUSEMOVE {
        if let Some(btn) = command_button_at_cursor(h) {
            if !IsWindowEnabled(btn).as_bool() {
                arm_leave(btn);
                show_tip_for(btn);
            }
        } else if state().live_tip.is_some() {
            dismiss_for(h, TipCause::Leave);
        }
    }
    DefSubclassProc(h, msg, wp, lp)
}

pub(crate) unsafe fn note_clipboard() {
    if let Some(v) = probe_clipboard() {
        state().clip_pasteable = v;
    }
    sync_enabled();
}

unsafe fn probe_clipboard() -> Option<bool> {
    if OpenClipboard(None).is_err() {
        return None;
    }
    let hdrop = IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok();
    let mut count = 0u32;
    if hdrop {
        if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
            count = DragQueryFileW(HDROP(h.0), u32::MAX, None);
        }
    }
    let pic = IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() || IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok() || IsClipboardFormatAvailable(RegisterClipboardFormatW(w!("PNG"))).is_ok();
    _ = CloseClipboard();
    Some(clipboard_pasteable(hdrop && count > 0, pic))
}

pub(crate) unsafe fn install_clipboard(main: HWND) {
    _ = AddClipboardFormatListener(main);
}

pub(crate) unsafe fn remove_clipboard(main: HWND) {
    _ = RemoveClipboardFormatListener(main);
}

/// Where the keyboard is when a toolbar action returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusSeat {
    /// A command button, More, Change, or a child of one of those.
    Toolbar,
    /// The main frame itself. A menu or a dialog often returns focus here,
    /// and a failed `SetFocus` on the button lands here too.
    Frame,
    /// The toolbar's tracking tooltip took the keyboard.
    Tooltip,
    /// `GetFocus` is null or the window is gone.
    Lost,
    /// A dialog, the address bar, rename, the editor, or some other window.
    Elsewhere,
}

/// One window on the parent walk from `GetFocus` toward the main frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FocusHop {
    pub id: i32,
    pub parent_is_main: bool,
    pub is_main: bool,
    pub is_tooltip: bool,
}

/// The file list gets the keyboard back when nothing the action opened still
/// has it. The toolbar editor, Settings, Properties, rename, and the address
/// bar are [`FocusSeat::Elsewhere`] and keep focus. The top strip and the left
/// column use the same control ids, and More runs inside the same command, so
/// one decision covers both bars and the overflow menu.
pub(crate) fn restore_list_after_toolbar(seat: FocusSeat) -> bool {
    matches!(seat, FocusSeat::Toolbar | FocusSeat::Frame | FocusSeat::Tooltip | FocusSeat::Lost)
}

/// `hops` is `GetFocus`, then each parent, including the main frame when the
/// walk reaches it. A command slot, More, and Change count only when that
/// window's parent is the main frame, so a dialog that reuses a command id
/// does not. Reaching the frame from any other window keeps focus there.
pub(crate) fn seat_from_chain(hops: &[FocusHop]) -> FocusSeat {
    let Some(first) = hops.first() else {
        return FocusSeat::Lost;
    };
    if first.is_tooltip {
        return FocusSeat::Tooltip;
    }
    if first.is_main {
        return FocusSeat::Frame;
    }
    for hop in hops {
        if hop.is_main {
            return FocusSeat::Elsewhere;
        }
        if hop.parent_is_main && is_command_control(hop.id) {
            return FocusSeat::Toolbar;
        }
    }
    FocusSeat::Elsewhere
}

/// One window on the parent walk. `id` is that window's control id.
/// `parent_is_main` is true only when its parent is the main frame.
#[cfg(test)]
pub(crate) fn toolbar_focus_from(id: i32, parent_is_main: bool) -> FocusSeat {
    seat_from_chain(&[FocusHop { id, parent_is_main, is_main: false, is_tooltip: false }])
}

fn focus_hop(h: HWND, main: HWND, tip: HWND) -> FocusHop {
    FocusHop {
        id: unsafe { GetDlgCtrlID(h) },
        parent_is_main: unsafe { GetParent(h).unwrap_or_default() == main },
        is_main: h == main,
        is_tooltip: !tip.0.is_null() && h == tip,
    }
}

/// `GetFocus` after the action, walked up to the main frame.
unsafe fn toolbar_seat_of(focus: HWND, main: HWND) -> FocusSeat {
    if main.0.is_null() || focus.0.is_null() || !IsWindow(Some(focus)).as_bool() {
        return FocusSeat::Lost;
    }
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    let mut hops = Vec::with_capacity(8);
    let mut h = focus;
    for _ in 0..8 {
        if h.0.is_null() {
            break;
        }
        let hop = focus_hop(h, main, tip);
        let at_main = hop.is_main;
        hops.push(hop);
        if at_main {
            break;
        }
        h = GetParent(h).unwrap_or_default();
    }
    seat_from_chain(&hops)
}

unsafe fn toolbar_seat_now(main: HWND) -> FocusSeat {
    toolbar_seat_of(GetFocus(), main)
}

thread_local! {
    static FOCUS_BUSY: Cell<bool> = const { Cell::new(false) };
}

struct FocusGuard;
impl Drop for FocusGuard {
    fn drop(&mut self) {
        FOCUS_BUSY.with(|c| c.set(false));
    }
}

unsafe fn restore_toolbar_focus(main: HWND) {
    if main.0.is_null() || FOCUS_BUSY.with(|c| c.replace(true)) {
        return;
    }
    let _guard = FocusGuard;
    if !restore_list_after_toolbar(toolbar_seat_now(main)) {
        return;
    }
    // The tip used to become the foreground window. `SetFocus` onto the list
    // only works while this frame is foreground.
    let tip = HWND(state().tip as *mut std::ffi::c_void);
    if !tip.0.is_null() && GetForegroundWindow() == tip {
        _ = SetForegroundWindow(main);
    }
    crate::focus_view(app(|a| a.s.active));
}

pub(crate) unsafe fn command(owner: HWND, id: usize) -> bool {
    let id = id as i32;
    if !is_command_control(id) {
        return false;
    }
    let origin = app(|a| bar_origin(a));
    let (doc, lay) = layout_now(&origin);
    let Some(fired) = action_for(id, &doc, &lay) else {
        return true;
    };
    if !lay.shown {
        return true;
    }
    // More opens its menu inside `run` and then runs the chosen command there,
    // so this single check covers the overflow path. The vertical column uses
    // the same buttons and the same `command` entry.
    run(owner, fired, &doc, &lay, id);
    // The button took focus on the click. Give it back to the active pane's
    // list unless the action left it on a field or a window it opened
    // (toolbar editor, properties, rename, address bar). More's menu and the
    // chosen command both return here, and the left column uses this entry.
    restore_toolbar_focus(owner);
    true
}

unsafe fn run(owner: HWND, fired: Run, doc: &Doc, lay: &BarLay, from: i32) {
    // Mouse-down already hid the bubble. Hide again here so a move during the
    // click, and a command chosen from More, cannot leave it up. The terminal
    // then takes the foreground and no leave arrives.
    dismiss_for(owner, TipCause::Press);
    let pane = app(|a| a.s.active);
    match fired {
        Run::Folder(path) => open_saved_folder(pane, &path),
        Run::File(path) => open_saved_file(owner, &path),
        Run::Command(action) => run_command(owner, action, doc, lay, from, pane),
    }
}

unsafe fn run_command(owner: HWND, action: Action, doc: &Doc, lay: &BarLay, from: i32, pane: usize) {
    let input = enable_input();
    if !enabled(action, &input) {
        return;
    }
    match action {
        Action::NewFolder => crate::new_folder(pane),
        Action::NewFile => crate::new_file(pane),
        Action::DeleteSelection => crate::delete_selection(pane, false),
        // Copy and cut both run on the worker. What it cannot take goes to the view's own Paste.
        Action::Paste => {
            if !crate::try_paste(pane) {
                crate::shell_paste(pane);
            }
        }
        Action::RenameInPlace => {
            let _ = crate::rename_inplace(pane);
        }
        Action::ViewMode => crate::popup_view_modes(owner, pane, button_point(from)),
        Action::Properties => crate::show_selection_properties(pane),
        Action::Mark => crate::marks::toggle_pane(pane),
        Action::EditToolbar => crate::ui::toolbaredit::open(owner),
        Action::Overflow => popup_overflow(owner, doc, lay),
        Action::CopyPath => crate::copy_location(pane),
        Action::BatchRename => crate::rename::open_pane(pane),
        Action::InvertSelection => crate::invert_selection(pane),
        Action::NewTab => crate::new_tab(),
        Action::CloseTab => {
            crate::flush_pending_tab();
            crate::close_tab(app(|x| x.s.tab));
        }
        Action::DuplicateTab => {
            crate::flush_pending_tab();
            crate::duplicate_tab(app(|x| x.s.tab));
        }
        Action::ShowHidden => crate::toggle_hidden(),
        Action::Zoom => crate::toggle_zoom(),
        Action::QuickLook => crate::open_peek(pane),
        Action::PreviewPane => crate::toggle_preview(),
        Action::SplitRight => crate::header_split(pane, false),
        Action::SplitBelow => crate::header_split(pane, true),
        // Preset folder and file buttons replaced the pick-at-click command.
        Action::Open => {}
        Action::PaneColor => crate::popup_pane_tint(owner, pane, button_point(from)),
        Action::TabColor => popup_tab_color(owner, from),
        Action::ClosePane => crate::header_close(pane),
        Action::Terminal => open_terminal(owner, pane),
        Action::TerminalAdmin => open_terminal_admin(owner, pane),
    }
}

unsafe fn button_point(id: i32) -> POINT {
    let (main, vertical_tabs) = app(|a| (a.main, a.s.vertical_tabs));
    let mut rc = RECT::default();
    if let Ok(b) = GetDlgItem(Some(main), id) {
        _ = GetWindowRect(b, &mut rc);
    }
    // Top strip: the menu opens under the button. Left column: it opens to the right,
    // so a button at the bottom of the column is not pushed off the screen.
    if vertical_tabs {
        POINT { x: rc.left, y: rc.bottom }
    } else {
        POINT { x: rc.right, y: rc.top }
    }
}

unsafe fn open_saved_folder(pane: usize, path: &str) {
    if !target_present(path) {
        let _ = MessageBeep(MB_OK);
        return;
    }
    let _ = crate::navigate(pane, path);
}

unsafe fn open_saved_file(owner: HWND, path: &str) {
    if !target_present(path) {
        let _ = MessageBeep(MB_OK);
        return;
    }
    let _ = ShellExecuteW(Some(owner), w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
}

unsafe fn popup_tab_color(owner: HWND, from: i32) {
    crate::flush_pending_tab();
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING, 1, &HSTRING::from(crate::tr(crate::Key::None)));
    for (k, &(_, label)) in crate::TAB_COLORS.iter().enumerate() {
        let _ = AppendMenuW(menu, MF_STRING, (2 + k) as usize, &HSTRING::from(crate::tr(label)));
    }
    let pt = button_point(from);
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, owner, None).0 as usize;
    _ = DestroyMenu(menu);
    let tab = app(|a| a.s.tab);
    match pick {
        1 => crate::set_tab_color(tab, None),
        n if n >= 2 && n - 2 < crate::TAB_COLORS.len() => crate::set_tab_color(tab, Some(crate::TAB_COLORS[n - 2].0)),
        _ => {}
    }
}

unsafe fn open_terminal(owner: HWND, pane: usize) {
    let shown = crate::pane_parsing_name(pane);
    let Some(dir) = crate::terminal_dir(&shown) else {
        let _ = MessageBeep(MB_ICONWARNING);
        return;
    };
    crate::open_here(owner, &dir);
}

unsafe fn open_terminal_admin(owner: HWND, pane: usize) {
    let shown = crate::pane_parsing_name(pane);
    let Some(dir) = crate::terminal_dir(&shown) else {
        let _ = MessageBeep(MB_ICONWARNING);
        return;
    };
    // UAC cancel stays quiet. `launch` already treats ERROR_CANCELLED that way.
    crate::launch_admin_terminal(owner, &dir);
}

/// Separator lines for the current bar. Empty when the bar is hidden or has no separators.
pub(crate) fn separator_rects() -> Vec<RECT> {
    let origin = app(|a| if a.main.0.is_null() { None } else { Some(bar_origin(a)) });
    let Some(origin) = origin else {
        return Vec::new();
    };
    let (_, lay) = layout_now(&origin);
    if !lay.shown {
        return Vec::new();
    }
    lay.separators.into_iter().filter(|r| r.live()).map(|r| r.to_rect()).collect()
}

unsafe fn popup_overflow(owner: HWND, doc: &Doc, lay: &BarLay) {
    let rows = hidden_runs(doc, lay);
    if rows.is_empty() {
        return;
    }
    let Ok(menu) = CreatePopupMenu() else { return };
    let input = enable_input();
    for (i, fired) in rows.iter().enumerate() {
        let flags = if run_enabled(fired, &input) { MF_STRING } else { MF_STRING | MF_GRAYED };
        _ = AppendMenuW(menu, flags, i + 1, &HSTRING::from(run_label(fired)));
    }
    let at = button_point(ID_CMD_OVERFLOW);
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, at.x, at.y, None, owner, None).0 as usize;
    _ = DestroyMenu(menu);
    if let Some(fired) = rows.get(pick.wrapping_sub(1)) {
        run(owner, fired.clone(), doc, lay, ID_CMD_OVERFLOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(vertical: bool, tab_w: i32, pieces: &[Piece]) -> BarLay {
        layout(pieces, &LayIn { vertical_tabs: vertical, tab_w, top: 4, bottom: 0, left: 0, dpi: 96, max_buttons: CMD_CAPACITY as usize })
    }

    /// `height` is the column, starting under a 32px tab row, at the window's left edge.
    fn column(height: i32, left: i32, pieces: &[Piece]) -> BarLay {
        layout(pieces, &LayIn { vertical_tabs: false, tab_w: 0, top: 32, bottom: 32 + height, left, dpi: 96, max_buttons: CMD_CAPACITY as usize })
    }

    fn defaults() -> Vec<Piece> {
        Doc::default_layout().pieces()
    }

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("maguro-toolbar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn missing_file_uses_the_default_and_does_not_create_one() {
        let dir = scratch("missing");
        let path = dir.join(FILE_NAME);
        assert!(matches!(read_file(&path), Read::Missing));
        assert!(!path.exists());
        let doc = or_default(read_file(&path));
        assert_eq!(doc.command_ids(), ["file.dir", "file.new", "file.delete", "file.paste", "edit.rename", "view.mode", "file.properties", "file.mark", "view.hidden", "pane.split.right", "pane.split.below", "pane.close", "file.terminal"]);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_uses_the_default_and_leaves_the_bytes() {
        let dir = scratch("corrupt");
        let path = dir.join(FILE_NAME);
        let original = b"{ this is not json";
        std::fs::write(&path, original).unwrap();
        assert!(matches!(read_file(&path), Read::Corrupt));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(or_default(read_file(&path)).command_ids(), Doc::default_layout().command_ids());
        std::fs::write(&path, b"[]").unwrap();
        assert!(matches!(read_file(&path), Read::Corrupt));
        assert_eq!(std::fs::read(&path).unwrap(), b"[]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_keeps_commands_icons_program_tokens_and_extra_fields() {
        let raw = r#"{
            "version": 1,
            "note": "keep",
            "items": [
                {"type": "command", "id": "file.dir", "icon": {"mode": "auto"}},
                {"type": "command", "id": "file.paste", "icon": {"mode": "manual", "pack": "paste"}},
                {"type": "command", "id": "file.delete", "icon": {"mode": "manual", "file": "D:\\icons\\del.png"}},
                {"type": "command", "id": "edit.rename", "icon": {"mode": "automatic"}},
                {"type": "separator"},
                {"type": "folder", "path": "D:\\Work", "icon": {"mode": "auto"}},
                {"type": "file", "path": "D:\\Work\\a.txt", "icon": {"mode": "auto"}},
                {"type": "program", "exe": "D:\\tools\\run.exe", "args": "\"%path%\"", "dir": "%dir%", "icon": {"mode": "auto"}},
                {"type": "widget", "name": "later"}
            ]
        }"#;
        let doc = parse(raw.as_bytes()).unwrap();
        let back = parse(doc.to_json().as_bytes()).unwrap();
        assert_eq!(back, doc);
        assert_eq!(doc.extra.get("note").and_then(|v| v.as_str()), Some("keep"));
        let json = doc.to_json();
        assert!(json.contains("%path%"), "{json}");
        assert!(json.contains("%dir%"), "{json}");
        assert!(json.contains("del.png"), "{json}");
        assert!(json.contains("widget"), "{json}");
        let pieces = doc.pieces();
        assert!(pieces.iter().any(|p| matches!(p, Piece::Separator)));
        assert_eq!(pieces.iter().filter(|p| matches!(p, Piece::Command { .. })).count(), 4);
        assert_eq!(pieces.len(), 7, "folder and file targets stay; program and unknown items do not");
        match &pieces[5] {
            Piece::Folder { path, icon } => {
                assert_eq!(path, "D:\\Work");
                assert_eq!(icon, &IconChoice::Auto);
                assert_eq!(piece_icon(&pieces[5]), Some(LineIcon::OpenFolder));
            }
            other => panic!("folder {other:?}"),
        }
        match &pieces[6] {
            Piece::File { path, icon } => {
                assert_eq!(path, "D:\\Work\\a.txt");
                assert_eq!(icon, &IconChoice::Auto);
                assert_eq!(piece_icon(&pieces[6]), Some(LineIcon::PinFile));
            }
            other => panic!("file {other:?}"),
        }
        match &pieces[1] {
            Piece::Command { action, icon } => {
                assert_eq!(*action, Action::Paste);
                assert_eq!(icon, &IconChoice::Manual { pack: Some("paste".into()), file: None });
                assert_eq!(drawn_icon(*action, icon), LineIcon::Paste);
            }
            _ => panic!("paste"),
        }
        match &pieces[2] {
            Piece::Command { action, icon } => {
                assert_eq!(drawn_icon(*action, icon), LineIcon::Del, "a file icon is stored and the command icon is drawn");
            }
            _ => panic!("delete"),
        }
    }

    #[test]
    fn save_writes_toolbar_json_and_leaves_neighbours_alone() {
        let dir = scratch("save");
        std::fs::write(dir.join("maguro.json"), b"{\"version\":1}\n").unwrap();
        std::fs::write(dir.join("grids.ini"), b"sentinel").unwrap();
        let path = dir.join(FILE_NAME);
        let doc = Doc::default_layout();
        save(&path, &doc).unwrap();
        assert_eq!(or_default(read_file(&path)).command_ids(), doc.command_ids());
        assert_eq!(std::fs::read(dir.join("maguro.json")).unwrap(), b"{\"version\":1}\n");
        assert_eq!(std::fs::read(dir.join("grids.ini")).unwrap(), b"sentinel");
        assert_ne!(path.file_name().and_then(|n| n.to_str()), Some("maguro.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_commands_map_to_actions_and_rename_is_f2_not_batch() {
        let doc = Doc::default_layout();
        assert_eq!(doc.command_ids(), ["file.dir", "file.new", "file.delete", "file.paste", "edit.rename", "view.mode", "file.properties", "file.mark", "view.hidden", "pane.split.right", "pane.split.below", "pane.close", "file.terminal"]);
        assert_eq!(action_of("edit.rename"), Some(Action::RenameInPlace));
        assert_eq!(action_of("file.rename"), Some(Action::BatchRename), "file.rename is batch rename, available for the toolbar but not in the default layout");
        assert!(!doc.command_ids().iter().any(|id| *id == "file.rename"));
        assert_eq!(key_id(Action::RenameInPlace), None);
        assert_eq!(fixed_shortcut(Action::RenameInPlace), Some("F2"));
        assert_eq!(fixed_shortcut(Action::DeleteSelection), Some("Del"));
        assert_eq!(fixed_shortcut(Action::Paste), Some("Ctrl+V"));
        assert_eq!(key_id(Action::NewFolder), Some("file.dir"));
        assert_eq!(key_id(Action::NewFile), Some("file.new"));
        assert_eq!(key_id(Action::Mark), Some("file.mark"));
        assert_eq!(label_key(Action::RenameInPlace), crate::Key::Rename);
        assert_ne!(label_key(Action::RenameInPlace), crate::Key::BatchRename);
        let batch = crate::keys::index("file.rename").unwrap();
        assert_eq!(crate::keys::id_of(batch), "file.rename");
        assert_eq!(crate::keys::chord_text(batch), "Ctrl+Shift+R");
        let tip = tooltip(Action::RenameInPlace);
        assert_eq!(tip, "Rename (F2)");
        assert!(!tip.contains("Ctrl+Shift+R"));
        assert_eq!(tooltip(Action::NewFolder), "New folder (Ctrl+Shift+N)");
        assert_eq!(tooltip(Action::NewFile), "New text file (Ctrl+Alt+N)");
        assert_eq!(tooltip(Action::DeleteSelection), "Delete (Del)");
        assert_eq!(tooltip(Action::Paste), "Paste (Ctrl+V)");
        assert_eq!(crate::tr(crate::Key::CommandToolbar), "Command toolbar");
        assert_eq!(crate::tr(crate::Key::ToolbarOverflow), "More commands");
        assert_eq!(crate::tr(crate::Key::ToolbarChange), "Change");
        assert_eq!(crate::tr(crate::Key::ToolbarView), "View");
        assert!(doc.to_json().contains("\"type\": \"command\""));
        assert!(!doc.items.iter().any(|it| matches!(it, Item::Raw(_))));
        let pieces = doc.pieces();
        assert_eq!(pieces.len(), 14);
        assert!(matches!(pieces[8], Piece::Separator));
        match &pieces[9] {
            Piece::Command { action, .. } => assert_eq!(*action, Action::ShowHidden),
            _ => panic!("view.hidden"),
        }
        match &pieces[10] {
            Piece::Command { action, .. } => assert_eq!(*action, Action::SplitRight),
            _ => panic!("pane.split.right"),
        }
        match &pieces[11] {
            Piece::Command { action, .. } => assert_eq!(*action, Action::SplitBelow),
            _ => panic!("pane.split.below"),
        }
        match &pieces[12] {
            Piece::Command { action, .. } => assert_eq!(*action, Action::ClosePane),
            _ => panic!("pane.close"),
        }
        match &pieces[13] {
            Piece::Command { action, .. } => assert_eq!(*action, Action::Terminal),
            _ => panic!("file.terminal"),
        }
    }

    #[test]
    fn a_saved_toolbar_is_kept_when_the_built_in_default_grows() {
        let dir = scratch("saved-stays");
        let path = dir.join(FILE_NAME);
        let old = r#"{
  "version": 1,
  "items": [
    {"type": "command", "id": "file.dir", "icon": {"mode": "auto"}},
    {"type": "command", "id": "file.new", "icon": {"mode": "auto"}},
    {"type": "command", "id": "file.delete", "icon": {"mode": "auto"}},
    {"type": "command", "id": "file.paste", "icon": {"mode": "auto"}},
    {"type": "command", "id": "edit.rename", "icon": {"mode": "auto"}},
    {"type": "command", "id": "view.mode", "icon": {"mode": "auto"}},
    {"type": "command", "id": "file.properties", "icon": {"mode": "auto"}},
    {"type": "command", "id": "file.mark", "icon": {"mode": "auto"}}
  ]
}
"#;
        std::fs::write(&path, old).unwrap();
        let doc = or_default(read_file(&path));
        assert_eq!(doc.command_ids(), ["file.dir", "file.new", "file.delete", "file.paste", "edit.rename", "view.mode", "file.properties", "file.mark"]);
        assert_ne!(doc.command_ids(), Doc::default_layout().command_ids());
        assert!(!doc.pieces().iter().any(|p| matches!(p, Piece::Separator)));
        let mut reset = doc.clone();
        reset.reset_default();
        assert_eq!(reset.command_ids(), Doc::default_layout().command_ids());
        assert_eq!(reset.pieces().len(), 14);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_icons_reuse_the_catalog_marks() {
        let auto = IconChoice::Auto;
        assert_eq!(drawn_icon(Action::NewFolder, &auto), LineIcon::Group);
        assert_eq!(drawn_icon(Action::NewFile, &auto), LineIcon::NewFile);
        assert_eq!(drawn_icon(Action::DeleteSelection, &auto), LineIcon::Del);
        assert_eq!(drawn_icon(Action::Paste, &auto), LineIcon::Paste);
        assert_eq!(drawn_icon(Action::RenameInPlace, &auto), LineIcon::Rename);
        assert_eq!(drawn_icon(Action::ViewMode, &auto), LineIcon::View);
        assert_eq!(drawn_icon(Action::Properties, &auto), LineIcon::Properties);
        assert_eq!(drawn_icon(Action::Mark, &auto), LineIcon::Important);
        assert_eq!(drawn_icon(Action::EditToolbar, &auto), LineIcon::Edit);
        assert_eq!(drawn_icon(Action::Overflow, &auto), LineIcon::Chevrons);
        assert_eq!(drawn_icon(Action::Paste, &IconChoice::Manual { pack: Some("del".into()), file: None }), LineIcon::Del);
        let n = crate::line_icon_px(96);
        let c = crate::line_icon_coverage(n, 96, LineIcon::Chevrons);
        assert_eq!(n, 17);
        assert!(c.iter().any(|&a| a == 255));
        for k in 0..n {
            for (x, y) in [(k, 0), (0, k), (k, n - 1), (n - 1, k)] {
                assert!(c[(y * n + x) as usize] < 96, "chevrons edge {x},{y}");
            }
        }
        assert!((crate::line_icon_stroke(96) - 1.35).abs() < 1e-6);
    }

    #[test]
    fn wide_vertical_bar_keeps_change_left_of_the_right_strip() {
        let pieces = defaults();
        let lay = placed(true, 900, &pieces);
        assert!(lay.shown);
        assert!(lay.overflow.is_none(), "900px holds the default commands");
        assert_eq!(lay.commands.len(), 13);
        assert!(lay.hidden.is_empty());
        assert_eq!(lay.edit.right, 900);
        assert_eq!(lay.edit.right - lay.edit.left, 30);
        assert_eq!(lay.edit.bottom - lay.edit.top, 24);
        assert_eq!(lay.edit.top, 4);
        let mut prev = 0;
        for r in &lay.commands {
            assert!(r.left >= prev);
            assert_eq!(r.right - r.left, 30);
            assert!(r.right + 2 <= lay.edit.left, "command {:?} overlaps change {:?}", r, lay.edit);
            prev = r.right;
        }
        let icons = crate::icon_slots(1400, 4, 96, 4, true);
        let fitted = placed(true, icons.tab_w, &pieces);
        assert!(fitted.edit.right <= icons.tab_w);
        assert!(fitted.edit.right <= icons.list.left);
        if icons.drive_n > 0 {
            assert!(fitted.edit.right <= icons.drives[0].left);
        }
        for (_, r) in icons.icons {
            if r.right > r.left {
                assert!(fitted.edit.right <= r.left);
            }
        }
        let hidden_bar = crate::icon_slots(1400, 4, 96, 4, false);
        assert!(hidden_bar.tab_w > icons.tab_w, "hiding the right icons only widens the command area");
        let still = placed(true, hidden_bar.tab_w, &pieces);
        assert!(still.shown);
        assert!(still.edit.right <= hidden_bar.list.left);
    }

    #[test]
    fn tight_width_puts_overflow_beside_change() {
        let lay = placed(true, 200, &defaults());
        assert!(lay.shown);
        let over = lay.overflow.expect("overflow");
        assert_eq!(lay.edit.right, 200);
        assert_eq!(over.right + 2, lay.edit.left);
        assert_eq!(over.right - over.left, 30);
        assert!(lay.commands.len() < 13);
        assert!(!lay.hidden.is_empty());
        for r in &lay.commands {
            assert!(r.right + 2 <= over.left);
            assert!(r.right < lay.edit.left);
        }
        assert_eq!(lay.commands.len() + lay.separators.len() + lay.hidden.len(), defaults().len());
    }

    #[test]
    fn horizontal_tabs_stack_icons_in_a_narrow_column() {
        let lay = column(700, 0, &defaults());
        assert!(lay.shown);
        assert!(lay.overflow.is_none());
        assert_eq!(lay.commands.len(), 13);
        assert_eq!(lay.separators.len(), 1);
        assert!(lay.hidden.is_empty());
        assert_eq!(lay.edit.left, 0, "30px button fills the 30px column");
        assert_eq!(lay.edit.right - lay.edit.left, 30);
        assert_eq!(lay.edit.bottom - lay.edit.top, 24);
        assert_eq!(lay.edit.bottom, 32 + 700 - 4);
        assert!(lay.edit.top >= 32, "the column starts under the tab row");
        let mut prev = 32;
        for r in &lay.commands {
            assert_eq!((r.left, r.right), (lay.edit.left, lay.edit.right));
            assert!(r.top >= prev);
            assert_eq!(r.bottom - r.top, 24);
            assert!(r.bottom + 4 <= lay.edit.top);
            prev = r.bottom;
        }
        let beside = column(700, 244, &defaults());
        assert_eq!(beside.edit.left, 244, "open side panel: column starts after the panel and the gap");
        assert!(beside.commands.iter().all(|r| r.left >= 244));
    }

    #[test]
    fn short_column_puts_overflow_above_change() {
        let lay = column(120, 0, &defaults());
        assert!(lay.shown);
        let over = lay.overflow.expect("overflow");
        assert_eq!(over.bottom + 4, lay.edit.top);
        assert_eq!(over.left, lay.edit.left);
        assert_eq!(over.right - over.left, 30);
        assert!(lay.commands.len() < 13);
        assert!(!lay.hidden.is_empty());
        for r in &lay.commands {
            assert!(r.bottom + 4 <= over.top);
            assert!(r.bottom < lay.edit.top);
            assert!(r.top >= 32);
        }
        assert_eq!(lay.commands.len() + lay.separators.len() + lay.hidden.len(), defaults().len());
    }

    #[test]
    fn column_separator_is_a_horizontal_line() {
        let pieces = vec![Piece::Command { action: Action::NewFolder, icon: IconChoice::Auto }, Piece::Separator, Piece::Command { action: Action::Paste, icon: IconChoice::Auto }];
        let lay = column(400, 0, &pieces);
        assert_eq!(lay.separators.len(), 1);
        let sep = lay.separators[0];
        assert_eq!(sep.bottom - sep.top, 1);
        assert_eq!(sep.right - sep.left, 30);
        assert!(sep.top >= lay.commands[0].bottom);
        assert!(sep.bottom <= lay.commands[1].top);
        assert_eq!(sep.left, lay.commands[0].left);
    }

    #[test]
    fn empty_layout_still_pins_change_and_a_separator_takes_a_gap() {
        let edit_only = placed(true, 400, &[]);
        assert!(edit_only.shown);
        assert!(edit_only.commands.is_empty());
        assert!(edit_only.edit.live());
        assert!(edit_only.overflow.is_none());
        let pieces = vec![Piece::Command { action: Action::NewFolder, icon: IconChoice::Auto }, Piece::Separator, Piece::Command { action: Action::Paste, icon: IconChoice::Auto }];
        let lay = placed(true, 900, &pieces);
        assert_eq!(lay.commands.len(), 2);
        assert_eq!(lay.separators.len(), 1);
        let sep = lay.separators[0];
        assert!(sep.left >= lay.commands[0].right);
        assert!(sep.right <= lay.commands[1].left);
        assert!(sep.right - sep.left < 30);
    }

    fn hop(id: i32, parent_is_main: bool) -> FocusHop {
        FocusHop { id, parent_is_main, is_main: false, is_tooltip: false }
    }

    fn frame() -> FocusHop {
        FocusHop { id: 0, parent_is_main: false, is_main: true, is_tooltip: false }
    }

    #[test]
    fn list_focus_returns_only_while_the_toolbar_still_has_it() {
        // A click leaves focus on the button. More and Change are the same kind of control.
        // The left column and the top strip share these ids, so both bars use one rule.
        // A child of More is decided when the walk reaches More itself.
        assert_eq!(toolbar_focus_from(ID_CMD_FIRST, true), FocusSeat::Toolbar);
        assert_eq!(toolbar_focus_from(ID_CMD_FIRST + CMD_CAPACITY - 1, true), FocusSeat::Toolbar);
        assert_eq!(toolbar_focus_from(ID_CMD_OVERFLOW, true), FocusSeat::Toolbar);
        assert_eq!(toolbar_focus_from(ID_CMD_EDIT, true), FocusSeat::Toolbar);
        assert!(restore_list_after_toolbar(toolbar_focus_from(ID_CMD_OVERFLOW, true)));

        // The address bar and the filter are not toolbar buttons. A dialog that
        // reuses a command id fails the main-frame parent check and keeps focus.
        assert_eq!(toolbar_focus_from(ID_EDIT, true), FocusSeat::Elsewhere);
        assert_eq!(toolbar_focus_from(ID_FILTER, false), FocusSeat::Elsewhere);
        assert_eq!(toolbar_focus_from(ID_CMD_FIRST, false), FocusSeat::Elsewhere, "dialog reused a command id");
        assert_eq!(toolbar_focus_from(ID_CMD_OVERFLOW, false), FocusSeat::Elsewhere);
        assert_eq!(toolbar_focus_from(ID_CMD_FIRST - 1, true), FocusSeat::Elsewhere);
        assert_eq!(toolbar_focus_from(ID_CMD_EDIT + 1, true), FocusSeat::Elsewhere);
        assert!(!restore_list_after_toolbar(FocusSeat::Elsewhere));
        assert!(!restore_list_after_toolbar(toolbar_focus_from(ID_EDIT, true)));
        assert!(!restore_list_after_toolbar(toolbar_focus_from(ID_CMD_FIRST, false)));
    }

    #[test]
    fn frame_tooltip_and_lost_focus_return_to_the_list() {
        // More's menu and a closed dialog return the keyboard to the frame, not the button.
        // The tracking tip is a popup owned by that frame. Null focus is the same miss.
        assert_eq!(seat_from_chain(&[frame()]), FocusSeat::Frame);
        assert!(restore_list_after_toolbar(FocusSeat::Frame));
        let tip = FocusHop { id: 0, parent_is_main: true, is_main: false, is_tooltip: true };
        assert_eq!(seat_from_chain(&[tip, frame()]), FocusSeat::Tooltip);
        assert!(restore_list_after_toolbar(FocusSeat::Tooltip));
        assert_eq!(seat_from_chain(&[]), FocusSeat::Lost);
        assert!(restore_list_after_toolbar(FocusSeat::Lost));
        // A child of More is not itself a command id. The walk reaches More, then stops
        // before the frame so the frame does not hide the button.
        let child = hop(0, false);
        let more = hop(ID_CMD_OVERFLOW, true);
        assert_eq!(seat_from_chain(&[child, more, frame()]), FocusSeat::Toolbar);
        assert!(restore_list_after_toolbar(seat_from_chain(&[more, frame()])));
    }

    #[test]
    fn dialog_editor_and_rename_keep_focus() {
        // Owned editor: not a command id, parent is the frame. The walk must stop
        // at the frame instead of treating that as "focus is the frame".
        let editor = hop(0, true);
        assert_eq!(seat_from_chain(&[editor, frame()]), FocusSeat::Elsewhere);
        assert!(!restore_list_after_toolbar(seat_from_chain(&[editor, frame()])));
        // Address edit, then its pane, then the frame.
        let addr = hop(ID_EDIT, false);
        let pane = hop(0, true);
        assert_eq!(seat_from_chain(&[addr, pane, frame()]), FocusSeat::Elsewhere);
        // Inline rename sits under the shell view, not under a command button.
        let rename = hop(0, false);
        let view = hop(0, false);
        assert_eq!(seat_from_chain(&[rename, view, pane, frame()]), FocusSeat::Elsewhere);
        // A dialog that reused a command id, then its own frame, then our owner.
        let reused = hop(ID_CMD_FIRST, false);
        let dialog = hop(0, true);
        assert_eq!(seat_from_chain(&[reused, dialog, frame()]), FocusSeat::Elsewhere);
        assert!(!restore_list_after_toolbar(seat_from_chain(&[reused, dialog, frame()])));
    }

    #[test]
    fn grayed_rules_follow_selection_clipboard_and_folder() {
        let flags = |sel, receive, clip| EnableInput { selection: sel, folder_can_receive: receive, clipboard_pasteable: clip };
        let empty = flags(0, true, false);
        for action in [Action::DeleteSelection, Action::RenameInPlace, Action::Properties, Action::Mark] {
            assert!(!enabled(action, &empty), "{action:?}");
            assert!(enabled(action, &flags(2, true, false)), "{action:?}");
        }
        assert!(!enabled(Action::Paste, &flags(1, true, false)), "text clipboard");
        assert!(!enabled(Action::Paste, &flags(0, false, true)), "nowhere to paste");
        assert!(enabled(Action::Paste, &flags(0, true, true)));
        assert!(!clipboard_pasteable(false, false));
        assert!(clipboard_pasteable(true, false));
        assert!(clipboard_pasteable(false, true));
        assert!(!enabled(Action::NewFolder, &flags(0, false, false)));
        assert!(!enabled(Action::NewFile, &flags(0, false, false)));
        assert!(enabled(Action::NewFolder, &flags(0, true, false)));
        assert!(enabled(Action::NewFile, &flags(0, true, false)));
        assert!(enabled(Action::ViewMode, &empty));
        assert!(enabled(Action::EditToolbar, &empty));
        let this_pc = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
        assert!(!folder_can_receive(this_pc, false));
        assert!(!folder_can_receive("shell:Recent", true));
        assert!(!folder_can_receive(r"D:\data", true));
        assert!(folder_can_receive(r"D:\data", false));
    }

    #[test]
    fn edit_moves_inserts_and_restores_the_default_set() {
        let mut doc = Doc::default_layout();
        let n = doc.item_count();
        assert!(n >= 2);
        let first = doc.label_at(0);
        assert!(!doc.move_item(0, 0));
        assert!(!doc.move_item(0, 1));
        assert!(doc.move_item(0, 2));
        assert_ne!(doc.label_at(0), first);
        assert_eq!(doc.label_at(1), first);
        doc.insert_separator(1);
        assert!(doc.is_separator(1));
        assert!(doc.remove_at(1));
        assert!(!doc.is_separator(1));
        doc.insert_action(0, Action::Paste);
        assert_eq!(doc.label_at(0), crate::tr(label_key(Action::Paste)));
        doc.reset_default();
        assert_eq!(doc.item_count(), n);
        assert_eq!(doc.label_at(0), first);
    }

    #[test]
    fn settings_reset_does_not_name_toolbar_json() {
        let src = include_str!("shell/fileop.rs");
        let start = src.find("pub(crate) unsafe fn reset_prefs").expect("reset_prefs");
        let end = src[start..].find("pub(crate) unsafe fn import_settings").map(|i| start + i).unwrap_or(src.len());
        let body = &src[start..end];
        assert!(!body.contains("toolbar"));
        assert_eq!(FILE_NAME, "toolbar.json");
        assert_ne!(FILE_NAME, crate::settings::store::SETTINGS_FILE);
    }

    fn many(n: usize) -> Vec<Piece> {
        (0..n).map(|_| Piece::Command { action: Action::CopyPath, icon: IconChoice::Auto }).collect()
    }

    #[test]
    fn wide_bar_fits_past_sixteen_and_only_the_tail_overflows() {
        let pieces = many(24);
        let wide = placed(true, 1440, &pieces);
        assert!(wide.commands.len() >= 20, "1440px shows {}", wide.commands.len());
        assert!(wide.commands.len() > 16, "the old 16-slot cap must not apply");
        assert!(wide.overflow.is_none());
        assert!(wide.hidden.is_empty());

        // 720px, with More reserved, holds 20 commands and not 21.
        let mid = placed(true, 720, &pieces);
        assert_eq!(mid.commands.len(), 20);
        assert!(mid.overflow.is_some());
        assert_eq!(mid.hidden, (20..24).collect::<Vec<_>>());
        assert_eq!(mid.commands.len() + mid.hidden.len(), pieces.len());

        let narrow = placed(true, 280, &pieces);
        assert!(narrow.overflow.is_some());
        assert!(narrow.commands.len() < mid.commands.len());
        assert!(narrow.commands.len() > 0);
        assert_eq!(narrow.hidden, (narrow.commands.len()..pieces.len()).collect::<Vec<_>>());

        let huge = many(CMD_CAPACITY as usize + 8);
        let capped = placed(true, 8000, &huge);
        assert_eq!(capped.commands.len(), CMD_CAPACITY as usize, "window ceiling, not a 16 cap");
        assert!(capped.overflow.is_some());
        assert_eq!(capped.hidden.len(), 8);
    }

    #[test]
    fn vertical_column_overflows_by_height_not_by_sixteen() {
        let pieces = many(24);
        let tall = column(900, 0, &pieces);
        assert!(tall.commands.len() >= 20, "tall column shows {}", tall.commands.len());
        assert!(tall.overflow.is_none(), "900px holds {}", tall.commands.len());
        assert!(tall.hidden.is_empty());

        let short = column(220, 0, &pieces);
        assert!(short.overflow.is_some());
        assert!(short.commands.len() < tall.commands.len());
        assert!(short.commands.len() > 0);
        assert_eq!(short.hidden, (short.commands.len()..pieces.len()).collect::<Vec<_>>());
        assert!(short.commands.iter().all(|r| r.bottom <= short.overflow.unwrap().top));
    }

    #[test]
    fn command_ids_sit_between_chrome_and_menus() {
        assert!(ID_CMD_FIRST > ID_TB_WATCH);
        assert!(ID_CMD_EDIT < ID_HIDDEN as i32);
        assert!(ID_CMD_EDIT < crate::ID_DRIVE);
        assert!(ID_CMD_EDIT < ID_ACCENT as i32);
        assert_eq!(ID_CMD_OVERFLOW, ID_CMD_FIRST + CMD_CAPACITY);
        assert_eq!(ID_CMD_EDIT, ID_CMD_OVERFLOW + 1);
        assert!(CMD_CAPACITY > 16);
        assert!(is_command_control(ID_CMD_FIRST));
        assert!(is_command_control(ID_CMD_FIRST + CMD_CAPACITY - 1));
        assert!(is_command_control(ID_CMD_OVERFLOW));
        assert!(is_command_control(ID_CMD_EDIT));
        assert!(!is_command_control(ID_CMD_FIRST - 1));
        assert!(!is_command_control(ID_CMD_EDIT + 1));
        assert!(!is_command_control(crate::ID_DRIVE));
        assert!(!is_command_control(ID_TB_SET));
        assert!(!is_command_control(ID_HIDDEN as i32));
        assert_ne!(ID_CMD_FIRST, 250);
    }

    #[test]
    fn saved_pane_color_stays_pane_color_and_tab_color_is_new() {
        let raw = r#"{"version":1,"items":[
            {"type":"command","id":"pane.color","icon":{"mode":"auto"}},
            {"type":"command","id":"pane.close","icon":{"mode":"auto"}},
            {"type":"command","id":"file.terminal","icon":{"mode":"auto"}}
        ]}"#;
        let doc = parse(raw.as_bytes()).unwrap();
        let actions: Vec<_> = doc
            .pieces()
            .iter()
            .filter_map(|p| match p {
                Piece::Command { action, .. } => Some(*action),
                Piece::Separator | Piece::Folder { .. } | Piece::File { .. } => None,
            })
            .collect();
        assert_eq!(actions, [Action::PaneColor, Action::ClosePane, Action::Terminal]);
        let back = parse(doc.to_json().as_bytes()).unwrap();
        assert_eq!(back.command_ids(), ["pane.color", "pane.close", "file.terminal"]);
        assert!(!back.to_json().contains("tab.color"));
        assert_eq!(action_of("pane.color"), Some(Action::PaneColor));
        assert_eq!(action_of("tab.color"), Some(Action::TabColor));
        assert_eq!(action_of("file.terminal.admin"), Some(Action::TerminalAdmin));
        assert_eq!(command_id(Action::PaneColor), Some("pane.color"));
        assert_eq!(command_id(Action::TabColor), Some("tab.color"));
        assert_eq!(command_id(Action::TerminalAdmin), Some("file.terminal.admin"));
        assert_ne!(command_id(Action::TabColor), command_id(Action::PaneColor));
        let auto = IconChoice::Auto;
        assert_eq!(drawn_icon(Action::PaneColor, &auto), LineIcon::PaneColor);
        assert_eq!(drawn_icon(Action::TabColor, &auto), LineIcon::TabColor);
        assert_eq!(drawn_icon(Action::ClosePane, &auto), LineIcon::PaneClose);
        assert_eq!(drawn_icon(Action::TerminalAdmin, &auto), LineIcon::TerminalAdmin);
        assert_ne!(drawn_icon(Action::ClosePane, &auto), LineIcon::Del);
        assert_ne!(drawn_icon(Action::ClosePane, &auto), LineIcon::Close);
        assert_ne!(drawn_icon(Action::PaneColor, &auto), drawn_icon(Action::TabColor, &auto));
        assert_eq!(label_key(Action::TabColor), crate::Key::ToolbarTabColor);
        assert_eq!(label_key(Action::TerminalAdmin), crate::Key::ToolbarTerminalAdmin);
        assert_eq!(label_key(Action::PaneColor), crate::Key::ToolbarPaneColor);
        assert_eq!(tooltip(Action::TabColor), "Tab colour");
        assert_eq!(tooltip(Action::TerminalAdmin), "Open Terminal (Administrator)");
        assert_eq!(crate::tint_pick(1), Some(0));
        assert_eq!(crate::tint_pick(7), Some(6));
        assert_eq!(crate::tint_pick(0), None);
        assert_eq!(crate::tint_pick(8), None);
    }

    #[test]
    fn hidden_toggle_is_on_only_while_hidden_files_are_shown() {
        assert!(command_toggled(Action::ShowHidden, true));
        assert!(!command_toggled(Action::ShowHidden, false));
        for action in [Action::DeleteSelection, Action::PreviewPane, Action::Zoom, Action::PaneColor, Action::TabColor, Action::Terminal, Action::TerminalAdmin, Action::ClosePane] {
            assert!(!command_toggled(action, true), "{action:?}");
            assert!(!command_toggled(action, false), "{action:?}");
        }
    }

    #[test]
    fn tip_updates_when_the_slot_or_its_place_changes() {
        let tip = |id, text: &str, y| LiveTip { control: id, text: text.into(), left: 0, top: y, right: 30, bottom: y + 24 };
        assert_eq!(tip_step(None, None), TipStep::Idle);
        assert_eq!(tip_step(Some(&tip(1, "Delete", 0)), None), TipStep::Hide);
        assert_eq!(tip_step(None, Some(&tip(1, "Delete", 0))), TipStep::Show);
        let same = tip(1, "Delete", 0);
        assert_eq!(tip_step(Some(&same), Some(&same)), TipStep::Idle);
        // The slot kept its window and moved to a different command.
        assert_eq!(tip_step(Some(&tip(1, "Delete", 0)), Some(&tip(1, "Paste", 0))), TipStep::Show);
        // Resize moved the button under the pointer.
        assert_eq!(tip_step(Some(&tip(1, "Delete", 0)), Some(&tip(1, "Delete", 40))), TipStep::Show);
        // Pointer walked onto the next button, including a disabled one.
        assert_eq!(tip_step(Some(&tip(1, "Delete", 0)), Some(&tip(2, "Rename", 28))), TipStep::Show);
        assert_eq!(command_tip_side(true), crate::TipSide::Below);
        assert_eq!(command_tip_side(false), crate::TipSide::Right);
    }

    #[test]
    fn tracking_tip_hides_when_the_click_or_the_foreground_leaves_the_button() {
        let order = |cause, active, minimized, over, held| tip_order(cause, active, minimized, over, held);
        assert_eq!(order(TipCause::Hover, true, false, true, false), TipOrder::Show);
        // Still geometrically over the icon, but the terminal is now in front.
        assert_eq!(order(TipCause::Hover, false, false, true, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Hover, true, true, true, false), TipOrder::Hide);
        // Mouse-down has capture. A move during the click must not show the bubble.
        assert_eq!(order(TipCause::Hover, true, false, true, true), TipOrder::Hide);
        assert_eq!(order(TipCause::Hover, true, false, false, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Leave, true, false, false, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Press, true, false, true, true), TipOrder::Hide);
        assert_eq!(order(TipCause::Inactive, false, false, true, false), TipOrder::Hide);
        assert_eq!(order(TipCause::KillFocus, true, false, true, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Minimize, true, true, true, false), TipOrder::Hide);
        // The bar moved. Hide even if the pointer is still on the button; the next
        // move shows it at the new place. An inactive frame stays hidden.
        assert_eq!(order(TipCause::Relayout, true, false, true, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Relayout, false, false, true, false), TipOrder::Hide);
        assert_eq!(order(TipCause::Relayout, true, false, false, false), TipOrder::Hide);
        assert!(tip_hides_on_focus(false));
        assert!(!tip_hides_on_focus(true), "the bubble must not dismiss itself by taking focus");
        assert!(tip_rearm_leave(true, true, false));
        assert!(!tip_rearm_leave(false, true, false));
        assert!(!tip_rearm_leave(true, false, false));
        assert!(!tip_rearm_leave(true, true, true));
    }

    #[test]
    fn open_command_is_dropped_and_preset_targets_round_trip() {
        let raw = r#"{
            "version": 1,
            "note": "keep",
            "items": [
                {"type": "command", "id": "file.open", "icon": {"mode": "auto"}},
                {"type": "command", "id": "file.dir", "icon": {"mode": "auto"}},
                {"type": "folder", "path": "D:\\Work", "icon": {"mode": "manual", "pack": "open_folder"}},
                {"type": "file", "path": "D:\\Work\\a.txt", "icon": {"mode": "auto"}},
                {"type": "separator"},
                {"type": "command", "id": "file.open", "icon": {"mode": "manual", "pack": "del"}}
            ]
        }"#;
        let doc = parse(raw.as_bytes()).unwrap();
        assert!(!doc.has_action(Action::Open));
        assert!(!doc.to_json().contains("file.open"), "{}", doc.to_json());
        assert_eq!(doc.extra.get("note").and_then(|v| v.as_str()), Some("keep"));
        let pieces = doc.pieces();
        assert!(matches!(&pieces[0], Piece::Command { action: Action::NewFolder, .. }));
        assert!(matches!(&pieces[1], Piece::Folder { path, .. } if path == "D:\\Work"));
        assert!(matches!(&pieces[2], Piece::File { path, .. } if path == "D:\\Work\\a.txt"));
        assert!(matches!(pieces[3], Piece::Separator));
        assert_eq!(piece_icon(&pieces[1]), Some(LineIcon::OpenFolder));
        assert_eq!(piece_icon(&pieces[2]), Some(LineIcon::PinFile));
        let back = parse(doc.to_json().as_bytes()).unwrap();
        assert_eq!(back, doc);
        assert_eq!(doc.label_at(1), "Work");
        assert_eq!(doc.label_at(2), "a.txt");
        assert_eq!(doc.item_tip(1), "Work\r\nD:\\Work");
        assert_eq!(doc.item_tip(2), "a.txt\r\nD:\\Work\\a.txt");
        assert_eq!(target_tip("a.txt", r"D:\Work\a.txt"), "a.txt\r\nD:\\Work\\a.txt");
        assert_eq!(target_tip("notes", "notes"), "notes");
    }

    #[test]
    fn preset_targets_take_button_slots_and_keep_their_menu_names() {
        let mut doc = Doc::default_layout();
        let n = doc.item_count();
        doc.insert_action(0, Action::Open);
        assert_eq!(doc.item_count(), n);
        assert!(!doc.has_action(Action::Open));
        doc.insert_folder(0, "  ");
        doc.insert_file(0, "");
        assert_eq!(doc.item_count(), n);
        doc.insert_folder(0, r"D:\Archive");
        doc.insert_file(1, r"D:\Archive\note.txt");
        assert!(doc.is_folder_target(0));
        assert!(doc.is_file_target(1));
        assert_eq!(doc.glyph_at(0), Some(LineIcon::OpenFolder));
        assert_eq!(doc.glyph_at(1), Some(LineIcon::PinFile));
        doc.set_pack(1, Some("del"));
        assert_eq!(doc.glyph_at(1), Some(LineIcon::Del));
        assert!(doc.set_target_path(0, r"E:\Other"));
        assert_eq!(doc.target_path(0), Some(r"E:\Other"));
        assert!(!doc.set_target_path(2, r"E:\Nope"));
        assert!(doc.target_path(2).is_none());
        assert!(doc.pieces().iter().all(|p| !matches!(p, Piece::Command { action: Action::Open, .. })));

        let mut pieces = vec![Piece::Folder { path: r"D:\Archive".into(), icon: IconChoice::Auto }];
        pieces.extend(many(23));
        let lay = placed(true, 720, &pieces);
        assert_eq!(lay.commands.len(), 20);
        assert_eq!(lay.command_at[0], 0);
        assert!(!lay.hidden.contains(&0));
        assert!(lay.hidden.iter().any(|&i| matches!(&pieces[i], Piece::Command { .. })));
        let narrow = placed(true, 280, &pieces);
        assert_eq!(narrow.command_at.first().copied(), Some(0), "the folder stays in the visible row");
        assert!(narrow.hidden.iter().any(|&i| i > 0));
        let tail = Piece::File { path: r"C:\Users\Public\a.txt".into(), icon: IconChoice::Auto };
        let mut crowded = many(24);
        crowded.push(tail);
        let folded = placed(true, 400, &crowded);
        let last = crowded.len() - 1;
        assert!(folded.hidden.contains(&last));
        assert_eq!(run_label(&run_of(&crowded[last]).unwrap()), "a.txt");
        assert_eq!(run_tip(&run_of(&crowded[last]).unwrap()), "a.txt\r\nC:\\Users\\Public\\a.txt");
    }

    #[test]
    fn a_missing_target_is_not_present_and_a_real_path_is() {
        assert!(!target_present(""));
        assert!(!target_present("   "));
        assert!(!target_present(r"D:\no\such\maguro\toolbar-target"));
        let dir = scratch("target");
        let folder = dir.join("box");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(target_present(folder.to_str().unwrap()));
        assert!(target_present(file.to_str().unwrap()));
        assert!(!target_present(folder.join("missing").to_str().unwrap()));
        let mut doc = Doc::default_layout();
        doc.insert_file(0, file.to_str().unwrap());
        doc.insert_folder(0, folder.join("gone").to_str().unwrap());
        assert!(doc.target_missing(0));
        assert!(!doc.target_missing(1));
        assert!(target_present(r"\\server\share\folder"));
        assert!(target_present("//server/share/file.txt"));
        let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        if mask != 0 {
            if let Some(letter) = (b'A'..=b'Z').find(|c| mask >> (c - b'A') & 1 == 0) {
                let path = format!("{}:\\no-such-maguro", letter as char);
                assert!(!target_present(&path), "{path}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn change_still_opens_the_editor_when_the_bar_holds_targets() {
        let mut doc = Doc::default_layout();
        doc.insert_folder(0, r"D:\Work");
        doc.insert_file(1, r"D:\Work\missing.txt");
        let lay = placed(true, 1400, &doc.pieces());
        assert!(lay.edit.live(), "Change stays on the bar");
        let (run, icon) = slot_face(ID_CMD_EDIT, &doc, &lay).expect("Change");
        assert_eq!(run, Run::Command(Action::EditToolbar));
        assert_eq!(icon, LineIcon::Edit);
    }
}
