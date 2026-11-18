//! Enumerable LineIcon catalog for the toolbar preset pack.
//! Keys match the approved list. Korean names are data for a later edit window.
//! Nothing here draws a label under an icon.

use super::LineIcon;

/// One-line credit for the About window. No URL: the About body must stay a bare address.
pub(crate) const ICON_CREDIT: &str = "Line icons redrawn from IconMCP shapes. MIT, Copyright (c) 2026 UI8.";

pub(crate) struct IconDef {
    pub key: &'static str,
    pub kind: LineIcon,
    /// Korean name from the approved list. Not painted on the toolbar.
    pub name_ko: &'static str,
}

const ICONS: &[IconDef] = &[
    IconDef { key: "new_file", kind: LineIcon::NewFile, name_ko: "새 파일" },
    IconDef { key: "del", kind: LineIcon::Del, name_ko: "삭제" },
    IconDef { key: "rename", kind: LineIcon::Rename, name_ko: "이름 바꾸기" },
    IconDef { key: "duplicate", kind: LineIcon::Duplicate, name_ko: "복제" },
    IconDef { key: "move", kind: LineIcon::Move, name_ko: "이동" },
    IconDef { key: "properties", kind: LineIcon::Properties, name_ko: "속성" },
    IconDef { key: "edit", kind: LineIcon::Edit, name_ko: "편집" },
    IconDef { key: "save_as", kind: LineIcon::SaveAs, name_ko: "다른 이름으로 저장" },
    IconDef { key: "print", kind: LineIcon::Print, name_ko: "인쇄" },
    IconDef { key: "restore", kind: LineIcon::Restore, name_ko: "복원" },
    IconDef { key: "empty_trash", kind: LineIcon::EmptyTrash, name_ko: "휴지통 비우기" },
    IconDef { key: "shred", kind: LineIcon::Shred, name_ko: "완전 삭제" },
    IconDef { key: "recent", kind: LineIcon::Recent, name_ko: "최근 파일" },
    IconDef { key: "compare", kind: LineIcon::Compare, name_ko: "비교" },
    IconDef { key: "hidden", kind: LineIcon::Hidden, name_ko: "숨김 파일" },
    IconDef { key: "file_link", kind: LineIcon::FileLink, name_ko: "파일 링크" },
    IconDef { key: "find_dupes", kind: LineIcon::FindDupes, name_ko: "중복 찾기" },
    IconDef { key: "group", kind: LineIcon::Group, name_ko: "새 그룹" },
    IconDef { key: "open_folder", kind: LineIcon::OpenFolder, name_ko: "폴더 열기" },
    IconDef { key: "folder_tree", kind: LineIcon::FolderTree, name_ko: "폴더 트리" },
    IconDef { key: "root", kind: LineIcon::Root, name_ko: "루트" },
    IconDef { key: "network_folder", kind: LineIcon::NetworkFolder, name_ko: "네트워크 폴더" },
    IconDef { key: "symlink_folder", kind: LineIcon::SymlinkFolder, name_ko: "링크된 폴더" },
    IconDef { key: "folder_size", kind: LineIcon::FolderSize, name_ko: "폴더 크기" },
    IconDef { key: "cut", kind: LineIcon::Cut, name_ko: "잘라내기" },
    IconDef { key: "copy", kind: LineIcon::Copy, name_ko: "복사" },
    IconDef { key: "paste", kind: LineIcon::Paste, name_ko: "붙여넣기" },
    IconDef { key: "clip_history", kind: LineIcon::ClipHistory, name_ko: "클립보드 기록" },
    IconDef { key: "view", kind: LineIcon::View, name_ko: "표시" },
    IconDef { key: "view_list", kind: LineIcon::ViewList, name_ko: "목록" },
    IconDef { key: "view_icons", kind: LineIcon::ViewIcons, name_ko: "아이콘" },
    IconDef { key: "preview", kind: LineIcon::Preview, name_ko: "미리보기" },
    IconDef { key: "layout", kind: LineIcon::Layout, name_ko: "배치" },
    IconDef { key: "sidebar_left", kind: LineIcon::SidebarLeft, name_ko: "왼쪽 패널" },
    IconDef { key: "sidebar_right", kind: LineIcon::SidebarRight, name_ko: "오른쪽 패널" },
    IconDef { key: "fullscreen", kind: LineIcon::Fullscreen, name_ko: "전체 화면" },
    IconDef { key: "zoom_in", kind: LineIcon::ZoomIn, name_ko: "확대" },
    IconDef { key: "zoom_out", kind: LineIcon::ZoomOut, name_ko: "축소" },
    IconDef { key: "theme_light", kind: LineIcon::ThemeLight, name_ko: "밝은 테마" },
    IconDef { key: "theme_dark", kind: LineIcon::ThemeDark, name_ko: "어두운 테마" },
    IconDef { key: "back", kind: LineIcon::Back, name_ko: "뒤로" },
    IconDef { key: "fwd", kind: LineIcon::Fwd, name_ko: "앞으로" },
    IconDef { key: "up", kind: LineIcon::Up, name_ko: "위로" },
    IconDef { key: "down", kind: LineIcon::Down, name_ko: "아래로" },
    IconDef { key: "refresh", kind: LineIcon::Refresh, name_ko: "새로고침" },
    IconDef { key: "home", kind: LineIcon::Home, name_ko: "홈" },
    IconDef { key: "breadcrumb", kind: LineIcon::Breadcrumb, name_ko: "경로" },
    IconDef { key: "drive", kind: LineIcon::Drive, name_ko: "드라이브" },
    IconDef { key: "eject", kind: LineIcon::Eject, name_ko: "꺼내기" },
    IconDef { key: "server", kind: LineIcon::Server, name_ko: "서버" },
    IconDef { key: "disk_usage", kind: LineIcon::DiskUsage, name_ko: "디스크 사용량" },
    IconDef { key: "select_all", kind: LineIcon::SelectAll, name_ko: "모두 선택" },
    IconDef { key: "invert", kind: LineIcon::Invert, name_ko: "선택 반전" },
    IconDef { key: "deselect", kind: LineIcon::Deselect, name_ko: "선택 해제" },
    IconDef { key: "search", kind: LineIcon::Search, name_ko: "검색" },
    IconDef { key: "filter", kind: LineIcon::Filter, name_ko: "필터" },
    IconDef { key: "filter_clear", kind: LineIcon::FilterClear, name_ko: "필터 지우기" },
    IconDef { key: "glob", kind: LineIcon::Glob, name_ko: "와일드카드" },
    IconDef { key: "sort_name", kind: LineIcon::SortName, name_ko: "이름순" },
    IconDef { key: "sort_date", kind: LineIcon::SortDate, name_ko: "날짜순" },
    IconDef { key: "sort_size", kind: LineIcon::SortSize, name_ko: "크기순" },
    IconDef { key: "sort_asc", kind: LineIcon::SortAsc, name_ko: "오름차순" },
    IconDef { key: "sort_desc", kind: LineIcon::SortDesc, name_ko: "내림차순" },
    IconDef { key: "list", kind: LineIcon::List, name_ko: "탭 목록" },
    IconDef { key: "add", kind: LineIcon::Add, name_ko: "추가" },
    IconDef { key: "new_tab", kind: LineIcon::NewTab, name_ko: "새 탭" },
    IconDef { key: "close", kind: LineIcon::Close, name_ko: "닫기" },
    IconDef { key: "pane_close", kind: LineIcon::PaneClose, name_ko: "패널 닫기" },
    IconDef { key: "open_in", kind: LineIcon::OpenIn, name_ko: "다른 패널에서 열기" },
    IconDef { key: "split_side", kind: LineIcon::SplitSide, name_ko: "좌우 분할" },
    IconDef { key: "split_stack", kind: LineIcon::SplitStack, name_ko: "상하 분할" },
    IconDef { key: "star", kind: LineIcon::Star, name_ko: "즐겨찾기" },
    IconDef { key: "star_filled", kind: LineIcon::StarFilled, name_ko: "즐겨찾기됨" },
    IconDef { key: "star_add", kind: LineIcon::StarAdd, name_ko: "즐겨찾기에 추가" },
    IconDef { key: "important", kind: LineIcon::Important, name_ko: "중요 표시" },
    IconDef { key: "bookmark", kind: LineIcon::Bookmark, name_ko: "책갈피" },
    IconDef { key: "tag", kind: LineIcon::Tag, name_ko: "태그" },
    IconDef { key: "tab_color", kind: LineIcon::TabColor, name_ko: "탭 색" },
    IconDef { key: "pane_color", kind: LineIcon::PaneColor, name_ko: "패널 색" },
    IconDef { key: "pin_file", kind: LineIcon::PinFile, name_ko: "고정" },
    IconDef { key: "compress", kind: LineIcon::Compress, name_ko: "압축" },
    IconDef { key: "extract", kind: LineIcon::Extract, name_ko: "압축 풀기" },
    IconDef { key: "terminal", kind: LineIcon::Terminal, name_ko: "터미널" },
    IconDef { key: "terminal_admin", kind: LineIcon::TerminalAdmin, name_ko: "관리자 터미널" },
    IconDef { key: "gear", kind: LineIcon::Gear, name_ko: "설정" },
    IconDef { key: "save", kind: LineIcon::Save, name_ko: "이 탭 저장" },
    IconDef { key: "load", kind: LineIcon::Load, name_ko: "세션 열기" },
    IconDef { key: "help", kind: LineIcon::Help, name_ko: "도움말" },
    IconDef { key: "about", kind: LineIcon::About, name_ko: "정보" },
    IconDef { key: "shortcuts", kind: LineIcon::Shortcuts, name_ko: "단축키" },
    IconDef { key: "undo", kind: LineIcon::Undo, name_ko: "실행 취소" },
    IconDef { key: "redo", kind: LineIcon::Redo, name_ko: "다시 실행" },
    IconDef { key: "more", kind: LineIcon::More, name_ko: "더 보기" },
    IconDef { key: "basket", kind: LineIcon::Basket, name_ko: "파일 바구니" },
    IconDef { key: "watch", kind: LineIcon::Watch, name_ko: "폴더 감시" },
    IconDef { key: "lock", kind: LineIcon::Lock, name_ko: "잠금" },
    IconDef { key: "unlock", kind: LineIcon::Unlock, name_ko: "잠금 해제" },
    IconDef { key: "share", kind: LineIcon::Share, name_ko: "공유" },
    IconDef { key: "open_external", kind: LineIcon::OpenExternal, name_ko: "외부에서 열기" },
    IconDef { key: "download", kind: LineIcon::Download, name_ko: "다운로드" },
    IconDef { key: "upload", kind: LineIcon::Upload, name_ko: "업로드" },
    IconDef { key: "sync", kind: LineIcon::Sync, name_ko: "동기화" },
    IconDef { key: "permissions", kind: LineIcon::Permissions, name_ko: "권한" },
];

