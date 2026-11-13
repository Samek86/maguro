# -*- coding: utf-8 -*-
"""The 9 promo scenes. Timings follow the 2026-11-10 scripts (JA original, EN derived from event logs)."""
import os, sys, time, glob, shutil

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import rec
from rec import (
    launch, Rec, click, glide, down, up, keydown, keyup, drag, combo,
    moveto, shot, kill_ours, windows, dlg_center, picker_cell,
    ID_TB_LAYOUT, ID_TB_LOAD, ID_TB_BASKET, ID_TB_SET, u32, watchdog,
    child_center, minimize_others, find_class, enum_children,
    UserActivity, menu_item_center, uia_center, basket_status, resolve_or,
    rect, maguro,
)

def _F(cfg):
    return cfg["folders"], cfg["xy"], cfg["files"], cfg["labels"]

def _pace(cfg):
    return cfg.get("pace", "en")

def s01(cfg):
    F, xy, files, L = _F(cfg)
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["projects"]], "1")
    moveto(560, 640)
    time.sleep(1.0)
    r = Rec("01-pane-split", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.4 if ja else 1.45)
    r.log("click layout button")
    click(*dlg_center(h, ID_TB_LAYOUT), dur=0.6)
    time.sleep(1.0 if ja else 0.7)
    cell = picker_cell(1, 0) or (711, 87)
    glide(*cell, 0.5)
    r.log("click 1x2 (2 panes)"); click()
    time.sleep(1.3 if ja else 0.85)
    r.log("click layout button")
    click(*dlg_center(h, ID_TB_LAYOUT), dur=0.6)
    time.sleep(1.0 if ja else 0.7)
    cell = picker_cell(1, 1) or (711, 117)
    glide(*cell, 0.5)
    r.log("click 2x2 (4 panes)"); click()
    time.sleep(1.6 if ja else 0.9)
    click(*xy["pane3"], dur=0.5); time.sleep(0.3)
    r.log("click favorite %s" % F["design"]); click(*xy["fav_design"], dur=0.7)
    time.sleep(1.2 if ja else 0.85)
    click(*xy["pane2"], dur=0.6); time.sleep(0.3)
    r.log("click favorite %s" % F["photos"]); click(*xy["fav_photos"], dur=0.7)
    time.sleep(1.2 if ja else 0.85)
    click(*xy["pane4"], dur=0.6); time.sleep(0.3)
    r.log("click favorite %s" % F["invoices"]); click(*xy["fav_invoices"], dur=0.7)
    time.sleep(0.4)
    glide(*xy["park"], 0.8)
    r.log("done")
    time.sleep(1.8 if ja else 1.6)
    r.stop()

def s02(cfg):
    F, xy, files, L = _F(cfg)
    bak = r"M:\%s" % F["backup"]
    src = r"M:\%s" % F["footage"]
    for f in glob.glob(bak + "\\*"):
        os.remove(f)
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["footage"], F["backup"]], "1x2")
    moveto(600, 700)
    time.sleep(1.0)
    r = Rec("02-drag-drop", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.2 if ja else 0.7)
    glide(*xy["rubber_a"], 0.6); time.sleep(0.15)
    r.log("rubber-band select start"); down(); time.sleep(0.1)
    glide(*xy["rubber_b"], 0.8); time.sleep(0.2); up()
    r.log("3 files selected")
    time.sleep(0.6)
    glide(*xy["drag_src"], 0.6); time.sleep(0.2)
    r.log("drag start"); down(); time.sleep(0.15)
    glide(xy["drag_src"][0] + 15, xy["drag_src"][1] + 12, 0.15)
    glide(*xy["drag_dst"], 1.3)
    keydown(0x11); time.sleep(0.4)
    r.log("drop on right pane (copy, Ctrl)"); up(); time.sleep(0.1); keyup(0x11)
    time.sleep(0.5)
    glide(560, 1000, 0.8)
    SRC = sum(os.path.getsize(f) for f in glob.glob(src + "\\*"))
    moved = False
    t = time.time()
    while time.time() - t < 40:
        watchdog.check()
        if not moved:
            for hh, tt, cc, pp in windows():
                if cc == "OperationStatusWindow":
                    u32.SetWindowPos(hh, 0, 260, 740, 0, 0, 0x0001 | 0x0004 | 0x0010)
                    r.log("progress dialog appeared (moved)")
                    moved = True
        tot = sum(os.path.getsize(f) for f in glob.glob(bak + "\\*"))
        if tot >= SRC and not any(c == "OperationStatusWindow" for _, _, c, _ in windows()):
            break
        time.sleep(0.05)
    r.log("copy finished / dialog closed")
    time.sleep(4.0)
    r.log("end")
    r.stop()

