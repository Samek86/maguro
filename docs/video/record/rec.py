# -*- coding: utf-8 -*-
"""Shared helpers for Maguro promo-clip recording (mouse, ffmpeg, launch, idle, restore)."""
import ctypes, ctypes.wintypes as wt, time, subprocess, os, sys, json, shutil, threading

u32 = ctypes.windll.user32
k32 = ctypes.windll.kernel32
ctypes.windll.winmm.timeBeginPeriod(1)
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    u32.SetProcessDPIAware()

# --- paths, filled by init() ---
OUT = None
ROOT = None
EXE = None
LANG = "en"
CFG = None
PYTHON = sys.executable
HERE = os.path.dirname(os.path.abspath(__file__))

ID_TB_LAYOUT, ID_TB_LOAD, ID_TB_BASKET, ID_TB_SET = 201, 204, 208, 207

# 64-bit: MN_GETHMENU (and any other SendMessage) must not truncate to c_int
u32.SendMessageW.restype = ctypes.c_ssize_t
u32.SendMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]


class MI(ctypes.Structure):
    _fields_ = [("dx", wt.LONG), ("dy", wt.LONG), ("mouseData", wt.DWORD),
                ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]
class KI(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD),
                ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]
class _U(ctypes.Union):
    _fields_ = [("mi", MI), ("ki", KI), ("pad", ctypes.c_byte * 32)]
class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("u", _U)]

def _send(*inps):
    arr = (INPUT * len(inps))(*inps)
    u32.SendInput(len(inps), arr, ctypes.sizeof(INPUT))

def mouse_flag(f):
    i = INPUT(); i.type = 0; i.u.mi = MI(0, 0, 0, f, 0, 0); _send(i)

def pos():
    p = wt.POINT(); u32.GetCursorPos(ctypes.byref(p)); return p.x, p.y

SW, SH = u32.GetSystemMetrics(0), u32.GetSystemMetrics(1)

def moveto(x, y):
    ax = int(round(int(x) * 65535 / max(1, SW - 1)))
    ay = int(round(int(y) * 65535 / max(1, SH - 1)))
    i = INPUT(); i.type = 0; i.u.mi = MI(ax, ay, 0, 0x8001, 0, 0); _send(i)

def wake_cursor():
    for _ in range(6):
        for d in (3, -3):
            i = INPUT(); i.type = 0; i.u.mi = MI(d, 0, 0, 0x0001, 0, 0); _send(i)
            time.sleep(0.02)

def glide(x, y, dur=0.6, steps=None):
    watchdog.check()
    x0, y0 = pos()
    n = steps or max(12, int(dur * 90))
    for k in range(1, n + 1):
        watchdog.check()
        t = k / n
        e = t * t * (3 - 2 * t)
        moveto(x0 + (x - x0) * e, y0 + (y - y0) * e)
        time.sleep(dur / n)

def down(btn="l"): mouse_flag(0x0002 if btn == "l" else 0x0008)
def up(btn="l"): mouse_flag(0x0004 if btn == "l" else 0x0010)

def click(x=None, y=None, btn="l", dur=0.6):
    if x is not None:
        glide(x, y, dur)
    time.sleep(0.08); down(btn); time.sleep(0.06); up(btn)

def key(vk, upflag=False):
    i = INPUT(); i.type = 1; i.u.ki = KI(vk, 0, 0x0002 if upflag else 0, 0, 0); _send(i)
def keydown(vk): key(vk)
def keyup(vk): key(vk, True)
def combo(*vks):
    for v in vks: keydown(v); time.sleep(0.03)
    for v in reversed(vks): keyup(v); time.sleep(0.03)

def drag(x1, y1, x2, y2, dur=1.2, hold_mod=None):
    glide(x1, y1, 0.5); time.sleep(0.15)
    if hold_mod is not None:
        keydown(hold_mod)
    down(); time.sleep(0.15)
    glide(x1 + 12, y1 + 8, 0.15)
    glide(x2, y2, dur)
    time.sleep(0.35)
    up()
    if hold_mod is not None:
        time.sleep(0.05); keyup(hold_mod)

EnumWindows = u32.EnumWindows
WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

