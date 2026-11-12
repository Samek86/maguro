//! Favourite kinds beyond a plain folder or file.
//!
//! Older favourites are a name, a path, and an optional `file` flag. Those objects
//! still load, and a save writes them back without the new keys. A password is never
//! a field here: an RDP item keeps only `has_password`, and the secret lives in
//! Windows Credential Manager under `Maguro:rdp:<host>`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Map, Value};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{AssocQueryStringW, IShellItem, SHCreateItemFromParsingName, ASSOCF_NONE, ASSOCSTR_EXECUTABLE, SIGDN_NORMALDISPLAY};

use crate::Key;

/// How a leaf opens. Folder and file are the historical kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Folder,
    File,
    Special,
    Command,
    Url,
    Rdp,
}

impl ItemKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ItemKind::Folder => "folder",
            ItemKind::File => "file",
            ItemKind::Special => "special",
            ItemKind::Command => "command",
            ItemKind::Url => "url",
            ItemKind::Rdp => "rdp",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "folder" => Some(ItemKind::Folder),
            "file" => Some(ItemKind::File),
            "special" => Some(ItemKind::Special),
            "command" => Some(ItemKind::Command),
            "url" => Some(ItemKind::Url),
            "rdp" => Some(ItemKind::Rdp),
            _ => None,
        }
    }

    /// Command, URL, and RDP are not folders the star or a pane can navigate.
    pub(crate) fn navigable(self) -> bool {
        !matches!(self, ItemKind::Command | ItemKind::Url | ItemKind::Rdp)
    }

    pub(crate) fn launches(self) -> bool {
        !matches!(self, ItemKind::Folder | ItemKind::File)
    }

    pub(crate) fn label(self) -> Key {
        match self {
            ItemKind::Folder => Key::Folder,
            ItemKind::File => Key::File,
            ItemKind::Special => Key::KindSpecialFolder,
            ItemKind::Command => Key::KindCommand,
            ItemKind::Url => Key::KindUrl,
            ItemKind::Rdp => Key::KindRdp,
        }
    }
}

/// Ordered kinds in the add/edit dialog.
pub(crate) const KINDS: [ItemKind; 6] = [
    ItemKind::Folder,
    ItemKind::File,
    ItemKind::Special,
    ItemKind::Command,
    ItemKind::Url,
    ItemKind::Rdp,
];

/// Seconds before the temporary TERMSRV credential is removed. Omitted from the file when this.
pub(crate) const DEFAULT_CRED_SECS: u32 = 10;
const MAX_CRED_SECS: u32 = 3600;
const MAX_MEMO: usize = 2000;
const MAX_TARGET: usize = 1024;

/// Window sizes offered for an RDP session, width then height.
pub(crate) const RDP_WINDOWS: [(u32, u32); 10] = [
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RdpScreen {
    Auto,
    Full,
    Window { w: u32, h: u32 },
}

impl Default for RdpScreen {
    fn default() -> Self {
        RdpScreen::Auto
    }
}

impl RdpScreen {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() || t == "auto" {
            return Some(RdpScreen::Auto);
        }
        if t == "full" || t == "fullscreen" {
            return Some(RdpScreen::Full);
        }
        let (w, h) = t.split_once(['x', '×', '*'])?;
        let w: u32 = w.trim().parse().ok()?;
        let h: u32 = h.trim().parse().ok()?;
        screen_size(w, h)
    }

    pub(crate) fn token(self) -> String {
        match self {
            RdpScreen::Auto => "auto".into(),
            RdpScreen::Full => "full".into(),
            RdpScreen::Window { w, h } => format!("{w}x{h}"),
        }
    }
}

fn screen_size(w: u32, h: u32) -> Option<RdpScreen> {
    (w >= 640 && h >= 480 && w <= 7680 && h <= 4320).then_some(RdpScreen::Window { w, h })
}

/// A custom icon. `index` is the image inside an `.ico`, `.exe`, or `.dll`. Zero is the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ItemIcon {
    pub(crate) path: String,
    pub(crate) index: i32,
}

/// Extra fields on a leaf. Absent on every older favourite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FavItem {
    pub(crate) kind: ItemKind,
    pub(crate) color: Option<u32>,
    pub(crate) icon: Option<ItemIcon>,
    pub(crate) memo: String,
    pub(crate) args: String,
    pub(crate) workdir: String,
    pub(crate) admin: bool,
    pub(crate) user: String,
    pub(crate) screen: RdpScreen,
    pub(crate) rdp_extra: String,
    pub(crate) cred_secs: u32,
    pub(crate) has_password: bool,
}

impl Default for FavItem {
    fn default() -> Self {
        FavItem {
            kind: ItemKind::Folder,
            color: None,
            icon: None,
            memo: String::new(),
            args: String::new(),
            workdir: String::new(),
            admin: false,
            user: String::new(),
            screen: RdpScreen::Auto,
            rdp_extra: String::new(),
            cred_secs: DEFAULT_CRED_SECS,
            has_password: false,
        }
    }
}

impl FavItem {
    /// A folder or file with none of the optional fields. The file stores that as it always has.
    pub(crate) fn is_plain(&self) -> bool {
        matches!(self.kind, ItemKind::Folder | ItemKind::File)
            && self.color.is_none()
            && self.icon.is_none()
            && self.memo.is_empty()
            && self.args.is_empty()
            && self.workdir.is_empty()
            && !self.admin
            && self.user.is_empty()
            && self.screen == RdpScreen::Auto
            && self.rdp_extra.is_empty()
            && self.cred_secs == DEFAULT_CRED_SECS
            && !self.has_password
    }
}

/// One shell place the dialog lists. `path` is what a favourite stores and what the pane opens.
pub(crate) struct ShellPlace {
    pub(crate) path: &'static str,
    pub(crate) label: Key,
}

pub(crate) fn shell_places() -> &'static [ShellPlace] {
    &[
        ShellPlace { path: "::{645FF040-5081-101B-9F08-00AA002F954E}", label: Key::RecycleBinFolder },
        ShellPlace { path: "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", label: Key::ThisPc },
        ShellPlace { path: "shell:Desktop", label: Key::Desktop },
        ShellPlace { path: "shell:Personal", label: Key::KindDocuments },
        ShellPlace { path: "shell:Downloads", label: Key::DownloadsFolder },
        ShellPlace { path: "shell:My Music", label: Key::KindMusic },
        ShellPlace { path: "shell:My Pictures", label: Key::KindPictures },
        ShellPlace { path: "shell:My Video", label: Key::KindVideos },
        ShellPlace { path: "shell:ControlPanelFolder", label: Key::ControlPanel },
        ShellPlace { path: "shell:NetworkPlacesFolder", label: Key::Network },
        ShellPlace { path: "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}", label: Key::QuickAccess },
    ]
}

