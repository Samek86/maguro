# Maguro 광고 영상 녹화 / Promo video recording

한 번의 명령으로 EN·JA 원본 클립(60초 세로 광고의 소재)을 다시 찍습니다.
Next time, just run the command below — no need to rewrite scripts.

## 명령 / Command

```bat
py -3.10 docs\video\record\runall.py --lang all --exe C:\path\to\maguro.exe
```

- `--lang en` 또는 `--lang ja` 로 한쪽만 찍을 수 있습니다. `--lang all` 은 EN 다음 JA.
- `--out DIR` 으로 저장 위치를 바꿀 수 있습니다 (기본: `%TEMP%\MaguroAdOut\<lang>`).
- `--only s04,s07` 처럼 클립 일부만 다시 찍을 수 있습니다. 나중에 쓸 데모 파일이 필요하면 `s04` 뒤에 `SEED` 를 자동으로 넣습니다.
- `--idle-seconds 180` (기본 3분) — Jun 님이 이 시간 이상 자리를 비울 때까지 기다립니다.
- `--probe` 는 녹화 없이 창·버튼 위치만 확인하고 스크린샷을 남깁니다.

Python 3.10 (Pillow) 을 쓰세요. 3.13 에는 Pillow 가 없을 수 있습니다.

## 준비 / Requirements

- Windows, 화면 2560×1440, 96 DPI (100%)
- Python 3.10+ 와 Pillow (`py -3.10 -m pip install Pillow`)
- PATH 에 `ffmpeg`
- 실행 파일은 **휴대용 복사본**으로만 켜집니다 (`portable.txt`). 실제 `%APPDATA%\Maguro` 설정은 백업 후 그대로 복원합니다.

## 동작 / What it does

1. Jun 님이 `--idle-seconds` (기본 3분) 이상 자리를 비울 때까지 기다립니다. 녹화 중에 마우스·키보드를 쓰면 그 클립을 중단하고, 다시 그만큼 지난 뒤 재시도합니다. 주입된 입력(`LLMHF_INJECTED`)은 무시합니다.
2. 바탕화면을 검은색으로 바꾸고 다른 창을 최소화합니다 (끝나면 복원).
3. 데모 폴더를 `%TEMP%` 에 만들고 `M:\` 로 subst 합니다 (끝나면 제거). `G:\` 는 건드리지 않습니다.
4. 클립 10개와 정지 화면 2장을 찍습니다. 파일 이름·창 크기·ffmpeg 설정(gdigrab, libx264 crf 16, yuv420p, `draw_mouse 1`)은 2026-11-10 원본과 같습니다.
5. `*.events.txt` 와 `MANIFEST.draft.md` 를 남깁니다. 프레임에서 다시 잰 시각은 녹화 후에 확인하세요.

## 언어 / Languages

영어 세트는 Windows(Explorer) 가 한국어인 채 Maguro 만 영어입니다.

일본어 세트는 Maguro UI 와 데모 파일 이름을 일본어로 둡니다. 스크립트는 사용자 언어 목록의 맨 앞에 `ja` 를 잠시 두고, 프로세스에 `SetProcessPreferredUILanguages` 를 걸며, `Set-WinUILanguageOverride ja-JP` 를 잠깐 켭니다. **로그아웃은 하지 않습니다.** 끝나면 언어 목록과 override 를 복원합니다.

**Explorer 가 그리는 문자열**(열 이름, 복사 진행 창, 표준 오른쪽 클릭 메뉴)은 로그인 세션의 표시 언어를 따릅니다. override 와 explorer.exe 재시작만으로는 바뀌지 않았고, 세션을 다시 열어야 합니다. 이 PC(표시 언어 한국어)에서 2026-11-13 JA 세트는 Explorer 크롬이 한국어로 남았습니다. Maguro 자체 토스트·메뉴·즐겨찾기는 일본어입니다.

## 안전 / Safety

- `C:\WORK\Maguro`, `grids.ini`, `%APPDATA%\grids`, `C:\WORK\Maguro-latest` 는 건드리지 않습니다.
- `%APPDATA%\Maguro` 는 시작 전에 백업하고, 끝나면 복원한 뒤 `maguro.json` SHA-256 을 확인합니다.
- 실제 HKCU 의 기본 폴더 앱 설정은 바꾸지 않습니다.
- `G:\` subst 는 건드리지 않습니다. 이 스크립트는 `M:\` 만 씁니다.

## 클립 / Clips

`01-pane-split` … `09-hotkey`, `09-hotkey-fullscreen`, `still-4pane-light.png`, `still-4pane-dark.png`, `assets/`.

창 클립은 946×1192 @ (7, 0). `05` 는 1752×1292 @ (808, 102) (토스트가 들어가도록). `08` 은 1572×1192 (기어 메뉴). `09-hotkey-fullscreen` 은 2560×1440 @ 30 fps.

## 아이콘·메뉴 위치 / Locators

파일 아이콘과 오른쪽 클릭·기어 메뉴의 항목은 이름으로 찾습니다. 아이콘 크기가 바뀌거나 Explorer 메뉴에 Adobe 같은 항목이 더 있어도, 기어 서브메뉴가 더 넓어도 같은 스크립트로 찍을 수 있습니다.

- 파일: `uiafind.cs` 를 처음 실행 때 `csc` 로 컴파일한 뒤 UI Automation 으로 찾습니다 (`uiafind.exe` 는 커밋하지 않습니다). 영역으로 범위를 좁혀 같은 이름(예: hero-shot)이 여러 창에 있어도 맞춥니다.
- 메뉴: Win32 `MN_GETHMENU` + `GetMenuItemRect` 로 찾습니다 (64비트 안전).
- 파일 바구니 버튼: 캡션 텍스트로 `child_center()` 합니다.

`--only s08` 로 다크 모드만 다시 찍을 수 있습니다.