def s03(cfg):
    F, xy, files, L = _F(cfg)
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["documents"]], "1")
    moveto(*xy["park2"])
    time.sleep(1.0)
    r = Rec("03-favorites", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.0 if ja else 0.8)
    r.log("click favorite %s" % F["design"]); click(*xy["fav_design"], dur=0.8)
    time.sleep(1.8 if ja else 1.2)
    r.log("click favorite %s" % F["photos"]); click(*xy["fav_photos"], dur=0.6)
    time.sleep(1.8 if ja else 1.15)
    r.log("click favorite %s" % F["invoices"]); click(*xy["fav_invoices"], dur=0.6)
    time.sleep(0.3)
    glide(*xy["park2"], 0.9)
    time.sleep(1.6 if ja else 1.2)
    r.log("end")
    r.stop()

def s04(cfg):
    F, xy, files, L = _F(cfg)
    kill_ours()
    subprocess_seed()
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["photos"], F["design"], F["acme"]], "3")
    click(*dlg_center(h, ID_TB_BASKET))
    time.sleep(1.2)
    for hh, tt, cc, pp in windows():
        if cc == "MaguroBasket":
            u32.MoveWindow(hh, *cfg["BASKET_POS"], True)
    moveto(*xy["park2"])
    time.sleep(1.0)
    # Resolve icon centres by name (icon size / sort order changed since 2026-11-10).
    # hero-shot.jpg exists in BOTH Photos and Design, so each lookup is region-scoped.
    wx0, wy0, wx1, wy1 = rect(h)
    mid_x = (wx0 + wx1) // 2 + 40
    photos_reg = (wx0 + 180, wy0 + 70, mid_x, 780)
    design_reg = (mid_x, wy0 + 70, wx1 - 8, 700)
    photo_hero = resolve_or("photo_hero", uia_center(files["hero"], hwnd=h, region=photos_reg, control="ListItem"), xy["photo_hero"])
    photo_sunset = resolve_or("photo_sunset", uia_center(files["sunset"], hwnd=h, region=photos_reg, control="ListItem"), xy["photo_sunset"])
    design_brand = resolve_or("design_brand", uia_center(files["brand"], hwnd=h, region=design_reg, control="ListItem"), xy["design_brand"])
    design_logo = resolve_or("design_logo", uia_center(files["logo"], hwnd=h, region=design_reg, control="ListItem"), xy["design_logo"])
    r = Rec("04-basket", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.0)
    r.log("select %s" % files["hero"]); click(*photo_hero, dur=0.7)
    time.sleep(0.6)
    glide(*photo_sunset, 0.6)
    keydown(0x11); time.sleep(0.1)
    r.log("ctrl+click %s" % files["sunset"]); click(); time.sleep(0.1); keyup(0x11)
    time.sleep(0.6)
    r.log("drag 2 photos into basket")
    drag(*photo_sunset, *xy["basket_drop"], dur=1.3)
    r.log("dropped into basket (%s)" % (basket_status() or "?"))
    time.sleep(0.9 if ja else 0.55)
    r.log("select %s" % files["brand"]); click(*design_brand, dur=0.7)
    time.sleep(0.6)
    glide(*design_logo, 0.6)
    keydown(0x11); time.sleep(0.1)
    r.log("ctrl+click %s" % files["logo"]); click(); time.sleep(0.1); keyup(0x11)
    time.sleep(0.6)
    r.log("drag 2 design files into basket")
    drag(*design_logo, *xy["basket_drop2"], dur=1.3)
    r.log("dropped into basket (%s)" % (basket_status() or "?"))
    time.sleep(0.8)
    r.log("click %s pane (make active)" % F["acme"].split("\\")[-1])
    click(*xy["acme_pane"], dur=0.8)
    time.sleep(0.7)
    bh = find_class("MaguroBasket")
    sel = (child_center(bh, L["select_all"]) if bh else None) or xy["basket_select"]
    mov = (child_center(bh, L["move"]) if bh else None) or xy["basket_move"]
    r.log("click %s (basket)" % L["select_all"])
    click(*sel, dur=0.8)
    time.sleep(0.7)
    r.log("click %s" % L["move"])
    click(*mov, dur=0.6)
    time.sleep(1.0)
    glide(760, 1000, 0.9)
    time.sleep(3.2 if ja else 2.6)
    r.log("end")
    r.stop()

