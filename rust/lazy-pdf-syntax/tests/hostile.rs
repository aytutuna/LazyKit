mod common;

use std::time::{Duration, Instant};

use common::latin;
use common::open;
use common::pdf::{TestPdf, deflate};
use lazy_pdf_syntax::Object;

// More of what a hostile file may cost; the limits are loosened for a build with no optimizing.

fn must_finish_within(limit: Duration, started: Instant) {
    assert!(started.elapsed() < limit, "took {:?}", started.elapsed());
}

fn with_xref_stream(pdf: &mut TestPdf, entries: &str, data: &[u8]) -> Vec<u8> {
    let xref = pdf.position();
    pdf.stream_with(20, entries, data, None, true);
    [
        pdf.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat()
}

#[test]
fn a_size_that_cannot_be_decremented_is_not_a_panic() {
    let file = TestPdf::with_pages(1).finish_with("/Root 1 0 R /Size -9223372036854775808", 0, "");
    let mut document = open(file);
    assert_eq!(document.pages().len(), 1);
}

#[test]
fn a_startxref_past_the_file_in_any_width_is_rebuilt_from_a_scan() {
    let file = TestPdf::with_pages(1).finish_with("/Root 1 0 R", 0, "4294967306");
    let mut document = open(file);
    assert!(document.xref_rebuilt());
    assert_eq!(document.pages().len(), 1);
}

#[test]
fn unfiltered_contents_named_again_and_again_stay_under_the_cap() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(20, "", &vec![b'q'; 1 << 20], None, true);
    pdf.obj(21, &format!("[{}]", "20 0 R ".repeat(600)));
    let file = TestPdf::replace(&pdf.finish(), "/Contents 4 0 R", "/Contents 21 0 R");
    let mut document = open(file);
    let pages = document.pages();
    let contents = pages[0].contents(&mut document);
    assert!(contents.len() <= 300 << 20, "{} bytes", contents.len());
    assert!(!document.repairs().is_empty());
}

#[test]
fn a_cross_reference_stream_that_lists_more_than_pdf_has_numbers_is_refused() {
    let mut pdf = TestPdf::with_pages(1);
    let file = with_xref_stream(
        &mut pdf,
        "/Type /XRef /Size 21 /W [1 0 0] /Index [0 8388608 0 8388608] /Root 1 0 R /Filter /FlateDecode",
        &deflate(&[0; 64]),
    );
    let started = Instant::now();
    assert!(open(file).xref_rebuilt());
    must_finish_within(Duration::from_secs(5), started);
}

#[test]
fn a_cross_reference_stream_is_not_decoded_past_its_entries() {
    let mut pdf = TestPdf::with_pages(1);
    let file = with_xref_stream(
        &mut pdf,
        "/Type /XRef /Size 21 /W [1 0 0] /Index [0 21] /Root 1 0 R /Filter /FlateDecode",
        &deflate(&vec![0; 9 << 20]),
    );
    assert!(open(file).xref_rebuilt());
}

#[test]
fn a_key_heavy_decode_parameter_dictionary_is_read_in_linear_time() {
    let keys: String = (0..60_000).map(|i| format!("/K{i} 1 ")).collect();
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(
        20,
        &format!("/Filter /FlateDecode /DecodeParms <</A <</B <</C <<{keys}>>>>>>>>"),
        &deflate(b"q "),
        None,
        true,
    );
    pdf.obj(21, "[20 0 R]");
    let file = TestPdf::replace(&pdf.finish(), "/Contents 4 0 R", "/Contents 21 0 R");
    let mut document = open(file);
    let pages = document.pages();
    let started = Instant::now();
    assert_eq!(pages[0].contents(&mut document), b"q ");
    must_finish_within(Duration::from_secs(5), started);
}

#[test]
fn object_streams_that_do_not_fit_the_cache_together_are_not_decoded_without_end() {
    let (streams, per_stream, size) = (6u32, 50u32, 24usize << 20);
    let mut pdf = TestPdf::with_header("%PDF-1.5\n");
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    for k in 0..streams {
        let mut header = String::new();
        let mut body = String::new();
        for j in 0..per_stream {
            header.push_str(&format!("{} {} ", 1000 + k * per_stream + j, body.len()));
            body.push_str("<</A 1>> ");
        }
        let first = header.len();
        let mut data = latin(&format!("{header}{body}"));
        data.resize(size, b' ');
        pdf.stream_with(
            100 + k,
            &format!("/Type /ObjStm /N {per_stream} /First {first} /Filter /FlateDecode"),
            &deflate(&data),
            None,
            true,
        );
    }
    let file = [
        pdf.finish_without_xref(),
        latin("trailer\n<</Root 1 0 R>>\n"),
    ]
    .concat();
    let started = Instant::now();
    let mut document = open(file);
    let mut read = 0;
    for j in 0..per_stream {
        for k in 0..streams {
            let found = document.get(1000 + k * per_stream + j);
            if found.is_some_and(|o| matches!(*o, Object::Dict(_))) {
                read += 1;
            }
        }
    }
    assert!(read > 0);
    must_finish_within(Duration::from_secs(60), started);
}
