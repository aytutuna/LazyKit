use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::lexer::is_delimiter;

/// A name's bytes once the #xx escapes are undone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name(pub Vec<u8>);

impl Name {
    pub fn new(text: &str) -> Self {
        Name(text.as_bytes().to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn is(&self, text: &str) -> bool {
        self.0 == text.as_bytes()
    }

    /// The bytes as UTF-8 text, or one char per byte when they are not UTF-8.
    pub fn text(&self) -> String {
        match std::str::from_utf8(&self.0) {
            Ok(text) => text.to_owned(),
            Err(_) => crate::error::latin1(&self.0),
        }
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.push(b'/');
        for &b in &self.0 {
            if b > 0x20 && b < 0x7F && b != b'#' && !is_delimiter(b as i32) {
                out.push(b);
            } else {
                out.push(b'#');
                push_hex(out, b);
            }
        }
    }
}

fn push_hex(out: &mut Vec<u8>, b: u8) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    out.push(DIGITS[(b >> 4) as usize]);
    out.push(DIGITS[(b & 15) as usize]);
}

/// An integer or a real, kept as it was written ("-.5", "4.", "0012").
#[derive(Debug, Clone, PartialEq)]
pub struct Number {
    text: String,
    value: f64,
    integer_syntax: bool,
}

impl Number {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let value = text.parse::<f64>().unwrap_or(0.0);
        let integer_syntax = !text.contains('.');
        Number {
            text,
            value,
            integer_syntax,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        Self::from_text(value.to_string())
    }

    /// A real, written with at most six decimals and no exponent.
    pub fn from_f64(value: f64) -> Self {
        Self::from_text(format_real(value))
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// The integer value; none for a real or one out of range.
    pub fn integer(&self) -> Option<i64> {
        if self.integer_syntax {
            self.text.parse().ok()
        } else {
            None
        }
    }

    /// An integer, or a real that is whole (90.0).
    pub fn whole(&self) -> Option<i64> {
        self.integer().or_else(|| {
            let v = self.value;
            (v.is_finite() && v.abs() < i64::MAX as f64 && v == v.floor()).then_some(v as i64)
        })
    }
}

/// The reference engines round to six decimals (half to even on the scaled value) and print at most 15
/// significant digits.
fn format_real(value: f64) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    let rounded = if value.abs() < 1e16 {
        (value * 1e6).round_ties_even() / 1e6
    } else {
        value
    };
    if rounded == 0.0 {
        return "0".into();
    }
    let sci = format!("{:.14e}", rounded.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = exponent.parse::<i32>().unwrap() + 1;
    let mut integer = String::new();
    let mut fraction = String::new();
    if point <= 0 {
        integer.push('0');
        fraction.push_str(&"0".repeat((-point) as usize));
        fraction.push_str(&digits);
    } else {
        let point = point as usize;
        for i in 0..point {
            integer.push(digits.as_bytes().get(i).map_or('0', |d| *d as char));
        }
        if point < digits.len() {
            fraction.push_str(&digits[point..]);
        }
    }
    fraction.truncate(6);
    let fraction = fraction.trim_end_matches('0');
    let mut text = String::new();
    if rounded < 0.0 {
        text.push('-');
    }
    text.push_str(&integer);
    if !fraction.is_empty() {
        text.push('.');
        text.push_str(fraction);
    }
    text
}

/// A string's bytes, and whether it was written in hex.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfString {
    pub bytes: Vec<u8>,
    pub hex: bool,
}

impl PdfString {
    pub fn write_to(&self, out: &mut Vec<u8>) {
        if self.hex {
            out.push(b'<');
            for &b in &self.bytes {
                push_hex(out, b);
            }
            out.push(b'>');
            return;
        }
        out.push(b'(');
        for &b in &self.bytes {
            match b {
                b'\\' => out.extend_from_slice(b"\\\\"),
                b'(' => out.extend_from_slice(b"\\("),
                b')' => out.extend_from_slice(b"\\)"),
                // A bare CR in a literal string reads back as LF, so both are escaped to keep the byte.
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b if !(0x20..0x7F).contains(&b) => {
                    out.push(b'\\');
                    out.push(b'0' + (b >> 6));
                    out.push(b'0' + ((b >> 3) & 7));
                    out.push(b'0' + (b & 7));
                }
                b => out.push(b),
            }
        }
        out.push(b')');
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ref {
    pub number: u32,
    pub generation: u16,
}

/// Past this many entries a lookup goes through an index instead of a scan.
const INDEX_FROM: usize = 12;

/// A dictionary, its entries in the order they were written (a repeated key: the last one counts).
#[derive(Debug, Default)]
pub struct Dict {
    entries: Vec<(Name, Object)>,
    last: OnceLock<HashMap<Vec<u8>, usize>>,
}

impl Clone for Dict {
    fn clone(&self) -> Self {
        Dict {
            entries: self.entries.clone(),
            last: OnceLock::new(),
        }
    }
}

impl PartialEq for Dict {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Dict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[(Name, Object)] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Object> {
        self.get_bytes(key.as_bytes())
    }

    pub fn get_bytes(&self, key: &[u8]) -> Option<&Object> {
        self.last_index_of(key).map(|i| &self.entries[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.last_index_of(key.as_bytes()).is_some()
    }

    /// Appends without looking for the key; a repeat shadows the earlier entry.
    pub fn add(&mut self, key: Name, value: Object) {
        let at = self.entries.len();
        if let Some(index) = self.last.get_mut() {
            index.insert(key.0.clone(), at);
        }
        self.entries.push((key, value));
    }

    /// Replaces the value of the key (its last entry), or appends it.
    pub fn insert(&mut self, key: &str, value: Object) {
        match self.last_index_of(key.as_bytes()) {
            Some(i) => self.entries[i].1 = value,
            None => self.add(Name::new(key), value),
        }
    }

    /// Removes every entry with the key; false when there was none.
    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(name, _)| !name.is(key));
        if self.entries.len() == before {
            return false;
        }
        self.last = OnceLock::new();
        true
    }

    fn last_index_of(&self, key: &[u8]) -> Option<usize> {
        if self.entries.len() < INDEX_FROM {
            return self.entries.iter().rposition(|(name, _)| name.0 == key);
        }
        let index = self.last.get_or_init(|| {
            let mut map = HashMap::with_capacity(self.entries.len());
            for (i, (name, _)) in self.entries.iter().enumerate() {
                map.insert(name.0.clone(), i);
            }
            map
        });
        index.get(key).copied()
    }

    /// The entries that count: of a repeated key only the last, where it was written.
    pub fn live_entries(&self) -> impl Iterator<Item = &(Name, Object)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(i, (key, _))| self.last_index_of(&key.0) == Some(*i))
            .map(|(_, entry)| entry)
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"<<");
        let mut first = true;
        for (key, value) in self.live_entries() {
            if !first {
                out.push(b' ');
            }
            first = false;
            key.write_to(out);
            out.push(b' ');
            value.write_to(out);
        }
        out.extend_from_slice(b">>");
    }
}

