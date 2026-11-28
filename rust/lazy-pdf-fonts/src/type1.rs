//! A Type 1 font program (Adobe Type 1 Font Format) in PFA, PFB or the bare form of a PDF's FontFile: the
//! clear-text header, then the eexec-encrypted private part with Subrs and CharStrings.

use std::collections::HashMap;

use crate::bytes::{Span, parse_double};
use crate::names::standard_encoding;
use crate::outline::{Bounds, Outline};
use crate::type1_interp::Interpreter;

const MAX_GLYPHS: usize = 65536;
const MAX_SUBRS: i64 = 65536;

pub struct Type1Font {
    char_strings: HashMap<String, Vec<u8>>,
    pub(crate) subrs: Vec<Option<Vec<u8>>>,
    pub font_name: String,
    /// FontMatrix, 0.001 0 0 0.001 0 0 when the header has none.
    pub font_matrix: [f64; 6],
    pub font_bounds: Option<Bounds>,
    /// The built-in encoding: the glyph name of each code, none where none is assigned.
    pub built_in_encoding: Vec<Option<String>>,
    /// Glyph names in file order.
    pub glyph_names: Vec<String>,
}

pub(crate) struct Glyph {
    pub outline: Outline,
    pub width: f64,
}

impl Type1Font {
    pub fn has_glyph(&self, name: &str) -> bool {
        self.char_strings.contains_key(name)
    }

    pub(crate) fn char_string(&self, name: &str) -> Option<&Vec<u8>> {
        self.char_strings.get(name)
    }

    /// Advance width of the named glyph in font units, 0 when the glyph does not exist.
    pub fn width(&self, name: &str) -> f64 {
        self.run(name, true).map_or(0.0, |g| g.width)
    }

    /// Outline of the named glyph in font units (cubic curves), none when it does not exist. Flex sequences
    /// become two curves and accented characters (seac) are composed.
    pub fn outline(&self, name: &str) -> Option<Outline> {
        self.run(name, true).map(|g| g.outline)
    }

    pub(crate) fn run(&self, name: &str, allow_seac: bool) -> Option<Glyph> {
        if self.char_strings.contains_key(name) {
            Some(Interpreter::new(self, allow_seac).run(name))
        } else {
            None
        }
    }

