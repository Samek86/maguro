use crate::imports::*;
use crate::app::*;
use crate::ui::*;
use crate::session::*;
use crate::shell::*;
use crate::i18n::*;

// ---------- status bar ----------

// Bumped by each pane's selection-change notification; part of the status key.
pub(crate) static SEL_REV: [AtomicUsize; MAXP] = [const { AtomicUsize::new(0) }; MAXP];

// The shell's selection count can keep deleted rows until the next click. The status line
// shows `count` instead, until the view reports that same count or a different count arrives.
// A notification often bumps the revision while `shell` is still the stale number. That bump
// is not a new selection.
#[derive(Clone, Copy)]
struct Hold {
    count: i32,
    shell: i32,
    rev: usize,
}

fn holds() -> &'static Mutex<[Option<Hold>; MAXP]> {
    static HOLDS: Mutex<[Option<Hold>; MAXP]> = Mutex::new([None; MAXP]);
    &HOLDS
}

/// `(shown count, trust the shell selection for the size, drop the hold)`.
/// `held` is `(corrected count, shell count when the hold was taken, revision)`.
pub(crate) fn resolve_hold(shell: i32, held: Option<(i32, i32, usize)>, rev: usize) -> (i32, bool, bool) {
    let Some((count, was, hold_rev)) = held else { return (shell, true, false) };
    if shell == count {
        return (shell, true, true);
    }
    // The count moved. A click, or the view catching up to a different number.
    if rev != hold_rev && shell != was {
        return (shell, true, true);
    }
    (count, false, false)
}

fn selection_count(pane: usize, shell: i32) -> (i32, bool) {
    if pane >= MAXP {
        return (shell, true);
    }
    let rev = SEL_REV[pane].load(Ordering::Relaxed);
    let mut g = holds().lock().unwrap_or_else(|e| e.into_inner());
    let (count, trust, clear) = resolve_hold(shell, g[pane].map(|h| (h.count, h.shell, h.rev)), rev);
    if clear {
        g[pane] = None;
    }
    (count, trust)
}

/// Remember a selection count the shell has not caught up to, and force the next status paint.
/// `shell` is the count `IFolderView` reported when `count` was chosen.
pub(crate) fn hold_selection_count(pane: usize, count: i32, shell: i32) {
    if pane >= MAXP {
        return;
    }
    let mut g = holds().lock().unwrap_or_else(|e| e.into_inner());
    if count == shell {
        g[pane] = None;
    } else {
        let rev = SEL_REV[pane].fetch_add(1, Ordering::Relaxed) + 1;
        g[pane] = Some(Hold { count, shell, rev });
    }
    drop(g);
    app(|a| a.status_key.clear());
}

/// The row count changed. Repaint the status line from whatever the view reports now.
pub(crate) fn bump_status(pane: usize) {
    if pane >= MAXP {
        return;
    }
    SEL_REV[pane].fetch_add(1, Ordering::Relaxed);
    app(|a| a.status_key.clear());
}

// The selection split into summed file bytes and folders left to the size worker.
// partial: something wasn't counted (over 10k items, no file system path, unreadable, a drive root).
#[derive(Debug, PartialEq)]
pub(crate) struct Sel {
    pub(crate) file_bytes: u64,
    pub(crate) dirs: Vec<String>,
    pub(crate) partial: bool,
}

// n: the selection's count (None = it couldn't be read), path(k): item k's file system path.
pub(crate) fn collect_sel(n: Option<u32>, path: impl Fn(u32) -> Option<String>) -> Sel {
    let Some(n) = n else { return Sel { file_bytes: 0, dirs: Vec::new(), partial: true } };
    // ponytail: sums at most 10k selected items; bigger selections are marked partial
    let mut s = Sel { file_bytes: 0, dirs: Vec::new(), partial: n > 10_000 };
    for k in 0..n.min(10_000) {
        let Some(p) = path(k) else {
            s.partial = true;
            continue;
        };
        match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() && Path::new(&p).parent().is_some() => s.dirs.push(p),
            Ok(m) if m.is_file() => s.file_bytes += m.len(),
            // a drive or share root (its size is the free-space part), or unreadable
            _ => s.partial = true,
        }
    }
    s
}

