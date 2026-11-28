//! Zip-slip safe relative names. An entry that is not a normal file under the
//! destination is skipped, not written.

/// Windows device names, with or without an extension (`CON`, `con.txt`, `NUL`).
fn reserved(stem: &str) -> bool {
    let stem = stem.trim();
    if stem.is_empty() {
        return true;
    }
    let base = stem.split('.').next().unwrap_or(stem);
    let u = base.to_ascii_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((u.starts_with("COM") || u.starts_with("LPT")) && u.len() == 4 && u.as_bytes()[3].is_ascii_digit() && u.as_bytes()[3] != b'0')
}

fn bad_component(part: &str) -> bool {
    if part.is_empty() || part == "." || part == ".." {
        return true;
    }
    if part.ends_with([' ', '.']) {
        return true;
    }
    if part.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/')) {
        return true;
    }
    reserved(part)
}

/// `name` as a relative path using `\` separators.
/// `None` when the name is absolute, escapes the archive, or is not a legal
/// Windows file name.
pub(crate) fn sanitize(name: &str) -> Option<String> {
    let name = name.trim().trim_matches('\0');
    if name.is_empty() {
        return None;
    }
    let bytes = name.as_bytes();
    if name.starts_with(['\\', '/']) || name.starts_with("\\\\") {
        return None;
    }
    // `C:`, `C:\...`, `C:/...`
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut out = Vec::new();
    for part in name.split(['\\', '/']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if bad_component(part) {
            return None;
        }
        out.push(part);
    }
    if out.is_empty() {
        return None;
    }
    Some(out.join("\\"))
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn keeps_a_normal_nested_name() {
        assert_eq!(sanitize("sub/dir/hello.txt"), Some(r"sub\dir\hello.txt".into()));
        assert_eq!(sanitize(r"sub\dir\hello.txt"), Some(r"sub\dir\hello.txt".into()));
        assert_eq!(sanitize("./a/./b.txt"), Some(r"a\b.txt".into()));
    }

    #[test]
    fn rejects_zip_slip_and_absolute_names() {
        assert_eq!(sanitize("../secret.txt"), None);
        assert_eq!(sanitize(r"..\secret.txt"), None);
        assert_eq!(sanitize("foo/../../etc/passwd"), None);
        assert_eq!(sanitize("/etc/passwd"), None);
        assert_eq!(sanitize(r"\Windows\system.ini"), None);
        assert_eq!(sanitize(r"C:\Windows\system.ini"), None);
        assert_eq!(sanitize("C:foo.txt"), None);
        assert_eq!(sanitize(r"\\server\share\a"), None);
    }

    #[test]
    fn rejects_reserved_and_illegal_windows_names() {
        assert_eq!(sanitize("CON"), None);
        assert_eq!(sanitize("con.txt"), None);
        assert_eq!(sanitize("sub/NUL"), None);
        assert_eq!(sanitize("COM1"), None);
        assert_eq!(sanitize("LPT9.dat"), None);
        assert_eq!(sanitize("ok/COM0.txt"), Some(r"ok\COM0.txt".into()));
        assert_eq!(sanitize("a:b.txt"), None);
        assert_eq!(sanitize("has|pipe.txt"), None);
        assert_eq!(sanitize("star*.txt"), None);
        assert_eq!(sanitize("trail. "), None);
        assert_eq!(sanitize("trail."), None);
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("."), None);
    }
}