pub(crate) fn shell_place_label(path: &str) -> Option<Key> {
    let key = path.trim();
    shell_places().iter().find(|p| p.path.eq_ignore_ascii_case(key)).map(|p| p.label)
}

/// A `::{CLSID}` or `shell:` parsing name. These are not disk paths.
pub(crate) fn shell_parsing_name(path: &str) -> bool {
    let path = path.trim();
    path.starts_with("::") || path.get(..6).is_some_and(|h| h.eq_ignore_ascii_case("shell:"))
}

/// Empty, or the raw parsing name. A special folder with either shows the shell's name.
fn raw_shell_name(name: &str, path: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return true;
    }
    let path = path.trim();
    (!path.is_empty() && name.eq_ignore_ascii_case(path)) || shell_parsing_name(name)
}

/// Name drawn for a leaf. Special folders with no real display name resolve here, in the
/// language that is current now, so a saved favourite does not freeze one language's word.
pub(crate) fn fav_shown_name(kind: ItemKind, stored: &str, path: &str) -> String {
    let stored = stored.trim();
    let path = path.trim();
    if kind == ItemKind::Special && raw_shell_name(stored, path) {
        if let Some(key) = shell_place_label(path) {
            let label = crate::tr(key);
            if !label.is_empty() {
                return label.to_string();
            }
        }
        if let Some(label) = shell_namespace_name(path) {
            if !raw_shell_name(&label, path) {
                return label;
            }
        }
    }
    if !stored.is_empty() {
        return stored.to_string();
    }
    if path.is_empty() {
        String::new()
    } else {
        crate::short_name(path)
    }
}

/// OS display name for a shell parsing name, when it is not the parsing name itself.
pub(crate) fn shell_display_name(path: &str) -> Option<String> {
    let name = shell_namespace_name(path)?;
    if raw_shell_name(&name, path) { None } else { Some(name) }
}

/// `SIGDN_NORMALDISPLAY` for a shell parsing name. None when the shell cannot name it.
/// Names are cached for the process. The OS language does not change while Maguro is running,
/// and a tab paint must not ask the shell again.
pub(crate) fn shell_namespace_name(path: &str) -> Option<String> {
    let path = path.trim();
    if !shell_parsing_name(path) {
        return None;
    }
    let key = path.to_ascii_lowercase();
    if let Some(hit) = shell_name_cache().get(&key) {
        return hit.clone();
    }
    let (name, cacheable) = query_shell_namespace_name(path);
    if cacheable {
        shell_name_cache().insert(key, name.clone());
    }
    name
}

fn shell_name_cache() -> std::sync::MutexGuard<'static, HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// `(name, cacheable)`. A miss is cached only after COM was ready, so a not-yet-initialized
/// thread does not freeze `None` for a real shell place.
fn query_shell_namespace_name(path: &str) -> (Option<String>, bool) {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let balanced = hr.is_ok();
        let name = (|| {
            let wide = HSTRING::from(path);
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
            let pw = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
            let name = crate::pwstr(pw).trim().to_string();
            (!name.is_empty()).then_some(name)
        })();
        if balanced {
            CoUninitialize();
        }
        let cacheable = name.is_some() || balanced;
        (name, cacheable)
    }
}

/// Icon a leaf should draw, and whether a missing file on disk dims the row.
/// Command, URL, RDP, and shell places are not files that can be "missing".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FavGlyph {
    pub(crate) path: String,
    pub(crate) file: bool,
    pub(crate) index: i32,
    pub(crate) track_missing: bool,
}

pub(crate) fn fav_glyph(kind: ItemKind, path: &str, icon: Option<&ItemIcon>) -> FavGlyph {
    if let Some(icon) = icon {
        if !icon.path.trim().is_empty() {
            return FavGlyph {
                path: icon.path.clone(),
                file: true,
                index: icon.index,
                track_missing: matches!(kind, ItemKind::Folder | ItemKind::File),
            };
        }
    }
    let (icon_path, file) = match kind {
        ItemKind::Command => (command_icon_path(path), true),
        ItemKind::Url => (url_icon_path(), true),
        ItemKind::Rdp => (mstsc_exe(), true),
        ItemKind::File => (path.to_string(), true),
        ItemKind::Special | ItemKind::Folder => (path.to_string(), false),
    };
    FavGlyph {
        path: icon_path,
        file,
        index: -1,
        track_missing: matches!(kind, ItemKind::Folder | ItemKind::File),
    }
}

/// `track_missing` is false for anything that is not a folder or file on disk.
/// `target_missing` is the favourite path itself, not the icon we borrowed.
pub(crate) fn glyph_is_missing(g: &FavGlyph, target_missing: bool) -> bool {
    g.track_missing && target_missing
}

/// First token of a command line. Quotes keep a path that contains spaces.
fn command_program(raw: &str) -> String {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// Executable whose icon a command row draws. A bare name is resolved on `PATH`.
pub(crate) fn command_icon_path(raw: &str) -> String {
    let program = command_program(raw);
    if program.is_empty() {
        return String::new();
    }
    if program.contains(['\\', '/']) {
        return program;
    }
    search_path_file(&program).unwrap_or(program)
}

fn search_path_file(name: &str) -> Option<String> {
    let mut buf = vec![0u16; 32768];
    let file = HSTRING::from(name);
    let n = unsafe { SearchPathW(PCWSTR::null(), PCWSTR(file.as_ptr()), PCWSTR::null(), Some(&mut buf), None) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..n as usize]);
    (!s.is_empty()).then_some(s)
}

/// Browser (or other handler) registered for web addresses. Not a fake `page.url` file.
pub(crate) fn url_icon_path() -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        assoc_open_exe("http").or_else(|| assoc_open_exe("https")).or_else(|| {
            let explorer = command_icon_path("explorer.exe");
            std::path::Path::new(&explorer).is_file().then_some(explorer)
        }).unwrap_or_default()
    })
    .clone()
}

fn assoc_open_exe(query: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut n = buf.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, &HSTRING::from(query), &HSTRING::from("open"), Some(PWSTR(buf.as_mut_ptr())), &mut n)
    };
    if hr.is_err() {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(n as usize).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]).trim().trim_matches('"').to_string();
    std::path::Path::new(&s).is_file().then_some(s)
}

