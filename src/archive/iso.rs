//! A small ISO 9660 reader: primary volume descriptor and Joliet.
//!
//! Sector 16 starts the volume descriptors (`CD001`). A supplementary
//! descriptor whose escape sequence is `%/@`, `%/C`, or `%/E` is Joliet, and
//! its names (UTF-16BE) are preferred over the 8.3 primary names. Directory
//! records are walked from the root. Multi-extent files (flag 0x80) are
//! concatenated. UDF and Rock Ridge are not read.
//!
//! The on-disk layout follows ECMA-119. The test builds a tiny image in memory
//! so the reader does not depend on an external fixture.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::extract::{copy_limited, mark_readonly, ExtractError};
use super::names::sanitize;

const SECTOR: u64 = 2048;

struct Tree {
    extent: u32,
    size: u32,
    joliet: bool,
}

struct FileRec {
    rel: String,
    extent: u32,
    size: u64,
    dir: bool,
}

fn both32(buf: &[u8], at: usize) -> Option<u32> {
    let b = buf.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn both16(buf: &[u8], at: usize) -> Option<u16> {
    let b = buf.get(at..at + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn is_joliet(desc: &[u8]) -> bool {
    // Escape sequences sit at offset 88 of a supplementary descriptor.
    matches!(desc.get(88..91), Some([0x25, 0x2F, 0x40]) | Some([0x25, 0x2F, 0x43]) | Some([0x25, 0x2F, 0x45]))
}

fn decode_name(id: &[u8], joliet: bool) -> Option<String> {
    if id.is_empty() || id == [0] || id == [1] {
        return None;
    }
    let raw = if joliet {
        if id.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = id.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16(&units).ok()?
    } else {
        String::from_utf8_lossy(id).into_owned()
    };
    let raw = raw.trim_end_matches(|c: char| c == ' ' || c == '\0');
    let raw = raw.split(';').next().unwrap_or(raw).trim();
    if raw.is_empty() || raw == "\u{0}" || raw == "\u{1}" {
        return None;
    }
    sanitize(raw)
}

fn parse_records(data: &[u8], joliet: bool, prefix: &str) -> Vec<FileRec> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < data.len() {
        let len = data[i] as usize;
        if len == 0 {
            // Padding to the next sector.
            let next = (i / SECTOR as usize + 1) * SECTOR as usize;
            if next <= i || next >= data.len() {
                break;
            }
            i = next;
            continue;
        }
        if i + len > data.len() || len < 34 {
            break;
        }
        let rec = &data[i..i + len];
        let extent = both32(rec, 2).unwrap_or(0);
        let size = both32(rec, 10).unwrap_or(0) as u64;
        let flags = rec[25];
        let name_len = rec[32] as usize;
        if 33 + name_len > rec.len() {
            break;
        }
        let id = &rec[33..33 + name_len];
        let dir = flags & 0x02 != 0;
        if let Some(name) = decode_name(id, joliet) {
            let rel = if prefix.is_empty() { name } else { format!(r"{prefix}\{name}") };
            out.push(FileRec { rel, extent, size, dir });
        }
        i += len;
    }
    out
}

fn read_at(file: &mut File, at: u64, len: usize) -> Result<Vec<u8>, ExtractError> {
    file.seek(SeekFrom::Start(at)).map_err(|e| ExtractError::Io(e.to_string()))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).map_err(|e| ExtractError::Io(e.to_string()))?;
    Ok(buf)
}

fn find_tree(file: &mut File) -> Result<Tree, ExtractError> {
    let mut joliet: Option<Tree> = None;
    let mut primary: Option<Tree> = None;
    // Descriptors start at sector 16 and end at the terminator. Cap the scan.
    for n in 16..32 {
        let desc = read_at(file, n * SECTOR, SECTOR as usize)?;
        if desc.get(1..6) != Some(b"CD001") {
            break;
        }
        let kind = desc[0];
        if kind == 255 {
            break;
        }
        let extent = both32(&desc, 156 + 2).unwrap_or(0);
        let size = both32(&desc, 156 + 10).unwrap_or(0);
        let block = both16(&desc, 128).unwrap_or(2048);
        if block != 2048 || extent == 0 || size == 0 {
            continue;
        }
        let tree = Tree { extent, size, joliet: kind == 2 && is_joliet(&desc) };
        if kind == 1 {
            primary = Some(tree);
        } else if kind == 2 && is_joliet(&desc) {
            joliet = Some(tree);
        }
    }
    joliet.or(primary).ok_or_else(|| ExtractError::Bad("not an ISO 9660 image".into()))
}

fn list_tree(file: &mut File, tree: &Tree) -> Result<Vec<FileRec>, ExtractError> {
    let mut files = Vec::new();
    let mut stack = vec![("".to_string(), tree.extent, tree.size)];
    let mut seen = Vec::new();
    let mut guard = 0u32;
    while let Some((prefix, extent, size)) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            return Err(ExtractError::Bad("too many directories".into()));
        }
        if seen.contains(&extent) {
            continue;
        }
        seen.push(extent);
        let len = size.min(32 * 1024 * 1024) as usize;
        let data = read_at(file, extent as u64 * SECTOR, len)?;
        for rec in parse_records(&data, tree.joliet, &prefix) {
            if rec.dir {
                stack.push((rec.rel, rec.extent, rec.size as u32));
            } else {
                files.push(rec);
            }
        }
    }
    Ok(files)
}

