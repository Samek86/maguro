//! Uninstall cleanup script.
//!
//! The script text is a pure function of the paths the caller passes. Production
//! passes the real roaming AppData folder, `config_dir()`, and this process's exe
//! (`GetModuleFileNameW`). Tests pass their own paths. Nothing here reads an
//! environment variable to retarget those.
//!
//! Normal installs: the script removes exactly `%APPDATA%\Maguro`, then the exe.
//! Portable mode (`portable.txt` beside the exe) keeps settings in the exe's folder,
//! which may also hold the user's other files. That folder is never removed as a
//! whole. Only the files and subfolders Maguro itself creates are removed, each by
//! its full path, and then the exe.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetTickCount64};
use windows::Win32::System::Threading::{
    CreateProcessW, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_OK, MB_YESNO, SW_HIDE};
use windows::core::{HSTRING, PCWSTR, PWSTR};

const PORTABLE_FILES: &[&str] = &[
    "maguro.json",
    "folder-views.json",
    "toolbar.json",
    "marks.json",
    "marks.json.bak",
    "rename-undo.json",
    "ctx-probe.json",
    "maguro-list.log",
    "maguro-list.log.1",
    "last-trace.txt",
    "last-trace.txt.tmp",
    "crash-pending.txt",
    "favorite-remove.log",
    "favorite-remove.log.1",
];

const PORTABLE_DIRS: &[&str] = &["lang", "maguro-sessions", "rename-presets", "Log", "crashes"];

const EXE_RETRIES: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refuse {
    BadPath,
    Root,
    NotMaguro,
    NotUnderAppData,
    Exe,
    Portable,
}

pub(crate) struct ScriptReq<'a> {
    pub portable: bool,
    pub config_dir: &'a Path,
    pub appdata: &'a Path,
    pub exe: &'a Path,
    pub pid: u32,
}

/// Cmd text, UTF-8, no BOM. Paths are embedded after `chcp 65001` so a folder
/// with spaces or non-ASCII characters survives. `%` in a path is doubled.
pub(crate) fn build_script(req: &ScriptReq<'_>) -> Result<String, Refuse> {
    if req.pid == 0 {
        return Err(Refuse::BadPath);
    }
    let exe = loose(req.exe)?;
    allow_exe(&exe)?;
    let sweep = if req.portable {
        portable_sweep(req.config_dir, &exe)?
    } else {
        let app = loose(req.appdata)?;
        let cfg = loose(req.config_dir)?;
        allow_config_dir(&cfg, &app)?;
        dir_sweep(&cfg, &app)?
    };
    Ok(crlf(&assemble(req.pid, &exe, &sweep)))
}

fn assemble(pid: u32, exe: &str, sweep: &str) -> String {
    let mut lines = vec![
        "@echo off".to_string(),
        "setlocal EnableExtensions DisableDelayedExpansion".into(),
        "chcp 65001 >nul".into(),
        assign("PID", &pid.to_string()).expect("pid"),
        assign("EXE", exe).expect("exe"),
        "if \"%PID%\"==\"\" goto :selfdel".into(),
        "if \"%EXE%\"==\"\" goto :selfdel".into(),
        "set /a N=0".into(),
        ":waitloop".into(),
        "tasklist /FI \"PID eq %PID%\" /NH 2>nul | find \"%PID%\" >nul".into(),
        "if errorlevel 1 goto :sweep".into(),
        "set /a N+=1".into(),
        "if %N% GEQ 30 goto :selfdel".into(),
        "ping -n 2 127.0.0.1 >nul".into(),
        "goto :waitloop".into(),
        ":sweep".into(),
    ];
    lines.push(sweep.trim_end().to_string());
    lines.push(":delexe".into());
    lines.push("set /a TRY=0".into());
    lines.push(":trydel".into());
    lines.push("if \"%EXE%\"==\"\" goto :selfdel".into());
    lines.push("for %%I in (\"%EXE%\") do set \"EXELEAF=%%~nxI\"".into());
    lines.push("if \"%EXELEAF%\"==\"\" goto :selfdel".into());
    lines.push("if /I \"%EXELEAF%\"==\"grids.ini\" goto :selfdel".into());
    lines.push("del /f /q \"%EXE%\"".into());
    lines.push("if not exist \"%EXE%\" goto :selfdel".into());
    lines.push("set /a TRY+=1".into());
    lines.push(format!("if %TRY% GEQ {EXE_RETRIES} goto :selfdel"));
    lines.push("ping -n 2 127.0.0.1 >nul".into());
    lines.push("goto :trydel".into());
    lines.push(":selfdel".into());
    lines.push("(goto) 2>nul & del \"%~f0\"".into());
    lines.join("\n")
}

