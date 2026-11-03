# 스크린샷 캡처 기록 (docs/images-v3)

Maguro 0.2.3 README / 도움말용 스크린샷. 모든 제품 이미지는 **포터블 빌드만** 사용하고, AppData·레지스트리·grids.ini·설치본 경로를 쓰지 않는다.

## 공통 규칙

- **실행 파일:** 포터블 maguro.exe (작업 디렉터리 = exe 옆). 설정은 같은 폴더의 maguro.json만.
- **테마:** README 히어로 다크를 제외하면 라이트. 언어는 파일 접미사 .ko / .ja / (없음=en).
- **배경:** 흰 백드롭 창을 대상 창 뒤에 깔고 CopyFromScreen (스크립트: docs/capture/white-backdrop.ps1).
- **크롭:** DwmGetWindowAttribute(..., DWMWA_EXTENDED_FRAME_BOUNDS) 로 그림자·다크 가장자리 없이 프레임만.
- **가장자리 검사:** 왼쪽 어두운 픽셀 ≥3 또는 높이의 5% 초과면 거부. 다른 변은 5% 스트립 초과만 거부.
- **페인트:** ShowWindow는 셸 확장 열거 중 데드락 위험이 있어 쓰지 않는다. 대신 RedrawWindow(RDW_INVALIDATE|RDW_UPDATENOW|RDW_ALLCHILDREN) 후 캡처 (docs/capture/cap-lib.ps1).
- **닫기:** 대상 프로세스 PID의 창에만 WM_CLOSE (다른 Maguro 인스턴스에 닿지 않음).
- **재사용 스크립트:** docs/capture/ — 경로는 환경변수 MAGURO_PORTABLE, MAGURO_CFG_DIR, MAGURO_SHOT_DIR 로만 지정. 개인 경로·실서버·실비밀번호를 스크립트에 넣지 말 것.

## 서버 연결 창 — 가짜 데모 값만 (필수)

servers.png / servers.ko.png / servers.ja.png 는 **실제 호스트·자격 증명을 절대 넣지 않는다.**

| 필드 | 허용 값 |
| --- | --- |
| 표시 이름 | Demo Server |
| 호스트 | sftp.example.com |
| 포트 | 22 |
| 원격 경로 | /home/demo/projects |
| 사용자 | demo |
| 비밀번호 | 비움 (또는 UI가 가리는 점만). 실비밀번호 금지 |
| 종류 | SFTP 선택 |

- **만드는 법:** Ctrl+L → sftp://demo@sftp.example.com/home/demo/projects 입력 → Enter → MaguroServer 대화상자에서 위 필드를 SetEdit(ID 10–15)로 맞춘 뒤 SFTP 종류 클릭 → 캡처.
- **스크립트:** docs/capture/capture-servers-fake.ps1
- **금지:** lab.demo.local, 사내 호스트, 실계정, 즐겨찾기에 심어 둔 Lab SFTP를 그대로 찍기.
- **크기(2026-11-04 재촬영):** EN 400×560 ≈11KB, KO ≈10KB, JA ≈13KB. 가장자리 다크=0.

## 설정 › 패널·보기 (settings_panes*)

- **창:** MaguroSettings, 라이트, EN/KO/JA.
- **페이지:** 네비 인덱스 2 = Panes & view / 패널·보기 / ペインと表示.
- **조작:** Ctrl+, 로 설정 연 뒤 **클라이언트 좌표** WM_LBUTTONDOWN/UP 으로 네비 클릭 (Click-MaguroSettingsPage). 화면 마우스 클릭은 종종 General에 머무름.
- **데모 데이터:** dialogs.json / avorites.ko.json / hero_light.ja.json 시드 + lang 덮어쓰기. 특수 서버 불필요.
- **스크립트:** docs/capture/capture-settings-panes.ps1
- **크기(재촬영):** 986×633, EN≈34KB, KO≈32KB, JA≈47KB. 빈 흰 클라이언트(~7KB)는 RedrawWindow 누락 — 거부.
- **주의:** 캡처 전 셸 확장 열거가 끝날 때까지 ≈7초 대기.

## 이미지별 요약

