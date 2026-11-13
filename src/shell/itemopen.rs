//! Open a favourite that is not a plain folder or file.
//!
//! A special folder is browsed in the active pane when the shell namespace parses.
//! Control Panel is the exception: this browser cannot list its applets, so Explorer
//! opens it and the pane stays where it was. Otherwise Explorer opens a name that
//! does not parse. A command and a URL go through ShellExecuteExW.
//! An RDP item writes a temporary .rdp file and, when a password is saved, plants
//! TERMSRV/<host> for mstsc, then removes that credential after the chosen delay.
//! The password itself stays in the Maguro credential (`Maguro:rdp/<host>` is not
//! the target; the target is `Maguro:rdp:<host>`).

use std::time::Duration;

use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST, CRED_PERSIST_SESSION, CRED_TYPE_DOMAIN_PASSWORD,
};

use crate::imports::*;
use crate::remote::{delete_secret, read_secret, write_secret};
use crate::session::{
    command_launch, control_panel_path, cred_hold_secs, explorer_launch, rdp_bytes, rdp_launch, rdp_text, rdp_host_used, store_target, termsrv_target, url_launch,
    FavNode, ItemKind, SecretPlan,
};

/// A command, URL, special folder, or remote desktop item. Folders and files stay on the old path.
pub(crate) fn launches(node: &FavNode) -> bool {
    node.kind().launches()
}

pub(crate) unsafe fn open_leaf(owner: HWND, node: &FavNode) -> bool {
    match node.kind() {
        ItemKind::Special => open_special(owner, node.path.trim()),
        ItemKind::Command => {
            let args = node.item.as_ref().map(|i| i.args.as_str()).unwrap_or("");
            let dir = node.item.as_ref().map(|i| i.workdir.as_str()).unwrap_or("");
            let admin = node.item.as_ref().is_some_and(|i| i.admin);
            let launch = command_launch(node.path.trim(), args, dir, admin);
            shell_exec(owner, &launch, admin)
        }
        ItemKind::Url => shell_exec(owner, &url_launch(node.path.trim()), false),
        ItemKind::Rdp => open_rdp(owner, node),
        ItemKind::Folder | ItemKind::File => false,
    }
}

unsafe fn open_special(owner: HWND, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    // navigate() would replace this pane with an empty list. Explorer owns the folder.
    if control_panel_path(path) {
        return launch_explorer(owner, path);
    }
    let i = crate::active_pane();
    if crate::navigate(i, path) {
        if !crate::net_failed(i) {
            crate::focus_view(i);
        }
        return true;
    }
    shell_exec(owner, &explorer_launch(path), false)
}

/// `explorer.exe` for a shell place the pane must not browse. The current folder stays.
pub(crate) unsafe fn launch_explorer(owner: HWND, path: &str) -> bool {
    shell_exec(owner, &explorer_launch(path), false)
}

unsafe fn open_rdp(owner: HWND, node: &FavNode) -> bool {
    let host = node.path.trim();
    if host.is_empty() {
        return false;
    }
    let item = node.item.as_ref();
    let user = item.map(|i| i.user.as_str()).unwrap_or("");
    let screen = item.map(|i| i.screen).unwrap_or_default();
    let hold = cred_hold_secs(item.map(|i| i.cred_secs).unwrap_or(10));
    let mut secret = if item.is_some_and(|i| i.has_password) { read_secret(&store_target(host)) } else { None };
    if secret.as_ref().is_some_and(|s| s.is_empty()) {
        secret = None;
    }
    let will_plant = secret.is_some() && !user.is_empty();
    let text = rdp_text(host, user, screen, !will_plant);
    let path = rdp_temp();
    if std::fs::write(&path, rdp_bytes(&text)).is_err() {
        wipe_opt(&mut secret);
        return false;
    }
    let planted = if let Some(secret) = secret.as_deref().filter(|_| will_plant) {
        plant_termsrv(host, user, secret)
    } else {
        Plant { wrote: false, previous: None }
    };
    wipe_opt(&mut secret);
    let launch = rdp_launch(&path.to_string_lossy(), host);
    let ok = shell_exec(owner, &launch, false);
    let target = termsrv_target(host);
    if !ok {
        let _ = std::fs::remove_file(&path);
        finish_termsrv(&target, planted);
        return false;
    }
    let file = path.clone();
    std::thread::spawn(move || {
        // mstsc reads the file and the TERMSRV credential as it starts. Two seconds covers that,
        // and a longer credential delay waits the rest.
        let file_wait = 2u64;
        std::thread::sleep(Duration::from_secs(file_wait));
        let _ = std::fs::remove_file(&file);
        let cred_wait = hold.max(file_wait);
        if cred_wait > file_wait {
            std::thread::sleep(Duration::from_secs(cred_wait - file_wait));
        }
        finish_termsrv(&target, planted);
    });
    true
}

fn finish_termsrv(target: &str, plant: Plant) {
    if !plant.wrote {
        return;
    }
    let _ = delete_domain(target);
    if let Some(prev) = plant.previous {
        let _ = write_domain_raw(target, &prev);
        prev.wipe();
    }
}

fn rdp_temp() -> std::path::PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("maguro-rdp-{}-{n}.rdp", std::process::id()))
}