/// What the dialog collected. `password` is memory only and is never written to the favourites file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) kind: ItemKind,
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) color: Option<u32>,
    pub(crate) icon: Option<ItemIcon>,
    pub(crate) memo: String,
    pub(crate) args: String,
    pub(crate) workdir: String,
    pub(crate) admin: bool,
    pub(crate) user: String,
    pub(crate) screen: RdpScreen,
    pub(crate) rdp_extra: String,
    pub(crate) cred_secs: u32,
    pub(crate) password: String,
    pub(crate) clear_password: bool,
}

impl Draft {
    pub(crate) fn blank(kind: ItemKind) -> Self {
        Draft {
            kind,
            name: String::new(),
            target: String::new(),
            color: None,
            icon: None,
            memo: String::new(),
            args: String::new(),
            workdir: String::new(),
            admin: false,
            user: String::new(),
            screen: RdpScreen::Auto,
            rdp_extra: String::new(),
            cred_secs: DEFAULT_CRED_SECS,
            password: String::new(),
            clear_password: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    NeedPath,
    NeedProgram,
    NeedUrl,
    BadUrl,
    NeedHost,
    BadHost,
    NeedShell,
    BadShell,
    BadCred,
    BadIcon,
}

impl Fault {
    pub(crate) fn key(self) -> Key {
        match self {
            Fault::NeedPath => Key::NeedItemPath,
            Fault::NeedProgram => Key::NeedProgram,
            Fault::NeedUrl => Key::NeedUrl,
            Fault::BadUrl => Key::BadUrl,
            Fault::NeedHost => Key::NeedHost,
            Fault::BadHost => Key::BadHost,
            Fault::NeedShell => Key::NeedShell,
            Fault::BadShell => Key::BadShell,
            Fault::BadCred => Key::BadCredSeconds,
            Fault::BadIcon => Key::BadIconIndex,
        }
    }
}

pub(crate) fn validate(d: &Draft) -> Result<(), Fault> {
    let target = d.target.trim();
    if target.chars().count() > MAX_TARGET {
        return Err(Fault::NeedPath);
    }
    if d.icon.as_ref().is_some_and(|i| i.path.trim().is_empty() || i.index < 0 || i.index > 1000) {
        return Err(Fault::BadIcon);
    }
    match d.kind {
        ItemKind::Folder | ItemKind::File => {
            if target.is_empty() || target.chars().any(|c| c.is_control()) {
                return Err(Fault::NeedPath);
            }
        }
        ItemKind::Special => {
            if target.is_empty() {
                return Err(Fault::NeedShell);
            }
            if !valid_shell(target) {
                return Err(Fault::BadShell);
            }
        }
        ItemKind::Command => {
            if target.is_empty() || target.chars().any(|c| c.is_control()) {
                return Err(Fault::NeedProgram);
            }
        }
        ItemKind::Url => {
            if target.is_empty() {
                return Err(Fault::NeedUrl);
            }
            if !valid_url(target) {
                return Err(Fault::BadUrl);
            }
        }
        ItemKind::Rdp => {
            if target.is_empty() {
                return Err(Fault::NeedHost);
            }
            if !valid_host(target) {
                return Err(Fault::BadHost);
            }
            if d.cred_secs > MAX_CRED_SECS {
                return Err(Fault::BadCred);
            }
        }
    }
    Ok(())
}

pub(crate) fn valid_url(s: &str) -> bool {
    let s = s.trim();
    let Some((scheme, rest)) = s.split_once(':') else { return false };
    if scheme.len() < 2 || scheme.len() > 16 || !scheme.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let rest = rest.trim();
    if rest.is_empty() || rest.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ftp" | "mailto")
}

pub(crate) fn valid_shell(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 8 || s.len() > 260 || s.chars().any(|c| c.is_control()) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("shell:") {
        let rest = rest.trim();
        return !rest.is_empty() && !rest.contains(['\n', '"']);
    }
    clsid_body(s.strip_prefix("::").unwrap_or(""))
}

fn clsid_body(s: &str) -> bool {
    let s = s.trim();
    if !(s.starts_with('{') && s.ends_with('}') && s.len() == 38) {
        return false;
    }
    s.chars().enumerate().all(|(i, c)| match i {
        0 | 37 => true,
        9 | 14 | 19 | 24 => c == '-',
        _ => c.is_ascii_hexdigit(),
    })
}

pub(crate) fn valid_host(s: &str) -> bool {
    split_host(s.trim()).is_some()
}

/// Host and optional port. IPv6 is bracketed (`[::1]`, `[::1]:3389`).
pub(crate) fn split_host(s: &str) -> Option<(&str, Option<u16>)> {
    let s = s.trim();
    if s.is_empty() || s.len() > 253 + 8 || s.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\' | '/')) {
        return None;
    }
    if let Some(body) = s.strip_prefix('[') {
        let (addr, rest) = body.split_once(']')?;
        if addr.is_empty() || !addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':') || !addr.contains(':') {
            return None;
        }
        let host_end = s.len() - rest.len();
        let host = &s[..host_end];
        if rest.is_empty() {
            return Some((host, None));
        }
        let port = rest.strip_prefix(':')?;
        return Some((host, Some(parse_port(port)?)));
    }
    if let Some((host, port)) = s.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && !host.is_empty() && !host.contains(':') {
            return Some((host, Some(parse_port(port)?)));
        }
    }
    if s.contains(':') {
        return None;
    }
    Some((s, None))
}

fn parse_port(s: &str) -> Option<u16> {
    let n: u16 = s.parse().ok()?;
    (n > 0).then_some(n)
}

/// Build the leaf fields the tree stores. The password is not copied onto the item.
pub(crate) fn item_from_draft(d: &Draft, has_password: bool) -> Option<FavItem> {
    let mut item = FavItem {
        kind: d.kind,
        color: d.color,
        icon: d.icon.clone().filter(|i| !i.path.trim().is_empty()),
        memo: d.memo.trim().chars().take(MAX_MEMO).collect(),
        args: if d.kind == ItemKind::Command { d.args.clone() } else { String::new() },
        workdir: if d.kind == ItemKind::Command { d.workdir.trim().to_string() } else { String::new() },
        admin: d.kind == ItemKind::Command && d.admin,
        user: if d.kind == ItemKind::Rdp { d.user.trim().to_string() } else { String::new() },
        screen: if d.kind == ItemKind::Rdp { d.screen } else { RdpScreen::Auto },
        rdp_extra: if d.kind == ItemKind::Rdp { d.rdp_extra.trim().to_string() } else { String::new() },
        cred_secs: if d.kind == ItemKind::Rdp { d.cred_secs.min(MAX_CRED_SECS) } else { DEFAULT_CRED_SECS },
        has_password: d.kind == ItemKind::Rdp && has_password,
    };
    if item.icon.as_ref().is_some_and(|i| i.index < 0) {
        item.icon = None;
    }
    (!item.is_plain()).then_some(item)
}

