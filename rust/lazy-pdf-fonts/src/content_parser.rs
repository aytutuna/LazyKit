//! Content stream bytes to operations (ISO 32000-1 7.8.2, Annex A). Nothing in the data stops the parse: unknown
//! operators come back as they are, stray delimiters and unclosed arrays are made up for, and the work is linear
//! in the length.

use lazy_pdf_syntax::{Dict, Name, Number, Object, PdfString};

// Operands kept for one operator; the rest are dropped. Real operators take at most a few dozen.
const MAX_OPERANDS: usize = 512;
const MAX_DEPTH: usize = 64;
const MAX_OPERATOR_LENGTH: usize = 32;

const KNOWN: &[&str] = &[
    "b", "B", "b*", "B*", "BDC", "BI", "BMC", "BT", "BX", "c", "cm", "CS", "cs", "d", "d0", "d1",
    "Do", "DP", "EI", "EMC", "ET", "EX", "f", "F", "f*", "G", "g", "gs", "h", "i", "ID", "j", "J",
    "K", "k", "l", "m", "M", "MP", "n", "q", "Q", "re", "RG", "rg", "ri", "s", "S", "SC", "sc",
    "SCN", "scn", "sh", "T*", "Tc", "Td", "TD", "Tf", "Tj", "TJ", "TL", "Tm", "Tr", "Ts", "Tw",
    "Tz", "v", "w", "W", "W*", "y", "'", "\"",
];

/// One operator of a content stream with its operands. `start` and `end` are byte positions in the data it was
/// parsed from, from the first operand to just past the operator.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentOperation {
    pub operator: String,
    pub operands: Vec<Object>,
    pub start: usize,
    pub end: usize,
    /// For `BI`: the image, whose dictionary and data are not operands.
    pub image: Option<InlineImage>,
}

impl ContentOperation {
    /// Operand `index` as a number; none when it is missing or not one.
    pub fn number(&self, index: usize) -> Option<f64> {
        self.operands.get(index)?.as_number().map(Number::value)
    }
}

impl std::fmt::Display for ContentOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let operands: Vec<String> = self.operands.iter().map(ToString::to_string).collect();
        let text = format!("{} {}", operands.join(" "), self.operator);
        f.write_str(text.trim_start_matches(' '))
    }
}

/// Finds a colour space by its resource name.
pub type ColorSpaceLookup = dyn Fn(&str) -> Option<Object>;

/// The dictionary and data of a `BI ... ID ... EI` image (8.9.7). The dictionary keeps the abbreviations it was
/// written with; `data` is the bytes between ID and EI, filters still applied.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineImage {
    pub dictionary: Dict,
    pub data: Vec<u8>,
}

impl InlineImage {
    /// The dictionary with full key names, the form the filters and image decoder read. Colour space names that are
    /// not device spaces are looked up with `color_space`; the other abbreviations are left to those readers.
    pub fn expanded(&self, color_space: Option<&ColorSpaceLookup>) -> Dict {
        let mut result = Dict::new();
        for (key, value) in self.dictionary.entries() {
            let full = match key.text().as_str() {
                "BPC" => "BitsPerComponent",
                "CS" => "ColorSpace",
                "D" => "Decode",
                "DP" => "DecodeParms",
                "F" => "Filter",
                "H" => "Height",
                "IM" => "ImageMask",
                "I" => "Interpolate",
                "W" => "Width",
                other => {
                    result.add(Name::new(other), value.clone());
                    continue;
                }
            };
            let mut value = value.clone();
            if full == "ColorSpace"
                && let Object::Name(n) = &value
                && let Some(found) = color_space.and_then(|f| f(&n.text()))
            {
                value = found;
            }
            result.add(Name::new(full), value);
        }
        result
    }
}

/// More operations than the caller allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyOperations;

pub fn parse(data: &[u8]) -> Vec<ContentOperation> {
    Scanner::new(data)
        .run(i32::MAX as usize)
        .unwrap_or_default()
}

/// As [`parse`]; more than `max_operations` operations is an error.
pub fn parse_limited(
    data: &[u8],
    max_operations: usize,
) -> Result<Vec<ContentOperation>, TooManyOperations> {
    Scanner::new(data).run(max_operations)
}

fn is_white(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        0x28 | 0x29 | 0x3C | 0x3E | 0x5B | 0x5D | 0x7B | 0x7D | 0x2F | 0x25
    )
}