fn dir_sweep(cfg: &str, app: &str) -> Result<String, Refuse> {
    let expect = format!("{app}\\Maguro");
    allow_config_dir(&expect, app)?;
    let lines = vec![
        assign("CFG", cfg)?,
        assign("APP", app)?,
        assign("EXPECT", &expect)?,
        "if \"%CFG%\"==\"\" goto :selfdel".into(),
        "if \"%APP%\"==\"\" goto :selfdel".into(),
        "if \"%EXPECT%\"==\"\" goto :selfdel".into(),
        "for %%I in (\"%CFG%\") do set \"LEAF=%%~nxI\"".into(),
        "if /I not \"%LEAF%\"==\"Maguro\" goto :selfdel".into(),
        "if /I \"%LEAF%\"==\"grids\" goto :selfdel".into(),
        "if /I \"%LEAF%\"==\"grids.ini\" goto :selfdel".into(),
        "for %%I in (\"%CFG%\") do set \"FULL=%%~fI\"".into(),
        "for %%I in (\"%EXPECT%\") do set \"WANT=%%~fI\"".into(),
        "if \"%FULL%\"==\"\" goto :selfdel".into(),
        "if \"%WANT%\"==\"\" goto :selfdel".into(),
        "if /I not \"%FULL%\"==\"%WANT%\" goto :selfdel".into(),
        "if /I \"%FULL%\"==\"%APP%\" goto :selfdel".into(),
        "rd /s /q \"%FULL%\"".into(),
    ];
    Ok(lines.join("\n"))
}

fn portable_sweep(config_dir: &Path, exe: &str) -> Result<String, Refuse> {
    let root = loose(config_dir)?;
    let exe_parent = parent_of(exe).ok_or(Refuse::Exe)?;
    if !same(root.as_str(), exe_parent) {
        return Err(Refuse::Portable);
    }
    if is_drive_or_unc_root(exe_parent) {
        return Err(Refuse::Root);
    }
    let mut lines = vec![
        assign("EXEPARENT_SRC", exe)?,
        "for %%I in (\"%EXEPARENT_SRC%\") do set \"EXEDIR=%%~dpI\"".into(),
        "if \"%EXEDIR%\"==\"\" goto :selfdel".into(),
    ];
    for name in PORTABLE_FILES {
        lines.extend(portable_one(&root, name, false)?);
    }
    for name in PORTABLE_DIRS {
        lines.extend(portable_one(&root, name, true)?);
    }
    Ok(lines.join("\n"))
}

fn portable_one(root: &str, name: &str, dir: bool) -> Result<Vec<String>, Refuse> {
    if name.is_empty() || name.contains(['\\', '/', '*', '?', '"']) || name.eq_ignore_ascii_case("grids") || name.eq_ignore_ascii_case("grids.ini") {
        return Err(Refuse::Portable);
    }
    let path = format!("{root}\\{name}");
    loose(Path::new(&path))?;
    if !direct_child(root, &path) {
        return Err(Refuse::Portable);
    }
    let verb = if dir { "rd /s /q \"%ONE%\"" } else { "del /f /q \"%ONE%\"" };
    Ok(vec![
        assign("ONE", &path)?,
        "if \"%ONE%\"==\"\" goto :selfdel".into(),
        "for %%I in (\"%ONE%\") do set \"LEAF=%%~nxI\"".into(),
        format!("if /I not \"%LEAF%\"==\"{name}\" goto :selfdel"),
        "if /I \"%LEAF%\"==\"grids\" goto :selfdel".into(),
        "if /I \"%LEAF%\"==\"grids.ini\" goto :selfdel".into(),
        "for %%I in (\"%ONE%\") do set \"ONEFULL=%%~fI\"".into(),
        "for %%I in (\"%ONE%\") do set \"ONEPARENT=%%~dpI\"".into(),
        "if \"%ONEFULL%\"==\"\" goto :selfdel".into(),
        "if \"%ONEPARENT%\"==\"\" goto :selfdel".into(),
        // %%~dp of the exe ends with a slash. A child has that same parent. The exe folder itself does not.
        "if /I not \"%ONEPARENT%\"==\"%EXEDIR%\" goto :selfdel".into(),
        verb.into(),
    ])
}