/// Whether the saved leaf should say a password is on file, before Credential Manager is touched.
pub(crate) fn stored_has_password(old_has: bool, draft: &Draft) -> bool {
    if draft.kind != ItemKind::Rdp || draft.clear_password {
        return false;
    }
    if !draft.password.is_empty() {
        return true;
    }
    old_has
}

/// Credential Manager work for one save. `copy_from` is an old host whose secret moves when the
/// password box was left blank. `forget` hosts are removed only when no remaining RDP item uses them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SecretPlan {
    pub(crate) forget: Vec<String>,
    pub(crate) write_host: String,
    pub(crate) write_secret: String,
    pub(crate) copy_from: String,
}

pub(crate) fn secret_plan(old_kind: Option<ItemKind>, old_host: &str, old_has: bool, draft: &Draft) -> SecretPlan {
    let mut plan = SecretPlan { forget: Vec::new(), write_host: String::new(), write_secret: String::new(), copy_from: String::new() };
    let old_rdp = old_kind == Some(ItemKind::Rdp) && !old_host.is_empty();
    let new_host = draft.target.trim();
    let same = old_rdp && host_key(old_host) == host_key(new_host) && draft.kind == ItemKind::Rdp;
    if old_rdp && !same {
        plan.forget.push(old_host.to_string());
    }
    if draft.kind != ItemKind::Rdp {
        return plan;
    }
    if draft.clear_password {
        plan.forget.push(new_host.to_string());
        return plan;
    }
    if !draft.password.is_empty() {
        plan.write_host = new_host.to_string();
        plan.write_secret = draft.password.clone();
        return plan;
    }
    if old_has && old_rdp && !same {
        plan.copy_from = old_host.to_string();
        plan.write_host = new_host.to_string();
    }
    plan
}

pub(crate) fn host_key(host: &str) -> String {
    host.trim().to_ascii_lowercase()
}

/// Maguro's own credential. The favourites file stores only a flag, never this secret.
pub(crate) fn store_target(host: &str) -> String {
    format!("Maguro:rdp:{}", host_key(host))
}

/// The target mstsc reads. Written just before launch and removed after the delay.
pub(crate) fn termsrv_target(host: &str) -> String {
    format!("TERMSRV/{}", host.trim())
}

/// How long to leave the TERMSRV credential. Zero still waits a second so mstsc can read it.
pub(crate) fn cred_hold_secs(secs: u32) -> u64 {
    u64::from(secs.min(MAX_CRED_SECS).max(1))
}

pub(crate) fn mstsc_exe() -> String {
    match std::env::var("SystemRoot") {
        Ok(root) if !root.trim().is_empty() => format!("{}\\System32\\mstsc.exe", root.trim_end_matches(['\\', '/'])),
        _ => "mstsc.exe".into(),
    }
}

/// Quote one command-line argument the way `CommandLineToArgvW` reads it.
pub(crate) fn quote_arg(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.to_string();
    }
    let mut out = String::from("\"");
    let mut bs = 0u32;
    for c in s.chars() {
        if c == '\\' {
            bs += 1;
            continue;
        }
        if c == '"' {
            for _ in 0..(bs * 2 + 1) {
                out.push('\\');
            }
            out.push('"');
            bs = 0;
            continue;
        }
        for _ in 0..bs {
            out.push('\\');
        }
        bs = 0;
        out.push(c);
    }
    for _ in 0..(bs * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) file: String,
    pub(crate) verb: &'static str,
    pub(crate) parameters: String,
    pub(crate) directory: String,
}

pub(crate) fn command_launch(program: &str, args: &str, workdir: &str, admin: bool) -> Launch {
    Launch {
        file: program.trim().to_string(),
        verb: if admin { "runas" } else { "open" },
        parameters: args.to_string(),
        directory: workdir.trim().to_string(),
    }
}

pub(crate) fn url_launch(url: &str) -> Launch {
    Launch { file: url.trim().to_string(), verb: "open", parameters: String::new(), directory: String::new() }
}

pub(crate) fn rdp_parameters(rdp_file: &str, host: &str) -> String {
    format!("{} /v:{}", quote_arg(rdp_file), host.trim())
}

pub(crate) fn rdp_launch(rdp_file: &str, host: &str) -> Launch {
    Launch {
        file: mstsc_exe(),
        verb: "open",
        parameters: rdp_parameters(rdp_file, host),
        directory: String::new(),
    }
}

pub(crate) fn explorer_launch(path: &str) -> Launch {
    Launch { file: "explorer.exe".into(), verb: "open", parameters: quote_arg(path.trim()), directory: String::new() }
}

/// `.rdp` body. The password is not a line; mstsc reads TERMSRV from Credential Manager.
/// Clipboard sharing and remote audio on this PC are always written. A stored `rdp_extra` is not applied.
pub(crate) fn rdp_text(host: &str, user: &str, screen: RdpScreen, prompt: bool) -> String {
    let mut lines = Vec::new();
    lines.push(format!("full address:s:{}", host.trim()));
    let user = user.trim();
    if !user.is_empty() {
        lines.push(format!("username:s:{user}"));
    }
    match screen {
        RdpScreen::Auto => {}
        RdpScreen::Full => lines.push("screen mode id:i:2".into()),
        RdpScreen::Window { w, h } => {
            lines.push("screen mode id:i:1".into());
            lines.push(format!("desktopwidth:i:{w}"));
            lines.push(format!("desktopheight:i:{h}"));
        }
    }
    lines.push(format!("prompt for credentials:i:{}", if prompt { 1 } else { 0 }));
    lines.push("authentication level:i:2".into());
    lines.push("redirectclipboard:i:1".into());
    lines.push("audiomode:i:0".into());
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    text
}

