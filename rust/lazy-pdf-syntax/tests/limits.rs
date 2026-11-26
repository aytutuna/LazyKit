mod common;

use std::time::{Duration, Instant};

use common::fixtures::make_save_fixture;
use common::open;
use common::pdf::{TestPdf, deflate};
use common::{TestFilters, latin};
use lazy_pdf_syntax::{Document, Object, PdfString, Problem};

// What a hostile file may cost. The time limits are the C# engine's, loosened for a build with no optimizing.

fn row(kind: u8, second: u64, third: u8) -> [u8; 5] {
    [
        kind,
        (second >> 16) as u8,
        (second >> 8) as u8,
        second as u8,
        third,
    ]
}

fn must_finish_within(limit: Duration, started: Instant) {
    assert!(started.elapsed() < limit, "took {:?}", started.elapsed());
}

#[test]
fn trailers_inside_a_value_that_is_not_a_dictionary_are_scanned_once() {
    let depth = 40_000;
    let text = format!(
        "%PDF-1.4\n{}{}",
        "trailer (".repeat(depth),
        ")".repeat(depth)
    );
    let started = Instant::now();
    assert!(Document::open(latin(&text), Box::new(TestFilters)).is_err());
    must_finish_within(Duration::from_secs(10), started);
}

#[test]
fn a_repair_with_no_object_numbers_left_is_a_read_error() {
    let file = latin("%PDF-1.4\n8388607 0 obj\n<</Type/Page>>\nendobj\n");
    let refused = Document::open(file, Box::new(TestFilters)).err().unwrap();
    assert_eq!(refused.problem, Problem::Damaged);
}

