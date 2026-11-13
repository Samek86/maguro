//! HKCU writes for the plan in `plan`. No other hive is opened.

use std::sync::atomic::AtomicBool;

use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::imports::*;

use super::plan::{
    class_key, command_key, is_registered, path_fix_ops, register_ops, restore_ops, shell_key, split_launch, verb_key, ClassState, Op, CLASS_CREATED, DIRECTORY, DRIVE, PREV_DEFAULT,
    PREV_SET, SHELL_CREATED, VERB,
};

const OWNED: [&str; 2] = [DIRECTORY, DRIVE];

static SHOWN: AtomicBool = AtomicBool::new(false);

pub(crate) fn shown_on() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

/// Test-only cache write. Does not read or write the registry.
#[cfg(test)]
pub(crate) fn set_shown_for_test(on: bool) {
    SHOWN.store(on, Ordering::Relaxed);
}

pub(crate) fn reload_shown() {
    SHOWN.store(is_registered(&read_states()), Ordering::Relaxed);
}

/// ON writes our Directory and Drive verb. OFF deletes exactly the keys the plan names.
pub(crate) fn set_registered(on: bool) -> bool {
    let exe = current_exe();
    if on && exe.is_empty() {
        reload_shown();
        return false;
    }
    let states = read_states();
    let ops = if on { register_ops(&exe, &states) } else { restore_ops(&states) };
    let ok = unsafe { apply_ops(&ops) };
    reload_shown();
    ok
}

/// Startup: if both classes already point at Maguro, point the command at this exe.
pub(crate) fn repair_moved_exe() {
    let exe = current_exe();
    if exe.is_empty() {
        return;
    }
    let states = read_states();
    let ops = path_fix_ops(&exe, &states);
    if ops.is_empty() {
        return;
    }
    unsafe {
        _ = apply_ops(&ops);
    }
}

pub(crate) fn open_in_explorer(path: &str) {
    let param = super::plan::explorer_arg(path);
    if param.is_empty() {
        return;
    }
    let param = HSTRING::from(param);
    unsafe {
        _ = ShellExecuteW(None, w!("open"), w!("explorer.exe"), &param, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Filesystem arguments stay. shell: and CLSID arguments are opened with explorer.exe.
pub(crate) fn take_launch_args(args: Vec<String>) -> Vec<String> {
    let (pane, explorer) = split_launch(args);
    for path in explorer {
        open_in_explorer(&path);
    }
    pane
}

fn current_exe() -> String {
    let Ok(p) = std::env::current_exe() else { return String::new() };
    let s = p.to_string_lossy().into_owned();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn read_states() -> Vec<ClassState> {
    OWNED.iter().copied().map(|class| unsafe { read_class(class) }).collect()
}

unsafe fn read_class(class: &'static str) -> ClassState {
    let mut s = ClassState::absent(class);
    let Some(class_h) = open_key(&class_key(class), KEY_READ) else { return s };
    s.class_exists = true;
    s.class_values = value_names(class_h);
    s.class_other_subkeys = subkey_names(class_h).into_iter().filter(|n| !n.eq_ignore_ascii_case("shell")).collect();
    if let Some(shell_h) = open_key(&shell_key(class), KEY_READ) {
        s.shell_exists = true;
        s.shell_default = query_sz(shell_h, &HSTRING::new());
        s.shell_values = value_names(shell_h);
        let subs = subkey_names(shell_h);
        s.verb_exists = subs.iter().any(|n| n.eq_ignore_ascii_case(VERB));
        s.shell_other_subkeys = subs.into_iter().filter(|n| !n.eq_ignore_ascii_case(VERB)).collect();
        close(shell_h);
    }
    if s.verb_exists {
        if let Some(verb_h) = open_key(&verb_key(class), KEY_READ) {
            s.prev_default = query_sz(verb_h, &HSTRING::from(PREV_DEFAULT));
            s.prev_set = query_dword(verb_h, &HSTRING::from(PREV_SET));
            s.shell_created = query_dword(verb_h, &HSTRING::from(SHELL_CREATED));
            s.class_created = query_dword(verb_h, &HSTRING::from(CLASS_CREATED));
            close(verb_h);
        }
        if let Some(cmd_h) = open_key(&command_key(class), KEY_READ) {
            s.command = query_sz(cmd_h, &HSTRING::new());
            close(cmd_h);
        }
    }
    close(class_h);
    s
}

unsafe fn apply_ops(ops: &[Op]) -> bool {
    if ops.is_empty() {
        return true;
    }
    let mut ok = true;
    for op in ops {
        if !apply_op(op) {
            ok = false;
        }
    }
    if ok {
        notify_assoc();
    }
    ok
}

unsafe fn apply_op(op: &Op) -> bool {
    match op {
        Op::SetSz { key, name, data } => set_sz(key, name, data),
        Op::SetDword { key, name, data } => set_dword(key, name, *data),
        Op::DeleteValue { key, name } => delete_value(key, name),
        Op::DeleteTree { key } => gone(RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key))),
        Op::DeleteKey { key } => gone(RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(key))),
    }
}

unsafe fn notify_assoc() {
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
}

fn gone(err: WIN32_ERROR) -> bool {
    err.is_ok() || err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND
}

unsafe fn set_sz(sub: &str, name: &str, data: &str) -> bool {
    let Some(key) = create_key(sub) else { return false };
    let n = HSTRING::from(name);
    let bytes = utf16z(data);
    let err = RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&bytes));
    close(key);
    err.is_ok()
}