fn allow_config_dir(cfg: &str, app: &str) -> Result<(), Refuse> {
    if is_drive_or_unc_root(cfg) || is_drive_or_unc_root(app) {
        return Err(Refuse::Root);
    }
    let leaf = leaf_of(cfg).ok_or(Refuse::NotMaguro)?;
    if leaf.eq_ignore_ascii_case("grids") || leaf.eq_ignore_ascii_case("grids.ini") {
        return Err(Refuse::NotMaguro);
    }
    if !leaf.eq_ignore_ascii_case("Maguro") {
        return Err(Refuse::NotMaguro);
    }
    if !direct_child(app, cfg) {
        return Err(Refuse::NotUnderAppData);
    }
    if cfg.to_ascii_lowercase().ends_with("\\grids.ini") || cfg.to_ascii_lowercase().ends_with("\\grids") {
        return Err(Refuse::NotMaguro);
    }
    Ok(())
}

fn allow_exe(exe: &str) -> Result<(), Refuse> {
    if is_drive_or_unc_root(exe) {
        return Err(Refuse::Root);
    }
    let leaf = leaf_of(exe).ok_or(Refuse::Exe)?;
    if leaf.eq_ignore_ascii_case("grids.ini") || leaf.eq_ignore_ascii_case("grids") {
        return Err(Refuse::Exe);
    }
    if parent_of(exe).is_none() {
        return Err(Refuse::Exe);
    }
    Ok(())
}

fn assign(var: &str, value: &str) -> Result<String, Refuse> {
    if value.is_empty() || value.contains(['"', '\n', '\r', '\0', '*', '?', '<', '>', '|']) {
        return Err(Refuse::BadPath);
    }
    Ok(format!("set \"{var}={}\"", value.replace('%', "%%")))
}

fn loose(path: &Path) -> Result<String, Refuse> {
    let raw = path.to_string_lossy();
    if raw.is_empty() || raw.contains(['"', '\n', '\r', '\0', '*', '?', '<', '>', '|']) {
        return Err(Refuse::BadPath);
    }
    if raw.split(['\\', '/']).any(|p| p == "..") {
        return Err(Refuse::BadPath);
    }
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw.as_ref()).replace('/', "\\");
    let s = stripped.trim_end_matches('\\').to_string();
    if s.is_empty() || is_drive_or_unc_root(&s) {
        return Err(Refuse::Root);
    }
    if leaf_of(&s).is_none() {
        return Err(Refuse::Root);
    }
    Ok(s)
}

fn leaf_of(s: &str) -> Option<&str> {
    let leaf = s.rsplit('\\').next().filter(|l| !l.is_empty())?;
    Some(leaf)
}

fn parent_of(s: &str) -> Option<&str> {
    let leaf = leaf_of(s)?;
    let parent = s[..s.len() - leaf.len()].trim_end_matches('\\');
    if parent.is_empty() { None } else { Some(parent) }
}

fn direct_child(parent: &str, child: &str) -> bool {
    let parent = parent.trim_end_matches('\\');
    if parent.is_empty() || child.len() <= parent.len() + 1 {
        return false;
    }
    if !child[..parent.len()].eq_ignore_ascii_case(parent) {
        return false;
    }
    if child.as_bytes()[parent.len()] != b'\\' {
        return false;
    }
    let leaf = &child[parent.len() + 1..];
    !leaf.is_empty() && !leaf.contains('\\')
}

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\').eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