fn is_regular(c: u8) -> bool {
    !is_white(c) && !is_delimiter(c)
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

struct Scanner<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Scanner<'a> {
    fn new(d: &'a [u8]) -> Self {
        Scanner { d, p: 0 }
    }

    fn run(&mut self, max_operations: usize) -> Result<Vec<ContentOperation>, TooManyOperations> {
        let mut ops: Vec<ContentOperation> = Vec::new();
        let mut operands: Vec<Object> = Vec::new();
        let mut op_start = 0;
        loop {
            self.skip_space();
            if self.p >= self.d.len() {
                break;
            }
            if ops.len() > max_operations {
                return Err(TooManyOperations);
            }
            if operands.is_empty() {
                op_start = self.p;
            }
            if let Some(value) = self.read_value(0) {
                if operands.len() < MAX_OPERANDS {
                    operands.push(value);
                }
                continue;
            }
            if is_delimiter(self.d[self.p]) {
                self.p += 1;
                continue;
            }
            let name = self.read_word();
            if name == "BI" {
                let image = self.read_inline_image();
                ops.push(ContentOperation {
                    operator: name,
                    operands: Vec::new(),
                    start: op_start,
                    end: self.p,
                    image: Some(image),
                });
            } else {
                ops.push(ContentOperation {
                    operator: name,
                    operands: std::mem::take(&mut operands),
                    start: op_start,
                    end: self.p,
                    image: None,
                });
            }
            operands.clear();
        }
        Ok(ops)
    }

    fn at(&self, position: usize) -> Option<u8> {
        self.d.get(position).copied()
    }

    fn skip_space(&mut self) {
        while self.p < self.d.len() {
            let c = self.d[self.p];
            if is_white(c) {
                self.p += 1;
            } else if c == 0x25 {
                while self.p < self.d.len() && self.d[self.p] != 10 && self.d[self.p] != 13 {
                    self.p += 1;
                }
            } else {
                return;
            }
        }
    }

    /// The operand at the position, or none (nothing consumed) at a keyword, a closing delimiter or the end.
    fn read_value(&mut self, depth: usize) -> Option<Object> {
        self.skip_space();
        if self.p >= self.d.len() {
            return None;
        }
        let c = self.d[self.p];
        match c {
            0x2F => return Some(Object::Name(self.read_name())),
            0x28 => return Some(self.read_literal()),
            0x3C => {
                return Some(if self.at(self.p + 1) == Some(0x3C) {
                    Object::Dict(self.read_dictionary(depth))
                } else {
                    self.read_hex()
                });
            }
            0x5B => return Some(self.read_array(depth)),
            _ => {}
        }
        if c.is_ascii_digit() || c == 0x2B || c == 0x2D || c == 0x2E {
            return Some(self.read_number());
        }
        if is_delimiter(c) {
            return None;
        }
        let mut end = self.p;
        while end < self.d.len() && is_regular(self.d[end]) {
            end += 1;
        }
        let literal = match &self.d[self.p..end] {
            b"true" => Object::Bool(true),
            b"false" => Object::Bool(false),
            b"null" => Object::Null,
            _ => return None,
        };
        self.p = end;
        Some(literal)
    }

    // "--5" is -5, "1.2.3" is 1.2 and then .3, "-" and "." are 0.
    fn read_number(&mut self) -> Object {
        let start = self.p;
        let negative = self.d[self.p] == 0x2D;
        while self.p < self.d.len() && (self.d[self.p] == 0x2B || self.d[self.p] == 0x2D) {
            self.p += 1;
        }
        let plain_sign = self.p - start <= 1;
        let digits_start = self.p;
        while self.p < self.d.len() && self.d[self.p].is_ascii_digit() {
            self.p += 1;
        }
        let whole = self.p - digits_start;
        let mut fraction = 0;
        if self.p < self.d.len() && self.d[self.p] == 0x2E {
            self.p += 1;
            let from = self.p;
            while self.p < self.d.len() && self.d[self.p].is_ascii_digit() {
                self.p += 1;
            }
            fraction = self.p - from;
        }
        let text = |from: usize, to: usize| lazy_pdf_syntax::latin1(&self.d[from..to]);
        let value = if whole == 0 && fraction == 0 {
            "0".to_string()
        } else if plain_sign {
            text(start, self.p)
        } else {
            format!(
                "{}{}",
                if negative { "-" } else { "" },
                text(digits_start, self.p)
            )
        };
        Object::Number(Number::from_text(value))
    }

    fn read_word(&mut self) -> String {
        let start = self.p;
        while self.p < self.d.len() && is_regular(self.d[self.p]) {
            self.p += 1;
        }
        let length = (self.p - start).min(MAX_OPERATOR_LENGTH);
        lazy_pdf_syntax::latin1(&self.d[start..start + length])
    }

    fn read_name(&mut self) -> Name {
        self.p += 1;
        let start = self.p;
        while self.p < self.d.len() && is_regular(self.d[self.p]) {
            self.p += 1;
        }
        let raw = &self.d[start..self.p];
        let mut bytes = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == 0x23
                && i + 2 < raw.len()
                && let (Some(a), Some(b)) = (hex(raw[i + 1]), hex(raw[i + 2]))
            {
                bytes.push(a * 16 + b);
                i += 2;
            } else {
                bytes.push(raw[i]);
            }
            i += 1;
        }
        Name(bytes)
    }

    fn read_literal(&mut self) -> Object {
        self.p += 1;
        let mut text = Vec::new();
        let mut depth = 1;
        while self.p < self.d.len() {
            let mut c = self.d[self.p];
            self.p += 1;
            if c == 0x5C && self.p < self.d.len() {
                let e = self.d[self.p];
                self.p += 1;
                match e {
                    0x6E => text.push(10),
                    0x72 => text.push(13),
                    0x74 => text.push(9),
                    0x62 => text.push(8),
                    0x66 => text.push(12),
                    13 => {
                        if self.at(self.p) == Some(10) {
                            self.p += 1;
                        }
                    }
                    10 => {}
                    0x30..=0x37 => {
                        let mut value = u32::from(e) - 0x30;
                        let mut k = 0;
                        while k < 2
                            && let Some(next) = self.at(self.p)
                            && (0x30..=0x37).contains(&next)
                        {
                            value = value * 8 + (u32::from(next) - 0x30);
                            self.p += 1;
                            k += 1;
                        }
                        // Three octal digits reach 511; (byte)511 is 255 in the unchecked C#.
                        text.push(value as u8);
                    }
                    other => text.push(other),
                }
                continue;
            }
            if c == 0x28 {
                depth += 1;
            } else if c == 0x29 {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            if c == 13 {
                if self.at(self.p) == Some(10) {
                    self.p += 1;
                }
                c = 10;
            }
            text.push(c);
        }
        Object::String(PdfString {
            bytes: text,
            hex: false,
        })
    }

    fn read_hex(&mut self) -> Object {
        self.p += 1;
        let mut text = Vec::new();
        let mut high: Option<u8> = None;
        while self.p < self.d.len() {
            let c = self.d[self.p];
            self.p += 1;
            if c == 0x3E {
                break;
            }
            let Some(h) = hex(c) else { continue };
            match high.take() {
                None => high = Some(h),
                Some(hi) => text.push(hi * 16 + h),
            }
        }
        if let Some(hi) = high {
            text.push(hi * 16);
        }
        Object::String(PdfString {
            bytes: text,
            hex: true,
        })
    }

    fn read_array(&mut self, depth: usize) -> Object {
        self.p += 1;
        let mut items = Vec::new();
        if depth >= MAX_DEPTH {
            return Object::Array(items);
        }
        loop {
            self.skip_space();
            if self.p >= self.d.len() {
                break;
            }
            if self.d[self.p] == 0x5D {
                self.p += 1;
                break;
            }
            if let Some(value) = self.read_value(depth + 1) {
                items.push(value);
                continue;
            }
            if is_delimiter(self.d[self.p]) {
                self.p += 1;
                continue;
            }
            // A missing ] ends at the next real operator.
            let from = self.p;
            if KNOWN.contains(&self.read_word().as_str()) {
                self.p = from;
                break;
            }
        }
        Object::Array(items)
    }

    fn read_dictionary(&mut self, depth: usize) -> Dict {
        self.p += 2;
        let mut dictionary = Dict::new();
        if depth >= MAX_DEPTH {
            return dictionary;
        }
        loop {
            self.skip_space();
            if self.p >= self.d.len() {
                break;
            }
            if self.d[self.p] == 0x3E {
                self.p += if self.at(self.p + 1) == Some(0x3E) {
                    2
                } else {
                    1
                };
                break;
            }
            if self.d[self.p] == 0x2F {
                let key = self.read_name();
                self.skip_space();
                if self.at(self.p) == Some(0x3E) && self.at(self.p + 1) == Some(0x3E) {
                    dictionary.add(key, Object::Null);
                } else if let Some(value) = self.read_value(depth + 1) {
                    dictionary.add(key, value);
                }
                continue;
            }
            if self.read_value(depth + 1).is_some() {
                continue;
            }
            if is_delimiter(self.d[self.p]) {
                self.p += 1;
                continue;
            }
            let from = self.p;
            if KNOWN.contains(&self.read_word().as_str()) {
                self.p = from;
                break;
            }
        }
        dictionary
    }

    fn read_inline_image(&mut self) -> InlineImage {
        let mut dictionary = Dict::new();
        loop {
            self.skip_space();
            if self.p >= self.d.len() {
                return InlineImage {
                    dictionary,
                    data: Vec::new(),
                };
            }
            if self.d[self.p] == 0x2F {
                let key = self.read_name();
                if let Some(value) = self.read_value(0) {
                    dictionary.add(key, value);
                }
                continue;
            }
            if self.read_value(0).is_some() {
                continue;
            }
            if is_delimiter(self.d[self.p]) {
                self.p += 1;
                continue;
            }
            match self.read_word().as_str() {
                "ID" => break,
                "EI" => {
                    return InlineImage {
                        dictionary,
                        data: Vec::new(),
                    };
                }
                _ => {}
            }
        }
        // One white space byte follows ID; CR LF counts as one.
        if self.at(self.p) == Some(13) && self.at(self.p + 1) == Some(10) {
            self.p += 2;
        } else if self.at(self.p).is_some_and(is_white) {
            self.p += 1;
        }
        let start = self.p;
        let (end, next) = self.find_image_end(&dictionary, start);
        self.p = next;
        InlineImage {
            dictionary,
            data: self.d[start..end].to_vec(),
        }
    }

    fn find_image_end(&mut self, dictionary: &Dict, start: usize) -> (usize, usize) {
        let d = self.d;
        if let Some(length) = known_length(dictionary)
            && length <= d.len() - start
        {
            let end = start + length;
            let mut q = end;
            while q < d.len() && is_white(d[q]) {
                q += 1;
            }
            if self.at(q) == Some(0x45)
                && self.at(q + 1) == Some(0x49)
                && (q + 2 >= d.len() || !is_regular(d[q + 2]))
            {
                return (end, q + 2);
            }
        }
        let mut i = start;
        while i + 1 < d.len() {
            let here = i;
            i += 1;
            if d[here] != 0x45 || d[here + 1] != 0x49 {
                continue;
            }
            if here > start && !is_white(d[here - 1]) {
                continue;
            }
            if here + 2 < d.len() && is_regular(d[here + 2]) {
                continue;
            }
            if !self.looks_like_content(here + 2) {
                continue;
            }
            return (if here > start { here - 1 } else { here }, here + 2);
        }
        (d.len(), d.len())
    }

    // Binary data holds "EI" by chance now and then; what follows a real one reads as a content stream.
    fn looks_like_content(&mut self, from: usize) -> bool {
        let d = self.d;
        let mut q = from;
        while q < d.len() && is_white(d[q]) {
            q += 1;
        }
        let end = d.len().min(q + 20);
        for &c in &d[q.min(end)..end] {
            if c > 126 || (c < 32 && !is_white(c)) {
                return false;
            }
        }
        let first = d.get(q).copied().unwrap_or(0);
        if q >= d.len() || !first.is_ascii_alphabetic() {
            return true;
        }
        let save = self.p;
        self.p = q;
        let word = self.read_word();
        self.p = save;
        KNOWN.contains(&word.as_str())
    }
}