unsafe fn set_dword(sub: &str, name: &str, data: u32) -> bool {
    let Some(key) = create_key(sub) else { return false };
    let n = HSTRING::from(name);
    let bytes = data.to_le_bytes();
    let err = RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&bytes));
    close(key);
    err.is_ok()
}

unsafe fn delete_value(sub: &str, name: &str) -> bool {
    let Some(key) = open_key(sub, KEY_READ | KEY_WRITE) else { return true };
    let n = HSTRING::from(name);
    let err = RegDeleteValueW(key, PCWSTR(n.as_ptr()));
    close(key);
    gone(err)
}

unsafe fn create_key(sub: &str) -> Option<HKEY> {
    let mut out = HKEY::default();
    let name = HSTRING::from(sub);
    let err = RegCreateKeyExW(HKEY_CURRENT_USER, &name, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, None, &mut out, None);
    err.is_ok().then_some(out)
}

unsafe fn open_key(sub: &str, sam: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let mut out = HKEY::default();
    let name = HSTRING::from(sub);
    let err = RegOpenKeyExW(HKEY_CURRENT_USER, &name, None, sam, &mut out);
    err.is_ok().then_some(out)
}

unsafe fn close(key: HKEY) {
    if !key.is_invalid() {
        _ = RegCloseKey(key);
    }
}

unsafe fn query_sz(key: HKEY, name: &HSTRING) -> Option<String> {
    let name = PCWSTR(name.as_ptr());
    let mut typ = REG_SZ;
    let mut n = 0u32;
    let err = RegQueryValueExW(key, name, None, Some(&mut typ), None, Some(&mut n));
    if !(err.is_ok() || err == ERROR_MORE_DATA) || (typ != REG_SZ && typ != REG_EXPAND_SZ) {
        return None;
    }
    if n == 0 {
        return Some(String::new());
    }
    let mut buf = vec![0u8; n as usize + 2];
    let mut n2 = buf.len() as u32;
    let err = RegQueryValueExW(key, name, None, Some(&mut typ), Some(buf.as_mut_ptr()), Some(&mut n2));
    if !err.is_ok() {
        return None;
    }
    let words = (n2 as usize / 2).min(buf.len() / 2);
    let u16s: Vec<u16> = buf[..words * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
    Some(String::from_utf16_lossy(&u16s[..end]))
}

unsafe fn query_dword(key: HKEY, name: &HSTRING) -> Option<u32> {
    let mut typ = REG_DWORD;
    let mut data = [0u8; 4];
    let mut n = 4u32;
    let err = RegQueryValueExW(key, name, None, Some(&mut typ), Some(data.as_mut_ptr()), Some(&mut n));
    (err.is_ok() && typ == REG_DWORD && n >= 4).then(|| u32::from_le_bytes(data))
}

unsafe fn subkey_names(key: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..10_000u32 {
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        let err = RegEnumKeyExW(key, i, Some(PWSTR(buf.as_mut_ptr())), &mut n, None, None, None, None);
        if err == ERROR_NO_MORE_ITEMS {
            break;
        }
        if !err.is_ok() {
            // A name that did not fit must still count, so a parent we did not fully read is not deleted.
            out.push(String::from("?"));
            break;
        }
        if n > 0 {
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
    }
    out
}

unsafe fn value_names(key: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..10_000u32 {
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        let err = RegEnumValueW(key, i, Some(PWSTR(buf.as_mut_ptr())), &mut n, None, None, None, None);
        if err == ERROR_NO_MORE_ITEMS {
            break;
        }
        if !err.is_ok() {
            out.push(String::from("?"));
            break;
        }
        // The default value has an empty name. Named values are the ones restore must keep.
        if n > 0 {
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
    }
    out
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}