    /// Parses a PFA, PFB or FontFile stream; none when no glyph can be found in it.
    pub fn parse(data: &[u8]) -> Option<Type1Font> {
        let (clear, encrypted) = split_segments(data);
        let mut font = Type1Font {
            char_strings: HashMap::new(),
            subrs: Vec::new(),
            font_name: String::new(),
            font_matrix: [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
            font_bounds: None,
            built_in_encoding: vec![None; 256],
            glyph_names: Vec::new(),
        };
        font.read_header(&clear);

        let private = if encrypted.is_empty() {
            clear
        } else {
            decrypt(&encrypted, 55665, 4)
        };
        font.read_private(&private);
        if font.char_strings.is_empty() {
            None
        } else {
            Some(font)
        }
    }

    fn read_header(&mut self, clear: &[u8]) {
        let mut s = Scanner::new(clear);
        if s.seek_after("/FontName")
            && let Some(name) = s.next()
            && name.starts_with('/')
        {
            self.font_name = without_slash(&name);
        }

        let mut s = Scanner::new(clear);
        if s.seek_after("/FontMatrix")
            && let Some(m) = s.numbers()
            && m.len() == 6
        {
            self.font_matrix = [m[0], m[1], m[2], m[3], m[4], m[5]];
        }

        let mut s = Scanner::new(clear);
        if s.seek_after("/FontBBox")
            && let Some(b) = s.numbers()
            && b.len() == 4
        {
            self.font_bounds = Some(Bounds {
                x_min: b[0],
                y_min: b[1],
                x_max: b[2],
                y_max: b[3],
            });
        }

        let mut s = Scanner::new(clear);
        while s.seek_after("/Encoding") {
            if self.read_encoding(&mut s) {
                break;
            }
        }
    }

    /// False when the key was not the encoding entry (a string in the notice can hold the same word).
    fn read_encoding(&mut self, s: &mut Scanner) -> bool {
        let first = s.next();
        if first.as_deref() == Some("StandardEncoding") {
            self.built_in_encoding = (0..256)
                .map(|c| standard_encoding(c).map(str::to_string))
                .collect();
            return true;
        }
        if parse_int32(first.as_deref()).is_none() {
            return false;
        }
        for _ in 0..8192 {
            let Some(t) = s.next() else { break };
            if t == "def" {
                break;
            }
            if t != "dup" {
                continue;
            }
            let code = s.next();
            let name = s.next();
            if let (Some(code), Some(name)) = (code, name)
                && name.starts_with('/')
                && let Some(c) = parse_code(&code)
                && (0..256).contains(&c)
            {
                self.built_in_encoding[c as usize] = Some(without_slash(&name));
            }
        }
        true
    }

    fn read_private(&mut self, d: &[u8]) {
        let mut len_iv = 4;
        let mut s = Scanner::new(d);
        if s.seek_after("/lenIV")
            && let Some(iv) = parse_int32(s.next().as_deref())
            && (-1..=64).contains(&iv)
        {
            len_iv = iv;
        }

        let mut s = Scanner::new(d);
        if s.seek_after("/Subrs")
            && let Some(count) = parse_int32(s.next().as_deref())
            && count > 0
            && count <= MAX_SUBRS
        {
            self.subrs = vec![None; count as usize];
            self.read_subrs(&mut s, len_iv);
        }

        let mut s = Scanner::new(d);
        if s.seek_after("/CharStrings") {
            self.read_char_strings(&mut s, len_iv);
        }
    }

    fn read_subrs(&mut self, s: &mut Scanner, len_iv: i64) {
        while let Some(t) = s.next() {
            if t.starts_with('/') {
                break;
            }
            if t != "dup" {
                continue;
            }
            let Some(index) = parse_int32(s.next().as_deref()) else {
                break;
            };
            let len = s.next_int();
            if s.next().is_none() {
                break;
            }
            let Some(bytes) = s.binary(len) else { break };
            if index >= 0 && (index as usize) < self.subrs.len() {
                self.subrs[index as usize] = Some(unlock(&bytes, len_iv));
            }
        }
    }

    fn read_char_strings(&mut self, s: &mut Scanner, len_iv: i64) {
        while let Some(t) = s.next() {
            if t == "end" || self.glyph_names.len() >= MAX_GLYPHS {
                break;
            }
            if !t.starts_with('/') {
                continue;
            }
            let len = s.next_int();
            if s.next().is_none() {
                break;
            }
            let Some(bytes) = s.binary(len) else { break };
            let name = without_slash(&t);
            if !self.char_strings.contains_key(&name) {
                self.char_strings
                    .insert(name.clone(), unlock(&bytes, len_iv));
                self.glyph_names.push(name);
            }
        }
    }
}

fn without_slash(text: &str) -> String {
    text.chars().skip(1).collect()
}

fn is_white(c: char) -> bool {
    c == ' ' || ('\t'..='\r').contains(&c)
}

/// `int.TryParse`: a sign and digits, white space around them allowed, the range of an i32.
fn parse_int32(text: Option<&str>) -> Option<i64> {
    let text = text?.trim_matches(is_white);
    text.parse::<i32>().ok().map(i64::from)
}

fn parse_code(token: &str) -> Option<i64> {
    if let Some(hash) = token.find('#')
        && hash > 0
    {
        let radix = parse_int32(Some(&token[..hash]))?;
        if !(2..=36).contains(&radix) {
            return None;
        }
        return radix_value(&token[hash + 1..], radix);
    }
    // NumberStyles.None: digits only.
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    token.parse::<i32>().ok().map(i64::from)
}

fn radix_value(digits: &str, radix: i64) -> Option<i64> {
    let mut v = 0i64;
    for ch in digits.chars() {
        let d = match ch {
            '0'..='9' => ch as i64 - '0' as i64,
            'a'..='z' => ch as i64 - 'a' as i64 + 10,
            'A'..='Z' => ch as i64 - 'A' as i64 + 10,
            _ => 99,
        };
        if d >= radix {
            return None;
        }
        v = v * radix + d;
        if v > 100_000 {
            return None;
        }
    }
    Some(v)
}

fn unlock(cipher: &[u8], len_iv: i64) -> Vec<u8> {
    if len_iv < 0 {
        cipher.to_vec()
    } else {
        decrypt(cipher, 4330, len_iv as usize)
    }
}

/// The eexec and charstring cipher (Type 1 Font Format, chapter 7); the first `skip` bytes are random padding.
pub(crate) fn decrypt(cipher: &[u8], key: u16, skip: usize) -> Vec<u8> {
    if cipher.len() <= skip {
        return Vec::new();
    }
    let mut plain = vec![0u8; cipher.len() - skip];
    let mut r = key;
    for (i, &c) in cipher.iter().enumerate() {
        if i >= skip {
            plain[i - skip] = c ^ (r >> 8) as u8;
        }
        r = (u16::from(c).wrapping_add(r))
            .wrapping_mul(52845)
            .wrapping_add(22719);
    }
    plain
}

fn split_segments(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    if data.len() > 6 && data[0] == 0x80 {
        let mut c = Vec::new();
        let mut e = Vec::new();
        let mut p = 0;
        while p + 6 <= data.len() && data[p] == 0x80 && (data[p + 1] == 1 || data[p + 1] == 2) {
            // The length is little-endian.
            let len = u32::from_le_bytes([data[p + 2], data[p + 3], data[p + 4], data[p + 5]]);
            let n = (len as usize).min(data.len() - p - 6);
            if data[p + 1] == 1 {
                if e.is_empty() {
                    c.extend_from_slice(&data[p + 6..p + 6 + n]);
                }
            } else {
                e.extend_from_slice(&data[p + 6..p + 6 + n]);
            }
            p += 6 + n;
        }
        return (c, e);
    }

    let key = b"eexec";
    let Some(at) = data.windows(key.len()).position(|w| w == key) else {
        return (data.to_vec(), Vec::new());
    };
    let mut start = at + 5;
    let clear = data[..start].to_vec();
    while start < data.len() && matches!(data[start], 0x20 | 0x09 | 0x0D | 0x0A) {
        start += 1;
    }
    let rest = &data[start..];
    let body = if is_hex(rest) {
        decode_hex(rest)
    } else {
        rest.to_vec()
    };
    (clear, body)
}

fn is_hex(d: &[u8]) -> bool {
    d.len() >= 4 && d[..4].iter().all(u8::is_ascii_hexdigit)
}

fn decode_hex(d: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(d.len() / 2);
    let mut high: Option<u8> = None;
    for &b in d {
        if matches!(b, 0x20 | 0x09 | 0x0D | 0x0A) {
            continue;
        }
        if !b.is_ascii_hexdigit() {
            break;
        }
        let v = (b as char).to_digit(16).unwrap_or(0) as u8;
        match high.take() {
            None => high = Some(v),
            Some(h) => result.push(h << 4 | v),
        }
    }
    result
}

/// A PostScript token reader over the font text; binary charstrings are skipped by length.
struct Scanner<'a> {
    d: Span<'a>,
    p: i64,
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        0x28 | 0x29 | 0x3C | 0x3E | 0x5B | 0x5D | 0x7B | 0x7D | 0x2F | 0x25
    )
}

