#![no_main]

use lazy_pdf_fonts::{ContentInterpreter, ContentVisitor, StandardFilters, parse_content};
use lazy_pdf_syntax::Document;
use lazy_pdf_text::{
    TextPage, TextPageOptions, TextRange, TextSearch, TextSearchOptions, TextSelection,
};
use libfuzzer_sys::fuzz_target;

struct Quiet;

impl ContentVisitor for Quiet {}

fn page(content: &[u8]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    let mut add = |pdf: &mut Vec<u8>, number: u32, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    add(&mut pdf, 1, b"<</Type /Catalog /Pages 2 0 R>>");
    add(
        &mut pdf,
        2,
        b"<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200]>>",
    );
    add(
        &mut pdf,
        3,
        b"<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <</Font <</F1 5 0 R>> /XObject <</X 6 0 R>>>>>>",
    );
    let mut stream = format!("<</Length {}>>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream");
    add(&mut pdf, 4, &stream);
    add(
        &mut pdf,
        5,
        b"<</Type /Font /Subtype /Type1 /BaseFont /Helvetica>>",
    );
    let mut form = format!(
        "<</Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {}>>\nstream\n",
        content.len()
    )
    .into_bytes();
    form.extend_from_slice(content);
    form.extend_from_slice(b"\nendstream");
    add(&mut pdf, 6, &form);
    let start = pdf.len();
    pdf.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<</Size 7 /Root 1 0 R>>\nstartxref\n{start}\n%%EOF\n").as_bytes(),
    );
    pdf
}

fuzz_target!(|data: &[u8]| {
    for op in parse_content(data) {
        assert!(op.start <= op.end && op.end <= data.len());
        let _ = op.to_string();
        if let Some(image) = &op.image {
            let _ = image.expanded(None);
        }
    }
    let Ok(mut doc) = Document::open(page(data), Box::new(StandardFilters)) else {
        return;
    };
    let pages = doc.pages();
    if let Some(first) = pages.first() {
        let mut visitor = Quiet;
        ContentInterpreter::new(&mut doc, &mut visitor).run_page(first);
        let options = TextPageOptions {
            include_artifacts: true,
            remove_duplicates: true,
            include_annotations: true,
        };
        let text = TextPage::create(&mut doc, first, &options);
        let _ = text.text();
        let _ = text.get_text(TextRange::new(0, text.chars.len()), true);
        let _ = text.quads(TextRange::new(0, text.chars.len()));
        if let Some(c) = text.chars.first() {
            let (x, y) = c.quad.center();
            let _ = text.hit_test(x, y, f64::INFINITY);
            let _ = text.caret_at(x, y);
            let _ = TextSelection::word_at(&text, x, y).get_text(true);
        }
        for (query, regex) in [("a b", false), (r"\w+", true)] {
            let search = TextSearchOptions {
                regex,
                whole_word: regex,
                ..TextSearchOptions::default()
            };
            let _ = TextSearch::find(&text, query, &search);
        }
    }
});
