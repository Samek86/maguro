//! UI Automation reads on one MTA thread. The UI thread never calls UIA.
//! There is no timer and no structure-change subscription: those keep a core awake
//! while the folder is idle. A query runs only when the pane asks for one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR, VT_I4};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Children, TreeScope_Descendants, UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_NamePropertyId,
};
use windows::core::BSTR;

use crate::imports::*;

use super::geom::{self, IRect};

/// Incremented only inside `FindAllBuildCache`. A folder with no marks never reaches it.
pub(crate) static QUERIES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub(crate) struct Hit {
    pub name: String,
    pub icon: IRect,
    pub item: IRect,
    pub image: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Snap {
    pub main: isize,
    pub pane: usize,
    pub epoch: u64,
    pub viewport: IRect,
    pub hits: Vec<Hit>,
    pub mode: i32,
    pub icon: i32,
    pub dpi: u32,
    pub nav: bool,
    pub note: String,
}

#[derive(Clone)]
pub(crate) struct Job {
    pub main: isize,
    pub pane: usize,
    pub view: isize,
    pub mode: i32,
    pub icon: i32,
    pub dpi: u32,
    pub epoch: u64,
    pub names: Vec<String>,
    pub nav: bool,
}

struct Bridge {
    queue: Vec<Job>,
    ready: Vec<Snap>,
    shutdown: bool,
}

struct Shared {
    bridge: Mutex<Bridge>,
    cv: Condvar,
    started: AtomicBool,
}

static SHARED: std::sync::OnceLock<Arc<Shared>> = std::sync::OnceLock::new();

fn shared() -> Arc<Shared> {
    SHARED
        .get_or_init(|| {
            Arc::new(Shared {
                bridge: Mutex::new(Bridge { queue: Vec::new(), ready: Vec::new(), shutdown: false }),
                cv: Condvar::new(),
                started: AtomicBool::new(false),
            })
        })
        .clone()
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Bridge> {
    shared.bridge.lock().unwrap_or_else(|e| e.into_inner())
}

/// Replace any queued read for this pane. One worker runs one query at a time.
pub(crate) fn enqueue(job: Job) {
    let shared = shared();
    ensure_worker(&shared);
    {
        let mut g = lock(&shared);
        if let Some(existing) = g.queue.iter_mut().find(|j| j.main == job.main && j.pane == job.pane) {
            *existing = job;
        } else {
            g.queue.push(job);
        }
    }
    shared.cv.notify_one();
}

pub(crate) fn take_ready(main: isize) -> Vec<Snap> {
    let shared = shared();
    let mut g = lock(&shared);
    let mut mine = Vec::new();
    g.ready.retain(|snap| {
        if snap.main == main {
            mine.push(snap.clone());
            false
        } else {
            true
        }
    });
    mine
}

pub(crate) fn shutdown() {
    let Some(shared) = SHARED.get() else { return };
    {
        let mut g = lock(shared);
        g.shutdown = true;
        g.queue.clear();
    }
    shared.cv.notify_one();
}

fn ensure_worker(shared: &Arc<Shared>) {
    if shared.started.swap(true, Ordering::AcqRel) {
        return;
    }
    let shared = shared.clone();
    std::thread::Builder::new().name("maguro-star".into()).spawn(move || worker(shared)).ok();
}

fn worker(shared: Arc<Shared>) {
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return;
        }
    }
    let auto: IUIAutomation = match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
        Ok(auto) => auto,
        Err(_) => {
            unsafe { CoUninitialize() };
            return;
        }
    };
    loop {
        let job = {
            let mut g = lock(&shared);
            loop {
                if g.shutdown {
                    drop(g);
                    drop(auto);
                    unsafe { CoUninitialize() };
                    return;
                }
                if !g.queue.is_empty() {
                    break g.queue.remove(0);
                }
                g = shared.cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        let snap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_job(&auto, &job))).ok();
        if let Some(snap) = snap {
            let mut g = lock(&shared);
            g.ready.retain(|old| !(old.main == snap.main && old.pane == snap.pane));
            let main = snap.main;
            let pane = snap.pane;
            g.ready.push(snap);
            drop(g);
            post_ready(main, pane);
        }
    }
}

