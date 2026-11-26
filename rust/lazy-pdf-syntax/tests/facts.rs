mod common;

use common::pdf::TestPdf;
use common::{TestFilters, open};
use lazy_pdf_syntax::{Document, Object, Problem};

fn with_metadata(xmp: &str) -> Document {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R /Metadata 4 0 R>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>")
        .stream(4, "/Type /Metadata /Subtype /XML", xmp);
    open(pdf.finish())
}

const ID: &str = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"";

#[test]
fn the_pdfa_claim_is_read_from_an_element() {
    let xmp = format!(
        "<?xpacket begin=\"x\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description {ID}><pdfaid:part>2</pdfaid:part><pdfaid:conformance>b</pdfaid:conformance></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    );
    let mut document = with_metadata(&xmp);
    assert_eq!(document.pdfa_part(), Some(2));
    assert_eq!(document.pdfa_conformance().as_deref(), Some("B"));
}

#[test]
fn the_pdfa_claim_is_read_from_attributes_under_any_prefix() {
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\" a:part=\"1\" a:conformance=\"A\"/></rdf:RDF></x:xmpmeta>";
    let mut document = with_metadata(xmp);
    assert_eq!(document.pdfa_part(), Some(1));
    assert_eq!(document.pdfa_conformance().as_deref(), Some("A"));
}

#[test]
fn metadata_that_is_not_safe_or_well_formed_claims_nothing() {
    let cases = [
        format!(
            "<!DOCTYPE x [<!ENTITY e \"1\">]><d {ID}><pdfaid:part>2</pdfaid:part></d>"
        ),
        format!("<x:xmpmeta xmlns:x=\"a\"><d {ID}><pdfaid:part>2</d></x:xmpmeta>"),
        "<x:xmpmeta xmlns:x=\"a\"><d xmlns:pdfaid=\"http://other/\"><pdfaid:part>2</pdfaid:part></d></x:xmpmeta>".to_string(),
        format!("<x:xmpmeta xmlns:x=\"a\"><d {ID}><pdfaid:part>two</pdfaid:part></d></x:xmpmeta>"),
    ];
    for xmp in cases {
        let mut document = with_metadata(&xmp);
        assert_eq!(document.pdfa_part(), None, "{xmp}");
        assert_eq!(document.pdfa_conformance(), None, "{xmp}");
    }
    let mut plain = open(TestPdf::simple(1));
    assert_eq!(plain.pdfa_part(), None);
}

#[test]
fn deeply_nested_metadata_is_not_followed() {
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"a\"><d {ID}>{}2{}</d></x:xmpmeta>",
        "<pdfaid:part>".repeat(100_000),
        "</pdfaid:part>".repeat(100_000)
    );
    assert_eq!(with_metadata(&xmp).pdfa_part(), None);
}

#[test]
fn a_linearization_dictionary_with_another_length_was_linearized_but_is_not_now() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Linearized 1 /L 1>>")
        .obj(2, "<</Type /Catalog /Pages 3 0 R>>")
        .obj(3, "<</Type /Pages /Kids [] /Count 0>>");
    let mut document = open(pdf.finish_with("/Root 2 0 R", 0, ""));
    assert!(document.was_linearized());
    assert!(!document.is_linearized());
    assert!(!open(TestPdf::simple(1)).was_linearized());
}

#[test]
fn an_encrypt_entry_that_is_not_a_dictionary_is_damage_not_encryption() {
    for entry in ["99 0 R", "5"] {
        let file =
            TestPdf::with_pages(1).finish_with(&format!("/Root 1 0 R /Encrypt {entry}"), 0, "");
        let refused = Document::open(file, Box::new(TestFilters)).err().unwrap();
        assert_eq!(refused.problem, Problem::Damaged, "{entry}");
    }
    let file =
        TestPdf::with_pages(1).finish_with("/Root 1 0 R /Encrypt <</Filter /Standard>>", 0, "");
    let refused = Document::open(file, Box::new(TestFilters)).err().unwrap();
    assert_eq!(refused.problem, Problem::Encrypted);
}

#[test]
fn a_crypt_stage_is_refused_only_for_a_name_other_than_identity() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream(
        20,
        "/Filter /Crypt /DecodeParms <</Name (Identity)>>",
        "q Q",
    )
    .stream(21, "/Filter /Crypt /DecodeParms <</Name /Identity>>", "q Q")
    .stream(22, "/Filter /Crypt /DecodeParms <</Name /Other>>", "q Q");
    let mut document = open(pdf.finish());
    for number in [20, 21] {
        let Object::Stream(stream) = (*document.get(number).unwrap()).clone() else {
            panic!("not a stream")
        };
        assert_eq!(document.stream_data(&stream).unwrap(), b"q Q");
    }
    let Object::Stream(stream) = (*document.get(22).unwrap()).clone() else {
        panic!("not a stream")
    };
    assert_eq!(
        document.stream_data(&stream).unwrap_err().problem,
        Problem::Unsupported
    );
}

#[test]
fn deeply_nested_metadata_with_a_lot_of_text_is_read_in_bounded_memory() {
    let depth = 250;
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"a\" {ID}>{}<pdfaid:part>2</pdfaid:part>{}{}</x:xmpmeta>",
        "<e>".repeat(depth),
        "t".repeat(4 << 20),
        "</e>".repeat(depth)
    );
    let mut document = with_metadata(&xmp);
    assert_eq!(document.pdfa_part(), Some(2));
}

#[test]
fn many_prefix_bindings_are_read_in_linear_time() {
    let bindings: String = (0..50_000)
        .map(|i| format!(" xmlns:p{i}=\"u{i}\""))
        .collect();
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"a\"{bindings} {ID}>{}<pdfaid:part>1</pdfaid:part></x:xmpmeta>",
        "<e/>".repeat(100_000)
    );
    let started = std::time::Instant::now();
    let mut document = with_metadata(&xmp);
    assert_eq!(document.pdfa_part(), Some(1));
    assert!(started.elapsed().as_secs() < 10);
}