/// Extract an ISO 9660 image into `dest`. Names that fail [`sanitize`] are skipped.
pub(crate) fn extract_file(
    src: &Path,
    dest: &Path,
    limit: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<u64, ExtractError> {
    let mut file = File::open(src).map_err(|e| ExtractError::Io(e.to_string()))?;
    let tree = find_tree(&mut file)?;
    let files = list_tree(&mut file, &tree)?;
    let mut written = 0u64;
    for rec in files {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ExtractError::Cancelled);
        }
        let Some(rel) = sanitize(&rec.rel) else { continue };
        let path: PathBuf = dest.join(rel.replace('\\', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ExtractError::Io(e.to_string()))?;
        }
        // Multi-extent: one record, one extent. A following record with the same
        // name is appended by opening the file again. The first write creates it.
        let append = path.exists();
        let mut out = if append {
            std::fs::OpenOptions::new().append(true).open(&path).map_err(|e| ExtractError::Io(e.to_string()))?
        } else {
            File::create(&path).map_err(|e| ExtractError::Io(e.to_string()))?
        };
        file.seek(SeekFrom::Start(rec.extent as u64 * SECTOR)).map_err(|e| ExtractError::Io(e.to_string()))?;
        let mut limited = file.by_ref().take(rec.size);
        copy_limited(&mut limited, &mut out, &mut written, limit, cancel)?;
        drop(out);
        mark_readonly(&path);
    }
    Ok(written)
}