fn post_ready(main: isize, pane: usize) {
    if main == 0 {
        return;
    }
    let hwnd = HWND(main as *mut core::ffi::c_void);
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = PostMessageW(Some(hwnd), super::WM_STAR, WPARAM(pane), LPARAM(0));
        }
    }
}

fn read_job(auto: &IUIAutomation, job: &Job) -> Snap {
    let mut empty = Snap {
        main: job.main,
        pane: job.pane,
        epoch: job.epoch,
        viewport: IRect::EMPTY,
        hits: Vec::new(),
        mode: job.mode,
        icon: job.icon,
        dpi: job.dpi,
        nav: job.nav,
        note: String::new(),
    };
    if job.view == 0 || job.names.is_empty() {
        empty.note = if job.view == 0 { "no-view".into() } else { "no-marks".into() };
        return empty;
    }
    let hwnd = HWND(job.view as *mut core::ffi::c_void);
    let (element, class) = match find_items_view(auto, hwnd) {
        Ok(found) => found,
        Err(err) => {
            empty.note = format!("no-items-view:{err}");
            return empty;
        }
    };
    let mut snap = query_items(auto, &element, job.mode, job.icon, job.dpi, &job.names);
    snap.main = job.main;
    snap.pane = job.pane;
    snap.epoch = job.epoch;
    snap.mode = job.mode;
    snap.icon = job.icon;
    snap.dpi = job.dpi;
    snap.nav = job.nav;
    if snap.note.is_empty() {
        snap.note = class;
    }
    snap
}