/// Every catalog entry, in the approved list order.
pub(crate) fn icons() -> &'static [IconDef] {
    ICONS
}

pub(crate) fn icon_by_key(key: &str) -> Option<&'static IconDef> {
    ICONS.iter().find(|e| e.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sicon::{line_icon_coverage, line_icon_px, line_icon_stroke};

    /// FNV-1a 64 of the coverage bytes. Recorded from `e13ef04` before the catalog move.
    fn fnv(data: &[u8]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn cov(dpi: i32, kind: LineIcon) -> (i32, Vec<u8>) {
        let n = line_icon_px(dpi);
        (n, line_icon_coverage(n, dpi, kind))
    }

    const APPROVED: [&str; 103] = [
        "new_file", "del", "rename", "duplicate", "move", "properties", "edit", "save_as", "print", "restore", "empty_trash", "shred", "recent", "compare", "hidden", "file_link", "find_dupes",
        "group", "open_folder", "folder_tree", "root", "network_folder", "symlink_folder", "folder_size",
        "cut", "copy", "paste", "clip_history",
        "view", "view_list", "view_icons", "preview", "layout", "sidebar_left", "sidebar_right", "fullscreen", "zoom_in", "zoom_out", "theme_light", "theme_dark",
        "back", "fwd", "up", "down", "refresh", "home", "breadcrumb", "drive", "eject", "server", "disk_usage",
        "select_all", "invert", "deselect",
        "search", "filter", "filter_clear", "glob",
        "sort_name", "sort_date", "sort_size", "sort_asc", "sort_desc",
        "list", "add", "new_tab", "close", "pane_close", "open_in", "split_side", "split_stack",
        "star", "star_filled", "star_add", "important", "bookmark", "tag", "tab_color", "pane_color", "pin_file",
        "compress", "extract",
        "terminal", "terminal_admin",
        "gear", "save", "load", "help", "about", "shortcuts", "undo", "redo", "more",
        "basket", "watch", "lock", "unlock", "share", "open_external", "download", "upload", "sync", "permissions",
    ];

    /// (key, fnv, byte sum) at 96 DPI, then the same triple at 144 DPI.
    const STRIP_96: [(&str, u64, u32); 19] = [
        ("gear", 0xffe4637b95f6c9af, 25548),
        ("list", 0xe055642cc8afad02, 13917),
        ("layout", 0x186ddf92513fa36e, 22135),
        ("add", 0xc9a66a2e4f3cd7d6, 8539),
        ("group", 0x97b06349e24634d0, 19625),
        ("up", 0xaea516d301a6406a, 9147),
        ("down", 0x67fce877e4f4e212, 9147),
        ("rename", 0xfda01863169a65b0, 17801),
        ("del", 0x45a172cb5bfca217, 18846),
        ("back", 0x5b8a25919dc5fce2, 9147),
        ("fwd", 0xd5c129ab6cfa9d72, 9147),
        ("open_in", 0x96993983334b37e7, 14350),
        ("save", 0xe4aded864085d483, 22126),
        ("load", 0xa9cddd7449e03b13, 22536),
        ("basket", 0x6477599f36f56444, 16901),
        ("watch", 0xb76d61ef7948bf76, 15063),
        ("star", 0xe015f0f8767e54e1, 15792),
        ("star_filled", 0x55a040237c51fb5a, 31899),
        ("star_add", 0x76537e77bac0d54b, 18812),
    ];
    const STRIP_144: [(&str, u64, u32); 19] = [
        ("gear", 0x7851bbb62ba6e289, 68948),
        ("list", 0x031daa2b69c5319d, 35016),
        ("layout", 0xb618a46d85a4256d, 55056),
        ("add", 0x53653cfbac41d765, 21408),
        ("group", 0x9998f66171308c16, 50245),
        ("up", 0x012563ad27b08d81, 22884),
        ("down", 0x929fb145c4dbd119, 22884),
        ("rename", 0x7471ae77789e2389, 46484),
        ("del", 0xbd3a6bbfc86b310b, 48426),
        ("back", 0xa60834c5b551e369, 22884),
        ("fwd", 0xeb450c670d8ea541, 22884),
        ("open_in", 0x9d78d7f11813fe1d, 35212),
        ("save", 0x00573b52bbc3dc8b, 56748),
        ("load", 0xcab442ea32d52939, 57702),
        ("basket", 0x50818e3c4a44fddd, 43324),
        ("watch", 0xc6f5341c721e34bd, 36664),
        ("star", 0x008cc56ab85db315, 39520),
        ("star_filled", 0xe446e1c07f341b2b, 73978),
        ("star_add", 0x60192ec9ad40c16f, 47150),
    ];

    #[test]
    fn catalog_lists_every_approved_key_once() {
        assert_eq!(icons().len(), 103);
        assert_eq!(APPROVED.len(), 103);
        let got: Vec<&str> = icons().iter().map(|e| e.key).collect();
        assert_eq!(got, APPROVED);
        let mut seen = std::collections::HashSet::new();
        let mut kinds = std::collections::HashSet::new();
        for e in icons() {
            assert!(seen.insert(e.key), "duplicate key {}", e.key);
            assert!(kinds.insert(e.kind), "duplicate kind {}", e.key);
            assert!(!e.name_ko.is_empty(), "{}", e.key);
            assert_eq!(icon_by_key(e.key).map(|x| x.kind), Some(e.kind));
        }
        assert!(icon_by_key("missing").is_none());
        assert!(icon_by_key("").is_none());
    }

    #[test]
    fn strip_icons_keep_their_pre_move_coverage() {
        for dpi in [(96, &STRIP_96), (144, &STRIP_144)] {
            for &(key, hash, sum) in dpi.1 {
                let kind = icon_by_key(key).unwrap_or_else(|| panic!("missing {key}")).kind;
                let (n, c) = cov(dpi.0, kind);
                let got_sum: u32 = c.iter().map(|&b| b as u32).sum();
                assert_eq!(c.len(), (n * n) as usize, "{key}");
                assert_eq!(fnv(&c), hash, "{dpi:?} {key} coverage changed");
                assert_eq!(got_sum, sum, "{dpi:?} {key} coverage sum changed");
            }
        }
    }

    #[test]
    fn every_catalog_icon_uses_the_line_icon_box_and_keeps_a_clear_edge() {
        assert!((line_icon_stroke(96) - 1.35).abs() < 1e-6);
        for dpi in [96, 144] {
            let mut seen = Vec::new();
            for e in icons() {
                let (n, c) = cov(dpi, e.kind);
                assert_eq!(n, line_icon_px(dpi), "{}", e.key);
                assert_eq!(c.len(), (n * n) as usize, "{}", e.key);
                assert!(c.iter().any(|&a| a == 255), "{} at {dpi}: solid stroke core", e.key);
                for k in 0..n {
                    for (x, y) in [(k, 0), (0, k), (k, n - 1), (n - 1, k)] {
                        let a = c[(y * n + x) as usize];
                        assert!(a < 96, "{} at {dpi} edge {x},{y} is {a}", e.key);
                    }
                }
                seen.push((e.key, c));
            }
            for i in 0..seen.len() {
                for j in (i + 1)..seen.len() {
                    assert_ne!(seen[i].1, seen[j].1, "{} vs {}", seen[i].0, seen[j].0);
                }
            }
        }
    }

    #[test]
    fn new_icons_share_the_strip_stroke() {
        let (n, add) = cov(96, LineIcon::Add);
        let (_, down) = cov(96, LineIcon::Download);
        assert_eq!(n, 17);
        let at = |c: &[u8], x, y| c[(y * n + x) as usize];
        // Column 6 misses Add's vertical stroke and the download arrow shaft, so both
        // samples are one round 1.35 stroke on a pixel row.
        assert_eq!(at(&add, 6, 8), 255, "add bar");
        assert_eq!(at(&down, 6, 13), 255, "tray core");
        assert_eq!(at(&down, 6, 12), at(&add, 6, 7), "fringe above");
        assert_eq!(at(&down, 6, 14), at(&add, 6, 9), "fringe below");
        assert_eq!(line_icon_px(96), 17);
        assert_eq!(line_icon_px(144), 26);
    }

    #[test]
    fn credit_line_names_iconmcp_without_a_url() {
        assert!(ICON_CREDIT.contains("IconMCP"));
        assert!(ICON_CREDIT.contains("MIT"));
        assert!(ICON_CREDIT.contains("Copyright (c) 2026 UI8"));
        assert!(!ICON_CREDIT.contains("http"));
        assert!(!ICON_CREDIT.contains('?'));
    }
}
