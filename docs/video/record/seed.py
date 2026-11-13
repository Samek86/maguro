# -*- coding: utf-8 -*-
"""Create the demo folder tree used in the 2026-11-10 promo clips."""
import os, shutil, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import names

def grad(path, c1, c2, size=(1200, 800), circle=True):
    from PIL import Image, ImageDraw
    w, h = size
    im = Image.new("RGB", size)
    d = ImageDraw.Draw(im)
    for x in range(w):
        t = x / (w - 1)
        d.line([(x, 0), (x, h)], fill=tuple(int(c1[i] + (c2[i] - c1[i]) * t) for i in range(3)))
    if circle:
        ov = Image.new("RGBA", size, (0, 0, 0, 0))
        od = ImageDraw.Draw(ov)
        r = h // 6
        od.ellipse([w * 0.72 - r, h * 0.3 - r, w * 0.72 + r, h * 0.3 + r], fill=(255, 255, 255, 90))
        im = Image.alpha_composite(im.convert("RGBA"), ov).convert("RGB")
    ext = os.path.splitext(path)[1].lower()
    if ext in (".jpg", ".jpeg"):
        im.save(path, quality=90)
    else:
        im.save(path)

def flower(path):
    from PIL import Image, ImageDraw
    import math
    w, h = 1080, 1350
    im = Image.new("RGB", (w, h), (250, 246, 238))
    d = ImageDraw.Draw(im)
    for cx, cy, R, col in [(380, 560, 300, (200, 30, 40)), (760, 900, 260, (240, 170, 20))]:
        for k in range(14):
            a = k * math.pi * 2 / 14
            px, py = cx + math.cos(a) * R * 0.55, cy + math.sin(a) * R * 0.55
            d.ellipse([px - R * 0.35, py - R * 0.35, px + R * 0.35, py + R * 0.35], fill=col)
        d.ellipse([cx - R * 0.3, cy - R * 0.3, cx + R * 0.3, cy + R * 0.3], fill=(90, 50, 10))
    im.save(path)

def pdf(path):
    body = (b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n"
            b"2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n"
            b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]>>endobj\n"
            b"trailer<</Root 1 0 R>>\n%%EOF\n")
    open(path, "wb").write(body + b"%" + os.urandom(2000).hex().encode())

def blob(path, n=12000):
    open(path, "wb").write(os.urandom(n))

def big(path, size):
    with open(path, "wb") as f:
        f.truncate(size)

def text(path, s):
    open(path, "w", encoding="utf-8").write(s)

def seed(lang="en", root=None):
    cfg = names.get(lang)
    root = root or os.path.join(os.environ["TEMP"], cfg["temp_name"])
    demo = os.path.join(root, "demo")
    if os.path.exists(demo):
        shutil.rmtree(demo)
    os.makedirs(demo)

    def mk(*p):
        q = os.path.join(demo, *p)
        os.makedirs(q, exist_ok=True)
        return q

    F = cfg["folders"]
    if cfg["code"] == "ja":
        _seed_ja(mk, demo, F)
    else:
        _seed_en(mk, demo, F)
    print("seed ok", cfg["code"], demo)
    return demo

def _seed_en(mk, demo, F):
    p = mk("Projects")
    for sub in ["Apollo Website", "Brand Refresh", "Mobile App"]:
        q = mk("Projects", sub)
        pdf(os.path.join(q, "brief.pdf"))
        text(os.path.join(q, "notes.txt"), "notes\n")
    blob(os.path.join(p, "kickoff-deck.pptx"))
    text(os.path.join(p, "README.md"), "# Projects\n")
    blob(os.path.join(p, "timeline.xlsx"))

    p = mk("Design")
    for sub in ["Assets", "Exports"]:
        mk("Design", sub)
    flower(os.path.join(p, "banner-1080x1350.png"))
    pdf(os.path.join(p, "brand-guide.pdf"))
    grad(os.path.join(p, "color-palette.png"), (240, 30, 90), (255, 120, 30))
    grad(os.path.join(p, "hero-shot.jpg"), (20, 120, 200), (20, 40, 110))
    blob(os.path.join(p, "icon-set.zip"))
    text(os.path.join(p, "logo-final.svg"),
         '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">'
         '<circle cx="32" cy="32" r="28" fill="#0A9BD6"/></svg>\n')
    blob(os.path.join(p, "mockup-v3.psd"))
    pdf(os.path.join(p, "typography.pdf"))

    p = mk("Photos")
    for sub in ["Edited", "Raw", "Tokyo 2026"]:
        mk("Photos", sub)
    grad(os.path.join(p, "Tokyo 2026", "a.jpg"), (230, 60, 80), (250, 190, 40))
    grad(os.path.join(p, "Tokyo 2026", "b.jpg"), (250, 140, 40), (240, 40, 120))
    photos = {
        "city-night.jpg": ((20, 160, 110), (60, 220, 120)),
        "hero-shot.jpg": ((20, 120, 200), (20, 40, 110)),
        "mountain-trail.jpg": ((120, 30, 220), (60, 40, 200)),
        "product-01.jpg": ((30, 30, 35), (80, 80, 90)),
        "product-02.jpg": ((250, 80, 30), (250, 140, 60)),
        "product-03.jpg": ((0, 170, 250), (40, 110, 250)),
        "sunset-beach.jpg": ((250, 150, 20), (250, 210, 0)),
        "team-offsite.jpg": ((240, 40, 90), (250, 90, 50)),
    }
    for n, (a, b) in photos.items():
        grad(os.path.join(p, n), a, b)

    p = mk("Invoices")
    blob(os.path.join(p, "budget-2027.xlsx"))
    for i in range(1, 7):
        pdf(os.path.join(p, "INV-2026-%03d.pdf" % i))
    blob(os.path.join(p, "Q4-report.xlsx"))
    pdf(os.path.join(p, "tax-summary.pdf"))

    p = mk("Music")
    for n in ["opening.mp3", "bgm-loop.mp3", "ending.mp3"]:
        blob(os.path.join(p, n), 50000)

    p = mk("Documents")
    text(os.path.join(p, "checklist.txt"), "- quote\n- contract\n")
    blob(os.path.join(p, "contract-draft.docx"))
    blob(os.path.join(p, "meeting-notes.docx"))
    blob(os.path.join(p, "presentation.pptx"))
    blob(os.path.join(p, "Q4-report.xlsx"))

    p = mk("Footage")
    big(os.path.join(p, "behind-the-scenes.mov"), 900 * 1024 * 1024)
    big(os.path.join(p, "drone-flyover-4k.mp4"), 1400 * 1024 * 1024)
    big(os.path.join(p, "launch-event-4k.mp4"), 1300 * 1024 * 1024)

    p = mk("Clients", "Acme Corp")
    pdf(os.path.join(p, "proposal.pdf"))
    mk("Clients", "Yamada Trading")
    mk("Inbox")
    mk("Backup")

