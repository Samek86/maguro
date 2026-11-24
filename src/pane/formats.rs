//! Parsers for the preview formats that are not Office ZIP, HWP, or mail MIME.
//!
//! Pure functions on bytes. The preview worker calls them; the tests feed small buffers.

use std::io::{Read, Seek, SeekFrom};

use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

use super::docparse::{ansi, decode_text, html_text, zip_entries, zip_find, zip_read};

const CSV_MAX_COLS: usize = 64;

/// Drop columns past [`CSV_MAX_COLS`]. `csv_rows` keeps every field it is given; wide sheets are capped here
/// so a preview does not measure thousands of columns.
pub(crate) fn cap_columns(mut rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    for row in &mut rows {
        if row.len() > CSV_MAX_COLS {
            row.truncate(CSV_MAX_COLS);
        }
    }
    rows
}

// ---------- dates ----------

/// DOS date and time from a ZIP or Cabinet header. `0` is "no date".
pub(crate) fn dos_when(date: u16, time: u16) -> String {
    if date == 0 {
        return String::new();
    }
    let year = (date >> 9) as u32 + 1980;
    let month = ((date >> 5) & 0xf) as u32;
    let day = (date & 0x1f) as u32;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return String::new();
    }
    let hour = (time >> 11) as u32;
    let min = ((time >> 5) & 0x3f) as u32;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

/// Unix seconds as UTC. `0` is "no date" (a tar header left blank).
pub(crate) fn unix_when(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    let days = (secs / 86_400) as i64;
    let tod = (secs % 86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02} UTC")
}