pub(crate) unsafe fn selected_size(fv: &IFolderView2) -> Sel {
    let arr = fv.GetSelection(false).ok();
    let n = arr.as_ref().and_then(|a| a.GetCount().ok());
    collect_sel(n, |k| Some(pwstr(arr.as_ref()?.GetItemAt(k).ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?)))
}

// The " (…)" after "n selected". dirs: the folder total so far (bytes, done, partial), None = not started.
pub(crate) fn size_text(file_bytes: u64, has_dirs: bool, dirs: Option<(u64, bool, bool)>, collect_partial: bool) -> String {
    let (b, done, partial) = match dirs {
        _ if !has_dirs => (0, true, false),
        Some(d) => d,
        None => (0, false, false),
    };
    let (bytes, partial) = (file_bytes + b, partial || collect_partial);
    if !done {
        return match bytes {
            0 => tr(crate::Key::Calculating2).into(),
            b => format!(" ({}…)", human(b)),
        };
    }
    match (bytes, partial) {
        (0, false) if !has_dirs => String::new(),
        (0, true) if !has_dirs => tr(crate::Key::SizeUnknown).into(),
        (b, true) => format!(" ({}, {})", human(b), tr(crate::Key::Partial)),
        (b, false) => format!(" ({})", human(b)),
    }
}

// Folder sizes are summed by one worker thread. gen changes on every new target or cancel;
// the worker drops results of an older gen and stops walking once DIR_GEN moves on.
#[derive(Default)]
pub(crate) struct SizeJob {
    pub(crate) gen: usize,
    pub(crate) target: Vec<String>,
    pub(crate) bytes: u64,
    pub(crate) done: bool,
    pub(crate) partial: bool,
}

impl SizeJob {
    // True = a new walk is due. busy (a copy runs) or no folders cancel; the same target keeps
    // running, or keeps its result.
    pub(crate) fn want(&mut self, dirs: &[String], busy: bool) -> bool {
        if busy || dirs.is_empty() {
            (self.gen, self.done) = (self.gen + 1, false);
            self.target.clear();
            return false;
        }
        if self.target == dirs {
            return false;
        }
        *self = SizeJob { gen: self.gen + 1, target: dirs.to_vec(), ..Default::default() };
        true
    }

    pub(crate) fn publish(&mut self, gen: usize, bytes: u64, done: bool, partial: bool) -> bool {
        if self.gen != gen {
            return false;
        }
        (self.bytes, self.done, self.partial) = (bytes, done, partial);
        true
    }
}

pub(crate) static DIR_SIZE: Mutex<SizeJob> = Mutex::new(SizeJob { gen: 0, target: Vec::new(), bytes: 0, done: false, partial: false });
pub(crate) static DIR_WAKE: Condvar = Condvar::new();
pub(crate) static DIR_GEN: AtomicUsize = AtomicUsize::new(0); // copy of DIR_SIZE.gen, read without the lock

pub(crate) fn size_job() -> std::sync::MutexGuard<'static, SizeJob> {
    DIR_SIZE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn want_size(dirs: &[String], busy: bool) {
    let mut j = size_job();
    if j.want(dirs, busy) {
        static WORKER: Once = Once::new();
        WORKER.call_once(|| _ = std::thread::Builder::new().spawn(size_worker));
        DIR_WAKE.notify_one();
    }
    DIR_GEN.store(j.gen, Ordering::Relaxed);
}

