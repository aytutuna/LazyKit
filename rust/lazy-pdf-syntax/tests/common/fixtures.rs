use std::collections::BTreeMap;

use super::open;
use lazy_pdf_syntax::{Document, Object, PdfString, Ref, SaveOptions};

use super::latin;
use super::pdf::TestPdf;

/// The same small two-page file (catalog 1, page tree 2, pages 3 and 4, contents 5 and 6, info 7) laid out the ways a
/// file can be: classic table, cross-reference stream, objects in object streams and a hybrid. The reference engines
/// also lay it out encrypted; that is the crypt layer's.
const BODIES: [&str; 4] = [
    "<</Type /Catalog /Pages 2 0 R>>",
    "<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] /Resources <</Font <</F1 <</Type /Font /Subtype /Type1 /BaseFont /Helvetica>>>>>>>>",
    "<</Type /Page /Parent 2 0 R /Contents 5 0 R>>",
    "<</Type /Page /Parent 2 0 R /Contents 6 0 R>>",
];

const PAGE_ONE: &str = "BT /F1 12 Tf 20 100 Td (Page one) Tj ET";
const PAGE_TWO: &str = "BT /F1 12 Tf 20 100 Td (Page two) Tj ET";
const INFO: &str = "<</Title (Original title) /Author (Someone)>>";

fn file_id() -> Vec<u8> {
    (1..=16).collect()
}

fn id_hex() -> String {
    file_id().iter().map(|b| format!("{b:02X}")).collect()
}

pub fn make_save_fixture(kind: &str) -> Vec<u8> {
    match kind {
        "table" => table(),
        "stream" => layout(false),
        "hybrid" => layout(true),
        "objects" => {
            let mut document = open(table());
            document
                .save_rewrite(&SaveOptions {
                    use_object_streams: true,
                    document_id: None,
                })
                .bytes
        }
        other => panic!("{other}"),
    }
}

pub fn table() -> Vec<u8> {
    let mut pdf = TestPdf::new();
    for (i, body) in BODIES.iter().enumerate() {
        pdf.obj(i as u32 + 1, body);
    }
    pdf.stream(5, "", PAGE_ONE);
    pdf.stream(6, "", PAGE_TWO);
    pdf.obj(7, INFO);
    let id = id_hex();
    pdf.finish_with(
        &format!("/Root 1 0 R /Info 7 0 R /ID [<{id}> <{id}>]"),
        0,
        "",
    )
}

/// Cross-reference stream 8; with `hybrid` a classic table instead, page 3 in object stream 8 and the stream 9 that
/// lists it, which the table's /XRefStm points at.
fn layout(hybrid: bool) -> Vec<u8> {
    struct File {
        bytes: Vec<u8>,
        offsets: BTreeMap<u32, u64>,
    }
    impl File {
        fn write(&mut self, text: &str) {
            self.bytes.extend_from_slice(&latin(text));
        }
        fn obj(&mut self, number: u32, body: &str) {
            self.offsets.insert(number, self.bytes.len() as u64);
            self.write(&format!("{number} 0 obj\n{body}\nendobj\n"));
        }
        fn stream(&mut self, number: u32, entries: &str, data: &str) {
            self.obj(
                number,
                &format!(
                    "<<{entries} /Length {}>>\nstream\n{data}\nendstream",
                    data.chars().count()
                ),
            );
        }
    }
    let mut file = File {
        bytes: Vec::new(),
        offsets: BTreeMap::new(),
    };
    let id = id_hex();

    file.write("%PDF-1.5\n");
    for (i, body) in BODIES.iter().enumerate() {
        if !(hybrid && i == 2) {
            file.obj(i as u32 + 1, body);
        }
    }
    file.stream(5, "", PAGE_ONE);
    file.stream(6, "", PAGE_TWO);
    file.obj(7, INFO);
    if !hybrid {
        let start = file.bytes.len() as u64;
        let mut rows: Vec<u8> = Vec::new();
        let mut row = |kind: u8, second: u64, third: u8| {
            rows.push(kind);
            for b in (0..4).rev() {
                rows.push((second >> (b * 8)) as u8);
            }
            rows.push(third);
        };
        row(0, 0, 255);
        for offset in file.offsets.values() {
            row(1, *offset, 0);
        }
        row(1, start, 0);
        let data: String = rows.iter().map(|&b| b as char).collect();
        file.write(&format!(
            "8 0 obj\n<</Type /XRef /Size 9 /W [1 4 1] /Root 1 0 R /Info 7 0 R /ID [<{id}> <{id}>] /Length {}>>\nstream\n{data}\nendstream\nendobj\n",
            rows.len()
        ));
        file.write(&format!("startxref\n{start}\n%%EOF\n"));
        return file.bytes;
    }

    let page = BODIES[2];
    let header = "3 0 ";
    file.stream(
        8,
        &format!("/Type /ObjStm /N 1 /First {}", header.len()),
        &format!("{header}{page}"),
    );
    let xref_stream = file.bytes.len();
    let row: String = [2u8, 0, 0, 0, 8, 0].iter().map(|&b| b as char).collect();
    file.stream(9, "/Type /XRef /Size 10 /W [1 4 1] /Index [3 1]", &row);
    let table = file.bytes.len();
    let mut text = String::from("xref\n0 10\n0000000000 65535 f \n");
    for n in 1..10 {
        match file.offsets.get(&n) {
            Some(at) => text.push_str(&format!("{at:010} 00000 n \n")),
            None => text.push_str("0000000000 00000 f \n"),
        }
    }
    text.push_str(&format!(
        "trailer\n<</Size 10 /Root 1 0 R /Info 7 0 R /ID [<{id}> <{id}>] /XRefStm {xref_stream}>>\nstartxref\n{table}\n%%EOF\n"
    ));
    file.write(&text);
    file.bytes
}

/// A new title, new text on page two, and a new object the catalog points at.
pub fn edit(document: &mut Document) {
    let info = ref_of(document, "Info");
    let Some(Object::Dict(info)) = document.get_mut(info.number) else {
        panic!("no info")
    };
    info.insert(
        "Title",
        Object::String(PdfString {
            bytes: b"New title".to_vec(),
            hex: false,
        }),
    );
    let pages = document.pages();
    let contents = {
        let dict = pages[1].dict(document);
        let Some(Object::Ref(contents)) = dict.get("Contents") else {
            panic!("no contents")
        };
        *contents
    };
    let Some(Object::Stream(stream)) = document.get_mut(contents.number) else {
        panic!("no stream")
    };
    stream.data = Some(b"BT /F1 12 Tf 20 100 Td (Changed) Tj ET".to_vec());
    let mut note = lazy_pdf_syntax::Dict::new();
    note.insert(
        "Note",
        Object::String(PdfString {
            bytes: b"Added note".to_vec(),
            hex: false,
        }),
    );
    let note = document.add(Object::Dict(note)).unwrap();
    let root = ref_of(document, "Root");
    let Some(Object::Dict(catalog)) = document.get_mut(root.number) else {
        panic!("no catalog")
    };
    catalog.insert("Extra", Object::Ref(note));
}

fn ref_of(document: &Document, key: &str) -> Ref {
    match document.trailer().get(key) {
        Some(Object::Ref(r)) => *r,
        _ => panic!("trailer has no {key}"),
    }
}
