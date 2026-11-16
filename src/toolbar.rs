//! Command toolbar. `toolbar.json` sits next to `maguro.json` (roaming Maguro, or the exe
//! folder when `portable.txt` is present). It is never written into `maguro.json`.
//! Settings reset does not open this file. A missing file uses the default layout.
//! A file that cannot be parsed is left on disk and the default layout is used instead.
//!
//! Vertical window tabs draw the bar across the left of the top strip, inside `tab_w`.
//! Horizontal window tabs draw the same buttons down a narrow left column (about 40px
//! at 100%), where the vertical tab strip would sit. Buttons are icon-only.
//! The pinned Change button does not open an editor in this version.

use std::path::Path;

use serde_json::{Map, Value};

use std::sync::OnceLock;

use windows::Win32::System::DataExchange::{AddClipboardFormatListener, IsClipboardFormatAvailable, RemoveClipboardFormatListener};

use crate::app::*;
use crate::imports::*;
use crate::ui::LineIcon;

pub(crate) const FILE_NAME: &str = "toolbar.json";
const VERSION: u32 = 1;

/// How many command buttons can sit on the strip before the rest go into ».
pub(crate) const CMD_SLOTS: i32 = 16;

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
    /// Pinned. A click does nothing until the editor exists.
    EditToolbar,
    Overflow,
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
    Command { action: Action, icon: IconChoice },
    Separator,
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
        Self {
            shown: false,
            commands: Vec::new(),
            command_at: Vec::new(),
            separators: Vec::new(),
            overflow: None,
            edit: Px::empty(),
            hidden: Vec::new(),
        }
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
        LayIn {
            vertical_tabs: self.vertical_tabs,
            tab_w: self.tab_w,
            top: self.top,
            bottom: self.bottom,
            left: self.left,
            dpi: self.dpi,
            max_buttons: CMD_SLOTS as usize,
        }
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

struct State {
    doc: Doc,
    clip_pasteable: bool,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(State { doc: Doc::default_layout(), clip_pasteable: false }))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
        Action::DeleteSelection | Action::RenameInPlace | Action::Properties | Action::Mark => input.selection > 0,
        Action::Paste => input.folder_can_receive && input.clipboard_pasteable,
        Action::ViewMode | Action::Overflow => true,
        // Present and clickable-looking. The handler does not open an editor.
        Action::EditToolbar => true,
    }
}

impl Doc {
    pub(crate) fn default_layout() -> Self {
        const ACTIONS: [Action; 8] = [
            Action::NewFolder,
            Action::NewFile,
            Action::DeleteSelection,
            Action::Paste,
            Action::RenameInPlace,
            Action::ViewMode,
            Action::Properties,
            Action::Mark,
        ];
        Self {
            items: ACTIONS
                .into_iter()
                .filter_map(|action| {
                    let id = command_id(action)?.to_string();
                    Some(Item::Command { action: action_of(&id), id, icon: IconChoice::Auto })
                })
                .collect(),
            extra: Map::new(),
        }
    }

    pub(crate) fn command_ids(&self) -> Vec<&str> {
        self.items.iter().filter_map(|it| match it {
            Item::Command { id, .. } => Some(id.as_str()),
            _ => None,
        }).collect()
    }