def s05(cfg):
    F, xy, files, L = _F(cfg)
    inbox = r"M:\%s" % F["inbox"]
    for f in glob.glob(inbox + "\\*"):
        os.remove(f)
    src = r"M:\%s\%s" % (F["design"], files["typography"])
    staged = r"M:\%s" % files["feedback"]
    shutil.copy(src, staged)
    R = cfg["REGION_WATCH"]
    p, h = launch("light", [F["projects"], F["inbox"]], "1x2", win=cfg["WIN_WATCH"])
    moveto(1500, 800)
    time.sleep(1.5)
    r = Rec("05-folder-watch", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.8)
    os.replace(staged, os.path.join(inbox, files["feedback"]))
    r.log("new file %s moved into watched M:\\%s" % (files["feedback"], F["inbox"]))
    time.sleep(5.2)
    r.log("glide to toast"); glide(*xy["toast"], 1.3)
    time.sleep(0.3); r.log("clicked toast"); click()
    time.sleep(0.6)
    glide(1500, 800, 0.9)
    time.sleep(2.0 if ja else 1.4)
    r.log("end")
    r.stop()

def s06(cfg):
    F, xy, files, L = _F(cfg)
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["documents"]], "1")
    moveto(*xy["park2"])
    time.sleep(1.0)
    r = Rec("06-session", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.0)
    r.log("click %s button" % L["open_session"])
    click(*dlg_center(h, ID_TB_LOAD), dur=0.7)
    time.sleep(1.3 if ja else 0.85)
    r.log("click %s" % cfg["sess_photo"]); click(*xy["sess_photo"], dur=0.8)
    time.sleep(1.3 if ja else 0.85)
    r.log("click %s" % cfg["sess_client"]); click(*xy["sess_client"], dur=0.6)
    time.sleep(1.4 if ja else 0.85)
    r.log("click Open" if not ja else "click 開く")
    click(*xy["sess_open"], dur=0.9)
    time.sleep(0.6)
    glide(*xy["park"], 0.9)
    time.sleep(2.2 if ja else 1.5)
    r.log("end")
    r.stop()

def s07(cfg):
    F, xy, files, L = _F(cfg)
    R = cfg["REGION_WINDOW"]
    p, h = launch("light", [F["design"]], "1")
    moveto(480, 700)
    time.sleep(1.0)
    logo_file = resolve_or("logo_file", uia_center(files["logo"], hwnd=h, control="ListItem"), xy["logo_file"])
    r = Rec("07-context-menu", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.0)
    glide(*logo_file, 0.8); time.sleep(0.2)
    r.log("right-click %s" % files["logo"]); click(btn="r")
    time.sleep(0.9)
    who = resolve_or("menu_who", menu_item_center(L["who"]) or uia_center(L["who"], contains=True, control="MenuItem", timeout=1.5), xy["menu_who"])
    rename = resolve_or("menu_rename", menu_item_center(L["rename"]) or uia_center(L["rename"], contains=True, control="MenuItem", timeout=1.0), xy["menu_rename"])
    fav = resolve_or("menu_fav", menu_item_center(L["add_fav"]) or uia_center(L["add_fav"], contains=True, control="MenuItem", timeout=1.0), xy["menu_fav"])
    r.log("menu items who=%s rename=%s fav=%s" % (who, rename, fav))
    # Sweep down the (now longer) menu, then land on the Maguro rows.
    glide(who[0] - 10, 400, 0.6)
    glide(who[0], max(400, who[1] - 120), 1.0)
    r.log("hover Maguro item: %s" % L["who"]); glide(*who, 0.5)
    time.sleep(0.7)
    r.log("hover %s" % L["rename"]); glide(*rename, 0.3)
    time.sleep(0.7)
    r.log("hover %s" % L["add_fav"]); glide(*fav, 0.3)
    if cfg.get("close_menu_at_end"):
        time.sleep(0.5)
        r.log("close menu (click empty area)")
        click(*xy["close_menu"], dur=0.5)
        time.sleep(1.0)
    else:
        time.sleep(2.0)
        r.log("end (menu still open)")
    r.stop()
    if not cfg.get("close_menu_at_end"):
        combo(0x1B)