// The size of unfiltered data, when width, height and colour space say it; none when they do not, or when the
// size does not fit a long (the C# wraps there and reads on from a wrong place).
fn known_length(dictionary: &Dict) -> Option<usize> {
    if dictionary.contains_key("F") || dictionary.contains_key("Filter") {
        return None;
    }
    let get = |a: &str, b: &str| dictionary.get(a).or_else(|| dictionary.get(b));
    let number = |o: Option<&Object>| o.and_then(Object::as_number).map(Number::value);
    let w = number(get("W", "Width")).filter(|v| *v > 0.0)?;
    let h = number(get("H", "Height")).filter(|v| *v > 0.0)?;
    let mut bits: i64 = 8;
    if let Some(b) = number(get("BPC", "BitsPerComponent")) {
        bits = (b as i64).clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    }
    let components: i64;
    if matches!(get("IM", "ImageMask"), Some(Object::Bool(true))) {
        bits = 1;
        components = 1;
    } else {
        let mut space = get("CS", "ColorSpace");
        if let Some(Object::Array(items)) = space
            && !items.is_empty()
        {
            space = Some(&items[0]);
        }
        components = match space.and_then(Object::as_name).map(Name::text).as_deref() {
            Some("G" | "DeviceGray" | "I" | "Indexed") => 1,
            Some("RGB" | "DeviceRGB") => 3,
            Some("CMYK" | "DeviceCMYK") => 4,
            _ => 0,
        };
    }
    if components == 0 || ![1, 2, 4, 8, 16].contains(&bits) {
        return None;
    }
    let per_pixel = (w as i64).checked_mul(components)?;
    let per_row = per_pixel.checked_mul(bits)?;
    let padded = per_row.checked_add(7)?;
    let total = (padded / 8).checked_mul(h as i64)?;
    usize::try_from(total).ok()
}