// Lives until the process ends. The lock is never held across file system calls.
pub(crate) fn size_worker() {
    let mut seen = 0;
    loop {
        let (gen, target) = {
            let mut j = size_job();
            while j.target.is_empty() || j.gen == seen {
                j = DIR_WAKE.wait(j).unwrap_or_else(|e| e.into_inner());
            }
            (j.gen, j.target.clone())
        };
        seen = gen;
        let stop = || DIR_GEN.load(Ordering::Relaxed) != gen;
        // a copy only pauses the walk; the status line cancels it when it sees the copy
        let paused = || COPYING.load(Ordering::Relaxed) > 0;
        let (mut total, mut partial) = (0, false);
        let finished = target.iter().all(|root| {
            let (base, was) = (total, partial);
            let mut progress = |b, p| _ = size_job().publish(gen, base + b, false, was || p);
            let walked = crate::crash::guard(std::panic::AssertUnwindSafe(|| dir_bytes(Path::new(root), &stop, &paused, &mut progress)));
            match walked {
                Ok(Some((b, p))) => {
                    (total, partial) = (total + b, partial || p);
                    true
                }
                _ => false,
            }
        });
        if finished {
            size_job().publish(gen, total, true, partial);
        }
    }
}

// Pending directories kept on the walk stack. A deep chain stays small (one path at a time);
// a very wide folder stops enqueueing past this and is marked partial instead of growing without bound.
pub(crate) const DIR_STACK_CAP: usize = 8_192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirTotals {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    pub partial: bool,
}

// Bytes under `root`, not following links or junctions. Unreadable parts set the flag and are skipped.
// None = `stop` turned true. Checked before each directory read and each entry, also while `paused`;
// a single read_dir/metadata call that hangs (a dead share) still has to return first.
pub(crate) fn dir_bytes(root: &Path, stop: &dyn Fn() -> bool, paused: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, bool)) -> Option<(u64, bool)> {
    dir_totals(root, stop, paused, progress, DIR_STACK_CAP).map(|t| (t.bytes, t.partial))
}

/// Bytes, file count and folder count under `root`. `stack_cap` bounds pending directories.
/// The root counts as a folder. Reparse points (junctions, symlinks) are not entered and not counted.
/// A directory that cannot be listed is counted and marked partial.
pub(crate) fn dir_totals(root: &Path, stop: &dyn Fn() -> bool, paused: &dyn Fn() -> bool, progress: &mut dyn FnMut(u64, bool), stack_cap: usize) -> Option<DirTotals> {
    let go = || {
        while paused() && !stop() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        !stop()
    };
    match std::fs::symlink_metadata(root) {
        Ok(m) if m.is_dir() => {}
        Ok(m) => {
            let files = u64::from(m.is_file());
            return Some(DirTotals { bytes: if m.is_file() { m.len() } else { 0 }, files, folders: 0, partial: false });
        }
        Err(_) => return Some(DirTotals { bytes: 0, files: 0, folders: 0, partial: true }),
    }
    let (mut sum, mut files, mut folders, mut partial, mut n, mut last) = (0u64, 0u64, 1u64, false, 0usize, Instant::now());
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if !go() {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            partial = true;
            continue;
        };
        for e in rd {
            if !go() {
                return None;
            }
            let Ok(e) = e else {
                partial = true;
                continue;
            };
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    folders += 1;
                    if stack.len() >= stack_cap {
                        partial = true;
                    } else {
                        stack.push(e.path());
                    }
                }
                Ok(t) if t.is_file() => match e.metadata() {
                    Ok(m) => {
                        files += 1;
                        sum += m.len();
                    }
                    Err(_) => {
                        files += 1;
                        partial = true;
                    }
                },
                Ok(_) => {} // a link or junction: not followed, not counted
                Err(_) => partial = true,
            }
            n += 1;
            if n % 2000 == 0 || last.elapsed().as_millis() >= 200 {
                progress(sum, partial);
                last = Instant::now();
            }
        }
    }
    Some(DirTotals { bytes: sum, files, folders, partial })
}