    pub(crate) fn pieces(&self) -> Vec<Piece> {
        self.items.iter().filter_map(|it| match it {
            Item::Command { action: Some(action), icon, .. } => Some(Piece::Command { action: *action, icon: icon.clone() }),
            Item::Separator => Some(Piece::Separator),
            _ => None,
        }).collect()
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
}

impl Item {
    fn to_json(&self) -> Value {
        let icon = |icon: &IconChoice| icon_json(icon);
        match self {
            Item::Command { id, icon: choice, .. } => json_obj(&[("type", Value::from("command")), ("id", Value::from(id.clone())), ("icon", icon(choice))]),
            Item::Separator => json_obj(&[("type", Value::from("separator"))]),
            Item::Folder { path, icon: choice } => json_obj(&[("type", Value::from("folder")), ("path", Value::from(path.clone())), ("icon", icon(choice))]),
            Item::File { path, icon: choice } => json_obj(&[("type", Value::from("file")), ("path", Value::from(path.clone())), ("icon", icon(choice))]),
            Item::Program { exe, args, dir, icon: choice } => json_obj(&[
                ("type", Value::from("program")),
                ("exe", Value::from(exe.clone())),
                ("args", Value::from(args.clone())),
                ("dir", Value::from(dir.clone())),
                ("icon", icon(choice)),
            ]),
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
    let Some(obj) = v.and_then(|v| v.as_object()) else { return IconChoice::Auto };
    let mode = obj.get("mode").and_then(|m| m.as_str()).unwrap_or("auto");
    if mode != "manual" {
        return IconChoice::Auto;
    }
    let text = |key: &str| obj.get(key).and_then(|s| s.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    IconChoice::Manual { pack: text("pack"), file: text("file") }
}

fn parse_item(v: &Value) -> Item {
    let Some(obj) = v.as_object() else { return Item::Raw(v.clone()) };
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
    Ok(Doc { items: items.iter().map(parse_item).collect(), extra })
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
    let (btn_w, btn_h, gap, pad) = button_metrics(dpi);
    let sep_w = scale(dpi, 1).max(1);
    if input.tab_w < btn_w {
        return BarLay::hidden_bar();
    }
    let top = input.top;
    let edit = Px { left: input.tab_w - btn_w, top, right: input.tab_w, bottom: top + btn_h };
    let piece_w = |p: &Piece| match p {
        Piece::Command { .. } => btn_w,
        Piece::Separator => sep_w,
    };
    let take = |room: i32| -> usize {
        let mut n = 0;
        let mut buttons = 0usize;
        let mut used = 0i32;
        for (i, p) in pieces.iter().enumerate() {
            let add = piece_w(p) + if i == 0 { 0 } else { gap };
            if used + add > room {
                break;
            }
            if matches!(p, Piece::Command { .. }) {
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
        match p {
            Piece::Command { .. } => {
                commands.push(r);
                command_at.push(i);
            }
            Piece::Separator => separators.push(r),
        }
        x += w + gap;
    }
    BarLay {
        shown: true,
        commands,
        command_at,
        separators,
        overflow,
        edit,
        hidden: (n..pieces.len()).collect(),
    }
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
    let piece_h = |p: &Piece| match p {
        Piece::Command { .. } => btn_h,
        Piece::Separator => sep_h,
    };
    let take = |room: i32| -> usize {
        let mut n = 0;
        let mut buttons = 0usize;
        let mut used = 0i32;
        for (i, p) in pieces.iter().enumerate() {
            let add = piece_h(p) + if i == 0 { 0 } else { gap };
            if used + add > room {
                break;
            }
            if matches!(p, Piece::Command { .. }) {
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
        match p {
            Piece::Command { .. } => {
                commands.push(r);
                command_at.push(i);
            }
            Piece::Separator => separators.push(r),
        }
        y += h + gap;
    }
    BarLay {
        shown: true,
        commands,
        command_at,
        separators,
        overflow,
        edit,
        hidden: (n..pieces.len()).collect(),
    }
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
    Origin {
        vertical_tabs: a.s.vertical_tabs,
        tab_w: icons.tab_w,
        top: if a.s.vertical_tabs { btn_top } else { g.top },
        bottom: g.bot,
        left: crate::vtab_left(g.side, g.gap),
        dpi: a.dpi,
    }
}

pub(crate) struct SlotMove {
    pub id: i32,
    pub rect: RECT,
    pub show: bool,
}

pub(crate) fn moves(o: &Origin) -> Vec<SlotMove> {
    let (_, lay) = layout_now(o);
    let mut out = Vec::with_capacity(CMD_SLOTS as usize + 2);
    for i in 0..CMD_SLOTS {
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
    (ID_CMD_FIRST..ID_CMD_FIRST + CMD_SLOTS).contains(&id) || id == ID_CMD_OVERFLOW || id == ID_CMD_EDIT
}

pub(crate) fn icon_for_control(id: i32) -> Option<LineIcon> {
    if !is_command_control(id) {
        return None;
    }
    let origin = app(|a| {
        if a.main.0.is_null() {
            None
        } else {
            Some(bar_origin(a))
        }
    })?;
    let (doc, lay) = layout_now(&origin);
    if !lay.shown {
        return None;
    }
    if id == ID_CMD_EDIT {
        return lay.edit.live().then_some(LineIcon::Edit);
    }
    if id == ID_CMD_OVERFLOW {
        return lay.overflow.is_some().then_some(LineIcon::Chevrons);
    }
    let slot = (id - ID_CMD_FIRST) as usize;
    let at = *lay.command_at.get(slot)?;
    match doc.pieces().get(at)? {
        Piece::Command { action, icon } => Some(drawn_icon(*action, icon)),
        Piece::Separator => None,
    }
}

fn action_for(id: i32, doc: &Doc, lay: &BarLay) -> Option<Action> {
    if id == ID_CMD_EDIT && lay.edit.live() {
        return Some(Action::EditToolbar);
    }
    if id == ID_CMD_OVERFLOW && lay.overflow.is_some() {
        return Some(Action::Overflow);
    }
    if !(ID_CMD_FIRST..ID_CMD_FIRST + CMD_SLOTS).contains(&id) {
        return None;
    }
    let at = *lay.command_at.get((id - ID_CMD_FIRST) as usize)?;
    match doc.pieces().get(at)? {
        Piece::Command { action, .. } => Some(*action),
        Piece::Separator => None,
    }
}

fn hidden_actions(doc: &Doc, lay: &BarLay) -> Vec<Action> {
    let pieces = doc.pieces();
    lay.hidden.iter().filter_map(|i| match pieces.get(*i) {
        Some(Piece::Command { action, .. }) => Some(*action),
        _ => None,
    }).collect()
}

pub(crate) unsafe fn create_buttons(main: HWND) {
    for id in ID_CMD_FIRST..ID_CMD_FIRST + CMD_SLOTS {
        let b = crate::child(main, w!("BUTTON"), w!(""), BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), id, HFONT::default());
        _ = ShowWindow(b, SW_HIDE);
    }
    for id in [ID_CMD_OVERFLOW, ID_CMD_EDIT] {
        let b = crate::child(main, w!("BUTTON"), w!(""), BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), id, HFONT::default());
        _ = ShowWindow(b, SW_HIDE);
    }
    refresh_tips();
}

pub(crate) unsafe fn refresh_tips() {
    // Translated name of the bar. Buttons stay icon-only; this keeps the key live for readers.
    let _ = crate::tr(crate::Key::CommandToolbar);
    let main = app(|a| a.main);
    if main.0.is_null() {
        return;
    }
    let origin = app(|a| bar_origin(a));
    let (doc, lay) = layout_now(&origin);
    let pieces = doc.pieces();
    for i in 0..CMD_SLOTS {
        let id = ID_CMD_FIRST + i;
        let Ok(b) = GetDlgItem(Some(main), id) else { continue };
        let Some(at) = lay.command_at.get(i as usize).copied() else { continue };
        let Some(Piece::Command { action, .. }) = pieces.get(at) else { continue };
        set_button_tip(b, *action);
    }
    if let Ok(b) = GetDlgItem(Some(main), ID_CMD_OVERFLOW) {
        set_button_tip(b, Action::Overflow);
    }
    if let Ok(b) = GetDlgItem(Some(main), ID_CMD_EDIT) {
        set_button_tip(b, Action::EditToolbar);
    }
}

unsafe fn set_button_tip(btn: HWND, action: Action) {
    let name = crate::tr(label_key(action));
    if crate::text_of(btn) != name {
        _ = SetWindowTextW(btn, &HSTRING::from(name));
    }
    let tip = tooltip(action);
    crate::tip_text(btn, &tip, TTM_ADDTOOLW);
    crate::tip_text(btn, &tip, TTM_UPDATETIPTEXTW);
}

fn enable_input() -> EnableInput {
    let (pane, path) = app(|a| {
        let i = a.s.active;
        let path = a.s.paths.get(i).cloned().unwrap_or_default();
        (i, path)
    });
    let recent = crate::pane::recent::locked(pane);
    let sel = unsafe {
        crate::view_of::<IFolderView2>(pane).and_then(|fv| fv.ItemCount(SVGIO_SELECTION).ok()).unwrap_or(0) as u32
    };
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
    for i in 0..CMD_SLOTS {
        let id = ID_CMD_FIRST + i;
        let Ok(b) = GetDlgItem(Some(main), id) else { continue };
        let Some(at) = lay.command_at.get(i as usize).copied() else { continue };
        let Some(Piece::Command { action, .. }) = pieces.get(at) else { continue };
        set_enabled(b, enabled(*action, &input));
    }
}

unsafe fn set_enabled(btn: HWND, on: bool) {
    if IsWindowEnabled(btn).as_bool() == on {
        return;
    }
    _ = EnableWindow(btn, on);
    _ = InvalidateRect(Some(btn), None, false);
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
    let pic = IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok()
        || IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok()
        || IsClipboardFormatAvailable(RegisterClipboardFormatW(w!("PNG"))).is_ok();
    _ = CloseClipboard();
    Some(clipboard_pasteable(hdrop && count > 0, pic))
}

pub(crate) unsafe fn install_clipboard(main: HWND) {
    _ = AddClipboardFormatListener(main);
}

pub(crate) unsafe fn remove_clipboard(main: HWND) {
    _ = RemoveClipboardFormatListener(main);
}

pub(crate) unsafe fn command(owner: HWND, id: usize) -> bool {
    let id = id as i32;
    if !is_command_control(id) {
        return false;
    }
    let origin = app(|a| bar_origin(a));
    let (doc, lay) = layout_now(&origin);
    let Some(action) = action_for(id, &doc, &lay) else { return true };
    if !lay.shown {
        return true;
    }
    run(owner, action, &doc, &lay, id);
    true
}

unsafe fn run(owner: HWND, action: Action, doc: &Doc, lay: &BarLay, from: i32) {
    let input = enable_input();
    if !enabled(action, &input) {
        return;
    }
    let pane = app(|a| a.s.active);
    match action {
        Action::NewFolder => crate::new_folder(pane),
        Action::NewFile => crate::new_file(pane),
        Action::DeleteSelection => crate::delete_selection(pane, false),
        Action::Paste => {
            let _ = crate::try_paste(pane);
        }
        Action::RenameInPlace => {
            let _ = crate::rename_inplace(pane);
        }
        Action::ViewMode => crate::popup_view_modes(owner, pane, button_point(from)),
        Action::Properties => crate::show_selection_properties(pane),
        Action::Mark => crate::marks::toggle_pane(pane),
        Action::EditToolbar => {}
        Action::Overflow => popup_overflow(owner, doc, lay),
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

/// Separator lines for the current bar. Empty when the bar is hidden or has no separators.
pub(crate) fn separator_rects() -> Vec<RECT> {
    let origin = app(|a| {
        if a.main.0.is_null() {
            None
        } else {
            Some(bar_origin(a))
        }
    });
    let Some(origin) = origin else { return Vec::new() };
    let (_, lay) = layout_now(&origin);
    if !lay.shown {
        return Vec::new();
    }
    lay.separators.into_iter().filter(|r| r.live()).map(|r| r.to_rect()).collect()
}

unsafe fn popup_overflow(owner: HWND, doc: &Doc, lay: &BarLay) {
    let actions = hidden_actions(doc, lay);
    if actions.is_empty() {
        return;
    }
    let Ok(menu) = CreatePopupMenu() else { return };
    let input = enable_input();
    for (i, action) in actions.iter().enumerate() {
        let flags = if enabled(*action, &input) { MF_STRING } else { MF_STRING | MF_GRAYED };
        _ = AppendMenuW(menu, flags, i + 1, &HSTRING::from(crate::tr(label_key(*action))));
    }
    let at = button_point(ID_CMD_OVERFLOW);
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, at.x, at.y, None, owner, None).0 as usize;
    _ = DestroyMenu(menu);
    if let Some(action) = actions.get(pick.wrapping_sub(1)) {
        run(owner, *action, doc, lay, ID_CMD_OVERFLOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(vertical: bool, tab_w: i32, pieces: &[Piece]) -> BarLay {
        layout(pieces, &LayIn {
            vertical_tabs: vertical,
            tab_w,
            top: 4,
            bottom: 0,
            left: 0,
            dpi: 96,
            max_buttons: CMD_SLOTS as usize,
        })
    }

    /// `height` is the column, starting under a 32px tab row, at the window's left edge.
    fn column(height: i32, left: i32, pieces: &[Piece]) -> BarLay {
        layout(pieces, &LayIn {
            vertical_tabs: false,
            tab_w: 0,
            top: 32,
            bottom: 32 + height,
            left,
            dpi: 96,
            max_buttons: CMD_SLOTS as usize,
        })
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
        assert_eq!(doc.command_ids(), ["file.dir", "file.new", "file.delete", "file.paste", "edit.rename", "view.mode", "file.properties", "file.mark"]);
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
        assert_eq!(
            doc.command_ids(),
            ["file.dir", "file.new", "file.delete", "file.paste", "edit.rename", "view.mode", "file.properties", "file.mark"]
        );
        assert_eq!(action_of("edit.rename"), Some(Action::RenameInPlace));
        assert!(action_of("file.rename").is_none(), "file.rename is batch rename and is not a toolbar command");
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
        assert!(lay.overflow.is_none(), "900px holds the eight commands");
        assert_eq!(lay.commands.len(), 8);
        assert!(lay.hidden.is_empty());
        assert_eq!(lay.edit.right, 900);
        assert_eq!(lay.edit.right - lay.edit.left, 32);
        assert_eq!(lay.edit.bottom - lay.edit.top, 24);
        assert_eq!(lay.edit.top, 4);
        let mut prev = 0;
        for r in &lay.commands {
            assert!(r.left >= prev);
            assert_eq!(r.right - r.left, 32);
            assert!(r.right + 4 <= lay.edit.left, "command {:?} overlaps change {:?}", r, lay.edit);
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
        assert_eq!(over.right + 4, lay.edit.left);
        assert_eq!(over.right - over.left, 32);
        assert!(lay.commands.len() < 8);
        assert!(!lay.hidden.is_empty());
        for r in &lay.commands {
            assert!(r.right + 4 <= over.left);
            assert!(r.right < lay.edit.left);
        }
        assert_eq!(lay.commands.len() + lay.hidden.len(), 8);
    }

    #[test]
    fn horizontal_tabs_stack_icons_in_a_narrow_column() {
        let lay = column(700, 0, &defaults());
        assert!(lay.shown);
        assert!(lay.overflow.is_none());
        assert_eq!(lay.commands.len(), 8);
        assert!(lay.hidden.is_empty());
        assert_eq!(lay.edit.left, 4, "32px button centered in the 40px column");
        assert_eq!(lay.edit.right - lay.edit.left, 32);
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
        assert_eq!(beside.edit.left, 248, "open side panel: column starts after the panel and the gap");
        assert!(beside.commands.iter().all(|r| r.left >= 244));
    }

    #[test]
    fn short_column_puts_overflow_above_change() {
        let lay = column(120, 0, &defaults());
        assert!(lay.shown);
        let over = lay.overflow.expect("overflow");
        assert_eq!(over.bottom + 4, lay.edit.top);
        assert_eq!(over.left, lay.edit.left);
        assert_eq!(over.right - over.left, 32);
        assert!(lay.commands.len() < 8);
        assert!(!lay.hidden.is_empty());
        for r in &lay.commands {
            assert!(r.bottom + 4 <= over.top);
            assert!(r.bottom < lay.edit.top);
            assert!(r.top >= 32);
        }
        assert_eq!(lay.commands.len() + lay.hidden.len(), 8);
    }

    #[test]
    fn column_separator_is_a_horizontal_line() {
        let pieces = vec![
            Piece::Command { action: Action::NewFolder, icon: IconChoice::Auto },
            Piece::Separator,
            Piece::Command { action: Action::Paste, icon: IconChoice::Auto },
        ];
        let lay = column(400, 0, &pieces);
        assert_eq!(lay.separators.len(), 1);
        let sep = lay.separators[0];
        assert_eq!(sep.bottom - sep.top, 1);
        assert_eq!(sep.right - sep.left, 32);
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
        let pieces = vec![
            Piece::Command { action: Action::NewFolder, icon: IconChoice::Auto },
            Piece::Separator,
            Piece::Command { action: Action::Paste, icon: IconChoice::Auto },
        ];
        let lay = placed(true, 900, &pieces);
        assert_eq!(lay.commands.len(), 2);
        assert_eq!(lay.separators.len(), 1);
        let sep = lay.separators[0];
        assert!(sep.left >= lay.commands[0].right);
        assert!(sep.right <= lay.commands[1].left);
        assert!(sep.right - sep.left < 32);
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
    fn settings_reset_does_not_name_toolbar_json() {
        let src = include_str!("shell/fileop.rs");
        let start = src.find("pub(crate) unsafe fn reset_prefs").expect("reset_prefs");
        let end = src[start..].find("pub(crate) unsafe fn import_settings").map(|i| start + i).unwrap_or(src.len());
        let body = &src[start..end];
        assert!(!body.contains("toolbar"));
        assert_eq!(FILE_NAME, "toolbar.json");
        assert_ne!(FILE_NAME, crate::settings::store::SETTINGS_FILE);
    }
}