def windows():
    res = []
    def cb(h, l):
        if u32.IsWindowVisible(h):
            n = u32.GetWindowTextLengthW(h)
            b = ctypes.create_unicode_buffer(n + 1)
            u32.GetWindowTextW(h, b, n + 1)
            c = ctypes.create_unicode_buffer(256)
            u32.GetClassNameW(h, c, 256)
            pid = wt.DWORD()
            u32.GetWindowThreadProcessId(h, ctypes.byref(pid))
            res.append((h, b.value, c.value, pid.value))
        return True
    EnumWindows(WNDENUMPROC(cb), 0)
    return res

def rect(h):
    r = wt.RECT()
    try:
        ctypes.windll.dwmapi.DwmGetWindowAttribute(wt.HWND(h), 9, ctypes.byref(r), ctypes.sizeof(r))
        if r.right > r.left:
            return r.left, r.top, r.right, r.bottom
    except Exception:
        pass
    u32.GetWindowRect(h, ctypes.byref(r))
    return r.left, r.top, r.right, r.bottom

def center(h):
    x0, y0, x1, y1 = rect(h)
    return (x0 + x1) // 2, (y0 + y1) // 2

def maguro(pid=None):
    for h, t, c, p in windows():
        if t.endswith("- Maguro") and (pid is None or p == pid):
            return h
    return None

def front(h):
    u32.keybd_event(0x12, 0, 0, 0); u32.keybd_event(0x12, 0, 2, 0)
    u32.ShowWindow(h, 9); u32.SetForegroundWindow(h)

def dlg_item(main, cid):
    return u32.GetDlgItem(main, cid)

def dlg_center(main, cid):
    h = dlg_item(main, cid)
    if not h:
        raise RuntimeError("GetDlgItem(%s) failed" % cid)
    return center(h)

def find_class(cls, pid=None):
    for h, t, c, p in windows():
        if c == cls and (pid is None or p == pid):
            return h
    return None

def find_title(substr, pid=None):
    for h, t, c, p in windows():
        if substr in t and (pid is None or p == pid):
            return h
    return None

def picker_cell(col, row):
    """Screen centre of a 4x4 MaguroPicker cell. Falls back to None if not open."""
    h = find_class("MaguroPicker")
    if not h:
        return None
    x0, y0, x1, y1 = rect(h)
    # pick_geo at 96 DPI: pad=12, cell=24, gap=6
    pad, cell, gap = 12, 24, 6
    cx = x0 + pad + col * (cell + gap) + cell // 2
    cy = y0 + pad + row * (cell + gap) + cell // 2
    return cx, cy


SKIP_MINIMIZE = {
    "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW",
    "Windows.UI.Core.CoreWindow", "NotifyIconOverflowWindow",
    "ForegroundStaging", "ApplicationManager_DesktopShellWindow",
}

def minimize_others(keep=()):
    """Minimize every visible top-level window except those in `keep` and the shell."""
    keep = set(int(h) for h in keep if h)
    for h, t, c, p in windows():
        if int(h) in keep:
            continue
        if c in SKIP_MINIMIZE:
            continue
        if not t and c.startswith("Chrome_"):
            continue
        u32.ShowWindow(h, 6)  # SW_MINIMIZE
    time.sleep(0.3)

def enum_children(parent):
    res = []
    WNDENUMPROC2 = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, l):
        n = u32.GetWindowTextLengthW(h)
        b = ctypes.create_unicode_buffer(n + 1)
        u32.GetWindowTextW(h, b, n + 1)
        c = ctypes.create_unicode_buffer(256)
        u32.GetClassNameW(h, c, 256)
        res.append((h, b.value, c.value))
        return True
    u32.EnumChildWindows(parent, WNDENUMPROC2(cb), 0)
    return res

def child_center(parent, text, contains=True):
    """Centre of a child window whose title equals / contains `text`."""
    for h, t, c in enum_children(parent):
        ok = (text in t) if contains else (t == text)
        if ok and t:
            return center(h)
    return None


# ----- context-menu items (Win32 #32768) -----
MN_GETHMENU = 0x01E1
MF_BYPOSITION = 0x0400