fn is_drive_or_unc_root(s: &str) -> bool {
    let t = s.trim_end_matches('\\');
    if t.is_empty() {
        return true;
    }
    if t.len() == 2 && t.as_bytes().get(1) == Some(&b':') {
        return true;
    }
    if let Some(rest) = t.strip_prefix(r"\\") {
        return !rest.is_empty() && !rest.contains('\\');
    }
    false
}

fn crlf(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 16);
    for line in body.split('\n') {
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

fn module_exe() -> Option<PathBuf> {
    let mut buf = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    if s.is_empty() { None } else { Some(PathBuf::from(s)) }
}

fn system_cmd() -> Option<PathBuf> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let dir = String::from_utf16_lossy(&buf[..n]);
    Some(PathBuf::from(dir).join("cmd.exe"))
}

pub(crate) fn script_file_path() -> PathBuf {
    let ticks = unsafe { GetTickCount64() };
    std::env::temp_dir().join(format!("maguro-uninstall-{}-{ticks}.cmd", std::process::id()))
}

pub(crate) fn write_script(path: &Path, body: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// `cmd.exe /d /c "<script>"` with `CREATE_NO_WINDOW`. No console window.
/// Paths live in the script, which is UTF-8 without a BOM and starts with `chcp 65001`.
pub(crate) fn spawn_hidden(script: &Path) -> Result<(), String> {
    let cmd = system_cmd().ok_or_else(|| "cmd.exe".to_string())?;
    let script_s = loose(script).map_err(|_| "script path".to_string())?;
    let cmdline = format!("cmd.exe /d /c \"\"{script_s}\"\"");
    let app = wide(&cmd.to_string_lossy());
    let dir = wide(&cmd.parent().unwrap_or(Path::new(".")).to_string_lossy());
    let mut cmd_buf = wide(&cmdline);
    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE.0 as u16;
    let mut pi = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(cmd_buf.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
            None,
            PCWSTR(dir.as_ptr()),
            &si,
            &mut pi,
        )
    };
    if let Err(e) = ok {
        return Err(e.to_string());
    }
    unsafe {
        if !pi.hProcess.is_invalid() {
            let _ = CloseHandle(pi.hProcess);
        }
        if !pi.hThread.is_invalid() {
            let _ = CloseHandle(pi.hThread);
        }
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// Settings → About → Uninstall. The owner is the settings window.
/// A refused path does not touch the registry and does not exit.
pub(crate) unsafe fn from_settings(owner: HWND) {
    let portable = crate::store::portable_mode();
    let config = crate::store::config_dir();
    let appdata = crate::store::roaming_appdata().unwrap_or_default();
    let exe = module_exe().unwrap_or_default();
    let req = ScriptReq {
        portable,
        config_dir: &config,
        appdata: &appdata,
        exe: &exe,
        pid: std::process::id(),
    };
    let body = match build_script(&req) {
        Ok(body) => body,
        Err(_) => {
            tell(owner, crate::tr(crate::Key::UninstallRefused));
            return;
        }
    };
    let folder = loose(&config).unwrap_or_default();
    let exe_s = loose(&exe).unwrap_or_default();
    let text = if portable {
        crate::tf(crate::Key::UninstallConfirmPortable, &[("folder", &folder), ("exe", &exe_s)])
    } else {
        crate::tf(crate::Key::UninstallConfirm, &[("folder", &folder), ("exe", &exe_s)])
    };
    let title = crate::tr(crate::Key::UninstallMaguro);
    let answer = MessageBoxW(
        Some(owner),
        &HSTRING::from(text),
        &HSTRING::from(title),
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    );
    if answer != IDYES {
        return;
    }
    let path = script_file_path();
    if write_script(&path, &body).is_err() {
        tell(owner, crate::tr(crate::Key::UninstallCouldNotStart));
        return;
    }
    // Existing HKCU turn-off. Already off is a no-op. No other hive is opened.
    let _ = crate::folderapp::set_registered(false);
    crate::store::suppress_saves();
    if let Err(_e) = spawn_hidden(&path) {
        let _ = std::fs::remove_file(&path);
        crate::store::allow_saves();
        tell(owner, crate::tr(crate::Key::UninstallCouldNotStart));
        return;
    }
    crate::close_all_windows();
}

unsafe fn tell(owner: HWND, text: &str) {
    let title = crate::tr(crate::Key::UninstallMaguro);
    let owner = if owner.0.is_null() { None } else { Some(owner) };
    let _ = MessageBoxW(owner, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONWARNING);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(portable: bool, config: &'a Path, app: &'a Path, exe: &'a Path) -> ScriptReq<'a> {
        ScriptReq { portable, config_dir: config, appdata: app, exe, pid: 4242 }
    }

    fn app() -> &'static Path {
        Path::new(r"C:\Users\テスト 사용자\AppData\Roaming")
    }

    #[test]
    fn refuses_non_maguro_grids_roots_and_empty() {
        let exe = Path::new(r"C:\Program Files\Maguro\maguro.exe");
        let base = app();
        assert_eq!(build_script(&req(false, &base.join("grids"), base, exe)), Err(Refuse::NotMaguro));
        assert_eq!(build_script(&req(false, &base.join("grids.ini"), base, exe)), Err(Refuse::NotMaguro));
        assert_eq!(build_script(&req(false, &base.join("MaguroX"), base, exe)), Err(Refuse::NotMaguro));
        assert_eq!(build_script(&req(false, &base.join("Maguro").join("nested"), base, exe)), Err(Refuse::NotMaguro));
        assert_eq!(build_script(&req(false, &PathBuf::from(r"C:\Maguro"), base, exe)), Err(Refuse::NotUnderAppData));
        assert_eq!(build_script(&req(false, &PathBuf::from(r"D:\Maguro"), &PathBuf::from(r"C:\"), exe)), Err(Refuse::Root));
        assert_eq!(build_script(&req(false, &PathBuf::from(""), base, exe)), Err(Refuse::BadPath));
        assert_eq!(build_script(&req(false, &PathBuf::from(r"\"), base, exe)), Err(Refuse::Root));
        assert_eq!(build_script(&req(false, &base.join("Maguro"), base, Path::new(r"C:\"))), Err(Refuse::Root));
        assert_eq!(build_script(&req(false, &base.join("Maguro"), base, Path::new(r"C:\grids.ini"))), Err(Refuse::Exe));
        assert!(build_script(&req(false, &base.join("maguro"), base, exe)).is_ok());
    }

    #[test]
    fn quotes_spaces_percents_and_non_ascii_and_only_rds_maguro() {
        let base = app();
        let cfg = base.join("Maguro");
        let exe = Path::new(r"C:\Program Files\Maguro 100%\maguro.exe");
        let body = build_script(&req(false, &cfg, base, exe)).unwrap();
        assert!(body.starts_with("@echo off\r\n"));
        assert!(body.contains("chcp 65001 >nul"));
        assert!(body.contains("tasklist /FI \"PID eq %PID%\" /NH 2>nul | find \"%PID%\" >nul"));
        assert!(body.contains("if %N% GEQ 30 goto :selfdel"));
        assert!(body.contains("ping -n 2 127.0.0.1 >nul"));
        assert!(body.contains("(goto) 2>nul & del \"%~f0\""));
        assert!(body.contains("テスト 사용자"));
        assert!(body.contains("set \"EXE=C:\\Program Files\\Maguro 100%%\\maguro.exe\""));
        assert!(body.contains("rd /s /q \"%FULL%\""));
        assert!(!body.contains("rd /s /q \"%APP%\""));
        assert!(!body.contains("rd /s /q \"\\\""));
        assert!(!body.contains('*'));
        assert!(body.to_ascii_lowercase().contains("grids.ini"));
        assert!(body.contains("if /I \"%LEAF%\"==\"grids\" goto :selfdel"));
        let exe_at = body.find("set \"EXE=").unwrap();
        let rd_at = body.find("rd /s /q \"%FULL%\"").unwrap();
        let guard = body.find("if /I not \"%FULL%\"==\"%WANT%\" goto :selfdel").unwrap();
        assert!(guard < rd_at);
        assert!(exe_at < rd_at);
    }

    #[test]
    fn portable_deletes_named_children_and_not_the_exe_folder() {
        let root = Path::new(r"D:\tools\Maguro App 日本語");
        let exe = root.join("maguro.exe");
        let body = build_script(&req(true, root, Path::new(r"C:\unused"), &exe)).unwrap();
        assert!(body.contains("set \"ONE=D:\\tools\\Maguro App 日本語\\maguro.json\""));
        assert!(body.contains("set \"ONE=D:\\tools\\Maguro App 日本語\\lang\""));
        assert!(body.contains("set \"ONE=D:\\tools\\Maguro App 日本語\\Log\""));
        assert!(body.contains("set \"ONE=D:\\tools\\Maguro App 日本語\\crashes\""));
        assert!(body.contains("del /f /q \"%ONE%\""));
        assert!(body.contains("rd /s /q \"%ONE%\""));
        assert!(!body.contains("rd /s /q \"%FULL%\""));
        assert!(!body.contains("rd /s /q \"D:\\tools\\Maguro App 日本語\""));
        assert!(!body.contains('*'));
        assert!(body.contains("if /I not \"%ONEPARENT%\"==\"%EXEDIR%\" goto :selfdel"));
        assert!(body.contains("if /I \"%LEAF%\"==\"grids.ini\" goto :selfdel"));
        assert_eq!(build_script(&req(true, Path::new(r"D:\tools\other"), Path::new(r"C:\unused"), &exe)), Err(Refuse::Portable));
        assert_eq!(build_script(&req(true, Path::new(r"C:\"), Path::new(r"C:\"), Path::new(r"C:\maguro.exe"))), Err(Refuse::Root));
    }

    #[cfg(windows)]
    #[test]
    fn script_removes_a_temp_maguro_dir_and_fake_exe_and_keeps_grids() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let root = std::env::temp_dir().join(format!("maguro test 日本語 한글 {stamp}"));
        let appdata = root.clone();
        let config = appdata.join("Maguro");
        let grids = appdata.join("grids").join("grids.ini");
        let exe = appdata.join("fake maguro.exe");
        let script = std::env::temp_dir().join(format!("maguro-uninstall-test-{}-{stamp}.cmd", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(grids.parent().unwrap()).unwrap();
        std::fs::write(config.join("maguro.json"), b"{}\n").unwrap();
        std::fs::write(&grids, b"keep\n").unwrap();
        std::fs::write(&exe, b"not-a-real-exe").unwrap();

        let real_cfg = crate::store::config_dir();
        let real_exe = std::env::current_exe().unwrap_or_default();
        assert_ne!(config, real_cfg, "test config must not be the real settings folder");
        assert!(!config.starts_with(&real_cfg));
        assert_ne!(exe, real_exe);

        let child = std::process::Command::new(std::env::var("SystemRoot").unwrap() + r"\System32\ping.exe")
            .args(["-n", "3", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        struct Guard {
            root: PathBuf,
            script: PathBuf,
            child: Option<std::process::Child>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                if let Some(child) = self.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let _ = std::fs::remove_dir_all(&self.root);
                let _ = std::fs::remove_file(&self.script);
            }
        }
        let mut guard = Guard { root: root.clone(), script: script.clone(), child: Some(child) };
        let _child_pid = guard.child.as_ref().unwrap().id();
        let body = build_script(&req(false, &config, &appdata, &exe)).expect("script");
        let body = body_replace_pid(body, _child_pid);
        write_script(&script, &body).unwrap();
        spawn_hidden(&script).expect("spawn");

        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(45) {
            if !config.exists() && !exe.exists() && !script.exists() && grids.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(!config.exists(), "Maguro dir should be gone");
        assert!(!exe.exists(), "fake exe should be gone");
        assert!(grids.is_file(), "grids.ini must survive");
        assert_eq!(std::fs::read_to_string(&grids).unwrap(), "keep\n");
        assert!(!script.exists(), "script should delete itself");
        // The real settings file was not the target. Drop removes only this temp root.
        guard.child.take().map(|mut c| { let _ = c.wait(); });
    }

    fn body_replace_pid(body: String, pid: u32) -> String {
        body.replacen("set \"PID=4242\"", &format!("set \"PID={pid}\""), 1)
    }
}