def _seed_ja(mk, demo, F):
    p = mk("プロジェクト")
    for sub in ["コーポレートサイト", "ブランド刷新", "モバイルアプリ"]:
        q = mk("プロジェクト", sub)
        pdf(os.path.join(q, "企画書.pdf"))
        text(os.path.join(q, "メモ.txt"), "メモ\n")
    blob(os.path.join(p, "キックオフ資料.pptx"))
    text(os.path.join(p, "README.md"), "# プロジェクト一覧\n")
    blob(os.path.join(p, "スケジュール.xlsx"))

    p = mk("デザイン")
    for sub in ["素材", "書き出し"]:
        mk("デザイン", sub)
    flower(os.path.join(p, "バナー_1080x1350.png"))
    pdf(os.path.join(p, "ブランドガイド.pdf"))
    grad(os.path.join(p, "カラーパレット.png"), (240, 30, 90), (255, 120, 30))
    grad(os.path.join(p, "メインビジュアル.jpg"), (20, 120, 200), (20, 40, 110))
    blob(os.path.join(p, "アイコンセット.zip"))
    text(os.path.join(p, "ロゴ_最終.svg"),
         '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">'
         '<circle cx="32" cy="32" r="28" fill="#0A9BD6"/></svg>\n')
    blob(os.path.join(p, "モックアップ_v3.psd"))
    pdf(os.path.join(p, "タイポグラフィ.pdf"))

    p = mk("写真")
    for sub in ["レタッチ済み", "RAW", "東京 2026"]:
        mk("写真", sub)
    grad(os.path.join(p, "東京 2026", "a.jpg"), (230, 60, 80), (250, 190, 40))
    grad(os.path.join(p, "東京 2026", "b.jpg"), (250, 140, 40), (240, 40, 120))
    photos = {
        "夜景.jpg": ((20, 160, 110), (60, 220, 120)),
        "メインビジュアル.jpg": ((20, 120, 200), (20, 40, 110)),
        "山道.jpg": ((120, 30, 220), (60, 40, 200)),
        "商品_01.jpg": ((30, 30, 35), (80, 80, 90)),
        "商品_02.jpg": ((250, 80, 30), (250, 140, 60)),
        "商品_03.jpg": ((0, 170, 250), (40, 110, 250)),
        "夕日の海.jpg": ((250, 150, 20), (250, 210, 0)),
        "チーム合宿.jpg": ((240, 40, 90), (250, 90, 50)),
    }
    for n, (a, b) in photos.items():
        grad(os.path.join(p, n), a, b)

    p = mk("請求書")
    blob(os.path.join(p, "予算_2027.xlsx"))
    for i in range(1, 7):
        pdf(os.path.join(p, "請求書_2026-%03d.pdf" % i))
    blob(os.path.join(p, "第4四半期_報告.xlsx"))
    pdf(os.path.join(p, "税務まとめ.pdf"))

    p = mk("音楽")
    for n in ["オープニング.mp3", "BGM_ループ.mp3", "エンディング.mp3"]:
        blob(os.path.join(p, n), 50000)

    p = mk("書類")
    text(os.path.join(p, "チェックリスト.txt"), "□ 見積もり\n□ 契約\n")
    blob(os.path.join(p, "契約書_草案.docx"))
    blob(os.path.join(p, "議事録.docx"))
    blob(os.path.join(p, "プレゼン資料.pptx"))
    blob(os.path.join(p, "第4四半期_報告.xlsx"))

    p = mk("動画素材")
    big(os.path.join(p, "メイキング.mov"), 900 * 1024 * 1024)
    big(os.path.join(p, "ドローン空撮_4K.mp4"), 1400 * 1024 * 1024)
    big(os.path.join(p, "新製品発表会_4K.mp4"), 1300 * 1024 * 1024)

    p = mk("クライアント", "株式会社アクメ")
    pdf(os.path.join(p, "提案書.pdf"))
    mk("クライアント", "山田商事")
    mk("受信箱")
    mk("バックアップ")

if __name__ == "__main__":
    lang = "en"
    root = None
    args = sys.argv[1:]
    if args and not args[0].startswith("-"):
        # called like the original: python seed.py   (lang from MG_LANG / argv)
        pass
    if "--lang" in args:
        lang = args[args.index("--lang") + 1]
    elif os.environ.get("MG_LANG"):
        lang = os.environ["MG_LANG"]
    if os.environ.get("MG_ROOT"):
        root = os.environ["MG_ROOT"]
    seed(lang, root)