def _popup_menus():
    found = []
    def cb(h, l):
        c = ctypes.create_unicode_buffer(256)
        u32.GetClassNameW(h, c, 256)
        if c.value == "#32768" and u32.IsWindowVisible(h):
            found.append(h)
        return True
    EnumWindows(WNDENUMPROC(cb), 0)
    return found

def menu_item_center(substr, timeout=2.5):
    """Screen centre of a visible popup-menu item whose text contains `substr`.

    Uses MN_GETHMENU + GetMenuItemRect so Adobe / third-party extra items
    do not shift the Maguro entries out from under a hardcoded Y.
    """
    if not substr:
        return None
    needle = substr.lower()
    u32.GetMenuItemCount.argtypes = [ctypes.c_void_p]
    u32.GetMenuItemCount.restype = ctypes.c_int
    u32.GetMenuStringW.argtypes = [ctypes.c_void_p, wt.UINT, wt.LPWSTR, ctypes.c_int, wt.UINT]
    u32.GetMenuStringW.restype = ctypes.c_int
    u32.GetMenuItemRect.argtypes = [wt.HWND, ctypes.c_void_p, wt.UINT, ctypes.POINTER(wt.RECT)]
    u32.GetMenuItemRect.restype = wt.BOOL
    t0 = time.time()
    last = []
    while time.time() - t0 < timeout:
        for hwnd in _popup_menus():
            hmenu = u32.SendMessageW(hwnd, MN_GETHMENU, 0, 0)
            if not hmenu:
                continue
            n = u32.GetMenuItemCount(hmenu)
            items = []
            for i in range(n):
                buf = ctypes.create_unicode_buffer(512)
                u32.GetMenuStringW(hmenu, i, buf, 512, MF_BYPOSITION)
                text = (buf.value or "").replace("&", "").split("\t", 1)[0].strip()
                rc = wt.RECT()
                if not u32.GetMenuItemRect(hwnd, hmenu, i, ctypes.byref(rc)):
                    continue
                cx, cy = (rc.left + rc.right) // 2, (rc.top + rc.bottom) // 2
                items.append((text, cx, cy, rc.left, rc.top, rc.right, rc.bottom))
                if text and needle in text.lower():
                    return (cx, cy)
            last = items
        time.sleep(0.05)
    if last:
        sys.stderr.write("menu_item_center(%r) miss; items=%s\n" % (
            substr, [(te, x, y) for te, x, y, *_ in last]))
    return None


# ----- UI Automation (file icons, menu fallback) -----
_UIA_EXE = os.path.join(HERE, "uiafind.exe")

def _ensure_uia():
    if os.path.isfile(_UIA_EXE) and os.path.getsize(_UIA_EXE) > 1000:
        return _UIA_EXE
    src = os.path.join(HERE, "uiafind.cs")
    if not os.path.isfile(src):
        return None
    csc_candidates = [
        r"C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe",
        r"C:\Windows\Microsoft.NET\Framework\v4.0.30319\csc.exe",
    ]
    csc = next((c for c in csc_candidates if os.path.isfile(c)), None)
    if not csc:
        sys.stderr.write("uiafind: csc.exe not found\n")
        return None
    fw = os.path.dirname(csc)
    refs = []
    for name in ("UIAutomationClient.dll", "UIAutomationTypes.dll", "WindowsBase.dll"):
        for folder in (os.path.join(fw, "WPF"), fw,
                       r"C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\.NETFramework\v4.8",
                       r"C:\Windows\Microsoft.NET\Framework64\v4.0.30319\WPF"):
            path = os.path.join(folder, name)
            if os.path.isfile(path):
                refs.append("/r:" + path)
                break
    cmd = [csc, "/nologo", "/t:exe", "/out:" + _UIA_EXE] + refs + [src]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not os.path.isfile(_UIA_EXE):
        sys.stderr.write("uiafind compile failed: %s\n%s\n" % (r.stdout, r.stderr))
        return None
    return _UIA_EXE