/// FILETIME (100 ns since 1601-01-01) as UTC.
pub(crate) fn filetime_when(ft: u64) -> String {
    let secs = ft / 10_000_000;
    const EPOCH: u64 = 11_644_473_600;
    if secs < EPOCH {
        return String::new();
    }
    unix_when(secs - EPOCH)
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

// ---------- RTF ----------

/// Plain text of an RTF document. Control words are dropped, `\'hh` uses the `\ansicpg` code page,
/// and `\uN` is a Unicode scalar (signed 16-bit, or a positive value above 32767).
pub(crate) fn rtf_text(raw: &[u8]) -> String {
    let mut out = String::new();
    let mut ansi_buf: Vec<u8> = Vec::new();
    let mut cp = 1252u32;
    let mut uc: usize = 1;
    let mut depth = 0i32;
    let mut skip: Option<i32> = None;
    let mut i = 0usize;
    let flush = |out: &mut String, ansi_buf: &mut Vec<u8>, cp: u32, skip: Option<i32>| {
        if ansi_buf.is_empty() {
            return;
        }
        if skip.is_none() {
            out.push_str(&ansi(ansi_buf, cp));
        }
        ansi_buf.clear();
    };
    while i < raw.len() {
        match raw[i] {
            b'{' => {
                flush(&mut out, &mut ansi_buf, cp, skip);
                depth += 1;
                if skip.is_none() && rtf_skip_group(raw, i + 1) {
                    skip = Some(depth);
                }
                i += 1;
            }
            b'}' => {
                flush(&mut out, &mut ansi_buf, cp, skip);
                if skip == Some(depth) {
                    skip = None;
                }
                depth -= 1;
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= raw.len() {
                    break;
                }
                let symbol = raw[i];
                if !symbol.is_ascii_alphabetic() {
                    i += 1;
                    if symbol == b'\'' && i + 1 < raw.len() {
                        if let (Some(h), Some(l)) = (hex_val(raw[i]), hex_val(raw[i + 1])) {
                            if skip.is_none() {
                                ansi_buf.push(h << 4 | l);
                            }
                            i += 2;
                        }
                        continue;
                    }
                    if skip.is_none() {
                        match symbol {
                            b'\\' | b'{' | b'}' => {
                                flush(&mut out, &mut ansi_buf, cp, skip);
                                out.push(symbol as char);
                            }
                            b'~' => {
                                flush(&mut out, &mut ansi_buf, cp, skip);
                                out.push('\u{00a0}');
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                let start = i;
                while i < raw.len() && raw[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = &raw[start..i];
                let mut neg = false;
                if raw.get(i) == Some(&b'-') {
                    neg = true;
                    i += 1;
                }
                let mut val = 0i32;
                let mut any_digit = false;
                while i < raw.len() && raw[i].is_ascii_digit() {
                    any_digit = true;
                    val = val.saturating_mul(10).saturating_add((raw[i] - b'0') as i32);
                    i += 1;
                }
                if neg {
                    val = -val;
                }
                if raw.get(i) == Some(&b' ') {
                    i += 1;
                }
                if word == b"bin" && any_digit {
                    let n = val.max(0) as usize;
                    let end = i.saturating_add(n).min(raw.len());
                    i = end;
                    continue;
                }
                if skip.is_some() {
                    continue;
                }
                match word {
                    b"par" | b"line" | b"page" => {
                        flush(&mut out, &mut ansi_buf, cp, skip);
                        out.push('\n');
                    }
                    b"tab" => {
                        flush(&mut out, &mut ansi_buf, cp, skip);
                        out.push('\t');
                    }
                    b"ansicpg" if any_digit && val > 0 => {
                        flush(&mut out, &mut ansi_buf, cp, skip);
                        cp = val as u32;
                    }
                    b"uc" if any_digit => uc = val.max(0) as usize,
                    b"u" if any_digit => {
                        flush(&mut out, &mut ansi_buf, cp, skip);
                        let scalar = if val < 0 { (val as i16) as u16 as u32 } else { val as u32 };
                        if let Some(c) = char::from_u32(scalar) {
                            out.push(c);
                        }
                        skip_rtf_fallback(raw, &mut i, uc);
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            c => {
                if skip.is_none() {
                    ansi_buf.push(c);
                }
                i += 1;
            }
        }
    }
    flush(&mut out, &mut ansi_buf, cp, skip);
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines() {
        let t = line.trim_end();
        if t.is_empty() && lines.last().is_none_or(|p: &&str| p.is_empty()) {
            continue;
        }
        lines.push(t);
    }
    lines.join("\n").trim().to_string()
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn rtf_skip_group(raw: &[u8], mut i: usize) -> bool {
    while matches!(raw.get(i), Some(b' ' | b'\r' | b'\n' | b'\t')) {
        i += 1;
    }
    if raw.get(i) != Some(&b'\\') {
        return false;
    }
    i += 1;
    if raw.get(i) == Some(&b'*') {
        return true;
    }
    let start = i;
    while raw.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        i += 1;
    }
    matches!(
        &raw[start..i],
        b"fonttbl"
            | b"colortbl"
            | b"stylesheet"
            | b"info"
            | b"pict"
            | b"object"
            | b"header"
            | b"footer"
            | b"headerl"
            | b"headerr"
            | b"headerf"
            | b"footerl"
            | b"footerr"
            | b"footerf"
            | b"footnote"
            | b"annotation"
            | b"fldinst"
            | b"listtable"
            | b"listoverridetable"
            | b"rsidtbl"
            | b"generator"
            | b"datastore"
            | b"themedata"
            | b"colorschememapping"
            | b"latentstyles"
            | b"nonshppict"
            | b"shppict"
            | b"filetbl"
            | b"revtbl"
    )
}

fn skip_rtf_fallback(raw: &[u8], i: &mut usize, n: usize) {
    let mut left = n;
    while left > 0 && *i < raw.len() {
        if raw[*i] == b'\\' && raw.get(*i + 1) == Some(&b'\'') && *i + 3 < raw.len() {
            *i += 4;
            left -= 1;
            continue;
        }
        if matches!(raw[*i], b'\\' | b'{' | b'}' | b'\r' | b'\n') {
            break;
        }
        *i += 1;
        left -= 1;
    }
}

// ---------- OpenDocument ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sheet {
    pub(crate) name: String,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) cut: bool,
}

/// Body paragraphs of an ODT `content.xml`. Headings stay their own lines. List items start with a bullet.
pub(crate) fn odt_text(xml: &str) -> String {
    let mut out = String::new();
    let mut para = String::new();
    let mut text = false;
    let mut skip = 0i32;
    let mut bullet = false;
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        if text && skip == 0 {
            para.push_str(&super::docparse::unescape(&rest[..i]));
        }
        rest = &rest[i + 1..];
        let Some(j) = rest.find('>') else { break };
        let tag = &rest[..j];
        rest = &rest[j + 1..];
        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let close = tag.starts_with('/');
        let empty = tag.trim_end().ends_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if matches!(name, "office:annotation" | "text:tracked-changes" | "office:binary-data") {
            if !close && !empty {
                skip += 1;
            } else if close {
                skip = skip.saturating_sub(1);
            }
            continue;
        }
        if skip > 0 {
            continue;
        }
        match name {
            "text:h" | "text:p" if close => {
                let line = para.trim();
                if bullet {
                    if !line.is_empty() {
                        out.push_str("• ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    bullet = false;
                } else if !line.is_empty() || name == "text:p" {
                    out.push_str(line);
                    out.push('\n');
                }
                para.clear();
                text = false;
            }
            "text:h" | "text:p" if !empty => text = true,
            "text:list-item" if !close => bullet = true,
            "text:line-break" if !close => para.push('\n'),
            "text:tab" if !close => para.push('\t'),
            "text:s" if !close => {
                let n = xml_attr(tag, "text:c").and_then(|s| s.parse().ok()).unwrap_or(1usize).min(80);
                para.push_str(&" ".repeat(n));
            }
            _ => {}
        }
    }
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines() {
        if line.is_empty() && lines.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// Spreadsheet sheets from ODS `content.xml`, at most `max_rows` rows and `max_cols` columns in total.
pub(crate) fn ods_sheets(xml: &str, max_rows: usize, max_cols: usize) -> Vec<Sheet> {
    let mut sheets = Vec::new();
    let mut name = String::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_cell = false;
    let mut in_text = false;
    let mut row_rep = 1usize;
    let mut col_rep = 1usize;
    let mut cell_value = String::new();
    let mut cut = false;
    let mut total = 0usize;
    let mut rest = xml;
    let finish_row = |rows: &mut Vec<Vec<String>>, row: &mut Vec<String>, rep: usize, total: &mut usize, max_rows: usize, cut: &mut bool| {
        if row.is_empty() {
            return;
        }
        let empty = row.iter().all(|c| c.is_empty());
        let times = if empty { 1 } else { rep.max(1) };
        for _ in 0..times {
            if *total >= max_rows {
                *cut = true;
                return;
            }
            rows.push(row.clone());
            *total += 1;
        }
        row.clear();
    };
    while let Some(i) = rest.find('<') {
        if in_text {
            cell.push_str(&super::docparse::unescape(&rest[..i]));
        }
        rest = &rest[i + 1..];
        let Some(j) = rest.find('>') else { break };
        let tag = &rest[..j];
        rest = &rest[j + 1..];
        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let close = tag.starts_with('/');
        let empty = tag.trim_end().ends_with('/');
        let tname = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        match tname {
            "table:table" if !close => {
                if !rows.is_empty() || !name.is_empty() {
                    sheets.push(Sheet { name: name.clone(), rows: std::mem::take(&mut rows), cut });
                }
                name = xml_attr(tag, "table:name").unwrap_or("").to_string();
                cut = false;
            }
            "table:table" if close => {
                finish_row(&mut rows, &mut row, row_rep, &mut total, max_rows, &mut cut);
                sheets.push(Sheet { name: std::mem::take(&mut name), rows: std::mem::take(&mut rows), cut });
                if total >= max_rows {
                    break;
                }
            }
            "table:table-row" if !close => {
                row_rep = xml_attr(tag, "table:number-rows-repeated").and_then(|s| s.parse().ok()).unwrap_or(1usize);
                row.clear();
            }
            "table:table-row" if close => {
                finish_row(&mut rows, &mut row, row_rep, &mut total, max_rows, &mut cut);
                if total >= max_rows {
                    sheets.push(Sheet { name: std::mem::take(&mut name), rows: std::mem::take(&mut rows), cut: true });
                    break;
                }
            }
            "table:table-cell" | "table:covered-table-cell" if !close => {
                col_rep = xml_attr(tag, "table:number-columns-repeated").and_then(|s| s.parse().ok()).unwrap_or(1usize);
                cell_value = xml_attr(tag, "office:value")
                    .or_else(|| xml_attr(tag, "office:date-value"))
                    .or_else(|| xml_attr(tag, "office:boolean-value"))
                    .unwrap_or("")
                    .to_string();
                cell.clear();
                in_cell = true;
                if empty {
                    push_cell(&mut row, "", &cell_value, col_rep, max_cols);
                    in_cell = false;
                }
            }
            "table:table-cell" | "table:covered-table-cell" if close => {
                push_cell(&mut row, cell.trim(), &cell_value, col_rep, max_cols);
                cell.clear();
                in_cell = false;
                in_text = false;
            }
            "text:p" | "text:h" if in_cell && !close && !empty => {
                if !cell.is_empty() {
                    cell.push(' ');
                }
                in_text = true;
            }
            "text:p" | "text:h" if close => in_text = false,
            _ => {}
        }
    }
    if !rows.is_empty() || !name.is_empty() {
        sheets.push(Sheet { name, rows, cut });
    }
    sheets.retain(|s| !s.rows.is_empty() || !s.name.is_empty());
    sheets
}

fn push_cell(row: &mut Vec<String>, text: &str, value: &str, rep: usize, max_cols: usize) {
    let shown = if !text.is_empty() { text } else { value };
    let n = rep.max(1).min(max_cols.saturating_sub(row.len()).max(if row.len() >= max_cols { 0 } else { 1 }));
    for _ in 0..n {
        if row.len() >= max_cols {
            break;
        }
        row.push(shown.to_string());
    }
}

fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let at = tag.find(&key)? + key.len();
    let tail = tag.get(at..)?;
    let end = tail.find('"')?;
    tail.get(..end)
}

// ---------- EPUB ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EpubPlan {
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) chapters: Vec<String>,
}

/// `container.xml` plus the OPF: title, author, and chapter zip paths in spine order.
pub(crate) fn epub_plan(_container: &str, opf: &str, opf_path: &str) -> EpubPlan {
    let dir = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let title = xml_element(opf, "dc:title");
    let author = xml_element(opf, "dc:creator");
    let mut manifest = Vec::new();
    let mut rest = opf;
    while let Some(i) = rest.find("<item ") {
        rest = &rest[i..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let (Some(id), Some(href)) = (xml_attr(tag, "id"), xml_attr(tag, "href")) {
            manifest.push((id.to_string(), join_zip(dir, href)));
        }
        rest = &rest[end..];
    }
    let mut chapters = Vec::new();
    rest = opf;
    while let Some(i) = rest.find("<itemref ") {
        rest = &rest[i..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let Some(id) = xml_attr(tag, "idref") {
            if let Some((_, href)) = manifest.iter().find(|(k, _)| k == id) {
                chapters.push(href.clone());
            }
        }
        rest = &rest[end..];
    }
    EpubPlan { title, author, chapters }
}

pub(crate) fn epub_opf_path(container: &str) -> Option<String> {
    let i = container.find("full-path=\"")?;
    let tail = container.get(i + "full-path=\"".len()..)?;
    let end = tail.find('"')?;
    let path = tail.get(..end)?.replace('\\', "/");
    (!path.is_empty()).then_some(path)
}

/// Chapter HTML joined, stopped at `max_chars` of plain text.
pub(crate) fn epub_body<R: Read + Seek>(f: &mut R, max_chars: usize) -> Option<(EpubPlan, String, bool)> {
    let es = zip_entries(f)?;
    let container = zip_find(&es, "META-INF/container.xml").and_then(|e| zip_read(f, e, 256 * 1024))?;
    let opf_path = epub_opf_path(&decode_text(&container))?;
    let opf_bytes = zip_find(&es, &opf_path).and_then(|e| zip_read(f, e, 2 << 20))?;
    let opf = decode_text(&opf_bytes);
    let plan = epub_plan(&decode_text(&container), &opf, &opf_path);
    let mut body = String::new();
    let mut cut = false;
    for href in &plan.chapters {
        if body.len() >= max_chars {
            cut = true;
            break;
        }
        let Some(e) = zip_find(&es, href) else { continue };
        let room = (max_chars - body.len()).saturating_add(8 * 1024).min(1 << 20);
        let Some(bytes) = zip_read(f, e, room) else { continue };
        let text = html_text(&decode_text(&bytes));
        if body.len() + text.len() > max_chars {
            let mut n = max_chars - body.len();
            while n > 0 && !text.is_char_boundary(n) {
                n -= 1;
            }
            body.push_str(&text[..n]);
            cut = true;
            break;
        }
        if !body.is_empty() && !text.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&text);
    }
    Some((plan, body, cut))
}

fn xml_element(xml: &str, name: &str) -> String {
    let open = format!("<{name}");
    let Some(i) = xml.find(&open) else { return String::new() };
    let Some(tail) = xml.get(i + open.len()..) else { return String::new() };
    let Some(g) = tail.find('>') else { return String::new() };
    if tail[..g].trim_end().ends_with('/') {
        return String::new();
    }
    let rest = &tail[g + 1..];
    let close = format!("</{name}");
    let end = rest.find(&close).unwrap_or(rest.len());
    super::docparse::unescape(rest.get(..end).unwrap_or("")).trim().to_string()
}

fn join_zip(dir: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or(href);
    let href = percent_decode(&href.replace('\\', "/"));
    let mut parts: Vec<String> = if dir.is_empty() { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect() };
    for p in href.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." {
            parts.pop();
            continue;
        }
        parts.push(p.to_string());
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------- Outlook .msg property streams ----------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MsgParts {
    pub(crate) subject: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) cc: String,
    pub(crate) date: String,
    pub(crate) files: Vec<String>,
    pub(crate) body: String,
}

/// Property streams named `__substg1.0_PPPPTTTT`. Attachments live under `__attach_version1.0_*`.
pub(crate) fn msg_parts(streams: &[(String, Vec<u8>)]) -> MsgParts {
    let cp = streams.iter().find_map(|(n, b)| {
        let (id, ty) = prop_id(n)?;
        (id == 0x3FDE && ty == 0x0003 && b.len() >= 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }).unwrap_or(1252);
    let text = |id: u16| prop_text(streams, id, cp);
    let mut from = text(0x0C1A);
    let email = text(0x0065);
    if !email.is_empty() && !from.contains(&email) {
        if from.is_empty() {
            from = email;
        } else {
            from = format!("{from} <{email}>");
        }
    }
    let mut body = text(0x1000);
    if body.trim().is_empty() {
        let html = text(0x1013);
        if !html.is_empty() {
            body = html_text(&html);
        }
    }
    let mut files = Vec::new();
    for (name, bytes) in streams {
        if !name.to_ascii_lowercase().contains("__attach") {
            continue;
        }
        let Some((id, _)) = prop_id(name) else { continue };
        if matches!(id, 0x3707 | 0x3704 | 0x3001) {
            let s = prop_bytes(name, bytes, cp);
            if !s.is_empty() && !files.contains(&s) {
                files.push(s);
            }
        }
    }
    let date = streams.iter().find_map(|(n, b)| {
        let (id, ty) = prop_id(n)?;
        (matches!(id, 0x0039 | 0x0E06 | 0x0040) && ty == 0x0040 && b.len() >= 8).then(|| {
            filetime_when(u64::from_le_bytes(b[..8].try_into().unwrap()))
        })
    }).unwrap_or_default();
    MsgParts { subject: text(0x0037), from, to: text(0x0E04), cc: text(0x0E03), date, files, body: body.trim().to_string() }
}

fn prop_id(name: &str) -> Option<(u16, u16)> {
    let hex = name.rsplit(['_', '/']).next()?;
    if hex.len() < 8 {
        return None;
    }
    let id = u16::from_str_radix(&hex[..4], 16).ok()?;
    let ty = u16::from_str_radix(&hex[4..8], 16).ok()?;
    Some((id, ty))
}

fn prop_text(streams: &[(String, Vec<u8>)], id: u16, cp: u32) -> String {
    let mut ansi_hit = None;
    for (name, bytes) in streams {
        if name.to_ascii_lowercase().contains("__attach") {
            continue;
        }
        let Some((pid, ty)) = prop_id(name) else { continue };
        if pid != id {
            continue;
        }
        if ty == 0x001F {
            return utf16_z(bytes);
        }
        if ty == 0x001E {
            ansi_hit = Some(prop_bytes(name, bytes, cp));
        }
    }
    ansi_hit.unwrap_or_default()
}

fn prop_bytes(name: &str, bytes: &[u8], cp: u32) -> String {
    let Some((_, ty)) = prop_id(name) else { return String::new() };
    match ty {
        0x001F => utf16_z(bytes),
        0x001E => {
            let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
            ansi(&bytes[..end], cp)
        }
        _ => String::new(),
    }
}

fn utf16_z(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&u).trim().to_string()
}

// ---------- tar and gzip ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TarEntry {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) modified: String,
    pub(crate) dir: bool,
}

/// Entries of an uncompressed tar. `None` when the first block is not a tar header.
pub(crate) fn tar_entries<R: Read + Seek>(f: &mut R, max_n: usize) -> Option<(Vec<TarEntry>, bool)> {
    let mut out = Vec::new();
    let mut long_name: Option<String> = None;
    let mut cut = false;
    let mut first = true;
    let mut saw = false;
    loop {
        let mut block = [0u8; 512];
        if f.read_exact(&mut block).is_err() {
            break;
        }
        if block.iter().all(|&c| c == 0) {
            break;
        }
        let Some(h) = tar_header(&block) else {
            if first {
                return None;
            }
            break;
        };
        first = false;
        saw = true;
        let padded = (h.size.saturating_add(511)) & !511;
        match h.flag {
            b'L' | b'K' => {
                let n = h.size.min(64 * 1024) as usize;
                let mut buf = vec![0u8; n];
                if f.read_exact(&mut buf).is_err() {
                    break;
                }
                let rest = padded.saturating_sub(n as u64);
                if rest > 0 && f.seek(SeekFrom::Current(rest as i64)).is_err() {
                    break;
                }
                long_name = Some(decode_text(&buf).trim_end_matches('\0').trim().to_string());
            }
            b'x' | b'g' => {
                let n = h.size.min(256 * 1024) as usize;
                let mut buf = vec![0u8; n];
                if f.read_exact(&mut buf).is_err() {
                    break;
                }
                let rest = padded.saturating_sub(n as u64);
                if rest > 0 && f.seek(SeekFrom::Current(rest as i64)).is_err() {
                    break;
                }
                if let Some(p) = pax_path(&buf) {
                    long_name = Some(p);
                }
            }
            _ => {
                let name = long_name.take().filter(|s| !s.is_empty()).unwrap_or(h.name);
                let dir = h.flag == b'5' || name.ends_with('/');
                if !name.is_empty() {
                    if out.len() >= max_n {
                        cut = true;
                    } else {
                        out.push(TarEntry { name, size: if dir { 0 } else { h.size }, modified: unix_when(h.mtime), dir });
                    }
                }
                if padded > 0 && f.seek(SeekFrom::Current(padded as i64)).is_err() {
                    break;
                }
            }
        }
    }
    saw.then_some((out, cut))
}

struct TarHead {
    name: String,
    size: u64,
    mtime: u64,
    flag: u8,
}

fn tar_header(b: &[u8; 512]) -> Option<TarHead> {
    if !tar_checksum(b) {
        return None;
    }
    let name = tar_name(b);
    let size = tar_number(&b[124..136]);
    let mtime = tar_number(&b[136..148]);
    Some(TarHead { name, size, mtime, flag: b[156] })
}

fn tar_checksum(b: &[u8]) -> bool {
    let stored = octal(&b[148..156]);
    let mut sum = 0u32;
    for (i, &c) in b.iter().enumerate() {
        sum += if (148..156).contains(&i) { b' ' as u32 } else { c as u32 };
    }
    sum as u64 == stored
}

fn tar_name(b: &[u8]) -> String {
    let name = cstr(&b[0..100]);
    let prefix = if &b[257..262] == b"ustar" { cstr(&b[345..500]) } else { String::new() };
    let joined = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
    decode_text(joined.as_bytes())
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn octal(b: &[u8]) -> u64 {
    let mut n = 0u64;
    for &c in b {
        match c {
            b'0'..=b'7' => n = n.saturating_mul(8).saturating_add((c - b'0') as u64),
            0 | b' ' => {}
            _ => break,
        }
    }
    n
}

fn tar_number(b: &[u8]) -> u64 {
    if matches!(b.first(), Some(0x80 | 0xFF)) {
        let mut n = 0u64;
        for &c in b.iter().skip(1) {
            n = n.saturating_mul(256).saturating_add(c as u64);
        }
        return n;
    }
    octal(b)
}

fn pax_path(data: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(data).ok()?;
    let mut rest = s;
    while let Some(sp) = rest.find(' ') {
        let len: usize = rest[..sp].trim().parse().ok()?;
        if len < sp + 2 || len > rest.len() {
            break;
        }
        let rec = &rest[..len];
        rest = &rest[len..];
        let body = rec.get(sp + 1..)?.trim_end_matches('\n');
        if let Some(path) = body.strip_prefix("path=") {
            return Some(path.to_string());
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GzipMeta {
    pub(crate) name: String,
    pub(crate) mtime: u32,
    pub(crate) data_at: usize,
}

/// Gzip header in `prefix` (the member must start at byte 0). `None` if the header is not finished in the buffer.
pub(crate) fn gzip_meta(prefix: &[u8]) -> Option<GzipMeta> {
    if prefix.len() < 10 || prefix[0] != 0x1f || prefix[1] != 0x8b || prefix[2] != 8 {
        return None;
    }
    let flags = prefix[3];
    let mtime = u32::from_le_bytes(prefix[4..8].try_into().ok()?);
    let mut i = 10usize;
    if flags & 4 != 0 {
        if i + 2 > prefix.len() {
            return None;
        }
        let n = u16::from_le_bytes([prefix[i], prefix[i + 1]]) as usize;
        i = i.checked_add(2)?.checked_add(n)?;
    }
    let mut name = String::new();
    if flags & 8 != 0 {
        let start = i;
        while i < prefix.len() && prefix[i] != 0 {
            i += 1;
        }
        if i >= prefix.len() {
            return None;
        }
        name = decode_text(&prefix[start..i]);
        i += 1;
    }
    if flags & 16 != 0 {
        while i < prefix.len() && prefix[i] != 0 {
            i += 1;
        }
        if i >= prefix.len() {
            return None;
        }
        i += 1;
    }
    if flags & 2 != 0 {
        i = i.checked_add(2)?;
    }
    if i > prefix.len() {
        return None;
    }
    Some(GzipMeta { name, mtime, data_at: i })
}

/// Uncompressed size modulo 2^32, the last four bytes of a gzip member.
pub(crate) fn gzip_isize(tail: &[u8]) -> Option<u32> {
    let b: [u8; 4] = tail.get(tail.len().checked_sub(4)?..)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

/// Inflate a raw deflate stream, stopping after `cap` uncompressed bytes.
pub(crate) struct CapInflate<R> {
    src: R,
    state: Box<InflateState>,
    comp: Vec<u8>,
    pending: Vec<u8>,
    pending_at: usize,
    inflated: u64,
    cap: u64,
    src_eof: bool,
    stream_end: bool,
    failed: bool,
}

impl<R: Read> CapInflate<R> {
    pub(crate) fn new(src: R, cap: u64) -> Self {
        Self {
            src,
            state: InflateState::new_boxed(DataFormat::Raw),
            comp: Vec::new(),
            pending: Vec::new(),
            pending_at: 0,
            inflated: 0,
            cap,
            src_eof: false,
            stream_end: false,
            failed: false,
        }
    }

    pub(crate) fn hit_cap(&self) -> bool {
        self.inflated >= self.cap && !self.stream_end
    }

    fn pump(&mut self) -> std::io::Result<bool> {
        if self.comp.len() < 4096 && !self.src_eof {
            let mut tmp = [0u8; 8192];
            let n = self.src.read(&mut tmp)?;
            if n == 0 {
                self.src_eof = true;
            } else {
                self.comp.extend_from_slice(&tmp[..n]);
            }
        }
        if self.comp.is_empty() {
            self.stream_end = self.src_eof;
            return Ok(false);
        }
        let room = self.cap.saturating_sub(self.inflated).min(16 * 1024) as usize;
        if room == 0 {
            return Ok(false);
        }
        let mut output = vec![0u8; room];
        let res = inflate(&mut self.state, &self.comp, &mut output, MZFlush::None);
        if res.bytes_consumed > 0 {
            self.comp.drain(..res.bytes_consumed.min(self.comp.len()));
        }
        if res.bytes_written > 0 {
            self.pending.extend_from_slice(&output[..res.bytes_written]);
            self.inflated += res.bytes_written as u64;
        }
        match res.status {
            Ok(MZStatus::StreamEnd) => self.stream_end = true,
            Ok(_) => {}
            Err(MZError::Buf) if res.bytes_consumed > 0 || res.bytes_written > 0 => {}
            Err(_) => self.failed = true,
        }
        Ok(res.bytes_consumed > 0 || res.bytes_written > 0)
    }
}

impl<R: Read> Read for CapInflate<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.failed {
            return Ok(0);
        }
        let mut spins = 0;
        while self.pending_at >= self.pending.len() && !self.stream_end && !self.failed && self.inflated < self.cap {
            let progress = self.pump()?;
            if !progress {
                spins += 1;
                if spins > 2 {
                    break;
                }
            } else {
                spins = 0;
            }
        }
        let have = self.pending.len().saturating_sub(self.pending_at);
        if have == 0 {
            return Ok(0);
        }
        let n = have.min(buf.len());
        buf[..n].copy_from_slice(&self.pending[self.pending_at..self.pending_at + n]);
        self.pending_at += n;
        if self.pending_at > 64 * 1024 {
            self.pending.drain(..self.pending_at);
            self.pending_at = 0;
        }
        Ok(n)
    }
}

/// Tar entries from an already-inflated reader. File payloads are read and discarded, up to `skip_cap` bytes.
pub(crate) fn tar_entries_read<R: Read>(r: &mut R, max_n: usize, skip_cap: u64) -> Option<(Vec<TarEntry>, bool)> {
    let mut out = Vec::new();
    let mut long_name: Option<String> = None;
    let mut cut = false;
    let mut skipped = 0u64;
    let mut first = true;
    let mut saw = false;
    loop {
        let mut block = [0u8; 512];
        if read_full(r, &mut block).is_err() {
            break;
        }
        if block.iter().all(|&c| c == 0) {
            break;
        }
        let Some(h) = tar_header(&block) else {
            if first {
                return None;
            }
            break;
        };
        first = false;
        saw = true;
        let padded = (h.size.saturating_add(511)) & !511;
        if skipped.saturating_add(padded) > skip_cap && !matches!(h.flag, b'L' | b'K' | b'x' | b'g') {
            cut = true;
            break;
        }
        match h.flag {
            b'L' | b'K' | b'x' | b'g' => {
                let n = h.size.min(64 * 1024) as usize;
                let mut buf = vec![0u8; n];
                if read_full(r, &mut buf).is_err() {
                    break;
                }
                skipped += n as u64;
                let rest = padded.saturating_sub(n as u64);
                if !discard(r, rest, &mut skipped, skip_cap) {
                    cut = true;
                    break;
                }
                if matches!(h.flag, b'L' | b'K') {
                    long_name = Some(decode_text(&buf).trim_end_matches('\0').trim().to_string());
                } else if let Some(p) = pax_path(&buf) {
                    long_name = Some(p);
                }
            }
            _ => {
                let name = long_name.take().filter(|s| !s.is_empty()).unwrap_or(h.name);
                let dir = h.flag == b'5' || name.ends_with('/');
                if !name.is_empty() {
                    if out.len() >= max_n {
                        cut = true;
                        break;
                    }
                    out.push(TarEntry { name, size: if dir { 0 } else { h.size }, modified: unix_when(h.mtime), dir });
                }
                if !discard(r, padded, &mut skipped, skip_cap) {
                    cut = true;
                    break;
                }
            }
        }
    }
    saw.then_some((out, cut))
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<()> {
    let mut at = 0;
    while at < buf.len() {
        match r.read(&mut buf[at..])? {
            0 => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short")),
            n => at += n,
        }
    }
    Ok(())
}

fn discard(r: &mut impl Read, mut n: u64, skipped: &mut u64, cap: u64) -> bool {
    let mut tmp = [0u8; 8192];
    while n > 0 {
        if *skipped >= cap {
            return false;
        }
        let want = n.min(tmp.len() as u64).min(cap - *skipped) as usize;
        let got = match r.read(&mut tmp[..want]) {
            Ok(0) | Err(_) => return false,
            Ok(v) => v,
        };
        n -= got as u64;
        *skipped += got as u64;
    }
    true
}

// ---------- DDS and ICO ----------

/// Uncompressed, DXT1, DXT3, or DXT5 DDS. The first mip only, sampled down so the bitmap stays small.
pub(crate) fn decode_dds(bytes: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
    if bytes.len() < 128 || &bytes[..4] != b"DDS " {
        return None;
    }
    let h = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let w = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
        return None;
    }
    let fourcc = &bytes[84..88];
    if fourcc == b"DX10" {
        return None;
    }
    let flags = u32::from_le_bytes(bytes[80..84].try_into().ok()?);
    let pixels = if fourcc == b"DXT1" || fourcc == b"DXT3" || fourcc == b"DXT5" {
        decode_dxt(bytes.get(128..)?, w, h, fourcc)?
    } else if flags & 0x40 != 0 {
        decode_dds_rgb(bytes, w, h)?
    } else {
        return None;
    };
    Some(sample_down(w, h, pixels))
}

fn decode_dds_rgb(bytes: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let bits = u32::from_le_bytes(bytes[88..92].try_into().ok()?) as usize;
    let bpp = bits / 8;
    if bpp != 3 && bpp != 4 {
        return None;
    }
    let masks = [
        u32::from_le_bytes(bytes[92..96].try_into().ok()?),
        u32::from_le_bytes(bytes[96..100].try_into().ok()?),
        u32::from_le_bytes(bytes[100..104].try_into().ok()?),
        u32::from_le_bytes(bytes[104..108].try_into().ok()?),
    ];
    let tight = (w * bpp + 3) & !3;
    let stated = u32::from_le_bytes(bytes[20..24].try_into().ok()?) as usize;
    let pitch = if stated >= w * bpp && stated <= tight + 64 { stated } else { tight };
    let data = bytes.get(128..)?;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row = data.get(y * pitch..)?;
        for x in 0..w {
            let p = x * bpp;
            if p + bpp > row.len() {
                return None;
            }
            let pixel = match bpp {
                4 => u32::from_le_bytes(row[p..p + 4].try_into().ok()?),
                _ => u32::from_le_bytes([row[p], row[p + 1], row[p + 2], 0]),
            };
            let r = chan(pixel, masks[0]);
            let g = chan(pixel, masks[1]);
            let b = chan(pixel, masks[2]);
            let a = if masks[3] == 0 { 255 } else { chan(pixel, masks[3]) };
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    Some(out)
}

fn chan(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let v = (pixel & mask) >> shift;
    if bits >= 8 {
        (v >> (bits - 8)) as u8
    } else if bits == 0 {
        0
    } else {
        ((v * 255) / ((1u32 << bits) - 1)) as u8
    }
}

fn decode_dxt(data: &[u8], w: usize, h: usize, fourcc: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; w * h * 4];
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let block = if fourcc == b"DXT1" { 8 } else { 16 };
    for by in 0..bh {
        for bx in 0..bw {
            let at = (by * bw + bx) * block;
            let chunk = data.get(at..at + block)?;
            let (color, alphas) = if fourcc == b"DXT1" {
                (dxt_color(chunk), None)
            } else if fourcc == b"DXT3" {
                (dxt_color(&chunk[8..]), Some(dxt3_alpha(chunk)))
            } else {
                (dxt_color(&chunk[8..]), Some(dxt5_alpha(chunk)))
            };
            let bits = u32::from_le_bytes(chunk[if fourcc == b"DXT1" { 4 } else { 12 }..if fourcc == b"DXT1" { 8 } else { 16 }].try_into().ok()?);
            for py in 0..4 {
                for px in 0..4 {
                    let x = bx * 4 + px;
                    let y = by * 4 + py;
                    if x >= w || y >= h {
                        continue;
                    }
                    let idx = ((bits >> (2 * (py * 4 + px))) & 3) as usize;
                    let mut pxc = color[idx];
                    if let Some(a) = alphas {
                        pxc[3] = a[py * 4 + px];
                    }
                    let o = (y * w + x) * 4;
                    out[o..o + 4].copy_from_slice(&pxc);
                }
            }
        }
    }
    Some(out)
}

fn dxt_color(b: &[u8]) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let a = rgb565(c0);
    let c = rgb565(c1);
    let mut o = [[0u8; 4]; 4];
    o[0] = [a[2], a[1], a[0], 255];
    o[1] = [c[2], c[1], c[0], 255];
    if c0 > c1 {
        o[2] = mix(o[0], o[1], 2, 1);
        o[3] = mix(o[0], o[1], 1, 2);
    } else {
        o[2] = mix(o[0], o[1], 1, 1);
        o[3] = [0, 0, 0, 0];
    }
    o
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = (c >> 11) & 31;
    let g = (c >> 5) & 63;
    let b = c & 31;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

fn mix(a: [u8; 4], b: [u8; 4], wa: u16, wb: u16) -> [u8; 4] {
    let m = |i: usize| ((a[i] as u16 * wa + b[i] as u16 * wb) / (wa + wb)) as u8;
    [m(0), m(1), m(2), 255]
}

fn dxt3_alpha(b: &[u8]) -> [u8; 16] {
    let mut o = [0u8; 16];
    for i in 0..16 {
        let n = if i % 2 == 0 { b[i / 2] & 0xf } else { b[i / 2] >> 4 };
        o[i] = n * 17;
    }
    o
}

fn dxt5_alpha(b: &[u8]) -> [u8; 16] {
    let a0 = b[0];
    let a1 = b[1];
    let mut bits = 0u64;
    for i in 0..6 {
        bits |= (b[2 + i] as u64) << (8 * i);
    }
    let mut table = [0u8; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            table[i + 1] = (((a0 as u16) * (7 - i as u16) + (a1 as u16) * (i as u16)) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            table[i + 1] = (((a0 as u16) * (5 - i as u16) + (a1 as u16) * (i as u16)) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut o = [0u8; 16];
    for i in 0..16 {
        o[i] = table[((bits >> (3 * i)) & 7) as usize];
    }
    o
}

fn sample_down(w: usize, h: usize, px: Vec<u8>) -> (i32, i32, Vec<u8>) {
    let mut step = 1usize;
    while w / step > 2048 || h / step > 2048 || (w / step).saturating_mul(h / step) > 2_000_000 {
        step += 1;
        if step > 32 {
            break;
        }
    }
    if step == 1 {
        return (w as i32, h as i32, px);
    }
    let dw = (w / step).max(1);
    let dh = (h / step).max(1);
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            let sx = (x * step).min(w - 1);
            let sy = (y * step).min(h - 1);
            let s = (sy * w + sx) * 4;
            let d = (y * dw + x) * 4;
            out[d..d + 4].copy_from_slice(&px[s..s + 4]);
        }
    }
    (dw as i32, dh as i32, out)
}

/// Largest BMP frame inside an ICO or CUR. PNG-compressed frames are left to WIC (`None` when every frame is PNG).
pub(crate) fn decode_ico(bytes: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
    if bytes.len() < 22 || bytes[0] != 0 || bytes[1] != 0 || !matches!(bytes[2], 1 | 2) || bytes[3] != 0 {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || 6 + count * 16 > bytes.len() {
        return None;
    }
    let mut best: Option<(usize, usize, usize, usize)> = None; // area, index, offset, size
    for i in 0..count {
        let e = 6 + i * 16;
        let mut w = bytes[e] as usize;
        let mut h = bytes[e + 1] as usize;
        if w == 0 {
            w = 256;
        }
        if h == 0 {
            h = 256;
        }
        let size = u32::from_le_bytes(bytes[e + 8..e + 12].try_into().ok()?) as usize;
        let off = u32::from_le_bytes(bytes[e + 12..e + 16].try_into().ok()?) as usize;
        let area = w.saturating_mul(h);
        if best.as_ref().is_none_or(|b| area > b.0) {
            best = Some((area, i, off, size));
        }
        let _ = i;
    }
    let (_, _, off, size) = best?;
    let image = bytes.get(off..off.checked_add(size)?)?;
    if image.starts_with(b"\x89PNG") {
        return None;
    }
    decode_ico_bmp(image)
}

fn decode_ico_bmp(image: &[u8]) -> Option<(i32, i32, Vec<u8>)> {
    if image.len() < 40 {
        return None;
    }
    let header = u32::from_le_bytes(image[0..4].try_into().ok()?) as usize;
    if header < 40 || header > image.len() {
        return None;
    }
    let w = i32::from_le_bytes(image[4..8].try_into().ok()?);
    let h_raw = i32::from_le_bytes(image[8..12].try_into().ok()?);
    let bits = u16::from_le_bytes(image[14..16].try_into().ok()?);
    if w <= 0 || h_raw == 0 || w > 1024 {
        return None;
    }
    let h = (h_raw.unsigned_abs() as usize) / 2;
    let w = w as usize;
    if h == 0 || h > 1024 || !matches!(bits, 24 | 32) {
        return None;
    }
    let bpp = (bits / 8) as usize;
    let stride = (w * bpp + 3) & !3;
    let pixels = image.get(header..)?;
    if pixels.len() < stride * h {
        return None;
    }
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_y = h - 1 - y; // bottom-up
        let row = &pixels[src_y * stride..src_y * stride + w * bpp];
        for x in 0..w {
            let p = x * bpp;
            let (b, g, r, a) = if bpp == 4 {
                (row[p], row[p + 1], row[p + 2], row[p + 3])
            } else {
                (row[p], row[p + 1], row[p + 2], 255)
            };
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    Some((w as i32, h as i32, out))
}

// ---------- .url ----------

/// `URL=` and `IconFile=` from an Internet shortcut. The match ignores spaces around `=` and letter case.
pub(crate) fn url_fields(text: &str) -> Option<(String, Option<String>)> {
    let mut url = None;
    let mut icon = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim();
        let val = v.trim();
        if val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("URL") {
            url = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("IconFile") {
            icon = Some(val.to_string());
        }
    }
    url.map(|u| (u, icon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn dates_round_trip_known_days() {
        assert_eq!(unix_when(0), "");
        assert_eq!(unix_when(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(dos_when(0, 0), "");
        // 2020-01-02 03:04 -> year 40, month 1, day 2, hour 3, minute 4
        let date = (40 << 9) | (1 << 5) | 2;
        let time = (3 << 11) | (4 << 5);
        assert_eq!(dos_when(date, time), "2020-01-02 03:04");
        let ft = (11_644_473_600u64 + 1_700_000_000) * 10_000_000;
        assert_eq!(filetime_when(ft), "2023-11-14 22:13 UTC");
    }

    #[test]
    fn rtf_skips_font_table_and_decodes_unicode_and_codepage() {
        let raw = br#"{\rtf1\ansi\ansicpg1252\uc1{\fonttbl{\f0\fswiss Arial;}}Hello \u54620?\'e9 world\par{\*\comment hidden}tail}"#;
        assert_eq!(rtf_text(raw), "Hello 한é world\ntail");
        let neg = br#"{\rtf1\ansi\uc1 \u-10916?}"#;
        assert_eq!(rtf_text(neg), "한");
        let kr = br#"{\rtf1\ansi\ansicpg949 \'c7\'d1\'b1\'db}"#;
        assert_eq!(rtf_text(kr), "한글");
    }

    #[test]
    fn odt_paragraphs_lists_and_spaces() {
        let xml = r#"<office:body><office:text>
            <text:h text:outline-level="1">Title</text:h>
            <text:p>Hello <text:s text:c="2"/>there</text:p>
            <text:list><text:list-item><text:p>one</text:p></text:list-item></text:list>
            <office:annotation><text:p>secret</text:p></office:annotation>
            </office:text></office:body>"#;
        let t = odt_text(xml);
        assert!(t.contains("Title"), "{t}");
        assert!(t.contains("Hello   there"), "{t}");
        assert!(t.contains("• one"), "{t}");
        assert!(!t.contains("secret"), "{t}");
    }

    #[test]
    fn ods_repeats_and_values_stay_bounded() {
        let xml = r#"<office:spreadsheet>
            <table:table table:name="Sheet1">
              <table:table-row>
                <table:table-cell office:value-type="string"><text:p>Name</text:p></table:table-cell>
                <table:table-cell office:value-type="float" office:value="1.5"><text:p>1.5</text:p></table:table-cell>
                <table:table-cell table:number-columns-repeated="3"/>
              </table:table-row>
              <table:table-row table:number-rows-repeated="100000">
                <table:table-cell/>
              </table:table-row>
              <table:table-row>
                <table:table-cell><text:p>only</text:p></table:table-cell>
              </table:table-row>
            </table:table>
            <table:table table:name="Other">
              <table:table-row><table:table-cell><text:p>B</text:p></table:table-cell></table:table-row>
            </table:table>
            </office:spreadsheet>"#;
        let sheets = ods_sheets(xml, 10, 4);
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].name, "Sheet1");
        assert_eq!(sheets[0].rows[0], vec!["Name", "1.5", "", ""]);
        assert!(sheets[0].rows.len() <= 10, "{}", sheets[0].rows.len());
        assert_eq!(sheets[0].rows.last().unwrap(), &vec!["only".to_string()]);
        assert_eq!(sheets[1].rows, vec![vec!["B"]]);
        assert!(!sheets[0].cut);
    }

    fn stored_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in files {
            let name = name.as_bytes();
            let data = data.as_bytes();
            let off = out.len() as u32;
            let mut local = vec![0x50, 0x4b, 3, 4, 20, 0, 0, 0, 0, 0];
            local.extend([0u8; 4]);
            local.extend([0u8; 4]);
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((name.len() as u16).to_le_bytes());
            local.extend(0u16.to_le_bytes());
            local.extend(name);
            out.extend(local);
            out.extend(data);
            let mut c = vec![0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0, 0, 0];
            c.extend([0u8; 4]);
            c.extend([0u8; 4]);
            c.extend((data.len() as u32).to_le_bytes());
            c.extend((data.len() as u32).to_le_bytes());
            c.extend((name.len() as u16).to_le_bytes());
            c.extend([0u8; 12]);
            c.extend(off.to_le_bytes());
            c.extend(name);
            cd.extend(c);
        }
        let start = out.len() as u32;
        out.extend(&cd);
        out.extend([0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend([0u8; 2]);
        out
    }

    #[test]
    fn epub_reads_spine_order_and_strips_tags() {
        let container = r#"<container><rootfiles><rootfile full-path="OEBPS/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = r#"<package>
            <metadata><dc:title>The Boat</dc:title><dc:creator>Kim</dc:creator></metadata>
            <manifest>
              <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
              <item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/>
              <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
            </manifest>
            <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
            </package>"#;
        let plan = epub_plan(container, opf, "OEBPS/book.opf");
        assert_eq!(plan.title, "The Boat");
        assert_eq!(plan.author, "Kim");
        assert_eq!(plan.chapters, vec!["OEBPS/Text/c1.xhtml", "OEBPS/c2.xhtml"]);
        let z = stored_zip(&[
            ("META-INF/container.xml", container),
            ("OEBPS/book.opf", opf),
            ("OEBPS/Text/c1.xhtml", "<html><body><h1>One</h1><p>Alpha &amp; beta</p></body></html>"),
            ("OEBPS/c2.xhtml", "<p>Two</p>"),
        ]);
        let (got, body, cut) = epub_body(&mut std::io::Cursor::new(z), 10_000).unwrap();
        assert_eq!(got.title, "The Boat");
        assert!(!cut);
        assert!(body.contains("One") && body.contains("Alpha & beta") && body.contains("Two"), "{body}");
        assert!(body.find("One") < body.find("Two"), "{body}");
    }

    fn u16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).chain([0, 0]).collect()
    }

    #[test]
    fn msg_properties_and_attachments() {
        let ft = (11_644_473_600u64 + 1_700_000_000) * 10_000_000;
        let streams = vec![
            ("__substg1.0_0037001F".into(), u16le("안녕")),
            ("__substg1.0_0C1A001F".into(), u16le("Kim")),
            ("__substg1.0_0065001F".into(), u16le("kim@ex.com")),
            ("__substg1.0_0E04001F".into(), u16le("Lee")),
            ("__substg1.0_00390040".into(), ft.to_le_bytes().to_vec()),
            ("__substg1.0_1000001F".into(), u16le("body line")),
            ("__attach_version1.0_#00000000/__substg1.0_3707001F".into(), u16le("notes.pdf")),
            ("__substg1.0_3FDE0003".into(), 949u32.to_le_bytes().to_vec()),
        ];
        let m = msg_parts(&streams);
        assert_eq!(m.subject, "안녕");
        assert_eq!(m.from, "Kim <kim@ex.com>");
        assert_eq!(m.to, "Lee");
        assert_eq!(m.date, "2023-11-14 22:13 UTC");
        assert_eq!(m.body, "body line");
        assert_eq!(m.files, vec!["notes.pdf".to_string()]);
    }

    fn put_octal(dst: &mut [u8], val: u64) {
        let s = format!("{:0width$o}", val, width = dst.len() - 1);
        let b = s.as_bytes();
        let n = b.len().min(dst.len() - 1);
        dst[..n].copy_from_slice(&b[..n]);
        dst[n] = 0;
    }

    fn tar_member(name: &str, data: &[u8], mtime: u64, flag: u8) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        let nb = name.as_bytes();
        b[..nb.len().min(100)].copy_from_slice(&nb[..nb.len().min(100)]);
        put_octal(&mut b[100..108], 0o644);
        put_octal(&mut b[124..136], data.len() as u64);
        put_octal(&mut b[136..148], mtime);
        b[156] = flag;
        b[257..262].copy_from_slice(b"ustar");
        for i in 148..156 {
            b[i] = b' ';
        }
        let sum: u32 = b.iter().map(|&c| c as u32).sum();
        let cs = format!("{sum:06o}\0 ");
        b[148..156].copy_from_slice(cs.as_bytes());
        b.extend(data);
        let pad = (512 - (data.len() % 512)) % 512;
        b.extend(std::iter::repeat(0u8).take(pad));
        b
    }

    #[test]
    fn tar_lists_names_sizes_and_stops() {
        let mut bytes = tar_member("dir/a.txt", b"hello", 1_700_000_000, b'0');
        bytes.extend(tar_member("b.txt", b"xy", 0, b'0'));
        bytes.extend([0u8; 1024]);
        let (es, cut) = tar_entries(&mut std::io::Cursor::new(&bytes), 10).unwrap();
        assert!(!cut);
        assert_eq!(es[0].name, "dir/a.txt");
        assert_eq!(es[0].size, 5);
        assert_eq!(es[0].modified, "2023-11-14 22:13 UTC");
        assert_eq!(es[1].name, "b.txt");
        assert!(es[1].modified.is_empty());
        let (few, cut) = tar_entries(&mut std::io::Cursor::new(&bytes), 1).unwrap();
        assert!(cut && few.len() == 1);
        assert!(tar_entries(&mut std::io::Cursor::new(b"not a tar".to_vec()), 10).is_none());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    fn gzip_wrap(name: &str, data: &[u8], mtime: u32) -> Vec<u8> {
        let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);
        let mut out = vec![0x1f, 0x8b, 8, 0x08];
        out.extend(mtime.to_le_bytes());
        out.extend([0, 0]);
        out.extend(name.as_bytes());
        out.push(0);
        out.extend(deflated);
        out.extend(crc32(data).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn gzip_header_name_and_capped_inflate() {
        let gz = gzip_wrap("notes.txt", b"hello gzip", 1_700_000_000);
        let meta = gzip_meta(&gz).unwrap();
        assert_eq!(meta.name, "notes.txt");
        assert_eq!(meta.mtime, 1_700_000_000);
        assert_eq!(gzip_isize(&gz), Some(10));
        let mut inf = CapInflate::new(&gz[meta.data_at..], 100);
        let mut got = Vec::new();
        inf.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"hello gzip");
        assert!(!inf.hit_cap());
        let big = vec![7u8; 50_000];
        let gz = gzip_wrap("big.bin", &big, 1);
        let meta = gzip_meta(&gz).unwrap();
        let mut inf = CapInflate::new(&gz[meta.data_at..], 1000);
        let mut got = Vec::new();
        inf.read_to_end(&mut got).unwrap();
        assert!(got.len() <= 1000 + 16 * 1024, "{}", got.len());
        assert!(got.len() >= 1000, "{}", got.len());
        assert!(inf.hit_cap());
    }

    #[test]
    fn gzip_tar_streams_headers() {
        let mut tar = tar_member("a.txt", b"hi", 10, b'0');
        tar.extend(tar_member("b.txt", b"there", 10, b'0'));
        tar.extend([0u8; 1024]);
        let gz = gzip_wrap("pack.tar", &tar, 5);
        let meta = gzip_meta(&gz).unwrap();
        let mut inf = CapInflate::new(&gz[meta.data_at..], 1 << 20);
        let (es, cut) = tar_entries_read(&mut inf, 10, 1 << 20).unwrap();
        assert!(!cut);
        assert_eq!(es.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn dds_uncompressed_and_dxt1_solid() {
        let mut dds = vec![0u8; 128];
        dds[..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&2u32.to_le_bytes());
        dds[16..20].copy_from_slice(&2u32.to_le_bytes());
        dds[20..24].copy_from_slice(&8u32.to_le_bytes());
        dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        dds[80..84].copy_from_slice(&0x41u32.to_le_bytes()); // RGB + alpha
        dds[88..92].copy_from_slice(&32u32.to_le_bytes());
        dds[92..96].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
        dds[96..100].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
        dds[100..104].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
        dds[104..108].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
        // two by two opaque red, BGRA
        for _ in 0..4 {
            dds.extend([0, 0, 255, 255]);
        }
        let (w, h, px) = decode_dds(&dds).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&px[0..4], &[0, 0, 255, 255]);

        let mut dxt = vec![0u8; 128];
        dxt[..4].copy_from_slice(b"DDS ");
        dxt[4..8].copy_from_slice(&124u32.to_le_bytes());
        dxt[12..16].copy_from_slice(&4u32.to_le_bytes());
        dxt[16..20].copy_from_slice(&4u32.to_le_bytes());
        dxt[84..88].copy_from_slice(b"DXT1");
        // c0 = c1 = full red 565, indices 0
        let red = 0xF800u16;
        dxt.extend(red.to_le_bytes());
        dxt.extend(red.to_le_bytes());
        dxt.extend([0, 0, 0, 0]);
        let (w, h, px) = decode_dds(&dxt).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&px[0..4], &[0, 0, 255, 255]);
    }

    fn ico_bmp(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(w as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&((h as i32) * 2).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        for _y in 0..h {
            for _x in 0..w {
                dib.extend(bgra);
            }
        }
        let mask_stride = ((w + 31) / 32) * 4;
        dib.extend(std::iter::repeat(0u8).take(mask_stride * h));
        dib
    }

    #[test]
    fn ico_picks_the_largest_frame() {
        let small = ico_bmp(2, 2, [0, 0, 255, 255]); // red
        let large = ico_bmp(4, 4, [255, 0, 0, 255]); // blue
        let mut file = vec![0, 0, 1, 0, 2, 0];
        let mut entry = |bmp: &[u8], wh: u8, off: u32| {
            file.push(wh);
            file.push(wh);
            file.extend([0, 0, 1, 0, 32, 0]);
            file.extend((bmp.len() as u32).to_le_bytes());
            file.extend(off.to_le_bytes());
        };
        let off_small = 6 + 32;
        entry(&small, 2, off_small);
        entry(&large, 4, off_small + small.len() as u32);
        file.extend(&small);
        file.extend(&large);
        let (w, h, px) = decode_ico(&file).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert!(decode_ico(b"not ico").is_none());
    }

    #[test]
    fn url_ini_reads_url_and_icon() {
        let t = "[InternetShortcut]\r\nURL = https://example.com/a\r\nIconFile=C:\\a.ico\r\n";
        assert_eq!(url_fields(t), Some(("https://example.com/a".into(), Some("C:\\a.ico".into()))));
        assert!(url_fields("IconFile=x").is_none());
    }

    #[test]
    fn column_cap_keeps_the_first_sixty_four() {
        let wide: Vec<String> = (0..80).map(|n| n.to_string()).collect();
        let rows = cap_columns(vec![wide]);
        assert_eq!(rows[0].len(), 64);
        assert_eq!(rows[0][0], "0");
        assert_eq!(rows[0][63], "63");
    }
}
