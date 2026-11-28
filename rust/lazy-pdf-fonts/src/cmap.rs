//! A CMap (ISO 32000-1 9.7.5, 9.10.3): the codespace that cuts a string into character codes, and what the codes
//! map to, character ids for a composite font or text for a ToUnicode map. Codes are at most four bytes; a
//! four-byte code above 0x7FFFFFFF is a negative number. Text is kept as UTF-16 units so that the last unit of a
//! bfrange can step; a lone surrogate in a destination string reads back as U+FFFD.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::data::cmap_blob;
use crate::encodings::{inflate_blob, unicode_of_glyph_name};

const MAX_CODESPACES: usize = 100;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Copy)]
struct Codespace {
    length: usize,
    lo: u32,
    hi: u32,
}

/// Ranges of character codes with a value, searched by binary search. A code is keyed with its byte length, since
/// <41> and <0041> are different codes in a CMap with mixed code lengths.
struct CodeRanges<V> {
    added: Vec<Range<V>>,
    sorted: Vec<Range<V>>,
    length_mask: u32,
}

struct Range<V> {
    lo: u64,
    hi: u64,
    order: usize,
    value: V,
}

// Overlapping ranges are a defect of the file; looking a few back finds the one nested in a wider one.
const OVERLAP_PROBE: i64 = 4;

impl<V: Clone> CodeRanges<V> {
    fn new() -> Self {
        CodeRanges {
            added: Vec::new(),
            sorted: Vec::new(),
            length_mask: 0,
        }
    }

    fn count(&self) -> usize {
        self.added.len() + self.sorted.len()
    }

    fn add(&mut self, length: usize, lo: u32, hi: u32, value: V) {
        let key = (length as u64) << 32;
        let order = self.added.len();
        self.added.push(Range {
            lo: key | u64::from(lo),
            hi: key | u64::from(hi),
            order,
            value,
        });
        self.length_mask |= 1 << length;
    }

    /// Sorts what was added. Later additions win over earlier ones that cover the same code.
    fn freeze(&mut self) {
        let mut sorted = std::mem::take(&mut self.added);
        sorted.sort_by(|a, b| a.lo.cmp(&b.lo).then(a.order.cmp(&b.order)));
        self.sorted = sorted;
    }

    fn try_find(&self, length: usize, code: u32) -> Option<(u32, V)> {
        let key = (length as u64) << 32 | u64::from(code);
        let (mut low, mut high, mut at) = (0i64, self.sorted.len() as i64 - 1, -1i64);
        while low <= high {
            let mid = (low + high) >> 1;
            if self.sorted[mid as usize].lo <= key {
                at = mid;
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }
        let mut i = at;
        while i >= 0 && i > at - OVERLAP_PROBE {
            let r = &self.sorted[i as usize];
            if r.hi >= key {
                return Some((r.lo as u32, r.value.clone()));
            }
            i -= 1;
        }
        None
    }
}

pub struct CMap {
    cids: CodeRanges<i64>,
    notdefs: CodeRanges<i64>,
    texts: CodeRanges<Vec<u16>>,
    space_list: Vec<Codespace>,
    spaces: Vec<Codespace>,
    parent: Option<Arc<CMap>>,
    is_identity: bool,
    codes_are_unicode: bool,
    /// The CMapName, when the file gives one.
    pub name: Option<String>,
    /// Writing mode 1: glyphs advance downwards (/WMode).
    pub is_vertical: bool,
    /// True for a predefined name this library has no table for; its codes are read as two bytes and its CIDs as
    /// the codes.
    pub is_fallback: bool,
}

fn predefined_cache() -> &'static Mutex<HashMap<String, Arc<CMap>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<CMap>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl CMap {
    fn empty() -> CMap {
        CMap {
            cids: CodeRanges::new(),
            notdefs: CodeRanges::new(),
            texts: CodeRanges::new(),
            space_list: Vec::new(),
            spaces: Vec::new(),
            parent: None,
            is_identity: false,
            codes_are_unicode: false,
            name: None,
            is_vertical: false,
            is_fallback: false,
        }
    }