def uia_center(name, hwnd=None, region=None, contains=False, control=None, timeout=4.0):
    """Screen centre of a UI Automation element named `name`.

    `region` is (left, top, right, bottom) in screen pixels; only matches
    whose centre lies inside it are kept (needed when the same file name
    appears in two panes). `control` is a ControlType field name, e.g.
    'ListItem' or 'MenuItem'.
    """
    exe = _ensure_uia()
    if not exe or not name:
        return None
    t0 = time.time()
    last_err = ""
    while time.time() - t0 < timeout:
        cmd = [exe, "--name", name]
        if contains:
            cmd.append("--contains")
        if hwnd:
            cmd += ["--hwnd", str(int(hwnd))]
        if region:
            cmd += ["--region", "%d,%d,%d,%d" % tuple(int(x) for x in region)]
        if control:
            cmd += ["--control", control]
        r = subprocess.run(cmd, capture_output=True, text=True)
        last_err = (r.stderr or "").strip()
        if r.returncode == 0 and (r.stdout or "").strip():
            parts = r.stdout.split()
            return (int(parts[0]), int(parts[1]))
        time.sleep(0.15)
    if last_err:
        sys.stderr.write("uia_center(%r) failed: %s\n" % (name, last_err[:500]))
    return None


def basket_status(bh=None):
    """Text of File Basket child windows, e.g. a status line '4 items · 0 selected'."""
    bh = bh or find_class("MaguroBasket")
    if not bh:
        return ""
    texts = []
    for h, te, c in enum_children(bh):
        if te:
            texts.append(te)
    return " | ".join(texts)


def resolve_or(label, found, fallback):
    """Log which locator won and return a coordinate pair."""
    if found:
        sys.stderr.write("resolve %s -> %s (locator)\n" % (label, found))
        return found
    sys.stderr.write("resolve %s -> %s (fallback)\n" % (label, fallback))
    return fallback


class Rec:
    def __init__(self, name, region, fps=60):
        self.name = name
        self.t0 = None
        self.ev = []
        x, y, w, h = region
        w -= w % 2; h -= h % 2
        os.makedirs(OUT, exist_ok=True)
        self.path = os.path.join(OUT, name + ".mp4")
        args = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
                "-f", "gdigrab", "-framerate", str(fps), "-draw_mouse", "1",
                "-offset_x", str(x), "-offset_y", str(y),
                "-video_size", "%dx%d" % (w, h), "-i", "desktop",
                "-c:v", "libx264", "-preset", "veryfast", "-crf", "16",
                "-pix_fmt", "yuv420p", self.path]
        wake_cursor()
        watchdog.check()
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE)
        time.sleep(0.55)
        self.t0 = time.time()

    def log(self, s):
        t = time.time() - self.t0
        self.ev.append("%.2f\t%s" % (t, s))
        print("%.2f %s" % (t, s), flush=True)

    def stop(self):
        try:
            self.p.stdin.write(b"q"); self.p.stdin.flush()
        except Exception:
            pass
        try:
            self.p.wait(30)
        except Exception:
            self.p.kill()
        evp = os.path.join(OUT, self.name + ".events.txt")
        open(evp, "w", encoding="utf-8").write("\n".join(self.ev) + "\n")

    def abort(self):
        try:
            self.p.kill()
        except Exception:
            pass
        for p in (self.path, os.path.join(OUT, self.name + ".events.txt")):
            try:
                if os.path.exists(p):
                    os.remove(p)
            except Exception:
                pass

def shot(path, region=None):
    from PIL import ImageGrab
    im = ImageGrab.grab(bbox=region, all_screens=True)
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    im.save(path)
    return path

def kill_ours():
    if not ROOT:
        return
    pat = ROOT.replace("'", "''")
    ps = "Get-Process maguro -ErrorAction SilentlyContinue | ?{ $_.Path -like '%s*' } | Stop-Process -Force" % pat
    subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True)
    time.sleep(0.8)

