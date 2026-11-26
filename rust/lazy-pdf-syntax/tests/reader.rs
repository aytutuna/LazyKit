mod common;

use common::open;
use common::pdf::{TestPdf, deflate, object_stream_file};
use common::{TestFilters, latin};
use lazy_pdf_syntax::{Document, Lexer, Object, Problem, Ref};

fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn stream_of(document: &mut Document, number: u32) -> lazy_pdf_syntax::Stream {
    match &*document.get(number).unwrap() {
        Object::Stream(s) => s.clone(),
        other => panic!("{other}"),
    }
}

#[test]
fn a_plain_file_opens_without_repairs() {
    let mut document = open(TestPdf::simple(2));
    assert!(document.repairs().is_empty());
    assert_eq!(document.version(), "1.4");
    assert_eq!(document.pages().len(), 2);
    assert_eq!(document.next_object_number(), 7);
    assert!(!document.xref_rebuilt());
}

#[test]
fn objects_in_object_streams_are_read() {
    let (pdf, xref) = object_stream_file();
    let file = [
        pdf.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat();
    let mut document = open(file);
    assert!(document.repairs().is_empty());
    assert_eq!(document.version(), "1.5");
    assert!(document.xref_is_stream());
    assert_eq!(document.next_object_number(), 7);
    assert_eq!(document.object_numbers(), [1, 2, 3, 4, 5, 6]);
    let pages = document.pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_box(&mut document).width(), 100.0);
    assert_eq!(text(&pages[0].contents(&mut document)), "q Q");
}

#[test]
fn a_lost_cross_reference_stream_is_rebuilt_from_the_object_stream() {
    let (pdf, _) = object_stream_file();
    let file = [pdf.finish_without_xref(), latin("startxref\n12\n%%EOF\n")].concat();
    let mut document = open(file);
    assert!(document.xref_rebuilt());
    assert!(document.repairs().iter().any(|r| r.contains("Rebuilt")));
    assert_eq!(document.pages().len(), 1);
    assert!(document.catalog().is_some());
}

#[test]
fn nesting_has_a_limit() {
    let deep = |n: usize| latin(&format!("{}{}", "[".repeat(n), "]".repeat(n)));
    assert!(Lexer::new(&deep(150), 0).read_object(false).is_ok());
    let refused = Lexer::new(&deep(5000), 0).read_object(false).unwrap_err();
    assert_eq!(refused.problem, Problem::Unsupported);
    let dictionaries = latin(&"<</A ".repeat(5000));
    assert!(Lexer::new(&dictionaries, 0).read_object(false).is_err());
}

#[test]
fn a_cross_reference_loop_is_repaired() {
    let pdf = TestPdf::with_pages(1);
    let start = pdf.position();
    let mut document = open(pdf.finish_with(&format!("/Root 1 0 R /Prev {start}"), 0, ""));
    assert!(document.xref_rebuilt());
    assert!(document.repairs()[0].contains("point back at each other"));
    assert_eq!(document.pages().len(), 1);
}

#[test]
fn a_subsection_past_the_highest_object_number_is_repaired() {
    let pdf = TestPdf::with_pages(1);
    let start = pdf.position();
    let mut file = pdf.finish_without_xref();
    file.extend(latin(&format!(
        "xref\n0 99999999\n0000000000 65535 f \ntrailer\n<</Size 5 /Root 1 0 R>>\nstartxref\n{start}\n%%EOF\n"
    )));
    let document = open(file);
    assert!(document.xref_rebuilt());
    assert!(document.repairs()[0].contains("highest object number"));
}

#[test]
fn a_reference_loop_leads_nowhere() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.obj(10, "11 0 R").obj(11, "10 0 R");
    let mut document = open(pdf.finish_without_xref());
    let start = Object::reference(10, 0);
    assert!(document.resolve(&start).is_none());
    let missing = Object::reference(500, 0);
    assert!(document.resolve(&missing).is_none());
}

#[test]
fn repairs_stop_being_listed_after_a_hundred() {
    let mut pdf = TestPdf::with_pages(1);
    for n in 10..160 {
        pdf.obj(n, "<<>>");
    }
    let start = pdf.position();
    let mut table = String::from("xref\n0 160\n0000000000 65535 f \n");
    for n in 1..160 {
        // The objects the table lists past the first few are not where it says.
        let at = if n <= 4 { pdf.offset_of(n) } else { 3 };
        table.push_str(&format!("{at:010} 00000 n \n"));
    }
    table.push_str(&format!(
        "trailer\n<</Size 160 /Root 1 0 R>>\nstartxref\n{start}\n%%EOF\n"
    ));
    let mut file = pdf.finish_without_xref();
    file.extend(latin(&table));
    let mut document = open(file);
    for n in 10..160 {
        document.get(n);
    }
    assert_eq!(document.repairs().len(), 101);
    assert_eq!(
        document.repairs()[100],
        "More repairs were made than are listed."
    );
}

