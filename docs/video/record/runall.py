# -*- coding: utf-8 -*-
"""
One-command Maguro promo-clip recorder.

    py -3.10 docs\\video\\record\\runall.py --lang en|ja|all --exe PATH
        [--out DIR] [--langdir DIR] [--assets DIR] [--idle-seconds 180]
        [--probe] [--only s01,s02] [--skip-idle-check]
        [--bak DIR]
"""
import argparse, os, sys, time, shutil, traceback, subprocess, hashlib, datetime

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import names
import rec
from rec import (
    init, safety, watchdog, wait_idle, idle_ms, UserActivity,
    kill_ours, launch, shot, OUT, ROOT, EXE, LANG, CFG, PYTHON,
    dlg_center, ID_TB_LAYOUT, ID_TB_LOAD, ID_TB_BASKET, ID_TB_SET,
    windows, rect, front, maguro, UserActivity as UA,
)
import scenes
import seed as seedmod

def parse():
    ap = argparse.ArgumentParser(description="Record Maguro promo clips")
    ap.add_argument("--lang", default="en", choices=["en", "ja", "all"])
    ap.add_argument("--exe", required=True, help="path to maguro.exe (copied into a portable TEMP dir)")
    ap.add_argument("--out", default=None, help="output root (clips go to OUT/<lang>/)")
    ap.add_argument("--langdir", default=None, help="folder with lang.ja.ini etc.")
    ap.add_argument("--assets", default=None, help="folder with Maguro.ico / lockup PNGs")
    ap.add_argument("--idle-seconds", type=int, default=180)
    ap.add_argument("--probe", action="store_true", help="screenshot starting states, do not record")
    ap.add_argument("--only", default=None, help="comma-separated scene ids, e.g. s01,s02")
    ap.add_argument("--skip-idle-check", action="store_true")
    ap.add_argument("--bak", default=None, help="folder for %%APPDATA%%\\Maguro backup")
    ap.add_argument("--retries", type=int, default=4)
    return ap.parse_args()

def find_langdir(exe, explicit):
    cands = []
    if explicit:
        cands.append(explicit)
    d = os.path.dirname(os.path.abspath(exe))
    cands += [
        os.path.join(d, "lang"),
        os.path.join(d, "..", "..", "..", "lang"),  # unlikely
    ]
    # worktree next to a cargo target is not reliable; also try sibling of this file
    repo = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
    cands.append(os.path.join(repo, "lang"))
    for c in cands:
        c = os.path.abspath(c)
        if os.path.isfile(os.path.join(c, "lang.ja.ini")):
            return c
    return None

def find_assets(explicit):
    if explicit and os.path.isdir(explicit):
        return explicit
    repo = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
    for c in (os.path.join(repo, "assets"), os.path.join(repo, "docs", "assets")):
        if os.path.isdir(c):
            return c
    return None

def copy_assets(dest, assets):
    if not assets:
        print("no assets folder, skipping", flush=True)
        return
    ad = os.path.join(dest, "assets")
    os.makedirs(ad, exist_ok=True)
    names_ = ["Maguro.ico", "maguro-icon.svg", "maguro-exe.svg",
              "maguro-lockup-light.png", "maguro-lockup-dark.png"]
    for n in names_:
        src = os.path.join(assets, n)
        if os.path.isfile(src):
            shutil.copy2(src, os.path.join(ad, n))
    print("assets ->", ad, flush=True)

