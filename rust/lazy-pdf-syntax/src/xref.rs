//! Cross-reference sections: classic tables, cross-reference streams (ISO 32000-1 7.5.8), /Prev chains and the
//! /XRefStm of hybrid files (7.5.8.4).

use std::collections::{BTreeMap, HashSet};

use crate::document::{Document, Entry, EntryKind, MAX_OBJECT_NUMBER};
use crate::error::ReadError;
use crate::lexer::{Lexer, is_white_space};
use crate::object::{Dict, Object};

/// Longest /Prev chain followed (a loop is caught before that).
pub(crate) const MAX_SECTIONS: usize = 4096;

/// Bytes a cross-reference stream may hold beyond its entries.
const XREF_SLACK: usize = 64 << 10;

const TRAILER_KEYS: [&str; 4] = ["Root", "Info", "ID", "Encrypt"];
const XREF_STREAM_KEYS: [&str; 8] = [
    "Type",
    "W",
    "Index",
    "Length",
    "Filter",
    "DecodeParms",
    "Prev",
    "XRefStm",
];

type Section = BTreeMap<u32, Entry>;

impl Document {
    pub(crate) fn load_cross_references(&mut self) -> Result<(), ReadError> {
        let start = self.find_start_xref()?;
        self.xref_entries = 0;
        let mut visited = HashSet::new();
        let mut sections = Vec::new();
        let mut trailer: Option<Dict> = None;
        let mut highest: u32 = 0;
        let mut offset = Some(start);
        self.loading_xref = true;
        let walked = (|| -> Result<(), ReadError> {
            while let Some(at) = offset {
                if !visited.insert(at) {
                    return Err(ReadError::damaged(
                        "The cross-reference sections point back at each other (/Prev).",
                    ));
                }
                if visited.len() > MAX_SECTIONS {
                    return Err(ReadError::damaged("Too many cross-reference sections."));
                }
                sections.push(at);
                let (dictionary, is_stream, section_highest) = self.read_section(at)?;
                highest = highest.max(section_highest);
                match &mut trailer {
                    None => {
                        self.xref_is_stream = is_stream;
                        trailer = Some(dictionary.clone());
                    }
                    Some(trailer) => {
                        for key in TRAILER_KEYS {
                            if !trailer.contains_key(key)
                                && let Some(value) = dictionary.get(key)
                            {
                                trailer.insert(key, value.clone());
                            }
                        }
                    }
                }
                if let Some(Object::Number(n)) = dictionary.get("Size")
                    && let Some(size) = n.integer()
                    && (1..=MAX_OBJECT_NUMBER as i64 + 1).contains(&size)
                {
                    highest = highest.max((size - 1) as u32);
                }
                offset = match dictionary.get("Prev") {
                    None => None,
                    Some(Object::Number(prev))
                        if prev.value() > 0.0 && prev.value() < self.data.len() as f64 =>
                    {
                        Some(prev.value() as u64)
                    }
                    _ => return Err(ReadError::damaged("/Prev does not point into the file.")),
                };
            }
            Ok(())
        })();
        self.loading_xref = false;
        walked?;
        let mut trailer = trailer.unwrap_or_default();
        for key in XREF_STREAM_KEYS {
            trailer.remove(key);
        }
        self.trailer = trailer;
        self.start_xref = start;
        self.sections = sections;
        self.next_object_number = highest + 1;
        Ok(())
    }

    fn find_start_xref(&self) -> Result<u64, ReadError> {
        let Some(at) = crate::bytes::rfind(&self.data, b"startxref") else {
            return Err(ReadError::damaged("There is no startxref."));
        };
        let offset = Lexer::new(&self.data, at + "startxref".len()).try_read_unsigned();
        match offset {
            Some(o) if o > 0 && (o as u64) < self.data.len() as u64 => Ok(o as u64),
            _ => Err(ReadError::damaged(
                "startxref does not point into the file.",
            )),
        }
    }

