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

// Past the 256 colour spaces the interpreter keeps, each use is parsed again, so each one is charged to the budget:
// without it ten million of them are ten million parses.
#[test]
fn uncached_color_spaces_are_charged() {
    let names: String = (0..300)
        .map(|i| format!("/C{i} [/Indexed /DeviceGray 255 <00>] "))
        .collect();
    let mut content: String = (0..256).map(|i| format!("/C{i} cs\n")).collect();
    content += &"/C256 cs /C257 cs\n".repeat(1000);
    let mut doc = page(
        content.as_bytes(),
        &format!("/ColorSpace <<{names}>>"),
        |_| {},
    );
    assert!(run(&mut doc));
}

// A type 4 program larger than any real one is refused rather than read at every colour space that names it.
#[test]
fn huge_calculator_programs_are_refused() {
    use lazy_pdf_fonts::Function;
    let program = format!("{{{}}}", "0 pop ".repeat(100_000));
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    pdf.stream(
        3,
        "/FunctionType 4 /Domain [0 1] /Range [0 1]",
        program.as_bytes(),
    );
    let mut doc = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let function = doc.get(3).unwrap();
    assert!(Function::parse(&mut doc, &function).is_err());
}