    fn full(&self) -> bool {
        self.cids.count() + self.notdefs.count() + self.texts.count() >= MAX_ENTRIES
    }

    /// Reads an embedded CMap or a ToUnicode stream. Malformed parts are skipped and what came before them kept;
    /// the work is linear in the data and at most a million mappings are kept. The stream's /UseCMap, when it has
    /// one, is passed as `use_cmap`; a "usecmap" in the data names a predefined CMap and takes its place.
    pub fn parse(data: &[u8], use_cmap: Option<Arc<CMap>>) -> CMap {
        let mut map = CMap::empty();
        map.parent = use_cmap;
        run(data, &mut map);
        map.finish();
        map
    }

    /// The predefined CMap of that name: Identity-H and Identity-V, and Adobe's CJK ones that map to CIDs
    /// (UniGB-UCS2-H, 90ms-RKSJ-H, ETen-B5-V and the rest of the Japan1, GB1, CNS1 and Korea1 sets). Any other
    /// name gives a two-byte fallback with `is_fallback` set.
    pub fn predefined(name: &str) -> Arc<CMap> {
        if let Some(known) = predefined_cache().lock().expect("cmap cache").get(name) {
            return known.clone();
        }
        let loaded = if name == "Identity-H" || name == "Identity-V" {
            Some(identity(name))
        } else {
            load(name)
        };
        // A name that is none of them comes from the file, so it is not remembered.
        let Some(loaded) = loaded else {
            return Arc::new(fallback(name));
        };
        let mut cache = predefined_cache().lock().expect("cmap cache");
        cache
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(loaded))
            .clone()
    }

    fn set_parent(&mut self, parent: Arc<CMap>) {
        if self.parent.is_none() {
            self.parent = Some(parent);
        }
    }

    fn finish(&mut self) {
        self.cids.freeze();
        self.notdefs.freeze();
        self.texts.freeze();
        if let Some(parent) = &self.parent {
            for s in &parent.spaces {
                if self.space_list.len() < MAX_CODESPACES {
                    self.space_list.push(*s);
                }
            }
        }
        if self.space_list.is_empty() {
            // No codespace given: accept any byte at the lengths the mappings use.
            let mask = self.cids.length_mask | self.notdefs.length_mask | self.texts.length_mask;
            for n in 1..=4usize {
                if mask & (1 << n) != 0 {
                    self.space_list.push(Codespace {
                        length: n,
                        lo: 0,
                        hi: if n == 4 {
                            u32::MAX
                        } else {
                            (1u32 << (8 * n)) - 1
                        },
                    });
                }
            }
        }
        self.spaces = self.space_list.clone();
    }

    /// Cuts the next character code off `bytes` (ISO 32000-1 9.7.6.3): the shortest prefix inside a codespace
    /// range, ranges of different lengths mixed. Bytes that match no range are consumed as many as the shortest
    /// range that starts like them takes, so the string stays in step. Gives length 0 for empty input.
    pub fn next_code(&self, bytes: &[u8]) -> (i64, usize) {
        if bytes.is_empty() {
            return (0, 0);
        }
        let mut code = 0u32;
        for n in 1..=bytes.len().min(4) {
            code = code << 8 | u32::from(bytes[n - 1]);
            for s in &self.spaces {
                if s.length == n && within(s, code) {
                    return (signed(code), n);
                }
            }
        }

        let (mut take, mut shortest) = (0usize, 0usize);
        let first = u32::from(bytes[0]);
        for s in &self.spaces {
            shortest = if shortest == 0 {
                s.length
            } else {
                shortest.min(s.length)
            };
            let shift = 8 * (s.length as u32 - 1);
            if first >= (s.lo >> shift) & 0xFF && first <= (s.hi >> shift) & 0xFF {
                take = if take == 0 {
                    s.length
                } else {
                    take.min(s.length)
                };
            }
        }
        if take == 0 {
            take = if shortest == 0 { 1 } else { shortest };
        }
        take = take.min(bytes.len());
        code = 0;
        for &b in &bytes[..take] {
            code = code << 8 | u32::from(b);
        }
        (signed(code), take)
    }

    /// The character id of a code, or none when the CMap maps it to none. A code in a notdefrange gives that
    /// range's id. Without the code's length, the shortest length that has a mapping is taken; pass the length as
    /// `next_code` reports it for a code of known byte length.
    pub fn cid_of(&self, code: i64, length: usize) -> Option<i64> {
        let c = code as u32;
        self.find_cid(c, length, false)
            .or_else(|| self.find_cid(c, length, true))
    }

    fn find_cid(&self, code: u32, length: usize, notdef: bool) -> Option<i64> {
        if self.is_identity {
            return if notdef || code > 0xFFFF {
                None
            } else {
                Some(i64::from(code))
            };
        }
        let table = if notdef { &self.notdefs } else { &self.cids };
        for n in lengths(length) {
            if table.length_mask & (1 << n) != 0
                && let Some((lo, value)) = table.try_find(n, code)
            {
                if notdef {
                    return Some(value);
                }
                // (int)(code - lo) and the sum wrap.
                return Some(i64::from(
                    (value as i32).wrapping_add(code.wrapping_sub(lo) as i32),
                ));
            }
        }
        self.parent
            .as_ref()
            .and_then(|p| p.find_cid(code, length, notdef))
    }

    /// The text a ToUnicode CMap gives a code (a bfchar or bfrange), or none. One code may give several
    /// characters.
    pub fn unicode_of(&self, code: i64, length: usize) -> Option<String> {
        let c = code as u32;
        for n in lengths(length) {
            if self.texts.length_mask & (1 << n) != 0
                && let Some((lo, mut value)) = self.texts.try_find(n, c)
            {
                if c != lo
                    && let Some(&last) = value.last()
                {
                    let at = value.len() - 1;
                    value[at] = (u32::from(last).wrapping_add(c.wrapping_sub(lo))) as u16;
                }
                return Some(String::from_utf16_lossy(&value));
            }
        }
        if self.codes_are_unicode {
            return from_utf16_code(c);
        }
        self.parent
            .as_ref()
            .and_then(|p| p.unicode_of(code, length))
    }
}

