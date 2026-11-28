//! The predefined encodings of ISO 32000-1 Annex D as 256 glyph names indexed by character code, the Adobe Glyph
//! List with its rules for suffixes, ligature components and the uniXXXX and uXXXX forms, and the embedded
//! tables' decoder.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::data;

/// The embedded tables are raw-deflated and base64 encoded; this undoes both. The input is compiled in, never a
/// file's.
pub(crate) fn inflate_blob(base64: &str) -> Vec<u8> {
    let mut packed = Vec::with_capacity(base64.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for c in base64.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buffer = buffer << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            packed.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    miniz_oxide::inflate::decompress_to_vec(&packed).expect("The embedded table does not inflate.")
}

pub(crate) fn inflate_text(base64: &str) -> String {
    String::from_utf8_lossy(&inflate_blob(base64)).into_owned()
}

pub type Encoding = [Option<&'static str>; 256];

fn parse(runs: &'static str) -> Encoding {
    let mut names: Encoding = [None; 256];
    let mut code = 0usize;
    for token in runs.split_whitespace() {
        let mut name = token;
        if let Some(eq) = token.find('=')
            && eq != 0
        {
            code = token[..eq].parse().expect("Bad run in the encoding table.");
            name = &token[eq + 1..];
        }
        names[code] = Some(name);
        code += 1;
    }
    names
}

fn table(cell: &'static OnceLock<Encoding>, runs: &'static str) -> &'static Encoding {
    cell.get_or_init(|| parse(runs))
}

pub fn standard() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_STANDARD)
}

/// The codes Annex D leaves unused show a bullet, as the table's footnote says.
pub fn win_ansi() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_WIN_ANSI)
}

pub fn mac_roman() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_MAC_ROMAN)
}

pub fn mac_expert() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_MAC_EXPERT)
}

/// PDFDocEncoding, the one of text strings outside content streams (Annex D.3).
pub fn pdf_doc() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_PDF_DOC)
}

/// The built-in encoding of the Symbol font.
pub fn symbol() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_SYMBOL)
}

/// The built-in encoding of ZapfDingbats.
pub fn zapf_dingbats() -> &'static Encoding {
    static CELL: OnceLock<Encoding> = OnceLock::new();
    table(&CELL, data::ENCODING_ZAPF_DINGBATS)
}

/// The encoding a font's /Encoding name stands for, or none for a name that is none of them.
pub fn encoding_by_name(pdf_name: &str) -> Option<&'static Encoding> {
    match pdf_name {
        "StandardEncoding" => Some(standard()),
        "WinAnsiEncoding" => Some(win_ansi()),
        "MacRomanEncoding" => Some(mac_roman()),
        "MacExpertEncoding" => Some(mac_expert()),
        "PDFDocEncoding" => Some(pdf_doc()),
        _ => None,
    }
}

/// The first code of `encoding` that shows the glyph, or -1.
pub fn code_of(encoding: &[Option<&str>], glyph_name: &str) -> i64 {
    encoding
        .iter()
        .position(|n| *n == Some(glyph_name))
        .map_or(-1, |c| c as i64)
}

const MAX_NAME_LENGTH: usize = 256;

// Entries keep the order of the list: a code point with several names takes the first one.
#[derive(Default)]
struct List {
    names: Vec<String>,
    texts: HashMap<String, String>,
}

struct Lists {
    agl: List,
    zapf: List,
    names: HashMap<u32, String>,
    zapf_names: HashMap<u32, String>,
}

fn lists() -> &'static Lists {
    static CELL: OnceLock<Lists> = OnceLock::new();
    CELL.get_or_init(|| {
        let agl = read_list(data::GLYPH_LIST_AGL);
        let zapf = read_list(data::GLYPH_LIST_ZAPF);
        let names = build_names(&agl);
        let zapf_names = reverse(&zapf, &[]);
        Lists {
            agl,
            zapf,
            names,
            zapf_names,
        }
    })
}

