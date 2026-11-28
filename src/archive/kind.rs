//! Which file names this feature treats as archives. Zip is intentionally absent:
//! Explorer already opens a zip as a folder.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowseKind {
    SevenZ,
    Tar,
    TarGz,
    TarXz,
    TarBz2,
    Iso,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiskKind {
    Iso,
    Vhd,
    Vhdx,
}

fn file_name(path: &str) -> &str {
    path.trim().trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `name` ends with `suffix` and has at least one character before it.
/// `notes.tar.gz` matches `.tar.gz`. Comparison ignores ASCII case.
fn ends_with_ext(name: &str, suffix: &str) -> bool {
    let name_b = name.as_bytes();
    let suf = suffix.as_bytes();
    if name_b.len() <= suf.len() {
        return false;
    }
    if !name_b[name_b.len() - suf.len()..].eq_ignore_ascii_case(suf) {
        return false;
    }
    true
}

/// Archive Maguro opens as a folder. `.zip` is not one.
pub(crate) fn browse_kind(path: &str) -> Option<BrowseKind> {
    let name = file_name(path);
    if ends_with_ext(name, ".tar.gz") || ends_with_ext(name, ".tgz") {
        return Some(BrowseKind::TarGz);
    }
    if ends_with_ext(name, ".tar.xz") || ends_with_ext(name, ".txz") {
        return Some(BrowseKind::TarXz);
    }
    if ends_with_ext(name, ".tar.bz2") || ends_with_ext(name, ".tbz2") || ends_with_ext(name, ".tbz") {
        return Some(BrowseKind::TarBz2);
    }
    if ends_with_ext(name, ".tar") {
        return Some(BrowseKind::Tar);
    }
    if ends_with_ext(name, ".7z") {
        return Some(BrowseKind::SevenZ);
    }
    if ends_with_ext(name, ".iso") {
        return Some(BrowseKind::Iso);
    }
    None
}

/// Disk image the Virtual Disk API can attach. `.img` is tried as an ISO.
pub(crate) fn disk_kind(path: &str) -> Option<DiskKind> {
    let name = file_name(path);
    if ends_with_ext(name, ".iso") || ends_with_ext(name, ".img") {
        return Some(DiskKind::Iso);
    }
    if ends_with_ext(name, ".vhdx") {
        return Some(DiskKind::Vhdx);
    }
    if ends_with_ext(name, ".vhd") {
        return Some(DiskKind::Vhd);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_kinds_skip_zip() {
        assert_eq!(browse_kind(r"D:\a\foo.7z"), Some(BrowseKind::SevenZ));
        assert_eq!(browse_kind(r"D:\a\FOO.TAR.GZ"), Some(BrowseKind::TarGz));
        assert_eq!(browse_kind(r"D:\a\x.tgz"), Some(BrowseKind::TarGz));
        assert_eq!(browse_kind(r"D:\a\x.tar.xz"), Some(BrowseKind::TarXz));
        assert_eq!(browse_kind(r"D:\a\x.txz"), Some(BrowseKind::TarXz));
        assert_eq!(browse_kind(r"D:\a\x.tar.bz2"), Some(BrowseKind::TarBz2));
        assert_eq!(browse_kind(r"D:\a\x.tbz2"), Some(BrowseKind::TarBz2));
        assert_eq!(browse_kind(r"D:\a\x.tar"), Some(BrowseKind::Tar));
        assert_eq!(browse_kind(r"D:\a\cd.iso"), Some(BrowseKind::Iso));
        assert_eq!(browse_kind(r"D:\a\notes.zip"), None);
        assert_eq!(browse_kind(r"D:\a\notes.ZIP"), None);
        assert_eq!(browse_kind(r"D:\a\picture.png"), None);
        // `.tar.gz` is not a plain tar.
        assert_ne!(browse_kind(r"D:\a\x.tar.gz"), Some(BrowseKind::Tar));
    }

    #[test]
    fn disk_kinds() {
        assert_eq!(disk_kind(r"E:\cd.iso"), Some(DiskKind::Iso));
        assert_eq!(disk_kind(r"E:\raw.img"), Some(DiskKind::Iso));
        assert_eq!(disk_kind(r"E:\disk.vhd"), Some(DiskKind::Vhd));
        assert_eq!(disk_kind(r"E:\disk.vhdx"), Some(DiskKind::Vhdx));
        assert_eq!(disk_kind(r"E:\disk.vhd.txt"), None);
        assert_eq!(disk_kind(r"E:\notes.zip"), None);
    }
}
