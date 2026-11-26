use std::collections::BTreeMap;

use super::latin;

/// Builds small PDF files byte by byte, so that a test can break exactly one thing.
pub struct TestPdf {
    bytes: Vec<u8>,
    offsets: BTreeMap<u32, u64>,
}

impl TestPdf {
    pub fn new() -> TestPdf {
        TestPdf::with_header("%PDF-1.4\n")
    }

    pub fn with_header(header: &str) -> TestPdf {
        let mut pdf = TestPdf {
            bytes: Vec::new(),
            offsets: BTreeMap::new(),
        };
        pdf.write(header);
        pdf
    }

    pub fn position(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn write(&mut self, text: &str) -> &mut TestPdf {
        self.bytes.extend_from_slice(&latin(text));
        self
    }

    pub fn obj(&mut self, number: u32, body: &str) -> &mut TestPdf {
        self.offsets.insert(number, self.position());
        self.write(&format!("{number} 0 obj\n{body}\nendobj\n"))
    }

    /// A stream object; `length` is what /Length says (the data's length when none, omitted when negative).
    pub fn stream_with(
        &mut self,
        number: u32,
        entries: &str,
        data: &[u8],
        length: Option<i64>,
        endstream: bool,
    ) -> &mut TestPdf {
        self.offsets.insert(number, self.position());
        let declared = match length {
            Some(l) if l < 0 => String::new(),
            Some(l) => format!("/Length {l}"),
            None => format!("/Length {}", data.len()),
        };
        self.write(&format!(
            "{number} 0 obj\n<<{entries} {declared}>>\nstream\n"
        ));
        self.bytes.extend_from_slice(data);
        self.write(if endstream {
            "\nendstream\nendobj\n"
        } else {
            "\nendobj\n"
        })
    }

    pub fn stream(&mut self, number: u32, entries: &str, data: &str) -> &mut TestPdf {
        self.stream_with(number, entries, &latin(data), None, true)
    }

    /// The classic table and trailer, and the file; `shift` moves every offset by that much.
    pub fn finish_with(&self, trailer: &str, shift: i64, startxref: &str) -> Vec<u8> {
        let start = self.position();
        let size = self.offsets.keys().max().unwrap() + 1;
        let mut text = format!("xref\n0 {size}\n0000000000 65535 f \n");
        for n in 1..size {
            match self.offsets.get(&n) {
                Some(offset) => {
                    text.push_str(&format!("{:010} 00000 n \n", *offset as i64 + shift))
                }
                None => text.push_str("0000000000 00000 f \n"),
            }
        }
        let startxref = if startxref.is_empty() {
            start.to_string()
        } else {
            startxref.to_string()
        };
        text.push_str(&format!(
            "trailer\n<</Size {size} {trailer}>>\nstartxref\n{startxref}\n%%EOF\n"
        ));
        [self.bytes.clone(), latin(&text)].concat()
    }

    pub fn finish(&self) -> Vec<u8> {
        self.finish_with("/Root 1 0 R", 0, "")
    }

    pub fn finish_without_xref(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// A catalog (1), a page tree (2) and `count` pages (3...), each with a content stream after them.
    pub fn with_pages(count: u32) -> TestPdf {
        let mut pdf = TestPdf::new();
        pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
        let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", 3 + i)).collect();
        pdf.obj(
            2,
            &format!(
                "<</Type /Pages /Kids [{}] /Count {count} /MediaBox [0 0 200 300]>>",
                kids.join(" ")
            ),
        );
        for i in 0..count {
            pdf.obj(
                3 + i,
                &format!(
                    "<</Type /Page /Parent 2 0 R /Contents {} 0 R>>",
                    3 + count + i
                ),
            );
        }
        for i in 0..count {
            pdf.stream(
                3 + count + i,
                "",
                &format!("BT /F1 12 Tf ({}) Tj ET", i + 1),
            );
        }
        pdf
    }

    pub fn simple(count: u32) -> Vec<u8> {
        TestPdf::with_pages(count).finish()
    }

    /// The file with the first `find` replaced by `replacement`.
    pub fn replace(file: &[u8], find: &str, replacement: &str) -> Vec<u8> {
        let text: String = file.iter().map(|&b| b as char).collect();
        let at = text
            .find(find)
            .unwrap_or_else(|| panic!("\"{find}\" is not in the file."));
        latin(&format!(
            "{}{replacement}{}",
            &text[..at],
            &text[at + find.len()..]
        ))
    }
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 1)
}
fn rows(rows: &[[u8; 5]]) -> Vec<u8> {
    rows.concat()
}

/// Objects 1 to 3 in object stream 5, content stream 4, cross-reference stream 6.
pub fn object_stream_file() -> (TestPdf, u64) {
    object_stream_file_padded(0)
}

/// The same file with `pad` blanks after the last object in the object stream.
pub fn object_stream_file_padded(pad: usize) -> (TestPdf, u64) {
    const BODIES: [&str; 3] = [
        "<</Type /Catalog /Pages 2 0 R>>",
        "<</Type /Pages /Kids [3 0 R] /Count 1>>",
        "<</Type /Page /Parent 2 0 R /Contents 4 0 R /MediaBox [0 0 100 100]>>",
    ];
    let mut header = String::new();
    let mut body = String::new();
    for (i, text) in BODIES.iter().enumerate() {
        header.push_str(&format!("{} {} ", i + 1, body.len()));
        body.push_str(text);
        body.push('\n');
    }
    let mut pdf = TestPdf::with_header("%PDF-1.5\n");
    let content = pdf.position();
    pdf.stream(4, "", "q Q");
    let objects = pdf.position();
    pdf.stream_with(
        5,
        &format!(
            "/Type /ObjStm /N 3 /First {} /Filter /FlateDecode",
            header.len()
        ),
        &deflate(&latin(&format!("{header}{body}{}", " ".repeat(pad)))),
        None,
        true,
    );
    let xref_offset = pdf.position();
    let row = |kind: u8, field: u64, third: u8| {
        [
            kind,
            (field >> 16) as u8,
            (field >> 8) as u8,
            field as u8,
            third,
        ]
    };
    let data = rows(&[
        row(0, 0, 255),
        row(2, 5, 0),
        row(2, 5, 1),
        row(2, 5, 2),
        row(1, content, 0),
        row(1, objects, 0),
        row(1, xref_offset, 0),
    ]);
    pdf.stream_with(
        6,
        "/Type /XRef /Size 7 /W [1 3 1] /Root 1 0 R /Filter /FlateDecode ",
        &deflate(&data),
        None,
        true,
    );
    (pdf, xref_offset)
}

impl TestPdf {
    /// Where object `number` starts.
    pub fn offset_of(&self, number: u32) -> u64 {
        self.offsets[&number]
    }
}