// F5: the shell reloads the active pane, and selected folders are summed again.
// Unwatched folders (WSL, network, a watch that failed) drop their cache here.
pub(crate) fn resum_sizes() {
    want_size(&[], true);
    let paths = app(|a| a.s.paths.clone());
    super::refresh_paths(&paths);
    app(|a| a.status_key.clear());
}

// Polled by a 300ms timer; the expensive parts only run when the active pane's state changed.
// Other ticks only merge the folder size worker's progress into the stored text.
pub(crate) unsafe fn update_status() {
    mark_drive();
    let (i, path, main) = app(|a| (a.s.active, a.s.paths[a.s.active].clone(), a.main));
    let Some(fv) = view_of::<IFolderView2>(i) else { return };
    let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(0);
    let shell_sel = fv.ItemCount(SVGIO_SELECTION).unwrap_or(0);
    // A delete can leave the shell count on rows that are already gone. `trust` is false
    // while we show the corrected count and skip the size walk of those rows.
    let (sel, trust) = selection_count(i, shell_sel);
    let spec = filter_of(i);
    // while a background copy runs, the disk is left alone (size and free space come back after)
    let busy = COPYING.load(Ordering::Relaxed) > 0;
    let zoom = app(|a| a.zoom);
    let rev = SEL_REV[i].load(Ordering::Relaxed);
    let (show, net) = app(|a| (a.s.folder_sizes, a.s.folder_sizes_network));
    super::set_opts(show, net);
    let key = format!("{i}|{path}|{n}|{sel}|{}|{spec}|{busy}|{zoom:?}|{rev}|{show}|{net}|{trust}", fv.GetFocusedItem().unwrap_or(-1));
    if app(|a| a.status_key != key) {
        status_base(i, &path, main, &fv, n, sel, &spec, busy, zoom, trust);
        app(|a| a.status_key = key);
    }
    // Selected folders only. Walking visible rows (GetVisibleItem) touched the list every tick
    // and fought the scroll. A content change drops the cache; this asks for the selection again
    // and the text below is the only thing that updates.
    let folder = if show {
        app(|a| (!a.status_folders.is_empty()).then(|| (a.status_folders.clone(), a.status_files)))
    } else {
        None
    };
    if let Some((paths, _)) = &folder {
        let tagged: Vec<(String, super::Place)> = paths.iter().map(|p| (p.clone(), super::classify_ui(p))).collect();
        super::request_paths(i, &tagged, super::RANK_SELECTED);
    }
    let line = folder.as_ref().and_then(|(paths, (bytes, count, partial))| super::selection_text(paths, *bytes, *count, *partial));
    let job = {
        let j = size_job();
        (!j.target.is_empty()).then_some((j.bytes, j.done, j.partial))
    };
    let Some(rc) = app(|a| {
        let (head, size, tail) = a.status_base.clone();
        let size = if line.is_some() {
            line.as_ref().map(|s| format!("     ·     {s}")).unwrap_or_default()
        } else {
            size.map(|(f, d, p)| size_text(f, d, job, p)).unwrap_or_default()
        };
        if let Some(saved) = SAVED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            a.note = Some((saved, Instant::now() + std::time::Duration::from_secs(5)));
            a.lost = 0;
        }
        // a confirmation leads until its expiry: the line's end gets cut when it's too long
        a.note = a.note.take().filter(|(_, until)| Instant::now() < *until);
        let note = a.note.as_ref().map(|(s, _)| format!("{s}     ·     ")).unwrap_or_default();
        let drag = if a.drag_hint.is_empty() { String::new() } else { format!("{}     ·     ", a.drag_hint) };
        let status = format!("{drag}{note}{head}{size}{tail}");
        (a.status != status).then(|| {
            a.status = status;
            status_rect(&geo(a))
        })
    }) else {
        return;
    };
    _ = InvalidateRect(Some(main), Some(&rc), true);
}

