//! The byte-level layer of the PDF writer: the header, numbered objects with their byte offsets, streams, and the classic
//! cross-reference table and trailer. It knows nothing about pages or fonts. The same bytes as Lazy Scan's Swift and C#
//! object writers.

use std::fmt::Write as _;

pub struct ObjectWriter {
    out: Vec<u8>,
    /// Byte offset of object `i + 1`; `None` until it is written.
    offsets: Vec<Option<usize>>,
}

impl Default for ObjectWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectWriter {
    pub fn new() -> ObjectWriter {
        ObjectWriter {
            out: Vec::new(),
            offsets: Vec::new(),
        }
    }

    /// Bytes written so far: the offset the next byte lands at.
    pub fn position(&self) -> usize {
        self.out.len()
    }

    /// "%PDF-{version}" and the comment line of four bytes above 127 that marks the file as binary.
    pub fn write_header(&mut self, version: &str) {
        self.write_ascii(&format!("%PDF-{version}\n"));
        self.out.extend([b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
    }

    fn begin_object(&mut self, number: usize) {
        assert!(number >= 1, "object numbers start at 1");
        while self.offsets.len() < number {
            self.offsets.push(None);
        }
        assert!(
            self.offsets[number - 1].is_none(),
            "object {number} was already written"
        );
        self.offsets[number - 1] = Some(self.out.len());
        self.write_ascii(&format!("{number} 0 obj\n"));
    }

    /// A whole object whose value is `value` (a dictionary, an array, ...).
    pub fn write_object(&mut self, number: usize, value: &str) {
        self.begin_object(number);
        self.write_ascii(value);
        self.write_ascii("\nendobj\n");
    }

    /// A stream object: "<<{entries} /Length n>>", the data between "stream" and "endstream", each keyword on a line of
    /// its own. `entries` are the dictionary's other entries ("" for none); the length is always added last.
    pub fn write_stream(&mut self, number: usize, entries: &str, data: &[u8]) {
        self.begin_object(number);
        let separator = if entries.is_empty() { "" } else { " " };
        self.write_ascii(&format!(
            "<<{entries}{separator}/Length {}>>\nstream\n",
            data.len()
        ));
        self.out.extend(data);
        self.write_ascii("\nendstream\nendobj\n");
    }

    /// The cross-reference table of every object written, then the trailer with `trailer` after /Size (for example
    /// "/Root 1 0 R"), "startxref" and "%%EOF". Every number up to the highest used must have been written.
    pub fn finish(mut self, trailer: &str) -> Vec<u8> {
        let mut table = String::new();
        let start = self.out.len();
        let _ = write!(
            table,
            "xref\n0 {}\n0000000000 65535 f \n",
            self.offsets.len() + 1
        );
        for (i, offset) in self.offsets.iter().enumerate() {
            let offset = offset.unwrap_or_else(|| panic!("object {} was never written", i + 1));
            let _ = writeln!(table, "{offset:010} 00000 n ");
        }
        self.write_ascii(&table);
        let size = self.offsets.len() + 1;
        self.write_ascii(&format!(
            "trailer\n<</Size {size} {trailer}>>\nstartxref\n{start}\n%%EOF\n"
        ));
        self.out
    }

    /// Text that is ASCII by construction (PDF syntax).
    pub fn write_ascii(&mut self, text: &str) {
        debug_assert!(text.is_ascii());
        self.out.extend(text.as_bytes());
    }
}

/// A string in an object's dictionary: "(text)" with \, ( and ) escaped.
pub fn ascii_string(text: &str) -> String {
    assert!(text.is_ascii(), "PDF syntax strings are ASCII");
    let mut out = String::from("(");
    for c in text.chars() {
        if matches!(c, '\\' | '(' | ')') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(')');
    out
}