pub(crate) fn rdp_bytes(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Read optional item fields. A password key is ignored and is not returned.
pub(crate) fn pull_item(o: &Map<String, Value>, file: bool) -> (Option<FavItem>, bool) {
    let kind = o.get("kind").and_then(|v| v.as_str()).and_then(ItemKind::parse);
    let color = o.get("color").and_then(parse_color);
    let icon_path = o.get("icon").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let icon_index = o.get("icon_index").and_then(|v| v.as_i64()).unwrap_or(0).clamp(0, 1000) as i32;
    let memo = o.get("memo").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(MAX_MEMO).collect::<String>();
    let args = o.get("args").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let workdir = o.get("workdir").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let admin = o.get("admin").and_then(|v| v.as_bool()).unwrap_or(false);
    let user = o.get("user").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let screen = o.get("screen").and_then(|v| v.as_str()).and_then(RdpScreen::parse).unwrap_or(RdpScreen::Auto);
    let rdp_extra = o.get("rdp_extra").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let cred_secs = o.get("cred_delete").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_CRED_SECS as u64).min(MAX_CRED_SECS as u64) as u32;
    let has_password = o.get("has_password").and_then(|v| v.as_bool()).unwrap_or(false);
    let present = kind.is_some()
        || color.is_some()
        || !icon_path.is_empty()
        || !memo.is_empty()
        || !args.is_empty()
        || !workdir.is_empty()
        || admin
        || !user.is_empty()
        || !matches!(screen, RdpScreen::Auto)
        || !rdp_extra.is_empty()
        || cred_secs != DEFAULT_CRED_SECS
        || has_password;
    if !present {
        return (None, file);
    }
    let explicit = kind;
    let kind = explicit.unwrap_or(if file { ItemKind::File } else { ItemKind::Folder });
    // No kind key: a memo or colour on an old file leaf keeps `file`. An explicit kind decides.
    let file = if explicit.is_none() { file } else { kind == ItemKind::File };
    let item = FavItem {
        kind,
        color,
        icon: (!icon_path.is_empty()).then(|| ItemIcon { path: icon_path, index: icon_index }),
        memo,
        args: if kind == ItemKind::Command { args } else { String::new() },
        workdir: if kind == ItemKind::Command { workdir } else { String::new() },
        admin: kind == ItemKind::Command && admin,
        user: if kind == ItemKind::Rdp { user } else { String::new() },
        screen: if kind == ItemKind::Rdp { screen } else { RdpScreen::Auto },
        rdp_extra: if kind == ItemKind::Rdp { rdp_extra } else { String::new() },
        cred_secs: if kind == ItemKind::Rdp { cred_secs } else { DEFAULT_CRED_SECS },
        has_password: kind == ItemKind::Rdp && has_password,
    };
    if item.is_plain() {
        (None, file)
    } else {
        (Some(item), file)
    }
}

fn parse_color(v: &Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return (n <= 0xFF_FF_FF).then_some(n as u32);
    }
    let s = v.as_str()?.trim();
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

pub(crate) fn push_item(m: &mut Map<String, Value>, item: &FavItem) {
    if item.is_plain() {
        return;
    }
    if item.kind.launches() {
        m.insert("kind".into(), json!(item.kind.as_str()));
    }
    if let Some(c) = item.color {
        m.insert("color".into(), json!(format!("#{c:06X}")));
    }
    if let Some(icon) = &item.icon {
        if !icon.path.is_empty() {
            m.insert("icon".into(), json!(icon.path));
            if icon.index != 0 {
                m.insert("icon_index".into(), json!(icon.index));
            }
        }
    }
    if !item.memo.is_empty() {
        m.insert("memo".into(), json!(item.memo));
    }
    if item.kind == ItemKind::Command {
        if !item.args.is_empty() {
            m.insert("args".into(), json!(item.args));
        }
        if !item.workdir.is_empty() {
            m.insert("workdir".into(), json!(item.workdir));
        }
        if item.admin {
            m.insert("admin".into(), json!(true));
        }
    }
    if item.kind == ItemKind::Rdp {
        if !item.user.is_empty() {
            m.insert("user".into(), json!(item.user));
        }
        if item.screen != RdpScreen::Auto {
            m.insert("screen".into(), json!(item.screen.token()));
        }
        if !item.rdp_extra.is_empty() {
            m.insert("rdp_extra".into(), json!(item.rdp_extra));
        }
        if item.cred_secs != DEFAULT_CRED_SECS {
            m.insert("cred_delete".into(), json!(item.cred_secs));
        }
        if item.has_password {
            m.insert("has_password".into(), json!(true));
        }
    }
}

/// An RDP host whose secret should be considered when this node goes away.
pub(crate) fn rdp_host_of(item: Option<&FavItem>, path: &str) -> Option<String> {
    let item = item?;
    if item.kind == ItemKind::Rdp && item.has_password && !path.trim().is_empty() {
        Some(path.to_string())
    } else {
        None
    }
}