| 파일 | 용도 | 창/기능 | 테마·언어 | 데모 데이터 | 대략 크기 | 스크립트/방법 | 크롭 | 주의 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| address_toolbar.ja.png | 주소 도구 모음 | 툴바 클로즈업 | light/ja | 데모 | 12428 B (≈12KB) | 영역 캡처 | DWM/영역 |  |
| address_toolbar.ko.png | 주소 도구 모음 | 툴바 클로즈업 | light/ko | 데모 | 12734 B (≈12KB) | 영역 캡처 | DWM/영역 |  |
| address_toolbar.png | 주소 도구 모음 | 툴바 클로즈업 | light/en | 데모 | 12330 B (≈12KB) | 영역 캡처 | DWM/영역 |  |
| copy_progress.ja.png | 복사 진행 | 진행 토스트/창 | light/ja | help-demo copy | 4062 B (≈3–4KB) | 복사 시퀀스 | DWM | 작은 크기 — 내용 눈확인 |
| copy_progress.ko.png | 복사 진행 | 진행 토스트/창 | light/ko | help-demo copy | 2926 B (≈3–4KB) | 복사 시퀀스 | DWM | 작은 크기 — 내용 눈확인 |
| copy_progress.png | 복사 진행 | 진행 토스트/창 | light/en | help-demo copy | 3191 B (≈3–4KB) | 복사 시퀀스 | DWM | 작은 크기 — 내용 눈확인 |
| dialog_jump.ja.png | 대화상자 점프 | 점프 UI | light/ja | dialog_jump | 2011 B (≈2–4KB) | 점프 캡처 | DWM | 작은 크기 — 내용 눈확인 |
| dialog_jump.ko.png | 대화상자 점프 | 점프 UI | light/ko | dialog_jump | 2011 B (≈2–4KB) | 점프 캡처 | DWM | 작은 크기 — 내용 눈확인 |
| dialog_jump.png | 대화상자 점프 | 점프 UI | light/en | dialog_jump | 3615 B (≈2–4KB) | 점프 캡처 | DWM | 작은 크기 — 내용 눈확인 |
| favorites.ja.png | 즐겨찾기 | 사이드 즐겨찾기 | light/ja | favorites.json (데모 경로) | 496799 B (≈494KB) | favorites cfg | DWM | 실서버 항목 노출 금지 |
| favorites.ko.png | 즐겨찾기 | 사이드 즐겨찾기 | light/ko | favorites.json (데모 경로) | 495682 B (≈494KB) | favorites cfg | DWM | 실서버 항목 노출 금지 |
| favorites.png | 즐겨찾기 | 사이드 즐겨찾기 | light/en | favorites.json (데모 경로) | 494165 B (≈494KB) | favorites cfg | DWM | 실서버 항목 노출 금지 |
| feature_basket.ja.png | 파일 바구니 | Basket 창 | light/ja | basket 데모 | 28664 B (≈24–29KB) | feature 캡처 | DWM |  |
| feature_basket.ko.png | 파일 바구니 | Basket 창 | light/ko | basket 데모 | 27631 B (≈24–29KB) | feature 캡처 | DWM |  |
| feature_basket.png | 파일 바구니 | Basket 창 | light/en | basket 데모 | 24044 B (≈24–29KB) | feature 캡처 | DWM |  |
| feature_excel_readonly_tidy.ja.png | Excel 정리 | Excel tidy | light/ja | 엑셀 데모 통합문서 | 7219 B (≈5–7KB) | excel 캡처 | DWM |  |
| feature_excel_readonly_tidy.ko.png | Excel 정리 | Excel tidy | light/ko | 엑셀 데모 통합문서 | 5385 B (≈5–7KB) | excel 캡처 | DWM |  |
| feature_excel_readonly_tidy.png | Excel 정리 | Excel tidy | light/en | 엑셀 데모 통합문서 | 5837 B (≈5–7KB) | excel 캡처 | DWM |  |
| feature_folder_watch.ja.png | 폴더 감시 | Watch UI | light/ja | watch 데모 | 20318 B (≈16–20KB) | feature 캡처 | DWM |  |
| feature_folder_watch.ko.png | 폴더 감시 | Watch UI | light/ko | watch 데모 | 16147 B (≈16–20KB) | feature 캡처 | DWM |  |
| feature_folder_watch.png | 폴더 감시 | Watch UI | light/en | watch 데모 | 15756 B (≈16–20KB) | feature 캡처 | DWM |  |
| feature_win_shift_v.ja.png | Win+Shift+V | 클립보드/붙여넣기 기능 | light/ja | 데모 | 489434 B (≈487KB) | feature 캡처 | DWM |  |
| feature_win_shift_v.ko.png | Win+Shift+V | 클립보드/붙여넣기 기능 | light/ko | 데모 | 488294 B (≈487KB) | feature 캡처 | DWM |  |
| feature_win_shift_v.png | Win+Shift+V | 클립보드/붙여넣기 기능 | light/en | 데모 | 487245 B (≈487KB) | feature 캡처 | DWM |  |
| help.ja.png | 앱 내 도움말 | Help UI | light/ja | help-demo | 31512 B (≈22–32KB) | 도움말 캡처 | DWM |  |
| help.ko.png | 앱 내 도움말 | Help UI | light/ko | help-demo | 26057 B (≈22–32KB) | 도움말 캡처 | DWM |  |
| help.png | 앱 내 도움말 | Help UI | light/en | help-demo | 22372 B (≈22–32KB) | 도움말 캡처 | DWM |  |
| hero_dark.ja.png | README 히어로(다크) | MaguroMain 2패널 | dark | 동일 데모 | 496211 B (≈493KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | prefers-color-scheme |
| hero_dark.ko.png | README 히어로(다크) | MaguroMain 2패널 | dark | 동일 데모 | 494967 B (≈493KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | prefers-color-scheme |
| hero_dark.png | README 히어로(다크) | MaguroMain 2패널 | dark | 동일 데모 | 493437 B (≈493KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | prefers-color-scheme |
| hero_light.ja.png | README 히어로 | MaguroMain 2패널 | light | readme-demo 폴더 | 519136 B (≈494KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | 다크 히어로와 쌍 |
| hero_light.ko.png | README 히어로 | MaguroMain 2패널 | light | readme-demo 폴더 | 495682 B (≈494KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | 다크 히어로와 쌍 |
| hero_light.png | README 히어로 | MaguroMain 2패널 | light | readme-demo 폴더 | 494165 B (≈494KB) | 메인 캡처 스위트 | DWM bounds+흰배경 | 다크 히어로와 쌍 |
| layout.ja.png | 레이아웃 예시 | 다중 패널 | light/ja | 데모 | 296496 B (≈293KB) | layout cfg | DWM | ko/ja 변형 |
| layout.ko.png | 레이아웃 예시 | 다중 패널 | light/ko | 데모 | 294980 B (≈293KB) | layout cfg | DWM | ko/ja 변형 |
| layout.png | 레이아웃 예시 | 다중 패널 | light/en | 데모 | 293308 B (≈293KB) | layout cfg | DWM | ko/ja 변형 |
| layout_1.png | 레이아웃 그리드 | 1패널 | light/en | 데모 폴더 | 570306 B (≈570KB) | layout cfg | DWM |  |
| layout_2h.png | 레이아웃 그리드 | 2패널 좌우 | light/en | 데모 | 494165 B (≈494KB) | layout cfg | DWM |  |
| layout_2v.png | 레이아웃 그리드 | 2패널 상하 | light/en | 데모 | 463133 B (≈463KB) | layout cfg | DWM |  |
| layout_3.png | 레이아웃 그리드 | 3패널 | light/en | 데모 | 494950 B (≈495KB) | layout cfg | DWM |  |
| layout_3_stack.png | 레이아웃 그리드 | 3 세로 스택 | light/en | 데모 | 289383 B (≈289KB) | layout cfg | DWM |  |
| layout_3col.png | 레이아웃 그리드 | 3열 | light/en | 데모 | 310739 B (≈311KB) | layout cfg | DWM |  |
| layout_4.png | 레이아웃 그리드 | 4패널 | light/en | 데모 | 293308 B (≈293KB) | layout cfg | DWM |  |
| layout_4_tall.png | 레이아웃 그리드 | 4패널 세로형 | light/en | 데모 | 494906 B (≈495KB) | layout cfg | DWM |  |
| layout_5.png | 레이아웃 그리드 | 5패널 | light/en | 데모 | 301449 B (≈301KB) | layout cfg | DWM |  |
| layout_preset_112.png | 레이아웃 프리셋 | 1+1+2 | light/en | 데모 | 317206 B (≈317KB) | layout cfg | DWM |  |
| layout_preset_31.png | 레이아웃 프리셋 | 3+1 | light/en | 데모 | 213156 B (≈213KB) | layout cfg | DWM |  |
| pane_colors.ja.png | 패널 색 | 틴트 패널 | light/ja | pane_colors.json | 496910 B (≈494KB) | pane_colors cfg | DWM |  |
| pane_colors.ko.png | 패널 색 | 틴트 패널 | light/ko | pane_colors.json | 517857 B (≈494KB) | pane_colors cfg | DWM |  |
| pane_colors.png | 패널 색 | 틴트 패널 | light/en | pane_colors.json | 494165 B (≈494KB) | pane_colors cfg | DWM |  |
| pane_tabs.ja.png | 패널 탭 클로즈업 | 패널 탭 바 | light/ja | 데모 | 32331 B (≈32KB) | 영역 캡처 | DWM/영역 |  |
| pane_tabs.ko.png | 패널 탭 클로즈업 | 패널 탭 바 | light/ko | 데모 | 32317 B (≈32KB) | 영역 캡처 | DWM/영역 |  |
| pane_tabs.png | 패널 탭 클로즈업 | 패널 탭 바 | light/en | 데모 | 32227 B (≈32KB) | 영역 캡처 | DWM/영역 |  |
| panedrag_after.png | 패널 드래그 후 | 메인 | light/en | panedrag cfg | 515498 B (≈515KB) | 드래그 시퀀스 | DWM |  |
| panedrag_before.png | 패널 드래그 전 | 메인 | light/en | panedrag cfg | 516325 B (≈516KB) | 드래그 시퀀스 | DWM |  |
| panedrag_dup.png | 패널 복제 드롭 | 메인 | light/en | panedrag_dup | 708290 B (≈708KB) | 드래그 시퀀스 | DWM |  |
| preview.ja.png | 미리보기 패널 | 미리보기 열림 | light/ja | preview.json | 520856 B (≈518KB) | preview cfg | DWM |  |
| preview.ko.png | 미리보기 패널 | 미리보기 열림 | light/ko | preview.json | 519405 B (≈518KB) | preview cfg | DWM |  |
| preview.png | 미리보기 패널 | 미리보기 열림 | light/en | preview.json | 517716 B (≈518KB) | preview cfg | DWM |  |
| preview_peek.ja.png | 피크 미리보기 | 피크 오버레이 | light/ja | preview | 692214 B (≈546–692KB) | peek 캡처 | DWM |  |
| preview_peek.ko.png | 피크 미리보기 | 피크 오버레이 | light/ko | preview | 692213 B (≈546–692KB) | peek 캡처 | DWM |  |
| preview_peek.png | 피크 미리보기 | 피크 오버레이 | light/en | preview | 546238 B (≈546–692KB) | peek 캡처 | DWM |  |
| rename.ja.png | 일괄 이름 바꾸기 | Rename UI | light/ja | rename 데모 파일 | 38256 B (≈34KB) | rename cfg | DWM |  |
| rename.ko.png | 일괄 이름 바꾸기 | Rename UI | light/ko | rename 데모 파일 | 32478 B (≈34KB) | rename cfg | DWM |  |
| rename.png | 일괄 이름 바꾸기 | Rename UI | light/en | rename 데모 파일 | 33644 B (≈34KB) | rename cfg | DWM |  |
| search.ja.png | 하위폴더 검색 | 검색 UI | light/ja | 데모 트리 | 73526 B (≈70KB) | 검색 캡처 | DWM |  |
| search.ko.png | 하위폴더 검색 | 검색 UI | light/ko | 데모 트리 | 71721 B (≈70KB) | 검색 캡처 | DWM |  |
| search.png | 하위폴더 검색 | 검색 UI | light/en | 데모 트리 | 70164 B (≈70KB) | 검색 캡처 | DWM |  |
| servers.ja.png | 서버 연결 추가 | MaguroServer | light/ja | **가짜** Demo Server / sftp.example.com | 12940 B (≈11KB) | capture-servers-fake.ps1 | DWM | 실호스트·실비번 금지 |
| servers.ko.png | 서버 연결 추가 | MaguroServer | light/ko | **가짜** Demo Server / sftp.example.com | 10236 B (≈11KB) | capture-servers-fake.ps1 | DWM | 실호스트·실비번 금지 |
| servers.png | 서버 연결 추가 | MaguroServer | light/en | **가짜** Demo Server / sftp.example.com | 11651 B (≈11KB) | capture-servers-fake.ps1 | DWM | 실호스트·실비번 금지 |
| settings.ja.png | 설정 일반 | MaguroSettings General | light/ja | dialogs 시드 | 45197 B (≈34KB) | Ctrl+, | DWM | RedrawWindow 필수 |
| settings.ko.png | 설정 일반 | MaguroSettings General | light/ko | dialogs 시드 | 32700 B (≈34KB) | Ctrl+, | DWM | RedrawWindow 필수 |
| settings.png | 설정 일반 | MaguroSettings General | light/en | dialogs 시드 | 33616 B (≈34KB) | Ctrl+, | DWM | RedrawWindow 필수 |
| settings_ctxmenu.ja.png | 설정 우클릭 메뉴 | MaguroSettings CTX_PAGE | light/ja | 시드 | 45197 B (≈34KB) | 네비 페이지 8 | DWM | General 복제 아닌지 눈확인 |
| settings_ctxmenu.ko.png | 설정 우클릭 메뉴 | MaguroSettings CTX_PAGE | light/ko | 시드 | 32700 B (≈34KB) | 네비 페이지 8 | DWM | General 복제 아닌지 눈확인 |
| settings_ctxmenu.png | 설정 우클릭 메뉴 | MaguroSettings CTX_PAGE | light/en | 시드 | 33616 B (≈34KB) | 네비 페이지 8 | DWM | General 복제 아닌지 눈확인 |
| settings_general.ja.png | 설정 일반(별칭) | MaguroSettings General | light/ja | 동일 | 45453 B (≈34KB) | Ctrl+, | DWM |  |
| settings_general.ko.png | 설정 일반(별칭) | MaguroSettings General | light/ko | 동일 | 32735 B (≈34KB) | Ctrl+, | DWM |  |
| settings_general.png | 설정 일반(별칭) | MaguroSettings General | light/en | 동일 | 33922 B (≈34KB) | Ctrl+, | DWM |  |
| settings_panes.ja.png | 설정 패널·보기 | MaguroSettings page2 | light/ja | dialogs/fav 시드 | 46865 B (≈34KB) | capture-settings-panes.ps1 | DWM | 클라이언트 클릭으로 페이지 전환 |
| settings_panes.ko.png | 설정 패널·보기 | MaguroSettings page2 | light/ko | dialogs/fav 시드 | 31688 B (≈34KB) | capture-settings-panes.ps1 | DWM | 클라이언트 클릭으로 페이지 전환 |
| settings_panes.png | 설정 패널·보기 | MaguroSettings page2 | light/en | dialogs/fav 시드 | 34309 B (≈34KB) | capture-settings-panes.ps1 | DWM | 클라이언트 클릭으로 페이지 전환 |
| tab_list.png | 탭 목록 | MaguroTabList | light/en | tabs.json | 22972 B (≈23KB) | WM_COMMAND 탭목록 | DWM |  |
| tabs.ja.png | 창 탭·패널 탭 | MaguroMain | light/ja | tabs.json | 56875 B (≈54KB) | tabs cfg | DWM |  |
| tabs.ko.png | 창 탭·패널 탭 | MaguroMain | light/ko | tabs.json | 55452 B (≈54KB) | tabs cfg | DWM |  |
| tabs.png | 창 탭·패널 탭 | MaguroMain | light/en | tabs.json | 53804 B (≈54KB) | tabs cfg | DWM |  |
| workspaces.ja.png | 워크스페이스 | 슬롯 UI | light/ja | workspaces 시드 | 2964 B (≈2–3KB) | 워크스페이스 캡처 | DWM | 작은 크기 — 내용 눈확인 |
| workspaces.ko.png | 워크스페이스 | 슬롯 UI | light/ko | workspaces 시드 | 2564 B (≈2–3KB) | 워크스페이스 캡처 | DWM | 작은 크기 — 내용 눈확인 |
| workspaces.png | 워크스페이스 | 슬롯 UI | light/en | workspaces 시드 | 2602 B (≈2–3KB) | 워크스페이스 캡처 | DWM | 작은 크기 — 내용 눈확인 |

## 환경 변수 예시

`powershell
 = "C:\\path\\to\\maguro-app"
  = "C:\\path\\to\\readme-cfgs"
 = "C:\\path\\to\\shots"
powershell -NoProfile -ExecutionPolicy Bypass -File docs\\capture\\capture-servers-fake.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs\\capture\\capture-settings-panes.ps1
`

## 변경 이력 (관련)

- **2026-11-04:** servers*.png 를 가짜 값(sftp.example.com / Demo Server / 빈 비밀번호)으로 재촬영. settings_panes*.png 를 Panes 페이지·RedrawWindow 로 재촬영. docs/capture/* 추가.