    /// One section at `offset`: its trailer dictionary, whether it was a stream, and the highest number it uses.
    /// Its entries are merged in (newer ones already read win).
    fn read_section(&mut self, offset: u64) -> Result<(Dict, bool, u32), ReadError> {
        let mut lexer = Lexer::new(&self.data, offset as usize);
        lexer.skip_white_space();
        let start = lexer.pos;
        if lexer.read_keyword() == b"xref" {
            let mut section = Section::new();
            let (end, mut highest) = self.read_table(lexer.pos, &mut section)?;
            let mut lexer = Lexer::new(&self.data, end);
            if lexer.read_keyword() != b"trailer" {
                return Err(ReadError::damaged(
                    "A cross-reference table has no trailer.",
                ));
            }
            let Object::Dict(trailer) = lexer.read_object(false)? else {
                return Err(ReadError::damaged("The trailer is not a dictionary."));
            };
            // A hybrid file: the classic table's objects, then the ones only its stream lists (the table gives
            // those as free, or not at all).
            if let Some(stream_offset) = trailer.get("XRefStm") {
                let so = match stream_offset {
                    Object::Number(n) => n
                        .integer()
                        .filter(|so| *so > 0 && (*so as u64) < self.data.len() as u64),
                    _ => None,
                };
                let Some(so) = so else {
                    return Err(ReadError::damaged("/XRefStm does not point into the file."));
                };
                // Sections that name the same stream would decode it again for nothing: the newest named it first.
                if self.hybrid_streams.insert(so as u64) {
                    let mut stream_section = Section::new();
                    let (_, stream_highest) =
                        self.read_stream_section(so as usize, &mut stream_section)?;
                    highest = highest.max(stream_highest);
                    for (number, entry) in stream_section {
                        match section.get(&number) {
                            Some(listed) if listed.kind != EntryKind::Free => {}
                            _ => {
                                section.insert(number, entry);
                            }
                        }
                    }
                }
            }
            self.merge(section);
            return Ok((trailer, false, highest));
        }
        let mut entries = Section::new();
        let (dictionary, highest) = self.read_stream_section(start, &mut entries)?;
        self.merge(entries);
        Ok((dictionary, true, highest))
    }

    /// Entries of an older section count only where no newer one has spoken.
    fn merge(&mut self, section: Section) {
        for (number, entry) in section {
            self.entries.entry(number).or_insert(entry);
        }
    }

    /// A classic table's subsections ("first count", then count lines of "offset generation n|f"); the position
    /// after the last one. An offset past the file is kept: the object is looked for when it is wanted.
    fn read_table(&self, mut pos: usize, section: &mut Section) -> Result<(usize, u32), ReadError> {
        let mut highest = 0;
        loop {
            let save = pos;
            let Some(mut first) = self.read_unsigned(&mut pos) else {
                return Ok((save, highest));
            };
            let Some(count) = self.read_unsigned(&mut pos) else {
                return Err(ReadError::damaged(
                    "A cross-reference subsection header is not two numbers.",
                ));
            };
            if first + count > MAX_OBJECT_NUMBER as u64 + 1 {
                return Err(ReadError::damaged(format!(
                    "A cross-reference subsection goes past PDF's highest object number ({MAX_OBJECT_NUMBER})."
                )));
            }
            for i in 0..count {
                let (Some(entry_offset), Some(generation)) =
                    (self.read_unsigned(&mut pos), self.read_unsigned(&mut pos))
                else {
                    return Err(bad_entry(first + i));
                };
                while pos < self.data.len() && is_white_space(self.data[pos] as i32) {
                    pos += 1;
                }
                let kind = if pos < self.data.len() {
                    pos += 1;
                    self.data[pos - 1]
                } else {
                    0
                };
                if !matches!(kind, b'n' | b'f') || generation > 65535 {
                    return Err(bad_entry(first + i));
                }
                // A table that starts at 1 but opens with the free-list head was meant to start at 0.
                if i == 0 && first == 1 && kind == b'f' && generation == 65535 {
                    first = 0;
                }
                let number = (first + i) as u32;
                if number == 0 {
                    continue;
                }
                let in_use = kind == b'n' && entry_offset > 0;
                section.entry(number).or_insert(if in_use {
                    Entry::in_use(entry_offset, generation as u16)
                } else {
                    Entry::free(generation as u16)
                });
                if in_use {
                    highest = highest.max(number);
                }
            }
        }
    }

    /// Digits after white space; none (and `pos` unchanged) when there are none or too many.
    fn read_unsigned(&self, pos: &mut usize) -> Option<u64> {
        let mut p = *pos;
        while p < self.data.len() && is_white_space(self.data[p] as i32) {
            p += 1;
        }
        let mut value: u64 = 0;
        let mut digits = 0;
        while p < self.data.len() && self.data[p].is_ascii_digit() {
            digits += 1;
            if digits > 18 {
                return None;
            }
            value = value * 10 + (self.data[p] - b'0') as u64;
            p += 1;
        }
        if digits == 0 {
            return None;
        }
        *pos = p;
        Some(value)
    }