/// The text a glyph name stands for, or none when it stands for none (".notdef", "g123", "cid77"). A suffix
/// after the first period is dropped and "f_i" is the concatenation of "f" and "i".
pub fn unicode_of_glyph_name(glyph_name: &str) -> Option<String> {
    if glyph_name.is_empty() || glyph_name.encode_utf16().count() > MAX_NAME_LENGTH {
        return None;
    }
    let name = glyph_name.split('.').next().unwrap_or("");
    if name.is_empty() {
        return None;
    }
    if !name.contains('_') {
        return component(name);
    }

    let mut text = String::new();
    for part in name.split('_') {
        text += &component(part).unwrap_or_default();
    }
    if text.is_empty() { None } else { Some(text) }
}

/// A glyph name for the character: the AGL name, preferring the one the standard encodings use (space, not
/// nbspace), else uniXXXX or uXXXXX. ".notdef" for what is not a Unicode scalar value.
pub fn glyph_name_of(code_point: i64) -> String {
    let Some(c) = u32::try_from(code_point).ok().and_then(char::from_u32) else {
        return ".notdef".into();
    };
    if let Some(name) = lists().names.get(&(c as u32)) {
        return name.clone();
    }
    if (c as u32) <= 0xFFFF {
        format!("uni{:04X}", c as u32)
    } else {
        format!("u{:X}", c as u32)
    }
}

/// The ZapfDingbats glyph name ("a1") of the character, or none.
pub fn zapf_name_of(code_point: i64) -> Option<String> {
    u32::try_from(code_point)
        .ok()
        .and_then(|c| lists().zapf_names.get(&c).cloned())
}

fn component(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let l = lists();
    if let Some(text) = l.agl.texts.get(name).or_else(|| l.zapf.texts.get(name)) {
        return Some(text.clone());
    }
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.starts_with(&[0x75, 0x6E, 0x69])
        && units.len() > 3
        && (units.len() - 3).is_multiple_of(4)
    {
        let mut text = String::new();
        for start in (3..units.len()).step_by(4) {
            let unit = hex_value(&units[start..start + 4])?;
            text.push(char::from_u32(unit)?);
        }
        return Some(text);
    }
    if units[0] == 0x75 && (5..=7).contains(&units.len()) {
        let value = hex_value(&units[1..])?;
        return char::from_u32(value).map(String::from);
    }
    None
}

// The int parser accepts a sign and spaces, a glyph name must not.
fn hex_value(units: &[u16]) -> Option<u32> {
    let mut value = 0u32;
    for &unit in units {
        let digit = match unit {
            0x30..=0x39 => u32::from(unit) - 0x30,
            0x41..=0x46 => u32::from(unit) - 0x41 + 10,
            0x61..=0x66 => u32::from(unit) - 0x61 + 10,
            _ => return None,
        };
        value = value << 4 | digit;
    }
    Some(value)
}

fn read_list(packed: &str) -> List {
    let mut list = List::default();
    for line in inflate_text(packed).split('\n') {
        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() < 2 {
            continue;
        }
        let mut text = String::new();
        for part in &parts[1..] {
            let value = u32::from_str_radix(part, 16).expect("Bad entry in the glyph list.");
            text.push(char::from_u32(value).expect("Bad entry in the glyph list."));
        }
        let name = parts[0].to_string();
        if !list.texts.contains_key(&name) {
            list.names.push(name.clone());
            list.texts.insert(name, text);
        }
    }
    list
}

fn build_names(agl: &List) -> HashMap<u32, String> {
    let mut preferred: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for table in [
        standard(),
        win_ansi(),
        mac_roman(),
        mac_expert(),
        pdf_doc(),
        symbol(),
    ] {
        for name in table.iter().flatten() {
            if seen.insert(*name) {
                preferred.push((*name).to_string());
            }
        }
    }
    reverse(agl, &preferred)
}

// Single-character entries only; a name from preferred wins over the first one in list order.
fn reverse(list: &List, preferred: &[String]) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for name in preferred.iter().chain(&list.names) {
        if let Some(text) = list.texts.get(name)
            && let Some(cp) = single(text)
        {
            map.entry(cp).or_insert_with(|| name.clone());
        }
    }
    map
}

fn single(text: &str) -> Option<u32> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first as u32)
}