def s08(cfg):
    F, xy, files, L = _F(cfg)
    p, h = launch("light", [F["projects"], F["photos"], F["design"], F["invoices"]], "2x2")
    moveto(*xy["park"])
    time.sleep(1.5)
    shot(os.path.join(rec.OUT, "still-4pane-light.png"), (7, 0, 953, 1192))
    R = cfg["REGION_DARK"]
    r = Rec("08-dark-mode", R)
    ja = _pace(cfg) == "ja"
    time.sleep(1.2)
    r.log("click gear menu")
    click(*dlg_center(h, ID_TB_SET), dur=0.7)
    time.sleep(1.1)
    appear = resolve_or(
        "appear",
        menu_item_center(L["appearance"]) or uia_center(L["appearance"], contains=True, control="MenuItem", timeout=1.5),
        xy["appear"],
    )
    glide(appear[0] - 20, max(80, appear[1] - 80), 0.4)
    r.log("hover %s" % L["appearance"]); glide(*appear, 0.4)
    time.sleep(1.2 if ja else 0.8)
    dark = resolve_or(
        "dark",
        menu_item_center(L["dark"]) or uia_center(L["dark"], contains=True, control="MenuItem", timeout=1.5),
        xy["dark"],
    )
    r.log("hover %s" % L["dark"]); glide(*dark, 0.5)
    time.sleep(0.5)
    r.log("click %s" % L["dark"]); click()
    time.sleep(1.0)
    glide(*xy["park"], 0.9)
    time.sleep(4.5 if ja else 3.8)
    r.log("end")
    r.stop()
    moveto(2300, 700); time.sleep(1.5)
    # after theme restart the window is back; take the dark still
    shot(os.path.join(rec.OUT, "still-4pane-dark.png"), (7, 0, 953, 1192))

def s09(cfg):
    F, xy, files, L = _F(cfg)
    def take(name, win, region, fps):
        p, h = launch("light", [F["projects"], F["photos"], F["design"], F["invoices"]], "2x2", win=win)
        time.sleep(1.0)
        moveto(2400, 1300)
        u32.ShowWindow(h, 6)
        time.sleep(1.5)
        r = Rec(name, region, fps)
        time.sleep(1.8 if _pace(cfg) == "ja" else 1.3)
        r.log("press Win+Shift+E"); combo(0x5B, 0x10, 0x45)
        time.sleep(0.6)
        fg = u32.GetForegroundWindow()
        r.log("foreground after hotkey is Maguro: %s" % (fg == h))
        time.sleep(2.6)
        r.stop()
    take("09-hotkey", cfg["WIN_DEFAULT"], cfg["REGION_WINDOW"], 60)
    take("09-hotkey-fullscreen", cfg["WIN_HOTKEY_FULL"], cfg["REGION_FULL"], 30)

def subprocess_seed():
    env = dict(os.environ, MG_LANG=rec.LANG, MG_ROOT=rec.ROOT)
    subprocess.run([rec.PYTHON, os.path.join(HERE, "seed.py")], env=env, check=True)

# import subprocess used in s04
import subprocess

ORDER = ["s01", "s02", "s03", "s05", "s04", "SEED", "s06", "s07", "s08", "s09"]
FUNCS = dict(s01=s01, s02=s02, s03=s03, s04=s04, s05=s05, s06=s06, s07=s07, s08=s08, s09=s09)
