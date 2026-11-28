//! Extract a supported archive into a real directory.
//!
//! Entry names go through [`super::names::sanitize`]. Symlinks and hard links
//! are skipped. The running total of bytes written stops at `limit`. Files are
//! marked read-only so the cache does not look like a place to edit.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::iso;
use super::kind::{browse_kind, BrowseKind};
use super::names::sanitize;

#[derive(Debug)]
pub(crate) enum ExtractError {
    Io(String),
    Bad(String),
    Encrypted,
    TooLarge { bytes: u64, limit: u64 },
    Cancelled,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Io(s) | ExtractError::Bad(s) => write!(f, "{s}"),
            ExtractError::Encrypted => write!(f, "encrypted"),
            ExtractError::TooLarge { bytes, limit } => write!(f, "size {bytes} exceeds {limit}"),
            ExtractError::Cancelled => write!(f, "cancelled"),
        }
    }
}

pub(crate) fn mark_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(path, perms);
    }
}

pub(crate) fn copy_limited(
    reader: &mut impl Read,
    out: &mut impl Write,
    written: &mut u64,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<(), ExtractError> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExtractError::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| ExtractError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(());
        }
        let next = written.saturating_add(n as u64);
        if next > limit {
            return Err(ExtractError::TooLarge { bytes: next, limit });
        }
        out.write_all(&buf[..n]).map_err(|e| ExtractError::Io(e.to_string()))?;
        *written = next;
    }
}

fn place(dest: &Path, rel: &str) -> Result<PathBuf, ExtractError> {
    let mut path = dest.to_path_buf();
    for part in rel.split('\\') {
        if part.is_empty() {
            continue;
        }
        path.push(part);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ExtractError::Io(e.to_string()))?;
    }
    Ok(path)
}

fn write_file(dest: &Path, rel: &str, reader: &mut impl Read, written: &mut u64, limit: u64, cancel: &AtomicBool) -> Result<(), ExtractError> {
    let path = place(dest, rel)?;
    let mut out = File::create(&path).map_err(|e| ExtractError::Io(e.to_string()))?;
    copy_limited(reader, &mut out, written, limit, cancel)?;
    drop(out);
    mark_readonly(&path);
    Ok(())
}

fn extract_tar(reader: impl Read, dest: &Path, limit: u64, cancel: &AtomicBool) -> Result<u64, ExtractError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|e| ExtractError::Bad(e.to_string()))?;
    let mut written = 0u64;
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExtractError::Cancelled);
        }
        let mut entry = entry.map_err(|e| ExtractError::Bad(e.to_string()))?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() || kind.is_fifo() || kind.is_character_special() || kind.is_block_special() {
            continue;
        }
        let raw = entry.path().map_err(|e| ExtractError::Bad(e.to_string()))?;
        let raw = raw.to_string_lossy();
        let Some(rel) = sanitize(&raw) else { continue };
        if kind.is_dir() || rel.ends_with('\\') {
            let _ = place(dest, &rel)?;
            continue;
        }
        let declared = entry.header().size().unwrap_or(0);
        if written.saturating_add(declared) > limit {
            return Err(ExtractError::TooLarge { bytes: written.saturating_add(declared), limit });
        }
        write_file(dest, &rel, &mut entry, &mut written, limit, cancel)?;
    }
    Ok(written)
}

fn map_7z(err: sevenz_rust2::Error) -> ExtractError {
    match err {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => ExtractError::Encrypted,
        sevenz_rust2::Error::TotalSizeLimitExceeded { max, actual } => ExtractError::TooLarge { bytes: actual, limit: max },
        other => ExtractError::Bad(other.to_string()),
    }
}

fn extract_7z(src: &Path, dest: &Path, limit: u64, cancel: &AtomicBool) -> Result<u64, ExtractError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ExtractError::Cancelled);
    }
    let limits = sevenz_rust2::ExtractLimits::new().max_total_size(limit).max_entries(200_000).max_memory_kb(512 * 1024);
    let file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
    sevenz_rust2::decompress_with_extract_fn_and_limits(file, dest, limits, |entry, reader, path| {
        if cancel.load(Ordering::Relaxed) {
            return Err(sevenz_rust2::Error::Other("cancelled".into()));
        }
        let name = entry.name();
        if sanitize(name).is_none() && !name.is_empty() && name != "/" {
            // Drain so the decoder stays aligned, then skip.
            let _ = std::io::copy(reader, &mut std::io::sink());
            return Ok(true);
        }
        let wrote = sevenz_rust2::default_entry_extract_fn(entry, reader, path)?;
        if path.is_file() {
            mark_readonly(path);
        }
        Ok(wrote)
    })
    .map_err(map_7z)?;
    Ok(dir_bytes(dest))
}