#[test]
fn stream_data_stays_within_its_limit() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(
        9,
        "/Filter /FlateDecode",
        &deflate(&vec![0; 1 << 20]),
        None,
        true,
    );
    let mut document = open(pdf.finish());
    let stream = stream_of(&mut document, 9);
    assert_eq!(document.stream_data(&stream).unwrap().len(), 1 << 20);
    let refused = document
        .stream_data_limited(&stream, 1000, false)
        .unwrap_err();
    assert_eq!(refused.problem, Problem::Damaged);
}

#[test]
fn a_stream_may_name_the_identity_crypt_filter() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream(
        9,
        "/Filter /Crypt /DecodeParms <</Name /Identity>>",
        "plain",
    );
    pdf.stream(10, "/Filter /Crypt /DecodeParms <</Name /Other>>", "plain");
    pdf.stream(11, "/Filter /NoSuchThing", "plain");
    let mut document = open(pdf.finish());
    let identity = stream_of(&mut document, 9);
    assert_eq!(document.stream_data(&identity).unwrap(), b"plain");
    let named = stream_of(&mut document, 10);
    assert_eq!(
        document.stream_data(&named).unwrap_err().problem,
        Problem::Unsupported
    );
    let unknown = stream_of(&mut document, 11);
    assert_eq!(
        document.stream_data(&unknown).unwrap_err().problem,
        Problem::Unsupported
    );
}

#[test]
fn an_encrypted_file_is_refused() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.obj(9, "<</Filter /Standard /V 1>>");
    let file = pdf.finish_with("/Root 1 0 R /Encrypt 9 0 R", 0, "");
    let refused = Document::open(file, Box::new(TestFilters)).err().unwrap();
    assert_eq!(refused.problem, Problem::Encrypted);
}

#[test]
fn the_facts_come_from_the_catalog() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R /MarkInfo <</Marked true>> /AcroForm <</SigFlags 3 /XFA []>>>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>");
    let mut document = open(pdf.finish());
    assert!(document.is_tagged());
    assert!(document.is_signed());
    assert!(document.has_xfa());
    assert!(!document.is_certified());
    assert!(!document.is_linearized());
}

#[test]
fn a_file_with_no_pages_and_no_catalog_is_damaged() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Font>>");
    let refused = Document::open(pdf.finish_without_xref(), Box::new(TestFilters))
        .err()
        .unwrap();
    assert_eq!(refused.problem, Problem::Damaged);
}

#[test]
fn a_repaired_catalog_is_one_the_file_can_use() {
    let mut document = open(TestPdf::with_pages(2).finish_with("", 0, ""));
    assert_eq!(document.pages().len(), 2);
    let root = document.trailer().get("Root").cloned().unwrap();
    assert_eq!(
        root,
        Object::Ref(Ref {
            number: 1,
            generation: 0
        })
    );
    assert!(
        document
            .repairs()
            .iter()
            .any(|r| r.contains("Found the catalog"))
    );
}

#[test]
fn a_page_takes_what_it_lacks_from_the_nodes_above() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 2 /MediaBox [0 0 100 200] /Rotate 90 /Resources <</Font <<>>>>>>")
        .obj(3, "<</Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2>>")
        .obj(4, "<</Type /Page /Parent 3 0 R>>")
        .obj(5, "<</Type /Page /Parent 3 0 R /Rotate 270 /Resources <</XObject <<>>>>>>");
    let mut document = open(pdf.finish());
    let pages = document.pages();
    assert_eq!(pages[0].rotate(&mut document), 90);
    assert_eq!(pages[1].rotate(&mut document), 270);
    assert_eq!(pages[1].media_box(&mut document).height(), 200.0);
    assert!(
        pages[0]
            .resources(&mut document)
            .unwrap()
            .contains_key("Font")
    );
    assert!(
        pages[1]
            .resources(&mut document)
            .unwrap()
            .contains_key("XObject")
    );
}

#[test]
fn a_root_that_names_another_reference_is_still_a_file_that_saves_and_reopens() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "2 0 R")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>");
    let mut document = open(pdf.finish_with("/Root 1 0 R", 0, "99999999"));
    assert!(document.xref_rebuilt());
    assert_eq!(document.pages().len(), 1);
    for packed in [false, true] {
        let saved = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
            use_object_streams: packed,
            document_id: None,
        });
        let mut reopened = open(saved.bytes);
        assert_eq!(reopened.pages().len(), 1, "packed {packed}");
    }
}
