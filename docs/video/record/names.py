# -*- coding: utf-8 -*-
"""EN / JA demo names, session titles, and click-coordinate defaults."""

def _base():
    return dict(
        # toolbar child IDs (src/app/ids.rs) — resolved at runtime via GetDlgItem
        ID_TB_LAYOUT=201,
        ID_TB_SAVE=202,
        ID_TB_LIST=203,
        ID_TB_LOAD=204,   # Open session
        ID_TB_FAV=206,
        ID_TB_SET=207,    # gear
        ID_TB_BASKET=208,
        ID_TB_WATCH=209,
        WIN_DEFAULT="-7,0,953,1199",
        REGION_WINDOW=(7, 0, 946, 1192),
        REGION_WATCH=(808, 102, 1752, 1292),
        REGION_DARK=(7, 0, 1572, 1192),
        REGION_FULL=(0, 0, 2560, 1440),
        WIN_WATCH="801,102,1761,1301",
        WIN_HOTKEY_FULL="800,100,1760,1299",
        BASKET_POS=(26, 790, 560, 392),
        # picker cells (1x2 = row 0 col 1, 2x2 = row 1 col 1) — used if MaguroPicker is found
        PICK_1X2=(1, 0),  # (col, row) 0-based in the 4x4 grid
        PICK_2X2=(1, 1),
    )

EN = dict(_base(), **dict(
    code="en",
    temp_name="MaguroAdEn",
    lang="en",
    tab="Home",
    favs=["Projects", "Design", "Photos", "Invoices", "Music", "Documents"],
    folders=dict(
        projects="Projects",
        design="Design",
        photos="Photos",
        invoices="Invoices",
        music="Music",
        documents="Documents",
        footage="Footage",
        clients="Clients",
        acme="Clients\\Acme Corp",
        inbox="Inbox",
        backup="Backup",
        tokyo="Photos\\Tokyo 2026",
    ),
    sessions=[
        ("Client Work", "2x2", ["Projects", "Photos", "Design", "Invoices"], "2026-11-09 18:12"),
        ("Photo Shoot", "1x2", ["Photos", "Photos\\Tokyo 2026"], "2026-11-07 10:30"),
        ("Monthly Invoices", "1", ["Invoices"], "2026-11-03 16:05"),
    ],
    sess_photo="Photo Shoot",
    sess_client="Client Work",
    files=dict(
        hero="hero-shot.jpg",
        sunset="sunset-beach.jpg",
        brand="brand-guide.pdf",
        logo="logo-final.svg",
        feedback="client-feedback.pdf",
        typography="typography.pdf",
        drone="drone-flyover-4k.mp4",
    ),
    # Screen coords from the 2026-11-10 EN recording (window at 7,0).
    # File-grid coords are EN-specific (sort order differs from JA).
    xy=dict(
        fav_design=(42, 114),
        fav_photos=(36, 134),
        fav_invoices=(42, 154),
        pane3=(763, 470),
        pane2=(391, 1060),
        pane4=(763, 1060),
        park=(391, 520),
        park2=(560, 640),
        rubber_a=(566, 303),
        rubber_b=(236, 126),
        drag_src=(403, 180),
        drag_dst=(760, 430),
        # Photos (3-col medium icons, 2026-11-13 measure): hero middle-col row 2; sunset left-col row 4
        # Prefer uia_center at runtime — these are fallbacks only.
        photo_hero=(399, 268),
        photo_sunset=(280, 510),
        design_brand=(627, 345),  # brand-guide.pdf, Design pane 3-across
        design_logo=(727, 450),   # logo-final.svg

        acme_pane=(763, 931),
        basket_select=(382, 876),
        basket_move=(263, 876),
        basket_drop=(300, 1040),
        basket_drop2=(300, 1060),
        sess_photo=(71, 450),
        sess_client=(97, 396),
        sess_open=(778, 880),
        logo_file=(367, 300),      # logo-final.svg in 1-pane Design (6-across)
        menu_who=(720, 931),      # Maguro items sit below Adobe extras
        menu_rename=(720, 953),
        menu_fav=(720, 976),
        appear=(1000, 339),
        dark=(1337, 339),
        toast=(2347, 1329),
        close_menu=(480, 700),
    ),
    labels=dict(
        open_session="Open session",
        select_all="Select all",
        move="Move to active pane",
        appearance="Appearance",
        dark="Dark theme",
        who="Who is using this file?",
        rename="Batch rename",
        add_fav="Add to Maguro favorites",
        basket_cls="MaguroBasket",
    ),
    close_menu_at_end=True,
    # sleeps after Rec() starts, before each logged action — aims at 2026-11-10 EN event times
    pace="en",
))