def prefer_ui_lang(tag):
    """Make this process (and child processes) prefer `tag` for MUI resources.

    Explorer views hosted inside Maguro, and in-process shell dialogs, load
    their strings from the process preferred UI language. `tag` is e.g. 'ja-JP'.
    """
    if not tag:
        return False
    if "-" not in tag:
        tag = {"ja": "ja-JP", "ko": "ko-KR", "en": "en-US"}.get(tag.lower(), tag)
    raw = (tag + "\0\0").encode("utf-16-le")
    buf = ctypes.create_string_buffer(raw, len(raw))
    MUI_LANGUAGE_NAME = 0x8
    k = ctypes.windll.kernel32
    k.SetProcessPreferredUILanguages.argtypes = [wt.DWORD, ctypes.c_void_p, ctypes.POINTER(wt.ULONG)]
    k.SetProcessPreferredUILanguages.restype = wt.BOOL
    k.SetThreadPreferredUILanguages.argtypes = [wt.DWORD, ctypes.c_void_p, ctypes.POINTER(wt.ULONG)]
    k.SetThreadPreferredUILanguages.restype = wt.BOOL
    n1, n2 = wt.ULONG(0), wt.ULONG(0)
    ok1 = k.SetProcessPreferredUILanguages(MUI_LANGUAGE_NAME, buf, ctypes.byref(n1))
    ok2 = k.SetThreadPreferredUILanguages(MUI_LANGUAGE_NAME, buf, ctypes.byref(n2))
    langid = {"ja-JP": 0x0411, "ko-KR": 0x0412, "en-US": 0x0409}.get(tag)
    if langid:
        k.SetThreadUILanguage.argtypes = [ctypes.c_ushort]
        k.SetThreadUILanguage.restype = ctypes.c_ushort
        k.SetThreadUILanguage(langid)
    sys.stderr.write("prefer_ui_lang %s process=%s thread=%s\n" % (tag, bool(ok1), bool(ok2)))
    return bool(ok1 or ok2)


def launch(mode="light", start=None, layout="1", win=None):
    kill_ours()
    win = win or CFG["WIN_DEFAULT"]
    start = start or [CFG["folders"]["projects"]]
    env = dict(os.environ, MG_LAYOUT=layout, MG_WIN=win, MG_EXE=EXE or "", MG_ROOT=ROOT, MG_LANG=LANG)
    mk = os.path.join(HERE, "mkapp.py")
    subprocess.run([PYTHON, mk, mode] + list(start), env=env, check=True)
    exe = os.path.join(ROOT, "app", "maguro.exe")
    if (LANG or "").lower().startswith("ja"):
        prefer_ui_lang("ja-JP")
    p = subprocess.Popen([exe], cwd=os.path.join(ROOT, "app"))
    h = None
    for _ in range(150):
        h = maguro(p.pid)
        if h:
            break
        time.sleep(0.1)
    if not h:
        raise RuntimeError("Maguro window did not appear")
    time.sleep(2.5)
    front(h)
    minimize_others(keep=(h,))
    front(h)
    wake_cursor()
    time.sleep(2.0)
    watchdog.check()
    return p, h

# --------------- idle / user-activity watchdog ---------------

class UserActivity(Exception):
    pass

