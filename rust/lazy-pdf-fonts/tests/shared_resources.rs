mod common;

use std::time::Instant;

use common::pdf::TestPdf;
use lazy_pdf_fonts::{ContentInterpreter, ContentVisitor, StandardFilters};
use lazy_pdf_syntax::Document;

struct Nothing;
impl ContentVisitor for Nothing {}

// Forms that name one indirect /Resources share a single copy of it instead of each holding its own.
#[test]
fn distinct_forms_share_one_big_resource_dict() {
    for (forms, entries) in [(1000u32, 5000u32)] {
        let mut pdf = TestPdf::new();
        pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
        pdf.obj(
            2,
            "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
        );
        pdf.obj(
            3,
            "<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <</XObject 6 0 R>>>>",
        );
        let content: String = (0..forms).map(|i| format!("/X{i} Do\n")).collect();
        pdf.stream(4, "", content.as_bytes());
        let big: String = (0..entries).map(|i| format!("/K{i} {i} ")).collect();
        pdf.obj(5, &format!("<<{big}>>"));
        let xo: String = (0..forms)
            .map(|i| format!("/X{i} {} 0 R ", 10 + i))
            .collect();
        pdf.obj(6, &format!("<<{xo}>>"));
        for i in 0..forms {
            pdf.stream(
                10 + i,
                "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R",
                b"0 0 m",
            );
        }
        let bytes = pdf.finish();
        let mut doc = Document::open(bytes.clone(), Box::new(StandardFilters)).unwrap();
        let pages = doc.pages();
        let mut v = Nothing;
        let t = Instant::now();
        let mut i = ContentInterpreter::new(&mut doc, &mut v);
        i.run_page(&pages[0]);
        assert!(t.elapsed() < std::time::Duration::from_millis(300));
    }
}