/// Write or copy the Maguro credential. The favourites tree is updated only after this returns true.
pub(crate) fn stage_secret(plan: &mut SecretPlan) -> bool {
    if !plan.write_secret.is_empty() {
        let ok = write_secret(&store_target(&plan.write_host), &plan.write_secret);
        wipe(&mut plan.write_secret);
        return ok;
    }
    if !plan.copy_from.is_empty() {
        let Some(mut secret) = read_secret(&store_target(&plan.copy_from)) else { return false };
        let ok = write_secret(&store_target(&plan.write_host), &secret);
        wipe(&mut secret);
        return ok;
    }
    true
}

/// Drop Maguro credentials for hosts no remaining RDP item still claims.
pub(crate) fn forget_hosts(hosts: &[String]) {
    if hosts.is_empty() {
        return;
    }
    let tree = crate::app(|a| a.s.tree.clone());
    for host in hosts {
        if !rdp_host_used(&tree, host) {
            let _ = delete_secret(&store_target(host));
        }
    }
}

struct SavedCred {
    user: Vec<u16>,
    blob: Vec<u8>,
    persist: CRED_PERSIST,
}

impl SavedCred {
    fn wipe(mut self) {
        self.user.fill(0);
        self.blob.fill(0);
    }
}

struct Plant {
    wrote: bool,
    previous: Option<SavedCred>,
}

/// Remember the TERMSRV credential that was already there, then replace it for this launch.
fn plant_termsrv(host: &str, user: &str, secret: &str) -> Plant {
    let target = termsrv_target(host);
    let previous = read_domain(&target);
    let wrote = write_domain_password(&target, user, secret);
    Plant { wrote, previous }
}

fn write_domain_password(target: &str, user: &str, secret: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let mut name = wide_z(target);
    let mut user_z = wide_z(user);
    let mut blob = utf16_bytes(secret);
    let cred = CREDENTIALW {
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: PWSTR(name.as_mut_ptr()),
        UserName: PWSTR(user_z.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_SESSION,
        ..Default::default()
    };
    let ok = unsafe { CredWriteW(&cred, 0).is_ok() };
    blob.fill(0);
    name.fill(0);
    user_z.fill(0);
    ok
}

fn write_domain_raw(target: &str, saved: &SavedCred) -> bool {
    if target.is_empty() || saved.user.is_empty() {
        return false;
    }
    let mut name = wide_z(target);
    let mut user = saved.user.clone();
    let mut blob = saved.blob.clone();
    let cred = CREDENTIALW {
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: PWSTR(name.as_mut_ptr()),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: saved.persist,
        ..Default::default()
    };
    let ok = unsafe { CredWriteW(&cred, 0).is_ok() };
    blob.fill(0);
    user.fill(0);
    name.fill(0);
    ok
}

fn read_domain(target: &str) -> Option<SavedCred> {
    if target.is_empty() {
        return None;
    }
    let name = HSTRING::from(target);
    let mut cred = std::ptr::null_mut();
    let ok = unsafe { CredReadW(&name, CRED_TYPE_DOMAIN_PASSWORD, Some(0), &mut cred).is_ok() };
    if !ok || cred.is_null() {
        return None;
    }
    let saved = unsafe {
        let c = &*cred;
        let n = c.CredentialBlobSize as usize;
        let blob = if c.CredentialBlob.is_null() || n == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, n).to_vec()
        };
        let user = read_wide(c.UserName);
        SavedCred { user, blob, persist: c.Persist }
    };
    unsafe { CredFree(cred as *const _) };
    Some(saved)
}

fn delete_domain(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    unsafe { CredDeleteW(PCWSTR(wide_z(target).as_ptr()), CRED_TYPE_DOMAIN_PASSWORD, Some(0)).is_ok() }
}

fn read_wide(p: PWSTR) -> Vec<u16> {
    if p.0.is_null() {
        return vec![0];
    }
    let mut v = Vec::new();
    unsafe {
        let mut p = p.0;
        for _ in 0..512 {
            let u = *p;
            v.push(u);
            if u == 0 {
                break;
            }
            p = p.add(1);
        }
    }
    if v.last() != Some(&0) {
        v.push(0);
    }
    v
}

fn wide_z(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn utf16_bytes(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    bytes
}

fn wipe(s: &mut String) {
    let mut bytes = std::mem::take(s).into_bytes();
    bytes.fill(0);
}

fn wipe_opt(s: &mut Option<String>) {
    if let Some(text) = s.as_mut() {
        wipe(text);
    }
    *s = None;
}

unsafe fn shell_exec(owner: HWND, launch: &crate::session::Launch, admin: bool) -> bool {
    let file = HSTRING::from(launch.file.as_str());
    let verb = HSTRING::from(launch.verb);
    let params = HSTRING::from(launch.parameters.as_str());
    let dir = HSTRING::from(launch.directory.as_str());
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | if admin { SEE_MASK_NOASYNC } else { 0 },
        hwnd: owner,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if launch.parameters.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) },
        lpDirectory: if launch.directory.is_empty() { PCWSTR::null() } else { PCWSTR(dir.as_ptr()) },
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    match ShellExecuteExW(&mut sei) {
        Ok(()) => true,
        // Cancelling the UAC prompt is not a failure to show.
        Err(e) => admin && e.code() == ERROR_CANCELLED.to_hresult(),
    }
}
