# -*- coding: utf-8 -*-
"""Write a throw-away portable Maguro app dir (exe + portable.txt + maguro.json + sessions)."""
import os, json, shutil, sys, datetime

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import names

def L(kind):
    if kind == "1":
        return {"active": 0, "colr": [1.0], "columns": [1], "rowr": [[1.0]]}
    if kind == "1x2":
        return {"active": 0, "colr": [0.5, 0.5], "columns": [1, 1], "rowr": [[1.0], [1.0]]}
    if kind == "2x2":
        return {"active": 0, "colr": [0.5, 0.5], "columns": [2, 2], "rowr": [[0.5, 0.5], [0.5, 0.5]]}
    if kind == "3":
        return {"active": 2, "colr": [0.5, 0.5], "columns": [1, 2], "rowr": [[1.0], [0.5, 0.5]]}
    raise ValueError(kind)

def mkapp(mode="light", start=None, layout=None, win=None, lang=None, root=None, exe=None):
    lang = lang or os.environ.get("MG_LANG", "en")
    cfg = names.get(lang)
    ROOT = root or os.environ.get("MG_ROOT") or os.path.join(os.environ["TEMP"], cfg["temp_name"])
    APP = os.path.join(ROOT, "app")
    SRC_EXE = exe or os.environ.get("MG_EXE") or ""
    layout = layout or os.environ.get("MG_LAYOUT", "1")
    win = [int(x) for x in (win or os.environ.get("MG_WIN", cfg["WIN_DEFAULT"])).split(",")]
    start = list(start) if start else [cfg["folders"]["projects"]]
    os.makedirs(APP, exist_ok=True)

    dest_exe = os.path.join(APP, "maguro.exe")
    if SRC_EXE and os.path.isfile(SRC_EXE):
        if (not os.path.exists(dest_exe)
                or os.path.getmtime(SRC_EXE) > os.path.getmtime(dest_exe)
                or os.path.getsize(SRC_EXE) != os.path.getsize(dest_exe)):
            shutil.copy2(SRC_EXE, dest_exe)
        # lang files live next to the source exe, or in the worktree's lang/
        src_lang = os.path.join(os.path.dirname(SRC_EXE), "lang")
        if not os.path.isdir(src_lang):
            # cargo target: ../../../maguro-refs/ad-footage/lang is not reliable; try MG_LANGDIR
            src_lang = os.environ.get("MG_LANGDIR", "")
        dest_lang = os.path.join(APP, "lang")
        if os.path.isdir(src_lang) and not os.path.isdir(dest_lang):
            shutil.copytree(src_lang, dest_lang)
    if not os.path.exists(dest_exe):
        raise FileNotFoundError("maguro.exe not in %s and MG_EXE not set" % APP)
    open(os.path.join(APP, "portable.txt"), "w").close()
    for f in ["maguro.json", "folder-views.json", "last-trace.txt"]:
        q = os.path.join(APP, f)
        if os.path.exists(q):
            os.remove(q)

    M = lambda n: "M:\\" + n
    favs = cfg["favs"]
    lay = L(layout)
    panes = [{"index": i, "path": M(p), "view": [1, 96]} for i, p in enumerate(start)]
    tab = {"active": True, "layout": lay, "locked": False, "name": cfg["tab"], "panes": panes}
    w = {"left": win[0], "top": win[1], "right": win[2], "bottom": win[3], "maximized": False}
    backup = cfg["folders"]["backup"]
    inbox = cfg["folders"]["inbox"]
    conf = {
        "version": 1, "window": w, "side": 204, "side_dpi": 96,
        "layout": {"columns": lay["columns"], "colr": lay["colr"], "rowr": lay["rowr"], "active": lay["active"]},
        "panes": [{"index": p["index"], "path": p["path"]} for p in panes],
        "accent": "0A9BD6", "accent_dark": "0A9BD6", "dark": mode == "dark", "theme": mode,
        "bar": True, "stat": True, "view_new": 1, "start": 2, "start_path": M(backup),
        "ext": True, "tree": True,
        "fav_panel": True, "tree_panel": False, "explorer_pins_open": False, "explorer_files_open": False,
        "edge": True, "confirm": True, "del": 0, "new_name": "",
        "drop": 0, "layout_new": 3, "boot": 0, "boot_file": "", "autosave": False, "dot": False,
        "update_check": False, "dblup": True, "lang": cfg["lang"], "single": True, "session": "",
        "favorites": [{"path": M(n), "name": n} for n in favs],
        "recent": [],
        "tabs": [tab],
        "windows": [{"side": 204, "side_dpi": 96, "side_split": 0.5, "tabs": [tab], "window": w}],
        "keybindings": {},
        "favorites_tree": [{"name": n, "path": M(n)} for n in favs],
        "folder_watches": [{"path": M(inbox), "subfolders": False}],
        "ctx_win11_pack_migrated": True,
    }
    json.dump(conf, open(os.path.join(APP, "maguro.json"), "w", encoding="utf-8"),
              ensure_ascii=False, indent=2)

    sd = os.path.join(APP, "maguro-sessions")
    if os.path.exists(sd):
        shutil.rmtree(sd)
    os.makedirs(sd)

    def sess(name, kind, folders, stamp):
        lay2 = L(kind)
        cols = ",".join(str(c) for c in lay2["columns"])
        tabj = {"active": True, "color": "3D7EE0", "layout": lay2, "locked": False, "name": name,
                "panes": [{"index": i, "path": M(f), "view": [1, 96]} for i, f in enumerate(folders)]}
        lines = ["sidedpi=96", "side=204", "split=0.5", "cols=%s" % cols, "viewnew=1",
                 "colr=%s" % ",".join(str(x) for x in lay2["colr"]),
                 "rowr=%s" % ";".join(",".join(str(y) for y in r) for r in lay2["rowr"]),
                 "active=0", "theme=light", "lang=" + cfg["lang"]]
        for i, f in enumerate(folders):
            lines += ["p%d=%s" % (i, M(f)), "v%d=1,96" % i]
        lines.append("tabs=" + json.dumps([tabj], ensure_ascii=False))
        pth = os.path.join(sd, name + ".maguro")
        open(pth, "w", encoding="utf-8").write("\n".join(lines) + "\n")
        t = datetime.datetime.strptime(stamp, "%Y-%m-%d %H:%M").timestamp()
        os.utime(pth, (t, t))

    for name, kind, folders, stamp in cfg["sessions"]:
        sess(name, kind, folders, stamp)
    print("app ok", mode, layout, start, cfg["lang"])
    return APP

if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "light"
    start = sys.argv[2:] if len(sys.argv) > 2 else None
    mkapp(mode, start)