pub(crate) fn same_rdp_host(a: &str, b: &str) -> bool {
    host_key(a) == host_key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rdp(host: &str) -> Draft {
        let mut d = Draft::blank(ItemKind::Rdp);
        d.target = host.into();
        d.user = "ada".into();
        d.cred_secs = 10;
        d
    }

    #[test]
    fn url_shell_and_host_rules() {
        assert!(valid_url("https://example.com/a"));
        assert!(valid_url("http://a"));
        assert!(valid_url("ftp://files.example/a"));
        assert!(valid_url("mailto:a@b.c"));
        assert!(!valid_url("example.com"));
        assert!(!valid_url(r"C:\Windows"));
        assert!(!valid_url("shell:Desktop"));
        assert!(!valid_url("javascript:alert(1)"));
        assert!(!valid_url("https://has space"));

        assert!(valid_shell("shell:Desktop"));
        assert!(valid_shell("shell:My Music"));
        assert!(valid_shell("shell:Downloads"));
        assert!(valid_shell("::{645FF040-5081-101B-9F08-00AA002F954E}"));
        assert!(valid_shell("shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}"));
        assert!(!valid_shell("Desktop"));
        assert!(!valid_shell("::{not-a-guid}"));
        assert!(!valid_shell(""));
        assert!(!valid_shell("shell:"));

        assert_eq!(split_host("pc"), Some(("pc", None)));
        assert_eq!(split_host("pc:3389"), Some(("pc", Some(3389))));
        assert_eq!(split_host("[::1]"), Some(("[::1]", None)));
        assert_eq!(split_host("[::1]:3390"), Some(("[::1]", Some(3390))));
        assert!(split_host("pc:0").is_none());
        assert!(split_host("pc:99999").is_none());
        assert!(split_host("::1").is_none());
        assert!(split_host("bad host").is_none());
        assert!(split_host("").is_none());
        assert!(split_host("host/path").is_none());
    }

    #[test]
    fn validation_matches_the_kind() {
        let mut folder = Draft::blank(ItemKind::Folder);
        assert_eq!(validate(&folder), Err(Fault::NeedPath));
        folder.target = r"C:\Work".into();
        assert!(validate(&folder).is_ok());

        let mut cmd = Draft::blank(ItemKind::Command);
        assert_eq!(validate(&cmd), Err(Fault::NeedProgram));
        cmd.target = r"C:\Tools\build.bat".into();
        cmd.admin = true;
        assert!(validate(&cmd).is_ok());

        let mut url = Draft::blank(ItemKind::Url);
        url.target = "example.com".into();
        assert_eq!(validate(&url), Err(Fault::BadUrl));
        url.target = "https://example.com".into();
        assert!(validate(&url).is_ok());

        let mut shell = Draft::blank(ItemKind::Special);
        assert_eq!(validate(&shell), Err(Fault::NeedShell));
        shell.target = "Desktop".into();
        assert_eq!(validate(&shell), Err(Fault::BadShell));
        shell.target = "shell:Desktop".into();
        assert!(validate(&shell).is_ok());

        let mut host = rdp("");
        assert_eq!(validate(&host), Err(Fault::NeedHost));
        host.target = "bad host".into();
        assert_eq!(validate(&host), Err(Fault::BadHost));
        host.target = "pc:3389".into();
        host.rdp_extra = "password 51:b:secret\r\naudiomode:i:2".into();
        assert!(validate(&host).is_ok());
        let generated = rdp_text(&host.target, &host.user, RdpScreen::Full, false);
        assert_eq!(generated.matches("redirectclipboard:i:1").count(), 1, "{generated}");
        assert_eq!(generated.matches("audiomode:i:0").count(), 1, "{generated}");
        assert!(!generated.contains("password"), "{generated}");
        assert!(!generated.contains("audiomode:i:2"), "{generated}");
        assert!(!generated.contains("secret"), "{generated}");
        host.cred_secs = 9_999;
        assert_eq!(validate(&host), Err(Fault::BadCred));
        host.cred_secs = 0;
        assert!(validate(&host).is_ok());
    }

    #[test]
    fn rdp_text_always_shares_the_clipboard_and_plays_remote_audio_here() {
        let full = rdp_text("pc:3389", "ada", RdpScreen::Full, false);
        assert_eq!(
            full,
            concat!(
                "full address:s:pc:3389\r\n",
                "username:s:ada\r\n",
                "screen mode id:i:2\r\n",
                "prompt for credentials:i:0\r\n",
                "authentication level:i:2\r\n",
                "redirectclipboard:i:1\r\n",
                "audiomode:i:0\r\n",
            )
        );
        let window = rdp_text("pc", "", RdpScreen::Window { w: 1280, h: 720 }, true);
        assert_eq!(
            window,
            concat!(
                "full address:s:pc\r\n",
                "screen mode id:i:1\r\n",
                "desktopwidth:i:1280\r\n",
                "desktopheight:i:720\r\n",
                "prompt for credentials:i:1\r\n",
                "authentication level:i:2\r\n",
                "redirectclipboard:i:1\r\n",
                "audiomode:i:0\r\n",
            )
        );
        let auto = rdp_text("pc", "", RdpScreen::Auto, true);
        assert_eq!(
            auto,
            concat!(
                "full address:s:pc\r\n",
                "prompt for credentials:i:1\r\n",
                "authentication level:i:2\r\n",
                "redirectclipboard:i:1\r\n",
                "audiomode:i:0\r\n",
            )
        );
        for text in [&full, &window, &auto] {
            assert_eq!(text.matches("redirectclipboard:i:1").count(), 1, "{text}");
            assert_eq!(text.matches("audiomode:i:0").count(), 1, "{text}");
            assert!(!text.contains("password"), "{text}");
            assert!(!text.contains("audiomode:i:2"), "{text}");
        }
    }

    #[test]
    fn rdp_file_has_no_password_and_launch_uses_v() {
        let text = rdp_text("pc:3389", "ada", RdpScreen::Full, false);
        assert!(text.contains("full address:s:pc:3389"));
        assert!(text.contains("username:s:ada"));
        assert!(text.contains("screen mode id:i:2"));
        assert!(text.contains("prompt for credentials:i:0"));
        assert_eq!(text.matches("redirectclipboard:i:1").count(), 1);
        assert_eq!(text.matches("audiomode:i:0").count(), 1);
        assert!(!text.to_ascii_lowercase().contains("password"));
        assert!(!text.contains("secret"));

        let window = rdp_text("pc", "", RdpScreen::Window { w: 1280, h: 720 }, true);
        assert!(window.contains("screen mode id:i:1"));
        assert!(window.contains("desktopwidth:i:1280"));
        assert!(window.contains("desktopheight:i:720"));
        assert!(window.contains("prompt for credentials:i:1"));
        assert!(!window.contains("username:"));
        assert_eq!(window.matches("redirectclipboard:i:1").count(), 1);
        assert_eq!(window.matches("audiomode:i:0").count(), 1);

        let auto = rdp_text("pc", "", RdpScreen::Auto, true);
        assert!(!auto.contains("screen mode id"));
        assert!(!auto.contains("desktopwidth"));
        assert_eq!(auto.matches("redirectclipboard:i:1").count(), 1);
        assert_eq!(auto.matches("audiomode:i:0").count(), 1);

        let bytes = rdp_bytes("full address:s:pc\r\n");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(String::from_utf16_lossy(&bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()), "full address:s:pc\r\n");

        let file = r"C:\Users\ada\AppData\Local\Temp\maguro-rdp.rdp";
        assert_eq!(rdp_parameters(file, "pc:3389"), format!("{} /v:pc:3389", quote_arg(file)));
        let launch = rdp_launch(file, "pc:3389");
        assert!(launch.file.to_ascii_lowercase().ends_with("mstsc.exe"));
        assert_eq!(launch.verb, "open");
        assert!(launch.parameters.contains("/v:pc:3389"));
        assert!(!launch.parameters.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn command_url_and_explorer_arguments() {
        let cmd = command_launch(r"C:\Tools\build.bat", "--release", r"C:\src", true);
        assert_eq!(cmd.verb, "runas");
        assert_eq!(cmd.parameters, "--release");
        assert_eq!(cmd.directory, r"C:\src");
        let plain = command_launch("notepad.exe", "", "", false);
        assert_eq!(plain.verb, "open");
        assert!(plain.parameters.is_empty());
        assert!(plain.directory.is_empty());

        let url = url_launch(" https://example.com ");
        assert_eq!(url.file, "https://example.com");
        assert_eq!(url.verb, "open");
        assert!(url.parameters.is_empty());

        let exp = explorer_launch("shell:My Music");
        assert_eq!(exp.file, "explorer.exe");
        assert_eq!(exp.parameters, "\"shell:My Music\"");
        assert_eq!(quote_arg(r"C:\no\space"), r"C:\no\space");
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn credential_plan_never_puts_the_secret_in_the_item() {
        let mut made = rdp("PC");
        made.password = "s3cret-pl4in".into();
        let plan = secret_plan(None, "", false, &made);
        assert_eq!(plan.write_host, "PC");
        assert_eq!(plan.write_secret, "s3cret-pl4in");
        assert!(plan.forget.is_empty());
        assert!(stored_has_password(false, &made));
        let item = item_from_draft(&made, true).unwrap();
        assert!(item.has_password);
        assert!(!format!("{item:?}").contains("s3cret"));
        assert_eq!(store_target("PC"), store_target("pc"));
        assert_eq!(store_target("pc:3389"), "Maguro:rdp:pc:3389");
        assert_eq!(termsrv_target("pc:3389"), "TERMSRV/pc:3389");
        assert_ne!(store_target("pc"), termsrv_target("pc"));

        made.password.clear();
        let keep = secret_plan(Some(ItemKind::Rdp), "pc", true, &made);
        assert!(keep.write_secret.is_empty());
        assert!(keep.copy_from.is_empty());
        assert!(keep.forget.is_empty());
        assert!(stored_has_password(true, &made));

        made.clear_password = true;
        let clear = secret_plan(Some(ItemKind::Rdp), "pc", true, &made);
        assert_eq!(clear.forget, vec!["PC".to_string()]);
        assert!(!stored_has_password(true, &made));

        made.clear_password = false;
        made.target = "other".into();
        made.password = "next".into();
        let moved = secret_plan(Some(ItemKind::Rdp), "pc", true, &made);
        assert_eq!(moved.forget, vec!["pc".to_string()]);
        assert_eq!(moved.write_host, "other");
        assert_eq!(moved.write_secret, "next");

        made.password.clear();
        let copy = secret_plan(Some(ItemKind::Rdp), "pc", true, &made);
        assert_eq!(copy.copy_from, "pc");
        assert_eq!(copy.write_host, "other");
        assert_eq!(copy.forget, vec!["pc".to_string()]);

        let mut folder = Draft::blank(ItemKind::Folder);
        folder.target = r"C:\Work".into();
        let drop = secret_plan(Some(ItemKind::Rdp), "pc", true, &folder);
        assert_eq!(drop.forget, vec!["pc".to_string()]);
        assert!(drop.write_secret.is_empty());
        assert!(!stored_has_password(true, &folder));
        assert_eq!(cred_hold_secs(0), 1);
        assert_eq!(cred_hold_secs(10), 10);
    }

    #[test]
    fn plain_folder_and_file_add_no_keys_and_a_password_field_is_dropped() {
        let mut m = Map::new();
        m.insert("name".into(), json!("Desk"));
        m.insert("path".into(), json!(r"C:\WORK"));
        let (item, file) = pull_item(&m, false);
        assert!(item.is_none());
        assert!(!file);

        let mut leaked = Map::new();
        leaked.insert("name".into(), json!("pc"));
        leaked.insert("kind".into(), json!("rdp"));
        leaked.insert("path".into(), json!("pc"));
        leaked.insert("password".into(), json!("s3cret-pl4in"));
        leaked.insert("has_password".into(), json!(true));
        leaked.insert("user".into(), json!("ada"));
        let (item, file) = pull_item(&leaked, true);
        let item = item.unwrap();
        assert_eq!(item.kind, ItemKind::Rdp);
        assert!(!file);
        assert!(item.has_password);
        assert_eq!(item.user, "ada");
        let mut out = Map::new();
        push_item(&mut out, &item);
        let stored = Value::Object(out).to_string();
        assert!(stored.contains("has_password"));
        assert!(!stored.contains("s3cret-pl4in"));
        assert!(!stored.contains("\"password\""));

        let mut memo = Map::new();
        memo.insert("path".into(), json!(r"D:\Photos"));
        memo.insert("memo".into(), json!("vacation"));
        memo.insert("color".into(), json!("#3D7EE0"));
        let (item, file) = pull_item(&memo, false);
        let item = item.unwrap();
        assert_eq!(item.kind, ItemKind::Folder);
        assert!(!file);
        assert_eq!(item.memo, "vacation");
        assert_eq!(item.color, Some(0x3D7E_E0));
        let mut round = Map::new();
        push_item(&mut round, &item);
        assert!(round.get("kind").is_none());
        assert_eq!(round.get("color").and_then(|v| v.as_str()), Some("#3D7EE0"));
    }

    #[test]
    fn a_new_leaf_stores_no_colour_and_an_edit_keeps_an_old_one() {
        let mut fresh = Draft::blank(ItemKind::Folder);
        fresh.target = r"C:\Work".into();
        fresh.memo = "notes".into();
        let item = item_from_draft(&fresh, false).unwrap();
        assert!(item.color.is_none());
        let mut out = Map::new();
        push_item(&mut out, &item);
        assert!(out.get("color").is_none());

        let mut old = Map::new();
        old.insert("path".into(), json!(r"D:\Photos"));
        old.insert("color".into(), json!("#3D7EE0"));
        let (loaded, _) = pull_item(&old, false);
        let loaded = loaded.unwrap();
        assert_eq!(loaded.color, Some(0x3D7E_E0));
        let mut edited = Draft::blank(loaded.kind);
        edited.target = r"D:\Photos".into();
        edited.memo = "kept".into();
        edited.color = loaded.color;
        let saved = item_from_draft(&edited, false).unwrap();
        assert_eq!(saved.memo, "kept");
        assert_eq!(saved.color, Some(0x3D7E_E0));
        let mut round = Map::new();
        push_item(&mut round, &saved);
        assert_eq!(round.get("memo").and_then(|v| v.as_str()), Some("kept"));
        assert_eq!(round.get("color").and_then(|v| v.as_str()), Some("#3D7EE0"));
    }

    #[test]
    fn old_memo_and_rdp_extra_round_trip_and_are_not_generated() {
        let stale = "password 51:b:secret\r\naudiomode:i:2\r\nredirectclipboard:i:0";
        let raw: Value = serde_json::from_str(
            "{\"kind\":\"rdp\",\"path\":\"pc\",\"user\":\"ada\",\"memo\":\"vacation\",\"rdp_extra\":\"password 51:b:secret\\r\\naudiomode:i:2\\r\\nredirectclipboard:i:0\"}",
        )
        .expect("memo and rdp_extra stay legal");
        let (item, file) = pull_item(raw.as_object().unwrap(), false);
        let item = item.unwrap();
        assert!(!file);
        assert_eq!(item.kind, ItemKind::Rdp);
        assert_eq!(item.memo, "vacation");
        assert_eq!(item.rdp_extra, stale);
        let mut round = Map::new();
        push_item(&mut round, &item);
        assert_eq!(round.get("memo").and_then(|v| v.as_str()), Some("vacation"));
        assert_eq!(round.get("rdp_extra").and_then(|v| v.as_str()), Some(stale));

        let mut draft = Draft::blank(ItemKind::Rdp);
        draft.target = "pc:3389".into();
        draft.user = "ada".into();
        draft.memo = item.memo.clone();
        draft.rdp_extra = item.rdp_extra.clone();
        assert!(validate(&draft).is_ok());
        let saved = item_from_draft(&draft, false).unwrap();
        assert_eq!(saved.memo, "vacation");
        assert_eq!(saved.rdp_extra, stale);
        let text = rdp_text("pc:3389", "ada", RdpScreen::Full, false);
        assert_eq!(
            text,
            concat!(
                "full address:s:pc:3389\r\n",
                "username:s:ada\r\n",
                "screen mode id:i:2\r\n",
                "prompt for credentials:i:0\r\n",
                "authentication level:i:2\r\n",
                "redirectclipboard:i:1\r\n",
                "audiomode:i:0\r\n",
            )
        );
        assert!(!text.contains("password 51:b:secret"));
        assert!(!text.contains("audiomode:i:2"));
        assert!(!text.contains("redirectclipboard:i:0"));
    }

    #[test]
    fn screen_tokens_and_shell_catalog() {
        assert_eq!(RdpScreen::parse("auto"), Some(RdpScreen::Auto));
        assert_eq!(RdpScreen::parse("full"), Some(RdpScreen::Full));
        assert_eq!(RdpScreen::parse("1920x1080"), Some(RdpScreen::Window { w: 1920, h: 1080 }));
        assert_eq!(RdpScreen::parse("1920×720"), Some(RdpScreen::Window { w: 1920, h: 720 }));
        assert!(RdpScreen::parse("10x10").is_none());
        assert!(shell_places().len() >= 8);
        assert!(shell_places().iter().any(|p| p.path.contains("645FF040")));
        assert!(shell_places().iter().any(|p| p.path.contains("20D04FE0")));
        assert_eq!(shell_place_label("shell:Downloads"), Some(Key::DownloadsFolder));
        assert!(ItemKind::Special.navigable());
        assert!(!ItemKind::Rdp.navigable());
        assert!(ItemKind::Command.launches());
    }

    #[test]
    fn empty_special_name_follows_the_language_and_is_not_saved_as_a_guid() {
        use std::sync::atomic::Ordering;
        let bin = "::{645FF040-5081-101B-9F08-00AA002F954E}";
        let prev = crate::UI.load(Ordering::Relaxed);
        let shown = |lang: u8, name: &str, path: &str| {
            crate::UI.store(lang, Ordering::Relaxed);
            fav_shown_name(ItemKind::Special, name, path)
        };
        let en = shown(0, "", bin);
        let ko = shown(1, "", bin);
        let ja = shown(2, "", bin);
        assert_eq!((en.as_str(), ko.as_str(), ja.as_str()), ("Recycle Bin", "휴지통", "ごみ箱"));
        for label in [&en, &ko, &ja] {
            assert!(!label.contains("645FF040"), "{label}");
        }
        assert_eq!(shown(0, bin, bin), "Recycle Bin");
        assert_eq!(shown(1, "shell:RecycleBinFolder", bin), "휴지통");
        assert_eq!(shown(2, "Mine", bin), "Mine");
        assert_eq!(shown(0, "", "shell:Downloads"), "Downloads");
        let mut draft = Draft::blank(ItemKind::Special);
        draft.target = bin.into();
        let node = crate::node_from_draft(&draft, false);
        assert!(node.name.is_empty(), "display language is resolved when drawn, not stored");
        crate::UI.store(0, Ordering::Relaxed);
        let label = crate::node_label(&node);
        assert_eq!(label, "Recycle Bin");
        assert_eq!(crate::visible_rows(std::slice::from_ref(&node))[0].name, label);
        crate::UI.store(prev, Ordering::Relaxed);
    }

    #[test]
    fn command_url_shell_and_rdp_icons_are_not_missing_files() {
        let bin = "::{645FF040-5081-101B-9F08-00AA002F954E}";
        let cmd = fav_glyph(ItemKind::Command, "notepad.exe", None);
        assert!(!cmd.track_missing);
        assert!(std::path::Path::new(&cmd.path).is_file(), "{}", cmd.path);
        assert!(cmd.path.to_ascii_lowercase().ends_with("notepad.exe"));
        let quoted = command_icon_path(r#""notepad.exe""#);
        assert!(std::path::Path::new(&quoted).is_file(), "{quoted}");
        assert!(!glyph_is_missing(&cmd, true));

        let url = fav_glyph(ItemKind::Url, "https://example.com", None);
        assert!(!url.track_missing);
        assert_ne!(url.path, "page.url");
        assert!(url.path.is_empty() || std::path::Path::new(&url.path).is_file(), "{}", url.path);
        assert!(!glyph_is_missing(&url, true));

        let rdp = fav_glyph(ItemKind::Rdp, "desk.local", None);
        assert!(!rdp.track_missing);
        assert!(rdp.path.to_ascii_lowercase().ends_with("mstsc.exe"));
        assert!(!glyph_is_missing(&rdp, true));

        let special = fav_glyph(ItemKind::Special, bin, None);
        assert!(!special.track_missing);
        assert!(special.path.starts_with("::"));
        assert!(!glyph_is_missing(&special, true));
        let shell = fav_glyph(ItemKind::Special, "shell:Downloads", None);
        assert!(shell.path.eq_ignore_ascii_case("shell:Downloads"));

        let gone = r"C:\maguro-no-such-folder-31b3bdd";
        let folder = fav_glyph(ItemKind::Folder, gone, None);
        assert!(folder.track_missing);
        assert_eq!(folder.path, gone);
        assert!(glyph_is_missing(&folder, true));
        assert!(!glyph_is_missing(&folder, false));
        assert!(crate::path_is_absent(gone));
        assert!(!crate::path_is_absent(r"C:\Windows"));
    }
}
