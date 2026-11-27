mod common;

use std::time::{Duration, Instant};

use common::latin;
use common::open;
use common::pdf::{TestPdf, deflate, object_stream_file_padded};
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

#[test]
fn a_page_behind_a_reference_chain_keeps_its_own_entries() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "4 0 R")
        .obj(
            4,
            "<</Type /Page /MediaBox [0 0 100 200] /UserUnit 2 /TrimBox [1 1 50 50] /Contents 5 0 R>>",
        )
        .stream(5, "", "BT ET");
    let mut document = open(pdf.finish());
    let pages = document.pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].contents(&mut document), b"BT ET");
    assert_eq!(pages[0].user_unit(&mut document), 2.0);
    assert_eq!(pages[0].trim_box(&mut document).right, 50.0);
}

#[test]
fn an_object_stream_between_32_and_64_mb_is_read() {
    let (pdf, xref) = object_stream_file_padded(40 << 20);
    let file = [
        pdf.finish_without_xref(),
        latin(&format!(
            "startxref
{xref}
%%EOF
"
        )),
    ]
    .concat();
    let mut document = open(file);
    assert_eq!(document.pages().len(), 1);
}

#[test]
fn a_kids_array_that_holds_itself_is_walked_in_bounded_time() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids 5 0 R>>")
        .obj(5, "[<</Kids 5 0 R>> <</Kids 5 0 R>>]");
    let started = Instant::now();
    let mut document = open(pdf.finish());
    assert!(document.pages().is_empty());
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn an_object_stream_that_fails_is_not_inflated_again_for_each_member() {
    let mut pdf = TestPdf::with_header(
        "%PDF-1.5
",
    );
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    pdf.stream_with(
        5,
        "/Type /ObjStm /N 1 /First 5 /Filter /FlateDecode",
        &deflate(&vec![b' '; 70 << 20]),
        None,
        true,
    );
    let xref = pdf.position();
    let row = |kind: u8, field: u64, third: u8| {
        [
            kind,
            (field >> 16) as u8,
            (field >> 8) as u8,
            field as u8,
            third,
        ]
    };
    let mut rows = vec![row(0, 0, 255)];
    rows.push(row(1, pdf.offset_of(1), 0));
    rows.push(row(1, pdf.offset_of(2), 0));
    rows.extend([row(0, 0, 0), row(0, 0, 0)]);
    rows.push(row(1, pdf.offset_of(5), 0));
    rows.push(row(1, xref, 0));
    rows.extend((0..4).map(|i| row(2, 5, i)));
    let data: Vec<u8> = rows.concat();
    pdf.stream_with(
        6,
        "/Type /XRef /Size 11 /W [1 3 1] /Root 1 0 R /Filter /FlateDecode",
        &deflate(&data),
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(&format!(
            "startxref
{xref}
%%EOF
"
        )),
    ]
    .concat();
    let mut document = open(file);
    for number in 7..=10 {
        document.get(number);
    }
    let failed = document
        .repairs()
        .iter()
        .filter(|r| r.contains("Object stream 5 could not be read"))
        .count();
    assert_eq!(failed, 1);
}

#[test]
fn a_content_stream_that_keeps_failing_is_not_tried_a_million_times() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(20, "/Filter /FlateDecode", b"not flate data", None, true);
    pdf.obj(21, &format!("[{}]", "20 0 R ".repeat(5000)));
    let file = TestPdf::replace(&pdf.finish(), "/Contents 4 0 R", "/Contents 21 0 R");
    let mut document = open(file);
    let pages = document.pages();
    let started = Instant::now();
    pages[0].contents(&mut document);
    let failed = document
        .repairs()
        .iter()
        .filter(|r| r.contains("content stream could not be read"))
        .count();
    assert!(failed < 100, "{failed}");
    must_finish_within(Duration::from_secs(10), started);
}

#[test]
fn many_streams_with_a_wrong_length_do_not_each_scan_to_the_end() {
    let mut pdf = TestPdf::with_pages(1);
    for n in 30..20_030 {
        pdf.write(&format!(
            "{n} 0 obj
<</Length 999999>>
stream
X
endobj
"
        ));
    }
    pdf.write(&"x".repeat(8 << 20));
    pdf.write(
        "
endstream
",
    );
    let file = pdf.finish_without_xref();
    let started = Instant::now();
    let mut document = open(
        [
            file,
            latin(
                "startxref
0
%%EOF
",
            ),
        ]
        .concat(),
    );
    assert!(document.xref_rebuilt());
    for n in 30..20_030 {
        if let Some(object) = document.get(n)
            && let Object::Stream(stream) = &*object
        {
            document.raw_data(stream);
        }
    }
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn an_unfiltered_stream_longer_than_the_limit_is_refused_before_it_is_copied() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(20, "", &vec![b'a'; 1 << 20], None, true);
    let mut document = open(pdf.finish());
    let object = document.get(20).unwrap();
    let Object::Stream(stream) = &*object else {
        panic!("not a stream");
    };
    assert!(
        document
            .stream_data_limited(stream, 1 << 10, false)
            .is_err()
    );
    assert_eq!(
        document
            .stream_data_limited(stream, 2 << 20, false)
            .unwrap()
            .len(),
        1 << 20
    );
}