/// Same reader over a byte slice. Tests build the image in memory.
pub(crate) fn list_bytes(image: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ExtractError> {
    // Parse the volume descriptors straight from the image bytes.
    if image.len() < 17 * SECTOR as usize {
        return Err(ExtractError::Bad("image is too small".into()));
    }
    let mut joliet: Option<Tree> = None;
    let mut primary: Option<Tree> = None;
    for n in 16..32 {
        let at = n * SECTOR as usize;
        let desc = image.get(at..at + SECTOR as usize).ok_or_else(|| ExtractError::Bad("truncated".into()))?;
        if desc.get(1..6) != Some(b"CD001") {
            break;
        }
        if desc[0] == 255 {
            break;
        }
        let extent = both32(desc, 158).unwrap_or(0);
        let size = both32(desc, 166).unwrap_or(0);
        if desc[0] == 1 {
            primary = Some(Tree { extent, size, joliet: false });
        } else if desc[0] == 2 && is_joliet(desc) {
            joliet = Some(Tree { extent, size, joliet: true });
        }
    }
    let tree = joliet.or(primary).ok_or_else(|| ExtractError::Bad("not an ISO 9660 image".into()))?;
    let mut files = Vec::new();
    let mut stack = vec![("".to_string(), tree.extent, tree.size)];
    let mut seen = Vec::new();
    while let Some((prefix, extent, size)) = stack.pop() {
        if !seen.iter().any(|e: &u32| *e == extent) {
            seen.push(extent);
        } else {
            continue;
        }
        let at = extent as usize * SECTOR as usize;
        let end = at.saturating_add(size as usize).min(image.len());
        let data = image.get(at..end).ok_or_else(|| ExtractError::Bad("truncated directory".into()))?;
        for rec in parse_records(data, tree.joliet, &prefix) {
            if rec.dir {
                stack.push((rec.rel, rec.extent, rec.size as u32));
            } else {
                let start = rec.extent as usize * SECTOR as usize;
                let stop = start.saturating_add(rec.size as usize).min(image.len());
                let bytes = image.get(start..stop).unwrap_or_default().to_vec();
                files.push((rec.rel, bytes));
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both32_put(buf: &mut [u8], at: usize, v: u32) {
        buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
        buf[at + 4..at + 8].copy_from_slice(&v.to_be_bytes());
    }

    fn both16_put(buf: &mut [u8], at: usize, v: u16) {
        buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
        buf[at + 2..at + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn record(name: &[u8], extent: u32, size: u32, flags: u8) -> Vec<u8> {
        let mut len = 33 + name.len();
        if len % 2 != 0 {
            len += 1;
        }
        let mut rec = vec![0u8; len];
        rec[0] = len as u8;
        both32_put(&mut rec, 2, extent);
        both32_put(&mut rec, 10, size);
        rec[18] = 126; // year 2026
        rec[19] = 11;
        rec[20] = 28;
        rec[25] = flags;
        both16_put(&mut rec, 28, 1);
        rec[32] = name.len() as u8;
        rec[33..33 + name.len()].copy_from_slice(name);
        rec
    }

    fn utf16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    fn descriptor(kind: u8, joliet: bool, root: &[u8], sectors: u32) -> [u8; 2048] {
        let mut s = [0u8; 2048];
        s[0] = kind;
        s[1..6].copy_from_slice(b"CD001");
        s[6] = 1;
        both32_put(&mut s, 80, sectors);
        both16_put(&mut s, 128, 2048);
        if joliet {
            s[88] = 0x25;
            s[89] = 0x2F;
            s[90] = 0x45;
        }
        s[156..156 + root.len()].copy_from_slice(root);
        s
    }

    /// Primary name `HELLO.TXT;1`, Joliet name `Hello.txt`, plus a subdirectory
    /// and one zip-slip name the reader must drop.
    fn sample_iso() -> Vec<u8> {
        let file_sector = 22u32;
        let sub_file_sector = 23u32;
        let primary_dir = 19u32;
        let joliet_dir = 20u32;
        let primary_sub = 21u32;
        let joliet_sub = 24u32;
        let sectors = 25u32;
        let mut image = vec![0u8; sectors as usize * 2048];

        let dot = record(&[0], primary_dir, 2048, 0x02);
        let dotdot = record(&[1], primary_dir, 2048, 0x02);
        let hello = record(b"HELLO.TXT;1", file_sector, 9, 0);
        let sub = record(b"SUB", primary_sub, 2048, 0x02);
        let slip = record(b"..\\secret.txt", file_sector, 4, 0);
        let mut pdir = Vec::new();
        pdir.extend(dot);
        pdir.extend(dotdot);
        pdir.extend(hello);
        pdir.extend(sub);
        pdir.extend(slip);
        image[primary_dir as usize * 2048..primary_dir as usize * 2048 + pdir.len()].copy_from_slice(&pdir);

        let sub_dot = record(&[0], primary_sub, 2048, 0x02);
        let sub_up = record(&[1], primary_dir, 2048, 0x02);
        let nested = record(b"A.TXT;1", sub_file_sector, 3, 0);
        let mut sdir = Vec::new();
        sdir.extend(sub_dot);
        sdir.extend(sub_up);
        sdir.extend(nested);
        image[primary_sub as usize * 2048..primary_sub as usize * 2048 + sdir.len()].copy_from_slice(&sdir);

        let jdot = record(&[0], joliet_dir, 2048, 0x02);
        let jdotdot = record(&[1], joliet_dir, 2048, 0x02);
        let jhello = record(&utf16be("Hello.txt"), file_sector, 9, 0);
        let jsub = record(&utf16be("Sub"), joliet_sub, 2048, 0x02);
        let jslip = record(&utf16be("../secret.txt"), file_sector, 4, 0);
        let mut jdir = Vec::new();
        jdir.extend(jdot);
        jdir.extend(jdotdot);
        jdir.extend(jhello);
        jdir.extend(jsub);
        jdir.extend(jslip);
        image[joliet_dir as usize * 2048..joliet_dir as usize * 2048 + jdir.len()].copy_from_slice(&jdir);

        let jsdot = record(&[0], joliet_sub, 2048, 0x02);
        let jsup = record(&[1], joliet_dir, 2048, 0x02);
        let ja = record(&utf16be("A.txt"), sub_file_sector, 3, 0);
        let mut jsdir = Vec::new();
        jsdir.extend(jsdot);
        jsdir.extend(jsup);
        jsdir.extend(ja);
        image[joliet_sub as usize * 2048..joliet_sub as usize * 2048 + jsdir.len()].copy_from_slice(&jsdir);

        let hello_bytes = b"hello-iso";
        image[file_sector as usize * 2048..file_sector as usize * 2048 + hello_bytes.len()].copy_from_slice(hello_bytes);
        image[sub_file_sector as usize * 2048..sub_file_sector as usize * 2048 + 3].copy_from_slice(b"abc");

        let root_p = record(&[0], primary_dir, 2048, 0x02);
        let root_j = record(&[0], joliet_dir, 2048, 0x02);
        let pvd = descriptor(1, false, &root_p, sectors);
        let svd = descriptor(2, true, &root_j, sectors);
        let mut term = [0u8; 2048];
        term[0] = 255;
        term[1..6].copy_from_slice(b"CD001");
        term[6] = 1;
        image[16 * 2048..17 * 2048].copy_from_slice(&pvd);
        image[17 * 2048..18 * 2048].copy_from_slice(&svd);
        image[18 * 2048..19 * 2048].copy_from_slice(&term);
        image
    }

    #[test]
    fn reads_joliet_names_and_drops_zip_slip() {
        let image = sample_iso();
        let mut files = list_bytes(&image).unwrap();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"Hello.txt"), "{names:?}");
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(r"Sub\A.txt")), "{names:?}");
        assert!(names.iter().all(|n| !n.contains("secret")), "{names:?}");
        let hello = files.iter().find(|(n, _)| n == "Hello.txt").unwrap();
        assert_eq!(hello.1, b"hello-iso");
        let nested = files.iter().find(|(n, _)| n.eq_ignore_ascii_case(r"Sub\A.txt")).unwrap();
        assert_eq!(nested.1, b"abc");
    }

    #[test]
    fn extracts_to_a_folder_read_only() {
        let dir = std::env::temp_dir().join(format!("maguro-iso-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("tiny.iso");
        let dest = dir.join("out");
        std::fs::write(&iso, sample_iso()).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        let n = extract_file(&iso, &dest, 1024 * 1024, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert!(n >= 9);
        let hello = dest.join("Hello.txt");
        assert_eq!(std::fs::read(&hello).unwrap(), b"hello-iso");
        assert!(std::fs::metadata(&hello).unwrap().permissions().readonly());
        assert!(!dest.join("secret.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