/// A stream: its dictionary, and its data either where it starts in the bytes it was read from or in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    /// Where the data starts in the file read; none for a stream made in memory.
    pub data_offset: Option<usize>,
    /// The data as it is to be stored (filters applied); none while the data is still in the file.
    pub data: Option<Vec<u8>>,
    pub number: u32,
    pub generation: u16,
}

impl Stream {
    pub fn in_memory(dict: Dict, data: Vec<u8>) -> Self {
        Stream {
            dict,
            data_offset: None,
            data: Some(data),
            number: 0,
            generation: 0,
        }
    }
}

/// A PDF value (ISO 32000-1 7.3).
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Number(Number),
    Name(Name),
    String(PdfString),
    Array(Vec<Object>),
    Dict(Dict),
    Ref(Ref),
    Stream(Stream),
}

impl Object {
    pub fn int(value: i64) -> Object {
        Object::Number(Number::from_i64(value))
    }

    pub fn real(value: f64) -> Object {
        Object::Number(Number::from_f64(value))
    }

    pub fn name(text: &str) -> Object {
        Object::Name(Name::new(text))
    }

    pub fn reference(number: u32, generation: u16) -> Object {
        Object::Ref(Ref { number, generation })
    }

    /// The dictionary of a dictionary or of a stream.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Object::Name(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Object::Number(n) => Some(n),
            _ => None,
        }
    }

    /// The value in PDF syntax, as the writer puts it into a file. A stream shows its dictionary only:
    /// streams are written as indirect objects.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Object::Null => out.extend_from_slice(b"null"),
            Object::Bool(true) => out.extend_from_slice(b"true"),
            Object::Bool(false) => out.extend_from_slice(b"false"),
            Object::Number(n) => out.extend_from_slice(n.text.as_bytes()),
            Object::Name(name) => name.write_to(out),
            Object::String(text) => text.write_to(out),
            Object::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    item.write_to(out);
                }
                out.push(b']');
            }
            Object::Dict(dict) => dict.write_to(out),
            Object::Ref(r) => {
                out.extend_from_slice(format!("{} {} R", r.number, r.generation).as_bytes())
            }
            Object::Stream(stream) => stream.dict.write_to(out),
        }
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = Vec::new();
        self.write_to(&mut out);
        f.write_str(&crate::error::latin1(&out))
    }
}

impl fmt::Display for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = Vec::new();
        self.write_to(&mut out);
        f.write_str(&crate::error::latin1(&out))
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
