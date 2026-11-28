// Resources, fonts and graphics state that are big and direct must not be copied or serialized at every operator: each
// of these took seconds when they were, and finishes in well under a tenth of the limit now.
mod common;

use std::time::{Duration, Instant};

use common::pdf::TestPdf;
use lazy_pdf_fonts::{ContentInterpreter, ContentVisitor, StandardFilters};
use lazy_pdf_syntax::Document;

const LIMIT: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct Nothing;
impl ContentVisitor for Nothing {}

fn page(content: &[u8], resources: &str, more: impl FnOnce(&mut TestPdf)) -> Document {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        &format!("<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<{resources}>>>>"),
    );
    pdf.stream(4, "", content);
    more(&mut pdf);
    Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap()
}

fn run(doc: &mut Document) -> bool {
    let pages = doc.pages();
    let mut v = Nothing;
    let mut i = ContentInterpreter::new(doc, &mut v);
    i.run_page(&pages[0]);
    i.is_truncated()
}

// A direct font dictionary is cloned and serialized at every Tf.
#[test]
fn direct_font_per_tf() {
    let widths: String = "500 ".repeat(200_000);
    let content: String = "/F1 12 Tf\n".repeat(200);
    let resources = format!(
        "/Font <</F1 <</Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 199999 /Widths [{widths}]>>>>"
    );
    let mut doc = page(content.as_bytes(), &resources, |_| {});
    let t = Instant::now();
    run(&mut doc);
    assert!(t.elapsed() < LIMIT);
}

// A direct /XObject category dictionary is cloned at every Do.
#[test]
fn direct_category_per_do() {
    let entries: String = (0..100_000).map(|i| format!("/N{i} 5 0 R ")).collect();
    let content: String = "/N0 Do\n".repeat(200);
    let resources = format!("/XObject <<{entries}>>");
    let mut doc = page(content.as_bytes(), &resources, |pdf| {
        pdf.stream(5, "/Type /XObject /Subtype /Form /BBox [0 0 1 1]", b"");
    });
    let t = Instant::now();
    run(&mut doc);
    assert!(t.elapsed() < LIMIT);
}

// The soft mask dictionary is part of GraphicsState and deep-cloned at every q and every form entry.
#[test]
fn soft_mask_per_q() {
    let junk: String = "1 ".repeat(200_000);
    let content: String = std::iter::once("/G1 gs\n".to_string())
        .chain(std::iter::repeat_n("q Q\n".to_string(), 200))
        .collect();
    let mut doc = page(
        content.as_bytes(),
        "/ExtGState <</G1 <</SMask 5 0 R>>>>",
        |pdf| {
            pdf.obj(5, &format!("<</Type /Mask /S /Luminosity /BC [{junk}]>>"));
        },
    );
    let t = Instant::now();
    run(&mut doc);
    assert!(t.elapsed() < LIMIT);
}

// A form's direct /Resources dictionary is cloned at each entry.
#[test]
fn form_resources_per_do() {
    let junk: String = "1 ".repeat(200_000);
    let content: String = "/X Do\n".repeat(200);
    let mut doc = page(content.as_bytes(), "/XObject <</X 5 0 R>>", |pdf| {
        pdf.stream(
            5,
            &format!("/Subtype /Form /BBox [0 0 1 1] /Resources <</Junk [{junk}]>>"),
            b"",
        );
    });
    let t = Instant::now();
    run(&mut doc);
    assert!(t.elapsed() < LIMIT);
}