fn identity(name: &str) -> CMap {
    let mut map = CMap::empty();
    map.name = Some(name.to_string());
    map.is_vertical = name == "Identity-V";
    map.is_identity = true;
    map.space_list.push(Codespace {
        length: 2,
        lo: 0,
        hi: 0xFFFF,
    });
    map.finish();
    map
}

fn fallback(name: &str) -> CMap {
    let mut map = identity(name);
    map.is_vertical = name.ends_with("-V");
    map.is_fallback = true;
    map
}

fn load(name: &str) -> Option<CMap> {
    let blob = cmap_blob(name)?;
    let mut map = CMap::empty();
    map.name = Some(name.to_string());
    map.codes_are_unicode = name.starts_with("Uni");
    read_packed(&inflate_blob(blob), &mut map);
    map.finish();
    Some(map)
}

fn signed(code: u32) -> i64 {
    i64::from(code as i32)
}

fn within(s: &Codespace, code: u32) -> bool {
    for i in 0..s.length {
        let shift = 8 * (s.length - 1 - i) as u32;
        let b = (code >> shift) & 0xFF;
        if b < (s.lo >> shift) & 0xFF || b > (s.hi >> shift) & 0xFF {
            return false;
        }
    }
    true
}

// The code lengths to try: all four when it is not known, else just that one.
fn lengths(length: usize) -> std::ops::RangeInclusive<usize> {
    if length == 0 { 1..=4 } else { length..=length }
}