fn query_items(auto: &IUIAutomation, element: &IUIAutomationElement, mode: i32, icon: i32, dpi: u32, marks: &[String]) -> Snap {
    let viewport = refine_viewport(auto, element);
    let cache = match unsafe { auto.CreateCacheRequest() } {
        Ok(cache) => cache,
        Err(err) => return Snap { viewport, hits: Vec::new(), main: 0, pane: 0, epoch: 0, mode, icon, dpi, nav: false, note: format!("err:{err}") },
    };
    unsafe {
        let _ = cache.AddProperty(UIA_NamePropertyId);
        let _ = cache.AddProperty(UIA_BoundingRectanglePropertyId);
        let _ = cache.AddProperty(UIA_ClassNamePropertyId);
    }
    let Ok(cond) = item_condition(auto) else {
        return Snap { viewport, hits: Vec::new(), main: 0, pane: 0, epoch: 0, mode, icon, dpi, nav: false, note: "cond".into() };
    };
    QUERIES.fetch_add(1, Ordering::Relaxed);
    let found = unsafe { element.FindAllBuildCache(TreeScope_Children, &cond, &cache) };
    let mut array = match found {
        Ok(array) => array,
        Err(err) => return Snap { viewport, hits: Vec::new(), main: 0, pane: 0, epoch: 0, mode, icon, dpi, nav: false, note: format!("err:{err}") },
    };
    let mut count = unsafe { array.Length() }.unwrap_or(0);
    if count == 0 {
        if let Ok(deeper) = unsafe { element.FindAllBuildCache(TreeScope_Descendants, &cond, &cache) } {
            QUERIES.fetch_add(1, Ordering::Relaxed);
            count = unsafe { deeper.Length() }.unwrap_or(0);
            array = deeper;
        }
    }
    let mut hits = Vec::new();
    for index in 0..count {
        let Ok(item) = (unsafe { array.GetElement(index) }) else { continue };
        let name = unsafe { item.CachedName() }.map(|v| v.to_string()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let cached = unsafe { item.CachedBoundingRectangle() }.map(rect_of).unwrap_or(IRect::EMPTY);
        let mut item_rect = cached;
        let cached_off = item_rect.is_degenerate() || item_rect.x <= -20_000 || item_rect.y <= -20_000 || (!viewport.is_degenerate() && !item_rect.intersects(viewport));
        if cached_off {
            if let Ok(live) = unsafe { item.CurrentBoundingRectangle() } {
                let live_rect = rect_of(live);
                if !live_rect.is_degenerate() {
                    item_rect = live_rect;
                }
            }
        }
        let wanted = geom::could_match(&name, marks);
        let (icon, image) = if wanted {
            icon_rect(auto, &item, item_rect, mode, icon, dpi)
        } else {
            (IRect::EMPTY, false)
        };
        hits.push(Hit { name, icon, item: item_rect, image });
    }
    drop_stale_estimates(&mut hits);
    Snap { viewport, hits, main: 0, pane: 0, epoch: 0, mode, icon, dpi, nav: false, note: format!("n={count}") }
}

fn drop_stale_estimates(hits: &mut Vec<Hit>) {
    let any_image = hits.iter().any(|hit| hit.image);
    if !any_image {
        return;
    }
    for hit in hits.iter_mut() {
        if !hit.image && !hit.icon.is_degenerate() {
            hit.icon = IRect::EMPTY;
        }
    }
}

fn icon_rect(auto: &IUIAutomation, item: &IUIAutomationElement, item_rect: IRect, view_mode: i32, icon_size: i32, dpi: u32) -> (IRect, bool) {
    let mut best: Option<IRect> = None;
    let mut best_area = 0i32;
    let Ok(true_cond) = (unsafe { auto.CreateTrueCondition() }) else {
        return (geom::estimate_icon(item_rect, view_mode, icon_size, dpi), false);
    };
    let Ok(kids) = (unsafe { item.FindAll(TreeScope_Children, &true_cond) }) else {
        return (geom::estimate_icon(item_rect, view_mode, icon_size, dpi), false);
    };
    consider_images(&kids, &mut best, &mut best_area);
    if best.is_none() {
        if let Ok(deep) = unsafe { item.FindAll(TreeScope_Descendants, &true_cond) } {
            consider_images(&deep, &mut best, &mut best_area);
        }
    }
    match best {
        Some(rect) => (rect, true),
        None => (geom::estimate_icon(item_rect, view_mode, icon_size, dpi), false),
    }
}

fn consider_images(kids: &windows::Win32::UI::Accessibility::IUIAutomationElementArray, best: &mut Option<IRect>, best_area: &mut i32) {
    let n = unsafe { kids.Length() }.unwrap_or(0);
    for i in 0..n {
        let Ok(kid) = (unsafe { kids.GetElement(i) }) else { continue };
        let class = unsafe { kid.CurrentClassName() }.map(|v| v.to_string()).unwrap_or_default();
        if !is_image_class(&class) {
            continue;
        }
        let rect = unsafe { kid.CurrentBoundingRectangle() }.map(rect_of).unwrap_or(IRect::EMPTY);
        if rect.w < 8 || rect.h < 8 {
            continue;
        }
        let area = rect.w.saturating_mul(rect.h);
        if area > *best_area {
            *best_area = area;
            *best = Some(rect);
        }
    }
}

fn is_image_class(class_name: &str) -> bool {
    let lower = class_name.to_ascii_lowercase();
    lower.contains("image") || lower.contains("thumb") || lower == "icon"
}

fn item_condition(auto: &IUIAutomation) -> windows::core::Result<windows::Win32::UI::Accessibility::IUIAutomationCondition> {
    let mut class_var = variant_bstr("UIItem");
    let class_cond = unsafe { auto.CreatePropertyCondition(UIA_ClassNamePropertyId, &class_var)? };
    clear_variant(&mut class_var);
    let mut type_var = variant_i4(UIA_ListItemControlTypeId.0);
    let type_cond = unsafe { auto.CreatePropertyCondition(UIA_ControlTypePropertyId, &type_var)? };
    clear_variant(&mut type_var);
    unsafe { auto.CreateOrCondition(&class_cond, &type_cond) }
}

fn refine_viewport(auto: &IUIAutomation, list: &IUIAutomationElement) -> IRect {
    let mut viewport = unsafe { list.CurrentBoundingRectangle() }.map(rect_of).unwrap_or(IRect::EMPTY);
    if viewport.is_degenerate() {
        return viewport;
    }
    if let Some(header) = find_by_class(auto, list, "UIViewHeader") {
        if let Ok(rect) = unsafe { header.CurrentBoundingRectangle() } {
            let header_rect = rect_of(rect);
            let bottom = header_rect.bottom();
            if bottom > viewport.y && bottom < viewport.bottom() {
                let cut = bottom - viewport.y;
                viewport.y = bottom;
                viewport.h -= cut;
            }
        }
    }
    if let Ok(true_cond) = unsafe { auto.CreateTrueCondition() } {
        if let Ok(kids) = unsafe { list.FindAll(TreeScope_Children, &true_cond) } {
            let count = unsafe { kids.Length() }.unwrap_or(0);
            for index in 0..count {
                let Ok(kid) = (unsafe { kids.GetElement(index) }) else { continue };
                let class = unsafe { kid.CurrentClassName() }.map(|v| v.to_string()).unwrap_or_default();
                if !class.to_ascii_lowercase().contains("scrollbar") {
                    continue;
                }
                let Ok(rect) = (unsafe { kid.CurrentBoundingRectangle() }) else { continue };
                let bar = rect_of(rect);
                if bar.h > bar.w && bar.x >= viewport.x + viewport.w / 2 {
                    let overlap = viewport.right() - bar.x;
                    if overlap > 0 && overlap < viewport.w / 2 {
                        viewport.w -= overlap;
                    }
                } else if bar.w > bar.h && bar.y >= viewport.y + viewport.h / 2 {
                    let overlap = viewport.bottom() - bar.y;
                    if overlap > 0 && overlap < viewport.h / 2 {
                        viewport.h -= overlap;
                    }
                }
            }
        }
    }
    viewport
}

fn items_class(class_name: &str) -> bool {
    let lower = class_name.to_ascii_lowercase();
    lower.contains("itemsview") || lower == "directuihwnd" || lower == "syslistview32"
}

fn find_items_view(auto: &IUIAutomation, hwnd: HWND) -> windows::core::Result<(IUIAutomationElement, String)> {
    let root = unsafe { auto.ElementFromHandle(hwnd)? };
    let own = unsafe { root.CurrentClassName() }.map(|v| v.to_string()).unwrap_or_default();
    if items_class(&own) {
        return Ok((root, own));
    }
    if let Some(el) = find_by_class(auto, &root, "UIItemsView") {
        return Ok((el, "UIItemsView".into()));
    }
    if let Some(el) = find_by_class(auto, &root, "SysListView32") {
        return Ok((el, "SysListView32".into()));
    }
    if let Some(el) = find_by_class(auto, &root, "DirectUIHWND") {
        return Ok((el, "DirectUIHWND".into()));
    }
    let mut control = variant_i4(UIA_ListControlTypeId.0);
    let cond = unsafe { auto.CreatePropertyCondition(UIA_ControlTypePropertyId, &control)? };
    clear_variant(&mut control);
    match unsafe { root.FindFirst(TreeScope_Descendants, &cond) } {
        Ok(el) => Ok((el, "List".into())),
        Err(err) => Err(err),
    }
}

fn find_by_class(auto: &IUIAutomation, root: &IUIAutomationElement, class_name: &str) -> Option<IUIAutomationElement> {
    let mut value = variant_bstr(class_name);
    let cond = unsafe { auto.CreatePropertyCondition(UIA_ClassNamePropertyId, &value) }.ok()?;
    clear_variant(&mut value);
    unsafe { root.FindFirst(TreeScope_Descendants, &cond) }.ok()
}

fn rect_of(rect: RECT) -> IRect {
    IRect::from_ltrb(rect.left, rect.top, rect.right, rect.bottom)
}

fn variant_i4(value: i32) -> VARIANT {
    unsafe {
        let mut variant: VARIANT = std::mem::zeroed();
        let bytes = &mut variant as *mut VARIANT as *mut u8;
        (bytes as *mut u16).write(VT_I4.0);
        bytes.add(8).cast::<i32>().write(value);
        variant
    }
}

fn variant_bstr(text: &str) -> VARIANT {
    let raw = BSTR::from(text).into_raw();
    unsafe {
        let mut variant: VARIANT = std::mem::zeroed();
        let bytes = &mut variant as *mut VARIANT as *mut u8;
        (bytes as *mut u16).write(VT_BSTR.0);
        bytes.add(8).cast::<*const u16>().write(raw);
        variant
    }
}

fn clear_variant(variant: &mut VARIANT) {
    unsafe { let _ = VariantClear(variant); }
}