class _Watchdog:
    def __init__(self):
        self.hit = False
        self.active = False
        self._hooks = []
        self._thread = None
        self._mh = self._kh = None
        self._mcb = self._kcb = None

    def start(self):
        self.hit = False
        self.active = True
        LRESULT = ctypes.c_int64 if ctypes.sizeof(ctypes.c_void_p) == 8 else ctypes.c_long
        HOOKPROC = ctypes.WINFUNCTYPE(LRESULT, ctypes.c_int, wt.WPARAM, wt.LPARAM)
        u32.CallNextHookEx.restype = LRESULT
        u32.CallNextHookEx.argtypes = [ctypes.c_void_p, ctypes.c_int, wt.WPARAM, wt.LPARAM]
        u32.SetWindowsHookExW.restype = ctypes.c_void_p
        u32.SetWindowsHookExW.argtypes = [ctypes.c_int, HOOKPROC, ctypes.c_void_p, wt.DWORD]

        class MSLL(ctypes.Structure):
            _fields_ = [("pt", wt.POINT), ("mouseData", wt.DWORD), ("flags", wt.DWORD),
                        ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]
        class KBLL(ctypes.Structure):
            _fields_ = [("vkCode", wt.DWORD), ("scanCode", wt.DWORD), ("flags", wt.DWORD),
                        ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]

        def on_mouse(nCode, wParam, lParam):
            try:
                if nCode >= 0 and self.active:
                    info = ctypes.cast(lParam, ctypes.POINTER(MSLL)).contents
                    if (info.flags & 0x01) == 0:  # not LLMHF_INJECTED
                        self.hit = True
            except Exception:
                pass
            return u32.CallNextHookEx(None, nCode, wt.WPARAM(wParam), wt.LPARAM(lParam))

        def on_key(nCode, wParam, lParam):
            try:
                if nCode >= 0 and self.active:
                    info = ctypes.cast(lParam, ctypes.POINTER(KBLL)).contents
                    if (info.flags & 0x10) == 0:  # not LLKHF_INJECTED
                        self.hit = True
            except Exception:
                pass
            return u32.CallNextHookEx(None, nCode, wt.WPARAM(wParam), wt.LPARAM(lParam))

        self._mcb = HOOKPROC(on_mouse)
        self._kcb = HOOKPROC(on_key)

        def loop():
            self._mh = u32.SetWindowsHookExW(14, self._mcb, None, 0)  # WH_MOUSE_LL
            self._kh = u32.SetWindowsHookExW(13, self._kcb, None, 0)  # WH_KEYBOARD_LL
            msg = wt.MSG()
            while self.active:
                while u32.PeekMessageW(ctypes.byref(msg), None, 0, 0, 1):
                    u32.TranslateMessage(ctypes.byref(msg))
                    u32.DispatchMessageW(ctypes.byref(msg))
                time.sleep(0.02)
            if self._mh: u32.UnhookWindowsHookEx(self._mh)
            if self._kh: u32.UnhookWindowsHookEx(self._kh)

        self._thread = threading.Thread(target=loop, daemon=True)
        self._thread.start()
        time.sleep(0.1)

    def stop(self):
        self.active = False
        if self._thread:
            self._thread.join(timeout=2)
        self._thread = None

    def check(self):
        if self.active and self.hit:
            raise UserActivity("user moved the mouse or pressed a key")

    def pause(self):
        self.active = False
        self.hit = False

watchdog = _Watchdog()

class LASTINPUTINFO(ctypes.Structure):
    _fields_ = [("cbSize", wt.UINT), ("dwTime", wt.DWORD)]

def idle_ms():
    info = LASTINPUTINFO()
    info.cbSize = ctypes.sizeof(LASTINPUTINFO)
    if not u32.GetLastInputInfo(ctypes.byref(info)):
        return 0
    return int(k32.GetTickCount() - info.dwTime) & 0xFFFFFFFF

def wait_idle(seconds=180, poll=5):
    print("waiting for %d s idle (now %.0f s)..." % (seconds, idle_ms() / 1000.0), flush=True)
    while idle_ms() < seconds * 1000:
        time.sleep(poll)
        print("  idle %.0f s" % (idle_ms() / 1000.0), flush=True)
    print("idle ok", flush=True)

# --------------- desktop / language / appdata safety ---------------

SPI_GETDESKWALLPAPER = 0x0073
SPI_SETDESKWALLPAPER = 0x0014
SPIF_UPDATEINIFILE = 0x01
SPIF_SENDCHANGE = 0x02
COLOR_DESKTOP = 1

