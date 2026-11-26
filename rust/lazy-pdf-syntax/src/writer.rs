//! The header, numbered objects and streams, and the cross-reference section that closes a file or an update, as a
//! table (ISO 32000-1 7.5.4) or a stream (7.5.8).

use sha2::{Digest, Sha256};

use crate::filters::Filters;
use crate::object::{Dict, Name, Object, PdfString};

/// One cross-reference entry. Type 0 is free (`second` the next free number), type 1 in use (the offset), type 2
/// stored in an object stream (the stream's number, with `third` the index in it); for types 0 and 1 `third` is the
/// generation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct XrefEntry {
    pub number: u32,
    pub kind: u8,
    pub second: u64,
    pub third: u32,
}

impl XrefEntry {
    pub fn new(number: u32, kind: u8, second: u64, third: u32) -> XrefEntry {
        XrefEntry {
            number,
            kind,
            second,
            third,
        }
    }
}

/// Writes objects as bytes. `base` is the offset of the first byte this writer adds (an update starts after the
/// original).
pub(crate) struct ObjectWriter {
    pub out: Vec<u8>,
    base: u64,
}

impl ObjectWriter {
    pub fn new(base: u64) -> ObjectWriter {
        ObjectWriter {
            out: Vec::new(),
            base,
        }
    }

    /// The offset the next byte lands at.
    pub fn position(&self) -> u64 {
        self.base + self.out.len() as u64
    }

    /// "%PDF-version" and the comment of four high bytes that tells transfer programs the file is binary.
    pub fn write_header(&mut self, version: &str) {
        self.out
            .extend_from_slice(format!("%PDF-{version}\n").as_bytes());
        self.out
            .extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
    }

    /// A non-stream object; the offset it starts at.
    pub fn write_object(&mut self, number: u32, generation: u16, value: &Object) -> u64 {
        let at = self.begin(number, generation);
        value.write_to(&mut self.out);
        self.out.extend_from_slice(b"\nendobj\n");
        at
    }

    /// A stream object. `stored` is the data as it is to be stored, filters applied; /Length is written from what
    /// is stored.
    pub fn write_stream(
        &mut self,
        number: u32,
        generation: u16,
        dict: &Dict,
        stored: &[u8],
    ) -> u64 {
        let at = self.begin(number, generation);
        self.out.extend_from_slice(b"<<");
        let mut first = true;
        for (key, value) in dict.live_entries() {
            if key.is("Length") {
                continue;
            }
            if !first {
                self.out.push(b' ');
            }
            first = false;
            key.write_to(&mut self.out);
            self.out.push(b' ');
            value.write_to(&mut self.out);
        }
        if !first {
            self.out.push(b' ');
        }
        self.out
            .extend_from_slice(format!("/Length {}>>\nstream\n", stored.len()).as_bytes());
        self.out.extend_from_slice(stored);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
        at
    }

    pub fn write_ascii(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    fn begin(&mut self, number: u32, generation: u16) -> u64 {
        let at = self.position();
        self.out
            .extend_from_slice(format!("{number} {generation} obj\n").as_bytes());
        at
    }
}

/// A classic table of `entries` (ascending, no type 2), a subsection to each run of consecutive numbers, then the
/// trailer, "startxref" and "%%EOF".
pub(crate) fn write_table(writer: &mut ObjectWriter, entries: &[XrefEntry], trailer: &Dict) {
    let start = writer.position();
    let mut table = String::from("xref\n");
    for (first, count) in runs(entries) {
        table.push_str(&format!("{} {count}\n", entries[first].number));
        for entry in &entries[first..first + count] {
            table.push_str(&format!(
                "{:010} {:05}{}",
                entry.second,
                entry.third,
                if entry.kind == 0 { " f \n" } else { " n \n" }
            ));
        }
    }
    table.push_str("trailer\n");
    writer.write_ascii(&table);
    trailer.write_to(&mut writer.out);
    writer.write_ascii(&format!("\nstartxref\n{start}\n%%EOF\n"));
}

/// A cross-reference stream, object `number`, written last and listing itself. /W is [1 w x] with w wide enough for
/// the largest offset and x 2 bytes, more for an index past 65535. `dictionary` holds /Size, /Root and the rest of
/// what a trailer holds. With `compress` the data goes through the Flate filter.
pub(crate) fn write_stream(
    writer: &mut ObjectWriter,
    number: u32,
    mut entries: Vec<XrefEntry>,
    dictionary: &Dict,
    compress: Option<&dyn Filters>,
) {
    let start = writer.position();
    entries.push(XrefEntry::new(number, 1, start, 0));
    let mut width = 1;
    for entry in &entries {
        while width < 8 && entry.second >= 1u64 << (width * 8) {
            width += 1;
        }
    }
    let mut third_width = 2;
    for entry in &entries {
        while third_width < 4 && entry.third as u64 >= 1u64 << (third_width * 8) {
            third_width += 1;
        }
    }
    let mut data = Vec::with_capacity(entries.len() * (width + 1 + third_width));
    for entry in &entries {
        data.push(entry.kind);
        for b in (0..width).rev() {
            data.push((entry.second >> (b * 8)) as u8);
        }
        for b in (0..third_width).rev() {
            data.push((entry.third >> (b * 8)) as u8);
        }
    }

    let mut index = Vec::new();
    for (first, count) in runs(&entries) {
        index.push(Object::int(entries[first].number as i64));
        index.push(Object::int(count as i64));
    }
    let mut head = Dict::new();
    head.add(Name::new("Type"), Object::name("XRef"));
    head.add(
        Name::new("W"),
        Object::Array(vec![
            Object::int(1),
            Object::int(width as i64),
            Object::int(third_width as i64),
        ]),
    );
    head.add(Name::new("Index"), Object::Array(index));
    for (key, value) in dictionary.entries() {
        head.add(key.clone(), value.clone());
    }
    if let Some(filters) = compress {
        data = filters.flate(&data);
        head.add(Name::new("Filter"), Object::name("FlateDecode"));
    }
    writer.write_stream(number, 0, &head, &data);
    writer.write_ascii(&format!("startxref\n{start}\n%%EOF\n"));
}

/// The trailer's /ID: the file's first string and a second one.
pub(crate) fn id_pair(first: Vec<u8>, second: Vec<u8>) -> Object {
    Object::Array(vec![
        Object::String(PdfString {
            bytes: first,
            hex: true,
        }),
        Object::String(PdfString {
            bytes: second,
            hex: true,
        }),
    ])
}

/// Sixteen bytes that are a function of what the parts hold; /ID strings come from the content they name, so a save
/// is the same bytes every time it is made.
pub(crate) fn digest16(parts: &[&[u8]]) -> Vec<u8> {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize()[..16].to_vec()
}

/// The runs of consecutive numbers in `entries` (ascending): where each starts in the list and how long it is.
fn runs(entries: &[XrefEntry]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let mut j = i + 1;
        while j < entries.len() && entries[j].number == entries[j - 1].number + 1 {
            j += 1;
        }
        runs.push((i, j - i));
        i = j;
    }
    runs
}