def write_manifest_draft(out, lang, cfg, exe):
    lines = []
    lines.append("# Maguro — raw ad clips, %s (draft, recorded %s)" % (
        lang.upper(), datetime.datetime.now().strftime("%Y-%m-%d %H:%M")))
    lines.append("")
    lines.append("Source exe: `%s` (portable copy in %%TEMP%%). This is a draft; key-moment times should be read from frames." % exe)
    lines.append("")
    lines.append("| # | File | Dur | Size | fps | Notes |")
    lines.append("|---|------|-----|------|-----|-------|")
    import glob as G
    for i, name in enumerate(
            ["01-pane-split", "02-drag-drop", "03-favorites", "04-basket",
             "05-folder-watch", "06-session", "07-context-menu", "08-dark-mode",
             "09-hotkey", "09-hotkey-fullscreen"], 1):
        p = os.path.join(out, name + ".mp4")
        ev = os.path.join(out, name + ".events.txt")
        if not os.path.isfile(p):
            lines.append("| %s | %s.mp4 | — | missing | | |" % (i, name))
            continue
        info = ffprobe(p)
        evs = ""
        if os.path.isfile(ev):
            evs = "; ".join(x.strip().replace("\t", " ") for x in open(ev, encoding="utf-8") if x.strip())
        lines.append("| %s | %s.mp4 | %s | %s | %s | events: %s |" % (
            i, name, info.get("dur", "?"), info.get("wh", "?"), info.get("fps", "?"), evs))
    for n in ("still-4pane-light.png", "still-4pane-dark.png"):
        p = os.path.join(out, n)
        lines.append("| S | %s | — | %s | |" % (n, "ok" if os.path.isfile(p) else "missing"))
    path = os.path.join(out, "MANIFEST.draft.md")
    open(path, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print("wrote", path, flush=True)
    return path

def ffprobe(path):
    try:
        out = subprocess.check_output([
            "ffprobe", "-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate,duration",
            "-show_entries", "format=duration", "-of", "json", path],
            text=True)
        import json
        j = json.loads(out)
        st = (j.get("streams") or [{}])[0]
        dur = st.get("duration") or (j.get("format") or {}).get("duration")
        fps = st.get("r_frame_rate", "")
        try:
            a, b = fps.split("/")
            fps = "%.0f" % (float(a) / float(b))
        except Exception:
            pass
        try:
            dur = "%.1f s" % float(dur)
        except Exception:
            dur = str(dur)
        return {"dur": dur, "wh": "%sx%s" % (st.get("width"), st.get("height")), "fps": fps}
    except Exception as e:
        return {"dur": "?", "wh": "?", "fps": "?", "err": str(e)}

def probe_run(cfg):
    """Launch a few layouts and dump screenshots + toolbar rects. No long recording."""
    from rec import dlg_item, center, shot, launch, kill_ours, OUT
    F = cfg["folders"]
    dest = os.path.join(rec.OUT, "probe")
    os.makedirs(dest, exist_ok=True)
    info = []
    p, h = launch("light", [F["projects"]], "1")
    from rec import ID_TB_LAYOUT, ID_TB_LOAD, ID_TB_BASKET, ID_TB_SET, ID_TB_LAYOUT
    ids = [("LAYOUT", 201), ("SAVE", 202), ("LIST", 203), ("LOAD", 204),
           ("FAV", 206), ("SET", 207), ("BASKET", 208), ("WATCH", 209)]
    from rec import dlg_item, rect as rec_rect
    for name, cid in ids:
        w = dlg_item(h, cid)
        if w:
            r = rec_rect(w)
            info.append("%s id=%s hwnd=%s rect=%s center=(%s,%s)" % (
                name, cid, w, r, (r[0]+r[2])//2, (r[1]+r[3])//2))
        else:
            info.append("%s id=%s MISSING" % (name, cid))
    from rec import front, maguro
    shot(os.path.join(dest, "01-start.png"), (7, 0, 953, 1192))
    # click layout, shot picker
    from rec import click, dlg_center, find_class, picker_cell, time as _t
    import time as _time
    click(*dlg_center(h, 201), dur=0.4)
    _time.sleep(0.6)
    pk = find_class("MaguroPicker")
    if pk:
        info.append("picker rect=%s cell1x2=%s cell2x2=%s" % (rec_rect(pk), picker_cell(1, 0), picker_cell(1, 1)))
    shot(os.path.join(dest, "01-picker.png"), (7, 0, 953, 1192))
    kill_ours()
    p, h = launch("light", [F["photos"], F["design"], F["acme"]], "3")
    click(*dlg_center(h, 208), dur=0.4)
    _time.sleep(1.0)
    shot(os.path.join(dest, "04-basket.png"), (0, 0, 1200, 1400))
    kill_ours()
    p, h = launch("light", [F["projects"], F["photos"], F["design"], F["invoices"]], "2x2")
    shot(os.path.join(dest, "08-4pane.png"), (7, 0, 953, 1192))
    click(*dlg_center(h, 207), dur=0.4)
    _time.sleep(0.8)
    shot(os.path.join(dest, "08-gear.png"), (7, 0, 1572, 1192))
    kill_ours()
    open(os.path.join(dest, "toolbar.txt"), "w", encoding="utf-8").write("\n".join(info) + "\n")
    print("PROBE\n" + "\n".join(info), flush=True)
    return dest

def run_scene(name, cfg, retries, idle_seconds, skip_idle):
    fn = scenes.FUNCS[name]
    for attempt in range(1, retries + 1):
        try:
            watchdog.hit = False
            watchdog.active = True
            print("=== %s attempt %d ===" % (name, attempt), flush=True)
            fn(cfg)
            print("=== %s ok ===" % name, flush=True)
            return
        except UserActivity:
            print("USER ACTIVITY during %s — abort, will retry" % name, flush=True)
            try:
                kill_ours()
            except Exception:
                pass
            if skip_idle:
                time.sleep(2)
            else:
                wait_idle(idle_seconds)
        except Exception:
            traceback.print_exc()
            try:
                kill_ours()
            except Exception:
                pass
            if attempt == retries:
                raise
            time.sleep(2)
    raise RuntimeError("%s failed after %d retries" % (name, retries))

def run_lang(lang, args, out_root):
    langdir = find_langdir(args.exe, args.langdir)
    if langdir:
        os.environ["MG_LANGDIR"] = langdir
        print("langdir", langdir, flush=True)
    else:
        print("WARNING: langdir not found — Japanese UI may fall back to English", flush=True)

    out = os.path.join(out_root, "clips" if lang == "en" else "clips-ja")
    cfg = init(lang, args.exe, out=out)
    os.environ["MG_LANG"] = cfg["code"]
    os.environ["MG_ROOT"] = rec.ROOT
    os.environ["MG_EXE"] = os.path.abspath(args.exe)

    if lang == "ja":
        # put Japanese first so Explorer-drawn text is Japanese
        safety.set_lang_first("ja")

    try:
        seedmod.seed(lang, rec.ROOT)
        safety.subst_m(os.path.join(rec.ROOT, "demo"))
        if not args.skip_idle_check:
            wait_idle(args.idle_seconds)
        safety.prep_desktop()
        watchdog.start()
        if args.probe:
            probe_run(cfg)
        else:
            only = None
            if args.only:
                only = [s.strip() for s in args.only.split(",") if s.strip()]
                # s04 moves photos/design files into Acme; re-seed before later scenes.
                if "s04" in only:
                    i = only.index("s04")
                    later = [s for s in only[i+1:] if s.startswith("s")]
                    if later and "SEED" not in only[i:]:
                        only.insert(i + 1, "SEED")
            order = only or list(scenes.ORDER)
            for s in order:
                if s == "SEED":
                    kill_ours()
                    seedmod.seed(lang, rec.ROOT)
                    continue
                run_scene(s, cfg, args.retries, args.idle_seconds, args.skip_idle_check)
            copy_assets(rec.OUT, find_assets(args.assets))
            write_manifest_draft(rec.OUT, lang, cfg, args.exe)
    finally:
        try:
            watchdog.stop()
        except Exception:
            pass
        try:
            kill_ours()
        except Exception:
            pass
        try:
            safety.restore_desktop()
        except Exception as e:
            print("restore_desktop failed:", e, flush=True)
        try:
            safety.cleanup_temp()
        except Exception as e:
            print("cleanup_temp failed:", e, flush=True)
        if lang == "ja":
            try:
                safety.restore_lang()
            except Exception as e:
                print("restore_lang failed:", e, flush=True)
    return rec.OUT

def main():
    args = parse()
    exe = os.path.abspath(args.exe)
    if not os.path.isfile(exe):
        sys.exit("exe not found: %s" % exe)
    out_root = os.path.abspath(args.out or os.path.join(os.environ["TEMP"], "MaguroAdOut"))
    os.makedirs(out_root, exist_ok=True)
    bak = args.bak or os.path.join(out_root, "appdata-bak")
    safety.snapshot(bak)
    langs = ["en", "ja"] if args.lang == "all" else [args.lang]
    try:
        for lang in langs:
            print("######## LANG %s ########" % lang, flush=True)
            if not args.skip_idle_check:
                wait_idle(args.idle_seconds)
            run_lang(lang, args, out_root)
    finally:
        try:
            safety.restore_desktop()
        except Exception:
            pass
        try:
            safety.restore_lang()
        except Exception:
            pass
        try:
            safety.unsubst_m()
        except Exception:
            pass
        try:
            kill_ours()
        except Exception:
            pass
        h = safety.restore_appdata()
        print("FINAL maguro.json sha256=%s (expected %s)" % (h, safety.expected_hash), flush=True)
    print("done. output under", out_root, flush=True)

if __name__ == "__main__":
    main()