// The status text around the selection size, the window title, and a new folder size job.
pub(crate) unsafe fn status_base(i: usize, path: &str, main: HWND, fv: &IFolderView2, n: i32, sel: i32, spec: &str, busy: bool, zoom: Option<usize>, trust: bool) {
    let title = window_title(&app(|a| a.panes[i].as_ref().map(|p| p.disp.clone()).unwrap_or_default()));
    if text_of(main) != title {
        _ = SetWindowTextW(main, &HSTRING::from(title));
    }
    let mut parts = vec![tf(Key::ItemsN, &[("n", &n.to_string())])];
    let mut size = None;
    let mut dirs = Vec::new();
    let mut folders = Vec::new();
    let mut files = (0u64, 0u64, false);
    let (show, net) = app(|a| (a.s.folder_sizes, a.s.folder_sizes_network));
    if sel > 0 {
        // The corrected count is not the shell's selection yet, so summing those rows
        // reports "size unknown" for files that are already gone.
        if trust && !busy {
            if show {
                let pick = super::pick_selection(fv, net);
                if pick.folders.is_empty() {
                    size = Some((pick.file_bytes, false, pick.extra_partial));
                } else {
                    folders = pick.folders;
                    files = (pick.file_bytes, pick.file_count, pick.extra_partial);
                    super::request_paths(i, &pick.request, super::RANK_SELECTED);
                }
            } else {
                let s = selected_size(fv);
                size = Some((s.file_bytes, !s.dirs.is_empty(), s.partial));
                dirs = s.dirs;
            }
        }
        parts.push(tf(Key::SelectedCount, &[("sel", &sel.to_string())]));
    }
    if show {
        if !size_job().target.is_empty() {
            want_size(&[], false);
        }
    } else {
        want_size(&dirs, busy);
    }
    let head = parts.join("     ·     ");
    let mut parts = vec![String::new()];
    if !spec.is_empty() {
        parts.push(format!("{} {spec}", tr(crate::Key::Filter)));
    }
    let drive = path.get(1..2) == Some(":");
    // A UNC or mapped drive is not asked for free space here. GetDiskFreeSpaceExW waits out a dead
    // share, including a pane restored from the session before its off-thread navigate finishes.
    let share = unsafe { super::share_path(path) };
    if busy {
        parts.push(tr(crate::Key::CopyingMoving).into());
    } else if !share && (drive || path.starts_with(r"\\")) {
        let (mut free, mut total) = (0, 0);
        if GetDiskFreeSpaceExW(&HSTRING::from(path),Some(&mut free), Some(&mut total), None).is_ok() {
            let label = if drive { path.get(..2).unwrap_or("") } else { tr(crate::Key::Network) };
            parts.push(format!("{label} {} {} / {}", tr(crate::Key::Free), human(free), human(total)));
        }
    }
    if let Some(z) = zoom {
        let z = z + 1;
        parts.push(tf(Key::PaneMaximized, &[("z", &z.to_string())]));
    }
    app(|a| {
        a.status_folders = folders;
        a.status_files = files;
        a.status_base = (head, size, parts.join("     ·     "));
    });
}

#[cfg(test)]
mod tests {
    use super::resolve_hold;

    #[test]
    fn a_hold_replaces_a_stale_shell_count_until_the_view_agrees() {
        assert_eq!(resolve_hold(10, None, 3), (10, true, false));
        // Deleted rows are still counted by the shell. Show 0 and do not sum those rows.
        assert_eq!(resolve_hold(150, Some((0, 150, 4)), 4), (0, false, false));
        // The view caught up: trust it again and drop the hold.
        assert_eq!(resolve_hold(0, Some((0, 150, 4)), 4), (0, true, true));
        // A notification bumps the revision but the count is still the stale 150.
        assert_eq!(resolve_hold(150, Some((0, 150, 4)), 9), (0, false, false));
        // The count changed. The new selection wins.
        assert_eq!(resolve_hold(2, Some((0, 150, 4)), 5), (2, true, true));
    }
}


