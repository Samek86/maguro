<p align="center">
  <img alt="Maguro" src="assets/maguro-icon.svg" width="96">
</p>

<h1 align="center">Maguro</h1>

<p align="center">高速で軽量な、Windows 向けマルチペイン・ファイルマネージャ。<br>本物のエクスプローラーを、並べて。</p>

<p align="center">
  <a href="https://github.com/Samek86/maguro/releases/latest"><img alt="Windows版をダウンロード" src="https://img.shields.io/badge/%E3%83%80%E3%82%A6%E3%83%B3%E3%83%AD%E3%83%BC%E3%83%89-Windows%E7%89%88-0078D6?style=for-the-badge&logo=windows&logoColor=white"></a>
</p>

> **Maguro は efww の [efww/grids](https://github.com/efww/grids) のフォークです。** 実行ファイル名（`maguro.exe`）、ウィンドウクラス、設定フォルダ、ホットキー（Win+Shift+E）が異なります。

<p align="center">
  <a href="https://github.com/Samek86/maguro"><img alt="プラットフォーム: Windows" src="https://img.shields.io/badge/platform-Windows-0078D4?logo=windows&logoColor=white"></a>
  <a href="LICENSE"><img alt="ライセンス: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
  <a href="https://github.com/Samek86/maguro"><img alt="言語: Rust" src="https://img.shields.io/badge/language-Rust-000000?logo=rust&logoColor=white"></a>
  <a href="https://github.com/Samek86/maguro/releases/latest"><img alt="リリース 0.3.3" src="https://img.shields.io/badge/release-0.3.3-0A9BD6"></a>
</p>

<p align="center">
  <a href="README.md">English</a> | 日本語 | <a href="README.ko.md">한국어</a>
</p>

<br>

<p align="center">
  <a href="https://samek86.github.io/maguro/help/ja/"><img alt="オンラインヘルプを開く" src="https://img.shields.io/badge/%E3%82%AA%E3%83%B3%E3%83%A9%E3%82%A4%E3%83%B3%E3%83%98%E3%83%AB%E3%83%97%E3%82%92%E9%96%8B%E3%81%8F-0A9BD6?style=for-the-badge&logo=bookstack&logoColor=white"></a>
  <br><sub>使い方とショートカットの一覧はオンラインヘルプにあります。</sub>
</p>

<br>

<p align="center">
  <a href="https://samek86.github.io/maguro/images-v11/demo/demo-ja.mp4"><img src="docs/images-v11/demo/demo-ja.gif" width="360" alt="Maguroの動作画面"></a>
</p>

<p align="center">▶ <a href="https://samek86.github.io/maguro/images-v11/demo/demo-ja.mp4">フル動画を見る</a>（70秒）</p>

<br>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images-v11/ja/hero_dark.png">
    <img alt="Maguro、ライトテーマ、ペイン二つ" src="docs/images-v11/ja/hero_light.png">
  </picture>
</p>

どのペインも本物の Windows エクスプローラーです。右クリックメニュー、ドラッグ＆ドロップ、サムネイル、Windows のコピーダイアログがそのまま動きます。新規インストールは **ペインひとつ** で開きます。仕事が広がったら格子を広げてください。最大 4×4、十六ペインです。

コピーと移動は **IFileOperation** です。Windows の進行状況、元に戻す、見慣れた競合の確認がそのまま使えます。ネットワーク転送は、必要なときだけ robocopy に切り替えられます。

## 既定のフォルダーアプリとして使う（おすすめ）

このオプションをオンにすると、フォルダーやドライブをダブルクリックしたときに Maguro が開きます。先に Maguro を起動しておく必要はありません。

このオプションは **既定ではオフ** です。自分で **設定 → 一般 → 既定のフォルダーアプリとして使用**、または歯車メニューの **動作** からオンにします。

**設定 → 情報 → アンインストール** が Maguro を削除します。先に確認し、既定のボタンは「いいえ」です。「いいえ」では何も変わりません。「はい」は **既定のフォルダーアプリとして使用** をオフにし、設定フォルダー `%APPDATA%\Maguro` を削除します（`portable.txt` が exe の隣にあるときは、そのフォルダーの中で Maguro 自身が作ったファイルだけを削除します）。そのあと `maguro.exe` を削除して Maguro を終了します。

自分で削除するときは、このスイッチをオフにしてから削除してください。

1. **設定 → 一般**、または歯車メニューの **動作** で **既定のフォルダーアプリとして使用** をオフにします。
2. フォルダーがエクスプローラーで開くことを確認します。
3. Maguro を削除します。

すでに Maguro を削除してしまい、フォルダーが開かないときは、Maguro を再ダウンロードして実行し、スイッチをオフにしてから削除してください。

動きの詳細（書き込むレジストリ キー、オフにすると以前の既定へ戻ること）は、設定の [既定のフォルダーアプリ](#既定のフォルダーアプリ) にあります。

## 機能

### 1. レイアウトギャラリー — プリセットすべて

ペインひとつから始めて、組み込みの格子（最大 4×4）を選べます。境界をドラッグしてサイズ変更。名前付きセッションにはレイアウト・フォルダ・列・色ラベルが入り、開くと今のタブの横にウィンドウタブが増えます。

<p align="center"><img width="760" alt="マルチペイン・レイアウト" src="docs/images-v11/ja/layout.png"></p>

<table>
<tr>
<td><img alt="1 ペイン" src="docs/images-v11/common/layout_1.png"></td>
<td><img alt="横 2" src="docs/images-v11/common/layout_2h.png"></td>
<td><img alt="縦 2" src="docs/images-v11/common/layout_2v.png"></td>
</tr>
<tr>
<td><img alt="3 ペイン" src="docs/images-v11/common/layout_3.png"></td>
<td><img alt="縦積み 3" src="docs/images-v11/common/layout_3_stack.png"></td>
<td><img alt="3 列" src="docs/images-v11/common/layout_3col.png"></td>
</tr>
<tr>
<td><img alt="4 ペイン" src="docs/images-v11/common/layout_4.png"></td>
<td><img alt="縦長 4" src="docs/images-v11/common/layout_4_tall.png"></td>
<td><img alt="5 ペイン" src="docs/images-v11/common/layout_5.png"></td>
</tr>
<tr>
<td><img alt="1+1+2" src="docs/images-v11/common/layout_preset_112.png"></td>
<td><img alt="3+1" src="docs/images-v11/common/layout_preset_31.png"></td>
<td></td>
</tr>
</table>

### 2. ウィンドウタブと、ペイン内の入れ子タブ

ウィンドウ全体の Chrome 風タブに加え、各ペインの中にもタブがあります。フォルダーが 1 つのときはペイン タブ列は隠れ、新しいペイン タブのボタンは上へ（親フォルダー）の右にあります。2 つ以上でタブ列が出て、その + でタブを追加します。タブ名はアクティブなフォルダから付き、ツールチップにフルパスが出ます。タブの複製、別ウィンドウへの移動、セッション保存もできます。

<p align="center"><img width="760" alt="ウィンドウタブとペインタブ" src="docs/images-v11/ja/tabs.png"></p>

### 3. 縦のウィンドウタブ

ウィンドウのタブ一覧を、お気に入りパネルの右に縦に並べられます（設定 → 外観 → タブを縦に並べる、または歯車メニュー → 表示 → タブを縦に並べる。既定はオフ）。右端をドラッグして幅を変え、端をダブルクリックすると元の幅に戻ります。幅は記憶されます。

<p align="center"><img width="760" alt="縦のウィンドウタブ" src="docs/images-v11/ja/vertical_tabs.png"></p>


### 4. カスタマイズできるツールバー

コマンドツールバーはアイコンだけを表示します。ウィンドウタブが上段にあるときはペインの横の縦列、縦のウィンドウタブをオンにすると同じコマンドが横バーに移ります。同梱アイコンは約 99 個です。ウィンドウが狭くてボタンが収まりきらない分は、**その他** のオーバーフローメニューに入ります。

新規インストール、またはエディターの **デフォルトに戻す** では、新しいフォルダー、新しいテキスト ファイル、削除、貼り付け、名前の変更、表示、プロパティ、重要としてマークのあとに区切り、隠しファイルを表示、右にペインを複製、下にペインを複製、ペインを閉じる、ターミナルを開く、が初期配置です。すでに保存したツールバーはそのままです。

<p align="center"><img width="760" alt="縦のコマンドツールバー" src="docs/images-v11/ja/toolbar_vertical.png"></p>
<p align="center"><img width="760" alt="横のコマンドツールバー" src="docs/images-v11/ja/toolbar_horizontal.png"></p>

### 5. ツールバーエディター

縦列の下にある鉛筆ボタン、または横バーの同じボタンからエディターを開きます。バーにコマンドをドラッグして並べ替え、下のパレットから選びます。特定のフォルダーやファイルを開く対象を追加でき、その対象が一般の「開く」項目の代わりになります。エディターは横に広く縦に短い窓で、サイズを変えられます。一番小さくするとフッターは画面に残り、パレットはスクロールします。パレットカードの長い名前は 1 行で、入りきらない部分は省略記号になり、ツールチップに正式名が出ます。

<p align="center"><img width="560" alt="ツールバーエディター" src="docs/images-v11/ja/toolbar_editor.png"></p>

### 6. タブの固定名・ロック・色

タブをダブルクリック、または右クリック → 名前の変更で好きな名前を固定できます。ロックすれば誤って閉じにくくなります。色を付けると、関連する作業が一目で分かります。選択中の色付きタブだけ左に色の帯が出て、それ以外の色付きタブは薄い色だけになります。

### 7. ペインをドラッグして移動 — Ctrl+ドラッグでコピー

ペイン番号バッジかヘッダーをつまんでドラッグします。別ペインの中央に落とすと二つが入れ替わります。端に落とすとそのペインが分割されます。**Ctrl** を押しながらドラッグすると、今のペイン タブをコピーします。端では対象を分割してこのペインは残り、中央ではそのセルにペイン タブを追加します。

<p align="center">
  <img width="360" alt="ドラッグ前" src="docs/images-v11/common/panedrag_before.png">
  <img width="360" alt="入れ替え後" src="docs/images-v11/common/panedrag_after.png">
</p>
<p align="center"><img width="560" alt="Ctrl+ドラッグでペインを複製" src="docs/images-v11/common/panedrag_dup.png"></p>

### 8. ペインごとの色

ペインごとにアクセント色を付けられます。ソースと行き先を間違えにくい、忙しいマルチペイン向きの仕込みです。

<p align="center"><img width="760" alt="ペインごとの色" src="docs/images-v11/ja/pane_colors.png"></p>

### 9. 速さのための設計

このポータブルデモでのコールドスタート実測は **607 ms**。ネットワークフォルダへの貼り付けやドラッグは robocopy を使えます（設定 → ファイル操作。既定はオフ）。右クリック → 高速貼り付けは常にこちらで、Maguro 独自の進捗と上書き／スキップ／キャンセルが出ます。

**Everything 連携のペイン検索。** ペインで Ctrl+F すると入力のたびに絞り込み。サブフォルダをオンにすると [voidtools Everything](https://www.voidtools.com/) に聞いて一瞬で結果を出し、Everything が無ければディスクを歩きます。

<p align="center"><img width="760" alt="Everything 連携のペイン検索" src="docs/images-v11/ja/search.png"></p>

### 10. お気に入り — フォルダもファイルも、グループで

サイドバーは上下半分。上がお気に入り、下がフォルダツリー。お気に入りにはエクスプローラーのクイックアクセスに加え、自分で作ったグループ（フォルダもファイルも入れられ、ドラッグで並べ替え）が入ります。Ctrl+B でクイック一覧。パネルにファイルやフォルダを落とすとピン留めされます。

<p align="center"><img width="760" alt="グループ分けしたお気に入り" src="docs/images-v11/ja/favorites.png"></p>

### 11. クイックプレビュー

F3 でクイックルックを開きます。「スペースでプレビュー」がオン（既定）なら Space でも開きます。入力から 1 秒以内、または IME 変換中のスペースは名前の一部になります。

クイックルックとプレビュー ウィンドウ（Alt+P）は同じファイルを表示します。画像は JPEG、PNG、GIF、BMP、TIFF、WebP、ICO、DDS、カメラ RAW を含みます。HEIC、HEIF、AVIF は Windows イメージング コンポーネントを使い、対応する Windows コーデックが必要です。SVG は画像として描きます。同じ表示で PDF、Markdown、テキストとコード、CSV・TSV、表としての xlsx、RTF、OpenDocument、EPUB、メール（.eml と .msg。ヘッダー、添付の名前、本文）、ショートカット（.lnk と .url）、書庫の一覧（zip、tar、gzip、Cabinet）、HWP/HWPX/HWPT を出します。テキストの文字コードはバイトから決めます。Maguro が読める Office ファイル（docx、pptx など）は Maguro が描きます。HTML などそれ以外は登録された Windows のプレビュー ハンドラーを使います。動画と音声は Windows Media Foundation で再生します。

Ctrl+=、Ctrl+-、Ctrl+0 と Ctrl+ホイールで拡大・縮小します（10–800%）。ファイルを移しても倍率は残ります。画像はドラッグで移動し、ホイールでスクロールします。テキストはドラッグで選択し、Ctrl+A ですべて選択、Ctrl+C でコピーします。クイックルックでは ←/→ がファイル移動、Enter がファイルを開く、Esc・Space・F3 がウィンドウを閉じます。**F2** で名前変更、**Del** で表示中のファイルを削除します。K は再生と一時停止、J と L は 5 秒移動です。プレビュー ウィンドウでは Space が一時停止です。

<p align="center">
  <img width="480" alt="ペイン内プレビュー" src="docs/images-v11/ja/preview.png">
  <img width="240" alt="クイックルック" src="docs/images-v11/ja/preview_peek.png">
</p>

### 12. カスタムできる右クリックメニュー

設定 → 右クリックメニュー項目では、Windows のコマンドと Maguro のコマンドをそれぞれ隠せます。どちらもオンから始まり、隠しても Maguro のメニューだけで、エクスプローラーは変わりません。プロパティは常に一番下で、Maguro の項目はそのすぐ上です。

<p align="center"><img width="560" alt="右クリックメニューの設定" src="docs/images-v11/ja/settings_ctxmenu.png"></p>

### 13. 名前のプレビュー付き一括リネーム

ファイルを選んで Ctrl+Shift+R。検索／置換、連番、拡張子、プリセットを変えるたびに、確定前の新しい名前が一覧で更新されます。

<p align="center"><img width="560" alt="名前プレビュー付き一括リネーム" src="docs/images-v11/ja/rename.png"></p>

### 14. サーバー接続

お気に入りパネル（またはグループ）を右クリック → **サーバー接続を追加…**。SMB と WebDAV は普通の Windows フォルダとして開きます。SFTP と FTP は rclone と WinFsp でマウントしたあと、プレビューも右クリックも高速コピーも他のフォルダと同じように使えます。パスワードは Windows 資格情報マネージャーだけに置き、`maguro.json` には書きません。

<p align="center"><img width="420" alt="入力済みのサーバー接続ダイアログ" src="docs/images-v11/ja/servers.png"></p>


### 15. Excel の読み取り専用オープンと A1・100%に整える

`.xls`、`.xlsx`、`.xlsm`、`.csv` を右クリック → **読み取り専用で開く** でロックせずに Excel を開きます。**A1・100%に整える** は `.xlsx` と `.xlsm` だけです。選択を A1 へ、ズームを 100% へ、先頭シートの選択、`.bak.xlsx` バックアップの四つを個別に選べ、その選択は今回の起動中は覚えます。

<p align="center"><img width="420" alt="Excel を A1・100%に整える" src="docs/images-v11/ja/feature_excel_readonly_tidy.png"></p>

### 16. ファイルバスケット

Ctrl+Shift+B で常に手前の小さなバスケットを開きます。ファイルをドロップするか Ctrl+Alt+B でアクティブペインの選択を入れ、あとから任意のペインへコピー／移動します。パスはセッションではなく `maguro.json` だけに残ります。元ファイルがなくても一覧に残り、削除するまで表示されます。

<p align="center"><img width="420" alt="設定スタイルのコンパクト行のファイルバスケット" src="docs/images-v11/ja/feature_basket.png"></p>

### 17. フォルダー監視

Ctrl+Shift+W でフォルダー監視を開きます。フォルダー・アクティブペインに加え、**個別ファイル**も追加できます。行のチェックボックスは設定と同じティックです。Maguro の実行中だけ変更を通知します。監視中のファイルが消えると「ファイルが見つかりません」と表示されます。

<p align="center"><img width="560" alt="フォルダーとファイル行があるフォルダー監視" src="docs/images-v11/ja/feature_folder_watch.png"></p>

### 18. クリップボードのパスを開く（Alt+Z）

Maguro が起動していれば **Alt+Z** で、ほかのアプリが前面でもクリップボードのパスを開きます。フォルダーは新しいタブになり、Maguro が前面に出ます。ファイルは既定のアプリで開きます。

<p align="center"><img width="760" alt="クリップボードのパスを新しいタブで開いたところ" src="docs/images-v11/ja/feature_win_shift_v.png"></p>

### 19. タブ一覧

**Ctrl+Shift+L**、タブ行右端の一覧ボタン、またはタブ右クリックの **タブ一覧…** で、各タブの配置・ペイン・フォルダーを見ます。名前で絞り込み、Enter かダブルクリックで移動し、F2 で名前を変えます。各タブは代表フォルダーと残りの `+N` だけの一行から始まり、**Right** で展開、**Left** で折りたたみます。現在のタブは最初から開いています。フィルターは一致するタブだけを展開します。

<p align="center"><img width="560" alt="各タブの配置・ペイン・フォルダーを示すタブ一覧" src="docs/images-v11/en/tab_list.png"></p>

### 20. ワークスペース

名前付きレイアウトを最大九つ。**Ctrl+Alt+1…9** でスロットを開き、**Ctrl+Alt+Shift+1…9** で今のタブ・ペイン・フォルダーをそのスロットに保存します。歯車メニュー → **ワークスペース** で保存・読み込み・名前の変更・消去ができます。スロットは `maguro.json` にあります。

<p align="center"><img width="480" alt="歯車メニュー → ワークスペース" src="docs/images-v11/ja/workspaces.png"></p>

### 21. ダイアログジャンプ

ほかのアプリの開く／保存ダイアログが前面のとき、**Ctrl+Alt+G** で Maguro に開いているフォルダーを選び、ダイアログをそこに移します。仮想フォルダーは除きます。このキーは、そのようなダイアログが前面のときだけ登録されます。

<p align="center"><img width="420" alt="ダイアログジャンプで Maguro のフォルダーを選ぶ" src="docs/images-v11/ja/dialog_jump.png"></p>

### 22. 重要マーク

ファイルやフォルダーを右クリックして **重要としてマーク**、または **Ctrl+Alt+S**。すでにマークされていれば **重要マークを解除** です。複数選んだとき、マークされていないものが一つでもあれば選択をまとめてマークし、すべてマーク済みならまとめて解除します。**重要マークの星を表示**（既定はオン）は、フォルダーペインのマークしたファイルとフォルダーに小さな金色の星を描くか隠します。星を隠してもマークは残ります。ステータスバーには開いているフォルダーの件数が、たとえば **重要 6** と出ます。星は Maguro がフォルダーペインに描きます。シェルのアイコンオーバーレイではなく、レジストリも変更しません。フォルダーをマークしても、中のファイルに星は付きません。

<p align="center"><img width="720" alt="マークしたファイルとフォルダーの金色の星" src="docs/images-v11/common/feature_marks.png"></p>

名前の変更、移動、ごみ箱についていきます。Maguro の実行中に復元してもマークは残ります。コピーには付きません。ファイルが見つからなくなると、見つからない項目として 180 日後に外れ、一覧からそれより早く消すこともできます。歯車メニューの **重要リスト**（**Ctrl+Shift+I**）では、検索、**場所を開く**、マークの解除、**見つからない項目を整理**、**エクスポート**と**インポート**ができます。設定のエクスポート、インポート、初期化はマークを変えません。マークは設定の隣の `marks.json` にあります（`%APPDATA%\Maguro\marks.json`。`portable.txt` があるときは exe の隣）。`maguro.json` とは別のファイルです。その日の最初の保存で、直前のファイルを `marks.json.bak` に残します。`marks.json` が読めないときはファイルをそのまま残し、リストに理由と場所が出ます。

<p align="center"><img width="640" alt="検索、状態、エクスポートがある重要リスト" src="docs/images-v11/common/feature_marks_list.png"></p>

### テーマとアクセント

Windows に従うライト／ダーク、テーマごとにアクセント色の見本が六つ、またはカスタム（既定はセルリアン `#0A9BD6`）。設定ウィンドウは English・日本語・한국어 に対応しています。

<table>
<tr>
<td><img alt="設定" src="docs/images-v11/ja/settings.png"></td>
</tr>
<tr>
<td><img alt="ダークテーマ" src="docs/images-v11/ja/hero_dark.png"></td>
</tr>
</table>

📦 **小さな単体 exe。** インストール不要です。エクスプローラーに表示されるアイコンは Maguro の全面タイルです。

## インストール

[Releases](https://github.com/Samek86/maguro/releases/latest) から最新の `maguro.exe` を入手し、任意のフォルダに置いて実行してください。リリースは zip ではなく、exe と `maguro.exe.sha256` を上げています。

コード署名が無いため、初回は SmartScreen が警告することがあります。**詳細情報 → 実行** を選んでください。

**動作環境:** Windows 10（1903 以降）または Windows 11、x64。SFTP/FTP には [rclone](https://rclone.org/) と [WinFsp](https://winfsp.dev/) も必要です（接続追加時に Maguro が導入を案内できます）。

設定は `%APPDATA%\Maguro\maguro.json` にあります。`maguro.exe` の隣に `portable.txt` を置くと、設定も exe の隣に置かれます。名前付きセッションは設定ファイルの隣にある `maguro-sessions` 内の `.maguro` ファイルです（`%APPDATA%\Maguro\maguro-sessions`。`portable.txt` があるときは exe の隣）。

ソースからビルドする場合は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。

**設定 → 情報 → アンインストール** が Maguro を削除します。既定のフォルダーアプリの登録を外し、設定フォルダーと `maguro.exe` を削除して終了します。手で削除する手順は [既定のフォルダーアプリとして使う（おすすめ）](#既定のフォルダーアプリとして使うおすすめ) にあります。

## キーボードショートカット

既定値です。設定 → ショートカットに出るキーはそこで変更できます。一覧の全体はオンラインヘルプにあります。F2、Del、Ctrl+C、Ctrl+V、F5 のようにエクスプローラーが使うキーはそのままです。Esc と Space はクイックルックを閉じ、別のショートカット行ではありません。

| 区分 | キー | 動作 |
| --- | --- | --- |
| **タブ** | Ctrl+T | 新しいタブ |
|  | Ctrl+W | タブを閉じる |
|  | Ctrl+Tab / Ctrl+Shift+Tab | 次／前のタブ |
|  | Ctrl+Shift+T | 閉じたタブを開き直す |
|  | Ctrl+Shift+L | タブ一覧 |
| **ペイン** | Ctrl+1…9 | ペイン 1–9 にフォーカス |
|  | Ctrl+PageDown / Ctrl+PageUp | 次／前のペイン |
| **プレビュー** | F3 | クイックルック。「スペースでプレビュー」がオン（既定）なら Space でも開く |
|  | Esc / Space / F3 | クイックルックを閉じる |
|  | Alt+P | プレビュー ウィンドウの表示／非表示 |
|  | Ctrl+= / Ctrl+- / Ctrl+0 | クイックルックとプレビュー ウィンドウの拡大・縮小。Ctrl+ホイールも同じ |
|  | K / J / L | 一時停止または再生、5 秒戻る、5 秒進む |
|  | Space | プレビュー ウィンドウで一時停止または再生。クイックルックでは Space がウィンドウを閉じる |
| **ファイル** | Ctrl+Shift+C | フルパスをコピー |
|  | Ctrl+Shift+B | ファイルバスケット |
|  | Ctrl+Alt+B | 選択をバスケットへ |
|  | Ctrl+Shift+W | フォルダー監視 |
|  | Ctrl+Alt+S | 重要としてマーク、または解除 |
|  | Ctrl+Shift+I | 重要リスト |
|  | Alt+Z | クリップボードのフォルダーは新しいタブで Maguro を前面に、ファイルは既定のアプリで開く |
|  | Ctrl+Shift+R | 一括リネーム |
|  | Ctrl+Alt+R | 最近使ったファイル（Windows の最近使った項目、新しい順） |
|  | Ctrl+Enter | 選択した最近のファイルをフォルダーに表示 |
|  | Num+ または Ctrl+Shift+A / Num− / Num* | パターンで選択／解除／反転 |
|  | Ctrl+Alt+N | 新しいテキストファイル |
|  | Ctrl+Shift+N | 新しいフォルダ |
|  | F7 | このペインに新しいフォルダ（エクスプローラー自身のコマンド） |
|  | Ctrl+Alt+C | WinMerge で二つのペインを比較 |
|  | F2, Del, Ctrl+C / V, F5 | エクスプローラーと同じ。F5 はアクティブペインを再読み込みし、失敗したネットワークフォルダも再試行 |
| **その他** | Ctrl+B | クイックお気に入り |
|  | Ctrl+S | このタブを保存 |
|  | Ctrl+, | 設定 |
|  | F1 / Ctrl+/ | オンラインヘルプ／ショートカット一覧 |
|  | Ctrl+N | 新しいウィンドウ |
|  | Ctrl+Alt+1…9 | ワークスペースを開く |
|  | Ctrl+Alt+Shift+1…9 | ワークスペースを保存 |
|  | Ctrl+Alt+G | ほかのアプリの開く/保存ダイアログで Maguro のフォルダーへ移動 |
|  | Win+Shift+E | Maguro を前面に（グローバル） |

## 既知の問題

**Maguro を管理者として実行しないでください。** PowerToys Mouse Crosshairs のようなツールは、権限の高い Maguro ウィンドウ上でカーソルを追えません（UIPI）。Maguro は通常実行するか、Maguro と PowerToys の両方を管理者で実行してください。

## 設定

タブ行の歯車の名前は「設定と表示」で、そのメニューを開きます。設定はその中にあります（`Ctrl+,`）。お気に入りの星と歯車の間で、バスケットのボタンはファイルバスケットを開き（`Ctrl+Shift+B`）、目のボタンはフォルダー監視を開きます（`Ctrl+Shift+W`）。この二つのボタンはツールバーを隠すと一緒に隠れます。歯車は残ります。

言語（自動、English、および各言語ファイル）、フォント、ライト／ダーク、アクセント、ショートカットは設定で変更します。書き込み方と `maguro.json` のキー一覧は [docs/settings.md](docs/settings.md) にあります。

### 既定のフォルダーアプリ

スイッチは **設定 → 一般 → 既定のフォルダーアプリとして使用** で、歯車メニューの **動作** にもあります。**既定ではオフ**で、自分でオンにします。使う理由、オンにする方法、削除前の手順は [既定のフォルダーアプリとして使う（おすすめ）](#既定のフォルダーアプリとして使うおすすめ) にあります。

このオプションは `maguro.json` には保存されません。オンにすると、現在のユーザーのレジストリ（`HKCU\Software\Classes\Directory\shell` と `HKCU\Software\Classes\Drive\shell`）にだけ書き込みます。`Maguro` という動詞を追加し、そのコマンドがこの `maguro.exe` を実行するようにしたうえで、その動詞を既定にします。コンピューター全体（HKLM）の設定と `Folder` クラスは変更しません。

同じオプションを**オフ**にすると、その変更を取り消します。Maguro が保存しておいた以前の既定値を戻し、自分の動詞を削除します。

## 言語ファイル

English はプログラムに組み込まれており、フォールバックです。韓国語・日本語・フランス語・ドイツ語は `lang/lang.ko.ini` などとして同梱されます。初回起動時とバージョンが変わったときに、設定隣の `lang` フォルダへその四ファイルを書き直します。`lang.xx.user.ini` や自分で追加した `lang.xx.ini` には触れません。

ファイルは UTF-8 です。行は `key = text`。[meta] の `name` は言語一覧の表示名、`font` は任意の UI フォント。無いキーは English を使います。`lang.xx.user.ini` が上に載り、優先されます。

## ロードマップ

- [ ] コード署名付きビルド — 初回の SmartScreen 警告をなくす
- [x] English・한국어・日本語以外のインターフェース言語

## クレジット

Maguro は efww の [efww/grids](https://github.com/efww/grids) のフォークです。

[Q-Dir](https://www.softwareok.com/?seite=Freeware/Q-Dir) と [voidtools Everything](https://www.voidtools.com/) に着想を得ています。Maguro は独立したプロジェクトであり、Q-Dir およびその作者とは無関係です。

## ライセンス

[MIT](LICENSE) · © 2026 efww, Samek86