class _Safety:
    def __init__(self):
        self.wallpaper = None
        self.color = None
        self.langs = None
        self.appdata_bak = None
        self.appdata = os.path.join(os.environ.get("APPDATA", ""), "Maguro")
        self.expected_hash = "3899B420756BCFB7181EC82CDCCB1D345CD54F45E77AE35753BBFF80B2A37735"
        self.subst_made = False
        self.prep_done = False
        self.ui_override = None
        self.ui_override_set = False

    def snapshot(self, bak_dir):
        buf = ctypes.create_unicode_buffer(512)
        ctypes.windll.user32.SystemParametersInfoW(SPI_GETDESKWALLPAPER, 512, buf, 0)
        self.wallpaper = buf.value
        self.color = ctypes.windll.user32.GetSysColor(COLOR_DESKTOP)
        self.langs = self._lang_list()
        if os.path.isdir(self.appdata):
            os.makedirs(bak_dir, exist_ok=True)
            dest = os.path.join(bak_dir, "Maguro")
            if os.path.exists(dest):
                shutil.rmtree(dest)
            shutil.copytree(self.appdata, dest)
            self.appdata_bak = dest
        print("safety snapshot: wallpaper=%r color=%s langs=%s appdata_bak=%s hash=%s" % (
            self.wallpaper, self.color, self.langs, self.appdata_bak, self.hash_now()), flush=True)
        h = self.hash_now()
        if h and h.upper() != self.expected_hash:
            print("WARNING: maguro.json hash at start is %s (expected %s)" % (h, self.expected_hash), flush=True)

    def _lang_list(self):
        r = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "Get-WinUserLanguageList | ForEach-Object { $_.LanguageTag }"],
            capture_output=True, text=True)
        return [ln.strip() for ln in (r.stdout or "").splitlines() if ln.strip()]

    def _get_ui_override(self):
        r = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(Get-WinUILanguageOverride | ForEach-Object { $_.ToString() })"],
            capture_output=True, text=True)
        return (r.stdout or "").strip() or None

    def _set_ui_override(self, lang):
        """Set or clear (lang is None) the Windows UI language override. Does not sign out."""
        if lang:
            ps = "Set-WinUILanguageOverride -Language '%s'" % lang
        else:
            ps = "Set-WinUILanguageOverride"
        r = subprocess.run(["powershell", "-NoProfile", "-Command", ps],
                           capture_output=True, text=True)
        if r.returncode != 0:
            print("Set-WinUILanguageOverride failed:", r.stderr, flush=True)
            return False
        print("WinUILanguageOverride now=", self._get_ui_override(), flush=True)
        return True

    def set_lang_first(self, tag):
        """Temporarily put `tag` first in the user language list, and set
        WinUILanguageOverride so Explorer column headers / copy dialog /
        context menu are in that language. Does not sign out."""
        if not self.langs:
            self.langs = self._lang_list()
        others = [t for t in self.langs if t.lower() != tag.lower() and not t.lower().startswith(tag.lower() + "-")]
        first = tag
        for t in self.langs:
            if t.lower() == tag.lower() or t.lower().startswith(tag.lower() + "-"):
                first = t
                break
        ps = (
            "$l = New-WinUserLanguageList %s; " % first
            + "".join("$l.Add('%s'); " % t for t in others)
            + "Set-WinUserLanguageList $l -Force"
        )
        subprocess.run(["powershell", "-NoProfile", "-Command", ps], check=True)
        print("language list now first=", first, "rest=", others, flush=True)
        # Display language: ja-first alone is not enough for Explorer chrome.
        self.ui_override = self._get_ui_override()
        override = first if "-" in first else (first + "-JP" if first.lower() == "ja" else first)
        if first.lower().startswith("ja") and "-" not in override:
            override = "ja-JP"
        elif first.lower() in ("ko", "kr"):
            override = "ko-KR"
        if self._set_ui_override(override):
            self.ui_override_set = True
        # Explorer chrome (column headers, copy dialog) still follows the
        # Windows display language and needs a sign-out to fully switch —
        # we never sign the user out. prefer_ui_lang() is applied at launch.

    def restore_lang(self):
        if self.ui_override_set:
            self._set_ui_override(self.ui_override)  # None clears
            self.ui_override_set = False
            print("WinUILanguageOverride restored to", self.ui_override, flush=True)
        if not self.langs:
            return
        first, others = self.langs[0], self.langs[1:]
        ps = (
            "$l = New-WinUserLanguageList %s; " % first
            + "".join("$l.Add('%s'); " % t for t in others)
            + "Set-WinUserLanguageList $l -Force"
        )
        subprocess.run(["powershell", "-NoProfile", "-Command", ps], check=False)
        print("language list restored", self.langs, flush=True)

    def prep_desktop(self):
        subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(New-Object -ComObject Shell.Application).MinimizeAll()"],
            capture_output=True)
        # clear wallpaper, solid black
        ctypes.windll.user32.SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, "", SPIF_SENDCHANGE)
        arr_e = (ctypes.c_int * 1)(COLOR_DESKTOP)
        arr_v = (ctypes.c_int * 1)(0)
        ctypes.windll.user32.SetSysColors(1, arr_e, arr_v)
        minimize_others()
        self.prep_done = True
        time.sleep(0.4)

    def restore_desktop(self):
        if self.color is not None:
            arr_e = (ctypes.c_int * 1)(COLOR_DESKTOP)
            arr_v = (ctypes.c_int * 1)(self.color)
            ctypes.windll.user32.SetSysColors(1, arr_e, arr_v)
        if self.wallpaper is not None:
            ctypes.windll.user32.SystemParametersInfoW(
                SPI_SETDESKWALLPAPER, 0, self.wallpaper, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)
        subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "(New-Object -ComObject Shell.Application).UndoMinimizeAll()"],
            capture_output=True)
        self.prep_done = False
        print("desktop restored", flush=True)

    def restart_explorer(self):
        """Restart explorer.exe so shell UI (copy dialog, context menu) reloads the display language."""
        print("restarting explorer.exe", flush=True)
        subprocess.run(["taskkill", "/F", "/IM", "explorer.exe"], capture_output=True)
        time.sleep(1.0)
        subprocess.Popen([os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "explorer.exe")])
        for i in range(60):
            time.sleep(0.25)
            if find_class("Shell_TrayWnd"):
                time.sleep(1.2)
                print("explorer tray is back (%.1fs)" % ((i + 1) * 0.25 + 1.2), flush=True)
                return True
        print("WARNING: Shell_TrayWnd did not reappear", flush=True)
        return False

    def subst_m(self, demo):
        r = subprocess.run(["subst"], capture_output=True, text=True)
        for line in (r.stdout or "").splitlines():
            if line.upper().startswith("M:"):
                raise RuntimeError("M: already subst'd: %s" % line)
        subprocess.run(["subst", "M:", demo], check=True)
        self.subst_made = True
        print("subst M: ->", demo, flush=True)

    def unsubst_m(self):
        if self.subst_made:
            subprocess.run(["subst", "M:", "/d"], check=False)
            self.subst_made = False
            print("subst M: removed", flush=True)

    def hash_now(self):
        p = os.path.join(self.appdata, "maguro.json")
        if not os.path.isfile(p):
            return None
        import hashlib
        h = hashlib.sha256()
        with open(p, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 16), b""):
                h.update(chunk)
        return h.hexdigest().upper()

    def restore_appdata(self):
        if not self.appdata_bak:
            return
        if os.path.isdir(self.appdata):
            shutil.rmtree(self.appdata)
        shutil.copytree(self.appdata_bak, self.appdata)
        h = self.hash_now()
        print("appdata restored, hash=%s" % h, flush=True)
        if h != self.expected_hash:
            print("WARNING: hash after restore is %s, expected %s" % (h, self.expected_hash), flush=True)
        return h

    def cleanup_temp(self):
        kill_ours()
        self.unsubst_m()
        if ROOT and os.path.isdir(ROOT):
            # keep? task says remove temp portable copies
            try:
                shutil.rmtree(ROOT)
                print("removed", ROOT, flush=True)
            except Exception as e:
                print("could not remove %s: %s" % (ROOT, e), flush=True)

safety = _Safety()

def init(lang, exe, out=None, cfg=None):
    global OUT, ROOT, EXE, LANG, CFG
    import names
    CFG = cfg or names.get(lang)
    LANG = CFG["code"]
    EXE = os.path.abspath(exe)
    if not os.path.isfile(EXE):
        raise FileNotFoundError(EXE)
    ROOT = os.path.join(os.environ["TEMP"], CFG["temp_name"])
    os.makedirs(ROOT, exist_ok=True)
    OUT = os.path.abspath(out or os.path.join(os.environ["TEMP"], "MaguroAdOut", LANG))
    os.makedirs(OUT, exist_ok=True)
    print("init lang=%s exe=%s root=%s out=%s" % (LANG, EXE, ROOT, OUT), flush=True)
    return CFG