impl<'a> Scanner<'a> {
    fn new(d: &'a [u8]) -> Self {
        Scanner { d: Span(d), p: 0 }
    }

    fn seek_after(&mut self, key: &str) -> bool {
        let k = key.as_bytes();
        let mut i = self.p;
        while i + k.len() as i64 <= self.d.len() {
            if self.d.0[i as usize..].starts_with(k) {
                self.p = i + k.len() as i64;
                return true;
            }
            i += 1;
        }
        false
    }

    fn next(&mut self) -> Option<String> {
        let d = self.d.0;
        let len = d.len();
        let mut p = self.p as usize;
        while p < len {
            if is_space(d[p]) {
                p += 1;
            } else if d[p] == 0x25 {
                while p < len && d[p] != 10 && d[p] != 13 {
                    p += 1;
                }
            } else {
                break;
            }
        }
        if p >= len {
            self.p = p as i64;
            return None;
        }

        let start = p;
        p += 1;
        if d[start] == 0x2F || !is_delimiter(d[start]) {
            while p < len && !is_space(d[p]) && !is_delimiter(d[p]) {
                p += 1;
            }
        }
        self.p = p as i64;
        Some(lazy_pdf_syntax::latin1(&d[start..p]))
    }

    fn next_int(&mut self) -> i64 {
        // NumberStyles.AllowLeadingSign: no white space around the digits.
        match self.next().and_then(|t| t.parse::<i32>().ok()) {
            Some(v) => i64::from(v),
            None => -1,
        }
    }

    /// The bytes after the RD token: one separator, then `length` bytes. None when they do not fit.
    fn binary(&mut self, length: i64) -> Option<Vec<u8>> {
        if length < 0 || length > self.d.len() - self.p - 1 {
            return None;
        }
        let bytes = self.d.0[(self.p + 1) as usize..(self.p + 1 + length) as usize].to_vec();
        self.p += 1 + length;
        Some(bytes)
    }

    /// Numbers of a [ ... ] or { ... } list, none when the list is missing or holds anything else.
    fn numbers(&mut self) -> Option<Vec<f64>> {
        let bracket = self.next()?;
        if bracket != "[" && bracket != "{" {
            return None;
        }
        let mut list = Vec::new();
        while let Some(t) = self.next() {
            if t == "]" || t == "}" {
                break;
            }
            let v = parse_double(&t)?;
            if list.len() >= 16 {
                return None;
            }
            list.push(v);
        }
        Some(list)
    }
}