    /// A cross-reference stream (/Type /XRef) at `offset`: its dictionary and the highest number it uses.
    fn read_stream_section(
        &mut self,
        offset: usize,
        section: &mut Section,
    ) -> Result<(Dict, u32), ReadError> {
        let mut lexer = Lexer::new(&self.data, offset);
        let (number, generation) = lexer.read_object_header()?;
        let Object::Stream(mut stream) = lexer.read_object(true)? else {
            return Err(not_a_section());
        };
        if !matches!(stream.dict.get("Type"), Some(Object::Name(n)) if n.is("XRef")) {
            return Err(not_a_section());
        }
        stream.number = number;
        stream.generation = generation;
        let dictionary = stream.dict.clone();
        // A cross-reference stream's own entries are direct: nothing can be looked up yet.
        let widths = match dictionary.get("W") {
            Some(Object::Array(w)) if w.len() == 3 => w
                .iter()
                .map(|item| match item {
                    Object::Number(n) => n
                        .integer()
                        .filter(|v| (0..=8).contains(v))
                        .map(|v| v as usize),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>(),
            _ => None,
        };
        let Some(widths) = widths else {
            return Err(ReadError::damaged(
                "A cross-reference stream's /W is not three field widths.",
            ));
        };
        let size = match dictionary.get("Size") {
            Some(Object::Number(n)) => n.integer().filter(|s| *s >= 0),
            _ => None,
        };
        let Some(size) = size else {
            return Err(ReadError::damaged("A cross-reference stream has no /Size."));
        };
        let index: Vec<u64> = match dictionary.get("Index") {
            Some(Object::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Object::Number(n) => n
                        .integer()
                        .filter(|v| (0..=MAX_OBJECT_NUMBER as i64 + 1).contains(v))
                        .map(|v| v as u64),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    ReadError::damaged("A cross-reference stream's /Index is not numbers.")
                })?,
            _ => vec![0, size as u64],
        };
        if !index.len().is_multiple_of(2) {
            return Err(ReadError::damaged(
                "A cross-reference stream's /Index has an odd count.",
            ));
        }
        let entry_length: usize = widths.iter().sum();
        if entry_length == 0 {
            return Err(ReadError::damaged(
                "A cross-reference stream's entries are empty.",
            ));
        }
        let listed = index
            .chunks(2)
            .fold(0u64, |sum, pair| sum.saturating_add(pair[1]));
        if listed > MAX_OBJECT_NUMBER as u64 + 1 {
            return Err(ReadError::damaged(
                "A cross-reference stream lists more entries than PDF has object numbers.",
            ));
        }
        // Every entry of every section costs a table insert, so a chain of them is bounded together.
        self.xref_entries += listed;
        if self.xref_entries > 2 * (MAX_OBJECT_NUMBER as u64 + 1) {
            return Err(ReadError::damaged(
                "The cross-reference streams list too many entries.",
            ));
        }
        // A PNG predictor puts a tag byte in front of every row.
        let limit = listed as usize * (entry_length + 1) + XREF_SLACK;
        let data = self.stream_data_limited(&stream, limit, false)?;
        let mut highest = 0;
        let mut at = 0;
        for pair in index.chunks(2) {
            let (first, count) = (pair[0], pair[1]);
            if first + count > MAX_OBJECT_NUMBER as u64 + 1 {
                return Err(ReadError::damaged(format!(
                    "A cross-reference stream's /Index goes past PDF's highest object number ({MAX_OBJECT_NUMBER})."
                )));
            }
            for i in 0..count {
                if at + entry_length > data.len() {
                    return Err(ReadError::damaged(
                        "A cross-reference stream is shorter than its /Index says.",
                    ));
                }
                // A missing first field means type 1.
                let kind = if widths[0] == 0 {
                    1
                } else {
                    field(&data, at, widths[0])
                };
                let second = field(&data, at + widths[0], widths[1]);
                let third = field(&data, at + widths[0] + widths[1], widths[2]);
                at += entry_length;
                let number = (first + i) as u32;
                if number == 0 {
                    continue;
                }
                let entry = match kind {
                    0 => Entry::free(third.clamp(0, 65535) as u16),
                    1 if second > 0 => {
                        highest = highest.max(number);
                        Entry::in_use(second as u64, third.clamp(0, 65535) as u16)
                    }
                    2 if second > 0 && second <= i32::MAX as i64 => {
                        highest = highest.max(number);
                        Entry::compressed(second as u32)
                    }
                    // Types a later PDF version may add read as null (7.5.8.3), and so hide the entry of an older
                    // section.
                    _ => Entry::free(0),
                };
                section.entry(number).or_insert(entry);
            }
        }
        Ok((dictionary, highest))
    }
}

fn bad_entry(number: u64) -> ReadError {
    ReadError::damaged(format!(
        "Cross-reference entry {number} is not \"offset generation n|f\"."
    ))
}

fn not_a_section() -> ReadError {
    ReadError::damaged(
        "startxref points at neither a cross-reference table nor a cross-reference stream.",
    )
}

// Read as a signed 64-bit value, so an 8-byte field with the high bit set is negative, as in the reference engines.
fn field(data: &[u8], at: usize, width: usize) -> i64 {
    data[at..at + width]
        .iter()
        .fold(0i64, |value, b| (value << 8) | *b as i64)
}