fn dir_bytes(root: &Path) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(meta) = path.metadata() {
                n = n.saturating_add(meta.len());
            }
        }
    }
    n
}

pub(crate) fn extract(src: &Path, dest: &Path, limit: u64, cancel: &AtomicBool) -> Result<u64, ExtractError> {
    let name = src.to_string_lossy();
    let kind = browse_kind(&name).ok_or_else(|| ExtractError::Bad("unsupported archive".into()))?;
    std::fs::create_dir_all(dest).map_err(|e| ExtractError::Io(e.to_string()))?;
    match kind {
        BrowseKind::Iso => iso::extract_file(src, dest, limit, cancel),
        BrowseKind::SevenZ => extract_7z(src, dest, limit, cancel),
        BrowseKind::Tar => {
            let file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
            extract_tar(file, dest, limit, cancel)
        }
        BrowseKind::TarGz => {
            let file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
            extract_tar(flate2::read::GzDecoder::new(file), dest, limit, cancel)
        }
        BrowseKind::TarXz => {
            let file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
            extract_tar(lzma_rust2::XzReader::new(file, false), dest, limit, cancel)
        }
        BrowseKind::TarBz2 => {
            let file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
            extract_tar(bzip2::read::BzDecoder::new(file), dest, limit, cancel)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maguro-arc-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tar_bytes() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("dir/hello.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"hello"[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_path("dir/link.txt").unwrap();
        link.set_link_name("hello.txt").unwrap();
        link.set_size(0);
        link.set_cksum();
        builder.append(&link, std::io::empty()).unwrap();
        builder.into_inner().unwrap()
    }

    fn roundtrip(name: &str, bytes: &[u8]) {
        let dir = temp(name);
        let arch = dir.join(name);
        std::fs::write(&arch, bytes).unwrap();
        let dest = dir.join("out");
        let n = extract(&arch, &dest, 8 * 1024 * 1024, &AtomicBool::new(false)).unwrap();
        assert!(n >= 5, "{name} wrote {n}");
        let hello = dest.join("dir").join("hello.txt");
        assert_eq!(std::fs::read(&hello).unwrap(), b"hello");
        assert!(hello.metadata().unwrap().permissions().readonly());
        assert!(!dest.join("dir").join("link.txt").exists());
        assert!(!dir.join("outside.txt").exists());
        assert!(!dest.join("outside.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_gz_xz_and_bz2_roundtrip() {
        let raw = tar_bytes();
        roundtrip("plain.tar", &raw);

        let mut gz = Vec::new();
        {
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
        }
        roundtrip("plain.tar.gz", &gz);

        let mut xz = Vec::new();
        {
            let opts = lzma_rust2::XzOptions::with_preset(1);
            let mut enc = lzma_rust2::XzWriter::new(&mut xz, opts).unwrap();
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
        }
        roundtrip("plain.tar.xz", &xz);

        let mut bz = Vec::new();
        {
            let mut enc = bzip2::write::BzEncoder::new(&mut bz, bzip2::Compression::fast());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
        }
        roundtrip("plain.tar.bz2", &bz);
    }

    #[test]
    fn sevenz_roundtrip_and_password_maps() {
        let dir = temp("seven");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("dir")).unwrap();
        std::fs::write(src.join("dir").join("hello.txt"), b"hello").unwrap();
        let arch = dir.join("pack.7z");
        sevenz_rust2::compress_to_path(&src, &arch).unwrap();
        let dest = dir.join("out");
        let n = extract(&arch, &dest, 8 * 1024 * 1024, &AtomicBool::new(false)).unwrap();
        assert!(n >= 5);
        assert_eq!(std::fs::read(dest.join("dir").join("hello.txt")).unwrap(), b"hello");
        assert!(dest.join("dir").join("hello.txt").metadata().unwrap().permissions().readonly());
        let err = map_7z(sevenz_rust2::Error::PasswordRequired);
        assert!(matches!(err, ExtractError::Encrypted));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_over_the_size_cap() {
        let dir = temp("cap");
        let arch = dir.join("plain.tar");
        std::fs::write(&arch, tar_bytes()).unwrap();
        let err = extract(&arch, &dir.join("out"), 2, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, ExtractError::TooLarge { .. }), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