#[test]
fn a_shared_cross_reference_stream_is_read_once_per_file() {
    let (entries, sections) = (200_000, 2000);
    let mut pdf = TestPdf::with_pages(1);
    let stream = pdf.position();
    pdf.stream_with(
        9,
        &format!(
            "/Type /XRef /Size {} /Index [10 {entries}] /W [1 3 1] /Filter /FlateDecode",
            entries + 10
        ),
        &deflate(&vec![0; entries * 5]),
        None,
        true,
    );
    let first = pdf.finish();
    let text: String = first.iter().map(|&b| b as char).collect();
    let mut previous = text.rfind("xref\n0").unwrap();
    let end = first.len();
    let mut sections_text = String::new();
    for _ in 0..sections {
        let at = end + sections_text.len();
        sections_text.push_str(&format!(
            "xref\n0 1\n0000000000 65535 f \ntrailer\n<</Size {} /Root 1 0 R /Prev {previous} /XRefStm {stream}>>\n",
            entries + 10
        ));
        sections_text.push_str(&format!("startxref\n{at}\n%%EOF\n"));
        previous = at;
    }
    let started = Instant::now();
    let mut document = open([first, latin(&sections_text)].concat());
    assert_eq!(document.pages().len(), 1);
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn object_streams_that_live_in_each_other_are_cut_off() {
    // Object k is in object stream k + 1, which is in k + 2, and so on: the chain is as long as the file likes.
    let links = 1600u64;
    let mut rows = vec![row(0, 0, 255)];
    for k in 1..links {
        rows.push(row(2, k + 1, 0));
    }
    rows.push(row(0, 0, 0));
    let mut pdf = TestPdf::with_header("%PDF-1.5\n");
    let xref = pdf.position();
    rows.push(row(1, xref, 0));
    pdf.stream_with(
        links as u32 + 1,
        &format!("/Type /XRef /Size {} /W [1 3 1] /Root 1 0 R", links + 2),
        &rows.concat(),
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat();
    assert!(Document::open(file, Box::new(TestFilters)).is_err());
}

#[test]
fn shared_parameter_arrays_do_not_multiply_when_flattened() {
    let width = 400;
    let refs = |next: u32| vec![format!("{next} 0 R"); width].join(" ");
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream(9, "/DecodeParms 10 0 R", "q Q");
    pdf.obj(10, &format!("[{}]", refs(11)))
        .obj(11, &format!("[{}]", refs(12)))
        .obj(12, &format!("[{}]", refs(13)))
        .obj(13, "null");
    let mut document = open(pdf.finish());
    let Object::Stream(stream) = (*document.get(9).unwrap()).clone() else {
        panic!("not a stream")
    };
    let started = Instant::now();
    assert_eq!(document.stream_data(&stream).unwrap(), b"q Q");
    must_finish_within(Duration::from_secs(5), started);
}

#[test]
fn a_cross_reference_index_that_wraps_a_long_is_damaged() {
    let mut pdf = TestPdf::with_pages(1);
    let offsets: Vec<u64> = (1..=4).map(|n| pdf.offset_of(n)).collect();
    let xref = pdf.position();
    let mut rows = vec![row(0, 0, 255)];
    rows.extend(offsets.iter().map(|&o| row(1, o, 0)));
    rows.push(row(1, xref, 0));
    rows.push(row(1, 20, 0));
    pdf.stream_with(
        5,
        "/Type /XRef /Size 6 /W [1 3 1] /Index [0 6 9223372036854775807 1] /Root 1 0 R",
        &rows.concat(),
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat();
    let document = open(file);
    assert!(document.xref_rebuilt());
    assert!(document.object_numbers().iter().all(|n| *n > 0));
}

/// An update whose cross-reference stream lists object 5 with the given type bytes.
fn updated(entry: [u8; 4]) -> Vec<u8> {
    let file = TestPdf::simple(2);
    let text: String = file.iter().map(|&b| b as char).collect();
    let after = &text[text.rfind("startxref").unwrap() + "startxref".len()..];
    let previous: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let at = file.len();
    let data = [entry.to_vec(), vec![1, (at >> 8) as u8, at as u8, 0]].concat();
    let head = format!(
        "20 0 obj\n<</Type /XRef /Size 21 /W [1 2 1] /Index [5 1 20 1] /Root 1 0 R /Prev {previous} /Length {}>>\nstream\n",
        data.len()
    );
    let tail = format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n");
    [file, latin(&head), data, latin(&tail)].concat()
}

#[test]
fn an_entry_of_an_unknown_type_reads_as_null_and_hides_the_older_one() {
    let mut plain = open(TestPdf::simple(2));
    assert!(plain.get(5).is_some());
    let mut document = open(updated([3, 0, 0, 0]));
    assert!(document.get(5).is_none());
}

#[test]
fn an_in_use_entry_at_offset_zero_reads_as_null() {
    let mut document = open(updated([1, 0, 0, 0]));
    assert!(document.get(5).is_none());
}

#[test]
fn an_info_taken_out_stays_out_after_a_reopen() {
    for kind in ["table", "stream", "hybrid"] {
        let mut document = open(make_save_fixture(kind));
        assert!(document.info().is_some());
        document.trailer_mut().remove("Info");
        let mut reopened = open(document.save_incremental().unwrap());
        assert!(reopened.info().is_none(), "{kind}");
        let root = match reopened.trailer().get("Root") {
            Some(Object::Ref(r)) => r.number,
            _ => panic!("no root"),
        };
        let Some(Object::Dict(catalog)) = reopened.get_mut(root) else {
            panic!("no catalog")
        };
        catalog.insert("Touched", Object::int(1));
        let mut again = open(reopened.save_incremental().unwrap());
        assert!(again.info().is_none(), "{kind}");
    }
}

#[test]
fn a_size_beyond_the_number_limit_is_not_taken_at_its_word() {
    let file = TestPdf::replace(
        &make_save_fixture("stream"),
        "/Size 9 ",
        "/Size 9999999 /Index [0 9] ",
    );
    let mut document = open(file);
    assert!(!document.xref_rebuilt());
    assert!(document.next_object_number() <= 10);
    let info = match document.trailer().get("Info") {
        Some(Object::Ref(r)) => r.number,
        _ => panic!("no info"),
    };
    let Some(Object::Dict(info)) = document.get_mut(info) else {
        panic!("no info")
    };
    info.insert(
        "Author",
        Object::String(PdfString {
            bytes: b"Me".to_vec(),
            hex: false,
        }),
    );
    let saved = document.save_incremental().unwrap();
    assert!(!open(saved).xref_rebuilt());
}