// The Uni*-UCS2 and Uni*-UTF16 CMaps take Unicode itself as the code; UTF-16 pairs fill four bytes.
fn from_utf16_code(code: u32) -> Option<String> {
    if code <= 0xFFFF {
        return char::from_u32(code).map(String::from);
    }
    let (high, low) = ((code >> 16) as u16, code as u16);
    if (0xD800..0xDC00).contains(&high) && (0xDC00..0xE000).contains(&low) {
        Some(String::from_utf16_lossy(&[high, low]))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    End,
    Name,
    Hex,
    Number,
    Word,
    ArrayOpen,
    ArrayClose,
    Other,
}

const HEX_CAPACITY: usize = 256;

/// Tokens of a CMap file. A hex string is decoded into a small buffer and cut off at its size: a destination
/// string longer than that is not text a font maps one code to.
struct Lexer<'a> {
    data: &'a [u8],
    hex: [u8; HEX_CAPACITY],
    at: usize,
    text_start: usize,
    text_length: usize,
    hex_length: usize,
    number: i64,
}

fn is_space(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn hex_digit(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8]) -> Self {
        Lexer {
            data,
            hex: [0; HEX_CAPACITY],
            at: 0,
            text_start: 0,
            text_length: 0,
            hex_length: 0,
            number: 0,
        }
    }

    /// The name (without the slash) or the word last read.
    fn text(&self) -> &'a [u8] {
        &self.data[self.text_start..self.text_start + self.text_length]
    }

    fn next(&mut self) -> Kind {
        self.skip_space();
        if self.at >= self.data.len() {
            return Kind::End;
        }
        let c = self.data[self.at];
        let second = self.data.get(self.at + 1).copied();
        match c {
            b'/' => {
                self.at += 1;
                self.regular();
                return Kind::Name;
            }
            b'<' if second == Some(b'<') => {
                self.at += 2;
                return Kind::Other;
            }
            b'>' if second == Some(b'>') => {
                self.at += 2;
                return Kind::Other;
            }
            b'<' => {
                self.at += 1;
                self.read_hex();
                return Kind::Hex;
            }
            b'[' => {
                self.at += 1;
                return Kind::ArrayOpen;
            }
            b']' => {
                self.at += 1;
                return Kind::ArrayClose;
            }
            b'(' => {
                self.skip_string();
                return Kind::Other;
            }
            b'>' | b')' | b'{' | b'}' => {
                self.at += 1;
                return Kind::Other;
            }
            _ => {}
        }
        self.regular();
        if self.try_number() {
            Kind::Number
        } else {
            Kind::Word
        }
    }

    /// The bytes of a code in a hex string as a number; none when it is empty or longer than four bytes.
    fn code(&self) -> Option<(u32, usize)> {
        if self.hex_length < 1 || self.hex_length > 4 {
            return None;
        }
        let mut code = 0u32;
        for &b in &self.hex[..self.hex_length] {
            code = code << 8 | u32::from(b);
        }
        Some((code, self.hex_length))
    }

    /// The hex string as UTF-16BE text (ISO 32000-1 9.10.3); a single byte stands for the character of that value.
    fn utf16(&self) -> Vec<u16> {
        let length = self.hex_length.min(HEX_CAPACITY);
        if length == 1 {
            return vec![u16::from(self.hex[0])];
        }
        (0..length / 2)
            .map(|i| u16::from(self.hex[2 * i]) << 8 | u16::from(self.hex[2 * i + 1]))
            .collect()
    }

    fn skip_space(&mut self) {
        while self.at < self.data.len() {
            let c = self.data[self.at];
            if c == b'%' {
                while self.at < self.data.len()
                    && self.data[self.at] != 10
                    && self.data[self.at] != 13
                {
                    self.at += 1;
                }
            } else if is_space(c) {
                self.at += 1;
            } else {
                return;
            }
        }
    }

    fn regular(&mut self) {
        self.text_start = self.at;
        while self.at < self.data.len()
            && !is_space(self.data[self.at])
            && !is_delimiter(self.data[self.at])
        {
            self.at += 1;
        }
        self.text_length = self.at - self.text_start;
    }

    fn try_number(&mut self) -> bool {
        let t = self.text();
        let Some(&first) = t.first() else {
            return false;
        };
        let start = usize::from(first == b'-' || first == b'+');
        if start == t.len() || t.len() - start > 18 {
            return false;
        }
        let mut value = 0i64;
        for &byte in &t[start..] {
            if !byte.is_ascii_digit() {
                return false;
            }
            value = value * 10 + i64::from(byte - b'0');
        }
        self.number = if first == b'-' { -value } else { value };
        true
    }

    fn read_hex(&mut self) {
        self.hex_length = 0;
        let mut half: Option<u32> = None;
        while self.at < self.data.len() {
            let c = self.data[self.at];
            self.at += 1;
            if c == b'>' {
                break;
            }
            let Some(digit) = hex_digit(c) else { continue };
            match half.take() {
                None => half = Some(digit),
                Some(h) => self.store(h << 4 | digit),
            }
        }
        if let Some(h) = half {
            self.store(h << 4);
        }
    }

    // Past the buffer the length keeps counting, so a caller can tell the string was cut off.
    fn store(&mut self, value: u32) {
        if self.hex_length < HEX_CAPACITY {
            self.hex[self.hex_length] = value as u8;
        }
        if self.hex_length < i32::MAX as usize {
            self.hex_length += 1;
        }
    }

    fn skip_string(&mut self) {
        let mut depth = 0;
        while self.at < self.data.len() {
            let c = self.data[self.at];
            self.at += 1;
            if c == b'\\' {
                self.at += 1;
            } else if c == b'(' {
                depth += 1;
            } else if c == b')' {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }
}

/// Reads the PostScript subset CMaps are written in (ISO 32000-1 9.10.3, Adobe Technical Note 5014).
fn run(data: &[u8], map: &mut CMap) {
    let mut lx = Lexer::new(data);
    let mut last_name: &[u8] = &[];
    let mut kind = lx.next();
    while kind != Kind::End && !map.full() {
        if kind == Kind::Name {
            last_name = lx.text();
            if last_name == b"WMode" && lx.next() == Kind::Number {
                map.is_vertical = lx.number == 1;
            } else if last_name == b"CMapName" && lx.next() == Kind::Name {
                map.name = Some(lazy_pdf_syntax::latin1(lx.text()));
            }
        } else if kind == Kind::Word && lx.text() == b"usecmap" {
            let parent = CMap::predefined(&lazy_pdf_syntax::latin1(last_name));
            if !parent.is_fallback {
                map.set_parent(parent);
            }
        } else if kind == Kind::Word {
            section(&mut lx, map);
        }
        kind = lx.next();
    }
}

fn section(lx: &mut Lexer, map: &mut CMap) {
    match lx.text() {
        b"begincodespacerange" => code_spaces(lx, map),
        b"begincidrange" => cid_ranges(lx, map, false),
        b"begincidchar" => cid_chars(lx, map, false),
        b"beginnotdefrange" => cid_ranges(lx, map, true),
        b"beginnotdefchar" => cid_chars(lx, map, true),
        b"beginbfrange" => bf_ranges(lx, map),
        b"beginbfchar" => bf_chars(lx, map),
        _ => {}
    }
}

fn code_spaces(lx: &mut Lexer, map: &mut CMap) {
    while lx.next() == Kind::Hex {
        let Some(lo) = lx.code() else { return };
        if lx.next() != Kind::Hex {
            return;
        }
        let Some(hi) = lx.code() else { return };
        if lo.1 == hi.1 && map.space_list.len() < 100 {
            map.space_list.push(Codespace {
                length: lo.1,
                lo: lo.0,
                hi: hi.0,
            });
        }
    }
}

fn target(map: &mut CMap, notdef: bool) -> &mut CodeRanges<i64> {
    if notdef {
        &mut map.notdefs
    } else {
        &mut map.cids
    }
}

fn cid_ranges(lx: &mut Lexer, map: &mut CMap, notdef: bool) {
    while lx.next() == Kind::Hex && !map.full() {
        let Some(lo) = lx.code() else { return };
        if lx.next() != Kind::Hex {
            return;
        }
        let Some(hi) = lx.code() else { return };
        if lx.next() != Kind::Number {
            return;
        }
        if lo.1 == hi.1 && lo.0 <= hi.0 && (0..=i64::from(i32::MAX)).contains(&lx.number) {
            target(map, notdef).add(lo.1, lo.0, hi.0, lx.number);
        }
    }
}

fn cid_chars(lx: &mut Lexer, map: &mut CMap, notdef: bool) {
    while lx.next() == Kind::Hex && !map.full() {
        let Some(code) = lx.code() else { return };
        if lx.next() != Kind::Number {
            return;
        }
        if (0..=i64::from(i32::MAX)).contains(&lx.number) {
            target(map, notdef).add(code.1, code.0, code.0, lx.number);
        }
    }
}

fn bf_chars(lx: &mut Lexer, map: &mut CMap) {
    while lx.next() == Kind::Hex && !map.full() {
        let Some(code) = lx.code() else { return };
        let text: Vec<u16> = match lx.next() {
            Kind::Hex => lx.utf16(),
            Kind::Name => unicode_of_glyph_name(&lazy_pdf_syntax::latin1(lx.text()))
                .map(|s| s.encode_utf16().collect())
                .unwrap_or_default(),
            _ => return,
        };
        if !text.is_empty() {
            map.texts.add(code.1, code.0, code.0, text);
        }
    }
}

fn bf_ranges(lx: &mut Lexer, map: &mut CMap) {
    while lx.next() == Kind::Hex && !map.full() {
        let Some(lo) = lx.code() else { return };
        if lx.next() != Kind::Hex {
            return;
        }
        let Some(hi) = lx.code() else { return };
        match lx.next() {
            Kind::Hex => {
                let text = lx.utf16();
                if lo.1 == hi.1 && lo.0 <= hi.0 && !text.is_empty() {
                    map.texts.add(lo.1, lo.0, hi.0, text);
                }
            }
            Kind::ArrayOpen => {
                let mut code = lo.0;
                while lx.next() == Kind::Hex {
                    let text = lx.utf16();
                    if lo.1 == hi.1 && code <= hi.0 && !text.is_empty() && !map.full() {
                        map.texts.add(lo.1, code, code, text);
                    }
                    code = code.wrapping_add(1);
                }
            }
            _ => return,
        }
    }
}

// The packed layout, written by the script that made the data: varints, zigzag for the id deltas.
fn read_packed(b: &[u8], map: &mut CMap) {
    let mut at = 0;
    map.is_vertical = varint(b, &mut at) == 1;
    let name_length = varint(b, &mut at) as usize;
    if name_length > 0 {
        let name = String::from_utf8_lossy(&b[at..at + name_length]).into_owned();
        map.set_parent(CMap::predefined(&name));
    }
    at += name_length;

    let mut spaces = varint(b, &mut at);
    while spaces > 0 {
        let n = b[at] as usize;
        at += 1;
        let lo = varint(b, &mut at) as u32;
        let hi = varint(b, &mut at) as u32;
        map.space_list.push(Codespace { length: n, lo, hi });
        spaces -= 1;
    }
    let mut groups = varint(b, &mut at);
    while groups > 0 {
        let n = b[at] as usize;
        at += 1;
        let mut lo = 0u32;
        let mut cid = 0i64;
        let mut count = varint(b, &mut at);
        while count > 0 {
            lo = lo.wrapping_add(varint(b, &mut at) as u32);
            let span = varint(b, &mut at) as u32;
            let zigzag = varint(b, &mut at);
            cid += (zigzag >> 1) ^ -(zigzag & 1);
            map.cids
                .add(n, lo, lo.wrapping_add(span), i64::from(cid as i32));
            cid += i64::from(span) + 1;
            count -= 1;
        }
        groups -= 1;
    }
}

fn varint(b: &[u8], at: &mut usize) -> i64 {
    let mut value = 0i64;
    let mut shift = 0;
    loop {
        let x = b[*at];
        *at += 1;
        value |= i64::from(x & 0x7F) << shift;
        if x & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

#[cfg(test)]
#[path = "cmap_tests.rs"]
mod tests;