/// A cross-reference stream (object 20) for `with_pages(1)`, with an entry row `extra` after the real ones and
/// `padding` bytes beyond the entries.
fn xref_stream_file(extra: Option<[u8; 10]>, padding: usize) -> Vec<u8> {
    let pdf = TestPdf::with_pages(1);
    let xref = pdf.position();
    let mut rows = vec![0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0xff];
    for n in 1..=4u32 {
        let at = pdf.offset_of(n);
        rows.extend([
            1,
            0,
            0,
            0,
            0,
            0,
            (at >> 16) as u8,
            (at >> 8) as u8,
            at as u8,
            0,
        ]);
    }
    let mut index = String::from("0 5 20 1");
    rows.extend([
        1,
        0,
        0,
        0,
        0,
        0,
        (xref >> 16) as u8,
        (xref >> 8) as u8,
        xref as u8,
        0,
    ]);
    if let Some(row) = extra {
        rows.extend(row);
        index.push_str(" 30 1");
    }
    rows.resize(rows.len() + padding, 0);
    let mut file = TestPdf::with_pages(1);
    file.stream_with(
        20,
        &format!(
            "/Type /XRef /Size 21 /W [1 8 1] /Index [{index}] /Root 1 0 R /Filter /FlateDecode"
        ),
        &deflate(&rows),
        None,
        true,
    );
    [
        file.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat()
}

#[test]
fn a_cross_reference_stream_is_read_whole() {
    let document = open(xref_stream_file(None, 0));
    assert!(!document.xref_rebuilt());
}

#[test]
fn a_cross_reference_stream_longer_than_its_entries_is_refused() {
    let document = open(xref_stream_file(None, 1 << 20));
    assert!(document.xref_rebuilt());
}

#[test]
fn an_offset_with_the_high_bit_set_is_negative_as_in_the_reference() {
    let mut huge = [1u8, 0x80, 0, 0, 0, 0, 0, 0, 1, 0];
    let high_set = open(xref_stream_file(Some(huge), 0));
    huge[1] = 0x40;
    let high_clear = open(xref_stream_file(Some(huge), 0));
    assert!(!high_clear.xref_rebuilt() && !high_set.xref_rebuilt());
    assert_eq!(high_clear.next_object_number(), 31);
    assert_eq!(high_set.next_object_number(), 21);
}

fn read_raw(document: &mut lazy_pdf_syntax::Document, numbers: impl Iterator<Item = u32>) {
    for n in numbers {
        if let Some(object) = document.get(n)
            && let Object::Stream(stream) = &*object
        {
            // Over the limit, so the span is measured and not copied.
            assert!(document.stream_data_limited(stream, 16, false).is_err());
        }
    }
}

#[test]
fn streams_with_a_wrong_length_asked_for_from_the_end_do_not_each_scan_to_it() {
    let mut pdf = TestPdf::with_pages(1);
    for n in 30..5_030 {
        pdf.stream_with(n, "", b"X", Some(999_999), false);
    }
    pdf.write(&"x".repeat(8 << 20));
    pdf.write(
        "
endstream
",
    );
    let mut document = open(pdf.finish());
    assert!(!document.xref_rebuilt());
    let started = Instant::now();
    read_raw(&mut document, (30..5_030).rev());
    must_finish_within(Duration::from_secs(5), started);
}

#[test]
fn streams_that_end_at_a_far_endobj_do_not_each_scan_to_it() {
    let mut pdf = TestPdf::with_pages(1);
    for n in 30..5_030 {
        pdf.open_obj(
            n,
            "<</Length 999999>>
stream
X
",
        );
    }
    pdf.write(&"x".repeat(8 << 20));
    pdf.write(
        "
endobj
",
    );
    let mut document = open(pdf.finish());
    assert!(!document.xref_rebuilt());
    let started = Instant::now();
    read_raw(&mut document, 30..5_030);
    must_finish_within(Duration::from_secs(5), started);
}

#[test]
fn an_object_stream_that_failed_before_a_rebuild_is_read_again_from_its_new_definition() {
    let (pdf, xref) = object_stream_file_padded(0);
    let whole = [
        pdf.finish_without_xref(),
        latin(&format!("startxref\n{xref}\n%%EOF\n")),
    ]
    .concat();
    let start = pdf.offset_of(5) as usize;
    let end = xref as usize;
    let good = whole[start..end].to_vec();
    let mut broken = good.clone();
    let name = b"/FlateDecode";
    let at = broken.windows(name.len()).position(|w| w == name).unwrap();
    broken[at..at + name.len()].copy_from_slice(b"/FlateDecodx");
    let file = [whole[..start].to_vec(), broken, whole[end..].to_vec(), good].concat();
    let mut document = open(file);
    assert!(document.xref_rebuilt());
    assert_eq!(document.pages().len(), 1);
}

#[test]
fn direct_nodes_that_each_carry_a_large_resources_are_walked_in_bounded_time() {
    let resources: String = (0..1000).map(|i| format!("/F{i} 7 0 R ")).collect();
    let node = format!("<</Kids 5 0 R /Resources <<{resources}>>>>");
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids 5 0 R>>")
        .obj(5, &format!("[{node} {node}]"));
    let started = Instant::now();
    let mut document = open(pdf.finish());
    assert!(document.pages().is_empty());
    must_finish_within(Duration::from_secs(20), started);
}