JA = dict(_base(), **dict(
    code="ja",
    temp_name="MaguroAdJa",
    lang="ja",
    tab="ホーム",
    favs=["プロジェクト", "デザイン", "写真", "請求書", "音楽", "書類"],
    folders=dict(
        projects="プロジェクト",
        design="デザイン",
        photos="写真",
        invoices="請求書",
        music="音楽",
        documents="書類",
        footage="動画素材",
        clients="クライアント",
        acme="クライアント\\株式会社アクメ",
        inbox="受信箱",
        backup="バックアップ",
        tokyo="写真\\東京 2026",
    ),
    sessions=[
        ("クライアント案件", "2x2", ["プロジェクト", "写真", "デザイン", "請求書"], "2026-11-09 18:12"),
        ("撮影", "1x2", ["写真", "写真\\東京 2026"], "2026-11-07 10:30"),
        ("月次請求書", "1", ["請求書"], "2026-11-03 16:05"),
    ],
    sess_photo="撮影",
    sess_client="クライアント案件",
    files=dict(
        hero="メインビジュアル.jpg",
        sunset="夕日の海.jpg",
        brand="ブランドガイド.pdf",
        logo="ロゴ_最終.svg",
        feedback="先方フィードバック.pdf",
        typography="タイポグラフィ.pdf",
        drone="ドローン空撮_4K.mp4",
    ),
    xy=dict(
        fav_design=(42, 114),
        fav_photos=(36, 134),
        fav_invoices=(42, 154),
        pane3=(763, 470),
        pane2=(391, 1060),
        pane4=(763, 1060),
        park=(391, 520),
        park2=(560, 640),
        rubber_a=(566, 303),
        rubber_b=(236, 126),
        drag_src=(403, 180),
        drag_dst=(760, 430),
        photo_hero=(399, 268),
        photo_sunset=(399, 437),  # JA sort: sunset is middle-col (UIA overrides)
        design_brand=(627, 345),
        design_logo=(727, 450),

        acme_pane=(763, 931),
        basket_select=(382, 876),
        basket_move=(263, 876),
        basket_drop=(300, 1040),
        basket_drop2=(300, 1060),
        sess_photo=(71, 450),
        sess_client=(97, 396),
        sess_open=(778, 880),
        logo_file=(367, 300),
        menu_who=(720, 931),
        menu_rename=(720, 953),
        menu_fav=(720, 976),
        appear=(1000, 339),
        dark=(1337, 339),
        toast=(2347, 1329),
        close_menu=(480, 700),
    ),
    labels=dict(
        open_session="セッションを開く",
        select_all="すべて選択",
        move="アクティブペインへ移動",
        appearance="外観",
        dark="ダーク テーマ",
        who="このファイルを使用中のプログラム",
        rename="一括リネーム",
        add_fav="Maguro のお気に入りに追加",
        basket_cls="MaguroBasket",
    ),
    close_menu_at_end=False,
    pace="ja",
))

def get(lang):
    lang = (lang or "en").lower()
    if lang in ("ja", "jp", "japanese"):
        return JA
    return EN
