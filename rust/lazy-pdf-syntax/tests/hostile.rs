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

#[test]
fn many_listed_objects_that_run_to_the_end_of_the_data_are_not_each_parsed_to_it() {
    let count = 20_000;
    let refs: String = (10..10 + count).map(|n| format!("{n} 0 R ")).collect();
    let mut pdf = TestPdf::new();
    pdf.obj(1, &format!("<</Type /Catalog /Pages 2 0 R /X [{refs}]>>"))
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>");
    for n in 10..10 + count {
        pdf.open_obj(n, "(");
    }
    let started = Instant::now();
    let mut document = open(pdf.finish());
    let saved = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
        use_object_streams: false,
        document_id: None,
    });
    assert_eq!(
        open(saved.bytes).pages().len(),
        1,
        "{:?}",
        document.repairs().iter().take(3).collect::<Vec<_>>()
    );
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn many_members_that_run_to_the_end_of_their_object_stream_are_not_each_parsed_to_it() {
    let count = 20_000;
    let header: String = (0..count)
        .map(|i| format!("{} {} ", 10 + i, i * 100))
        .collect();
    let body = "(".repeat(count * 100 + 1000);
    let refs: String = (10..10 + count).map(|n| format!("{n} 0 R ")).collect();
    let mut pdf = TestPdf::with_header("%PDF-1.5\n");
    pdf.obj(1, &format!("<</Type /Catalog /Pages 2 0 R /X [{refs}]>>"))
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>")
        .stream(
            5,
            &format!("/Type /ObjStm /N {count} /First {}", header.len()),
            &format!("{header}{body}"),
        );
    let started = Instant::now();
    let mut document = open(pdf.finish_without_xref());
    let saved = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
        use_object_streams: false,
        document_id: None,
    });
    assert_eq!(open(saved.bytes).pages().len(), 1);
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn contents_named_again_and_again_are_not_read_again_and_again_when_they_decode_to_nothing() {
    // A zlib stream of empty stored blocks: 5 bytes each, nothing out.
    let mut raw = vec![0x78, 0x01];
    for _ in 0..400_000 {
        raw.extend_from_slice(&[0, 0, 0, 0xFF, 0xFF]);
    }
    raw.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF, 0, 0, 0, 1]);
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(20, "/Filter /FlateDecode", &raw, None, true);
    pdf.obj(21, &format!("[{}]", "20 0 R ".repeat(5000)));
    let file = TestPdf::replace(&pdf.finish(), "/Contents 4 0 R", "/Contents 21 0 R");
    let mut document = open(file);
    let pages = document.pages();
    let started = Instant::now();
    assert!(pages[0].contents(&mut document).len() < 5000);
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn members_after_a_skipped_header_pair_keep_their_place_in_the_object_stream() {
    let bodies = [
        "<</Type /Catalog /Pages 2 0 R>>",
        "<</Type /Pages /Kids [] /Count 0>>",
    ];
    let header = format!("99999999 0 1 0 2 {} ", bodies[0].len() + 1);
    let mut pdf = TestPdf::with_header("%PDF-1.5\n");
    pdf.stream(4, "", "q Q");
    pdf.stream_with(
        5,
        &format!("/Type /ObjStm /N 3 /First {}", header.len()),
        &latin(&format!("{header}{}\n{}\n", bodies[0], bodies[1])),
        None,
        true,
    );
    let file = [pdf.finish_without_xref(), latin("startxref\n12\n%%EOF\n")].concat();
    let mut document = open(file.clone());
    assert_eq!(document.entry(2).unwrap().stream_number, 5);
    *document.get_mut(4).unwrap() = Object::int(7);
    let saved = document.save_incremental().unwrap();
    let update = &saved[file.len()..];
    let from = update
        .windows(7)
        .position(|w| {
            w == b"stream
"
        })
        .unwrap()
        + 7;
    let (rows, _) = update[from..].as_chunks::<5>();
    let indexes: Vec<u16> = rows
        .iter()
        .filter(|row| row[..3] == [2, 0, 5])
        .map(|row| u16::from_be_bytes([row[3], row[4]]))
        .collect();
    assert_eq!(indexes, [1, 2]);
    assert!(!document.object_numbers().contains(&0));
    assert!(document.entry(0).is_none());
}

#[test]
fn a_rebuild_never_lists_object_zero() {
    let header = "0 0 1 0 2 32 ";
    let body = "<</Type /Catalog /Pages 2 0 R>>
<</Type /Pages /Kids [] /Count 0>>
";
    let mut pdf = TestPdf::with_header(
        "%PDF-1.5
",
    );
    pdf.stream_with(
        5,
        &format!("/Type /ObjStm /N 3 /First {}", header.len()),
        &latin(&format!("{header}{body}")),
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(
            "trailer
<</Root 1 0 R>>
",
        ),
    ]
    .concat();
    let document = open(file);
    assert!(!document.object_numbers().contains(&0));
    assert!(document.entry(0).is_none());
}

#[test]
fn a_catalog_that_replaces_a_plain_object_in_an_object_stream_is_found() {
    let header = "7 0 ";
    let mut pdf = TestPdf::with_header(
        "%PDF-1.5
",
    );
    pdf.obj(7, "<</Type /Other>>")
        .obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    pdf.stream_with(
        5,
        &format!("/Type /ObjStm /N 1 /First {}", header.len()),
        &latin(&format!("{header}<</Type /Catalog /Pages 2 0 R>>")),
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(
            "trailer
<</Root 99 0 R>>
",
        ),
    ]
    .concat();
    let document = open(file);
    let root = document.trailer().get("Root").cloned();
    let Some(Object::Ref(root)) = root else {
        panic!("no root");
    };
    assert_eq!(root.number, 7);
}

#[test]
fn jbig2_globals_are_not_inflated_again_for_each_image() {
    let bomb = deflate(&vec![0u8; 32 << 20]);
    let images = 30u32;
    let mut pdf = TestPdf::with_header(
        "%PDF-1.5
",
    );
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    pdf.stream_with(3, "/Filter /FlateDecode", &bomb, None, true);
    for k in 0..images {
        pdf.stream_with(
            10 + k,
            "/Width 8 /Height 8 /Filter /JBIG2Decode /DecodeParms <</JBIG2Globals 3 0 R>>",
            b"x",
            None,
            true,
        );
    }
    let file = [
        pdf.finish_without_xref(),
        latin(
            "trailer
<</Root 1 0 R>>
",
        ),
    ]
    .concat();
    let mut document = open(file);
    let started = Instant::now();
    for k in 0..images {
        let image = document.get(10 + k).unwrap();
        let Object::Stream(stream) = &*image else {
            panic!("not a stream");
        };
        let _ = document.stream_data(stream);
    }
    must_finish_within(Duration::from_secs(12), started);
}

#[test]
fn an_unfiltered_jbig2_globals_stream_is_held_to_the_limit() {
    let mut pdf = TestPdf::with_header(
        "%PDF-1.5
",
    );
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    pdf.stream_with(3, "", &vec![0u8; 33 << 20], None, true);
    pdf.stream_with(
        10,
        "/Width 8 /Height 8 /Filter /JBIG2Decode /DecodeParms <</JBIG2Globals 3 0 R>>",
        b"x",
        None,
        true,
    );
    let file = [
        pdf.finish_without_xref(),
        latin(
            "trailer
<</Root 1 0 R>>
",
        ),
    ]
    .concat();
    let mut document = open(file);
    let image = document.get(10).unwrap();
    let Object::Stream(stream) = &*image else {
        panic!("not a stream");
    };
    let error = document.stream_data(stream).unwrap_err();
    assert!(error.detail.contains("too large"), "{}", error.detail);
}

#[test]
fn the_deepest_allowed_nesting_and_page_tree_fit_a_1_mb_stack() {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let nested = format!("{}{}", "[".repeat(199), "]".repeat(199));
            let mut pdf = TestPdf::with_header(
                "%PDF-1.5
",
            );
            pdf.obj(1, &format!("<</Type /Catalog /Pages 2 0 R /A {nested}>>"));
            let mut parent = 2;
            for k in 0..256u32 {
                let kid = 3 + k;
                pdf.obj(
                    parent,
                    &format!("<</Type /Pages /Kids [{kid} 0 R] /Count 1 /B {nested}>>"),
                );
                parent = kid;
            }
            pdf.obj(
                parent,
                &format!("<</Type /Page /Parent 2 0 R /C {nested}>>"),
            );
            let file = [
                pdf.finish_without_xref(),
                latin(
                    "trailer
<</Root 1 0 R>>
",
                ),
            ]
            .concat();
            let mut document = open(file);
            assert_eq!(document.pages().len(), 1);
            // Found through the tree, not by the scan that stands in for it.
            assert!(
                !document
                    .repairs()
                    .iter()
                    .any(|r| r.contains("found by scanning"))
            );
            let _ = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
                use_object_streams: false,
                document_id: None,
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn streams_that_share_their_bytes_do_not_make_a_rewrite_grow_with_the_square() {
    let count = 1500u32;
    let mut pdf = TestPdf::new();
    let refs: Vec<String> = (0..count).map(|k| format!("{} 0 R", 10 + k)).collect();
    pdf.obj(
        1,
        &format!("<</Type /Catalog /Pages 2 0 R /A [{}]>>", refs.join(" ")),
    );
    pdf.obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    // No /Length and no endstream until the last one: each runs to the end. A filter the rewrite leaves alone
    // keeps the copies from deflating away.
    let mut seed = 12345u32;
    for k in 0..count {
        let data: Vec<u8> = (0..100)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                b'a' + (seed >> 24) as u8 % 26
            })
            .collect();
        pdf.stream_with(
            10 + k,
            "/Filter /DCTDecode",
            &data,
            Some(-1),
            k == count - 1,
        );
    }
    let file = pdf.finish();
    let mut document = open(file.clone());
    let saved = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
        use_object_streams: false,
        document_id: None,
    });
    assert!(saved.bytes.len() < 2 * file.len(), "{}", saved.bytes.len());
}

#[test]
fn direct_nodes_with_many_plain_keys_are_walked_in_bounded_time() {
    let junk: String = (0..1000).map(|i| format!("/J{i} 0 ")).collect();
    let node = format!("<</Kids 5 0 R {junk}>>");
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids 5 0 R>>")
        .obj(5, &format!("[{node} {node}]"));
    let started = Instant::now();
    let mut document = open(pdf.finish());
    assert!(document.pages().is_empty());
    must_finish_within(Duration::from_secs(20), started);
}

#[test]
fn pages_with_large_direct_resources_are_all_listed() {
    let resources: String = (0..300).map(|i| format!("/F{i} 7 0 R ")).collect();
    let count = 3000;
    let kids: String = (10..10 + count).map(|n| format!("{n} 0 R ")).collect();
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>").obj(
        2,
        &format!("<</Type /Pages /Kids [{kids}] /Count {count}>>"),
    );
    for n in 10..10 + count {
        pdf.obj(
            n,
            &format!("<</Type /Page /Parent 2 0 R /Resources <<{resources}>>>>"),
        );
    }
    let mut document = open(pdf.finish());
    assert_eq!(document.pages().len(), count as usize);
    assert!(document.repairs().is_empty());
}

#[test]
fn contents_cut_short_by_the_size_limit_are_noted() {
    let mut pdf = TestPdf::with_pages(1);
    let mut data = deflate(b"q");
    data.resize(1 << 20, 0);
    pdf.stream_with(20, "/Filter /FlateDecode", &data, None, true);
    pdf.obj(21, &format!("[{}]", "20 0 R ".repeat(600)));
    let file = TestPdf::replace(&pdf.finish(), "/Contents 4 0 R", "/Contents 21 0 R");
    let mut document = open(file);
    let pages = document.pages();
    pages[0].contents(&mut document);
    assert!(
        document
            .repairs()
            .iter()
            .any(|r| r.contains("past the size limit")),
        "{:?}",
        document.repairs()
    );
}

fn wrongly_listed_neighbours(read_first: u32) {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
        .obj(3, "<</Type /Page /Parent 2 0 R>>");
    let a_at = pdf.position();
    pdf.obj(10, &format!("<</S ({})>>", "a".repeat(300)));
    let b_at = pdf.position();
    pdf.obj(11, "<</B 1>>");
    for n in 100..3100 {
        pdf.open_obj(n, "(");
    }
    let file = pdf.finish();
    let file = TestPdf::replace(
        &file,
        &format!("{b_at:010} 00000 n"),
        &format!("{:010} 00000 n", a_at + 30),
    );
    let mut document = open(file);
    for n in 100..3100 {
        document.get(n);
    }
    let other = if read_first == 10 { 11 } else { 10 };
    assert!(document.get(read_first).is_some());
    let a = document.get(other).expect("both objects read");
    let a = if other == 10 {
        a
    } else {
        document.get(10).unwrap()
    };
    assert!(a.as_dict().is_some_and(|d| d.get("S").is_some()));
}

#[test]
fn an_object_listed_at_a_wrong_offset_does_not_clip_its_neighbour_once_the_parse_budget_is_spent() {
    wrongly_listed_neighbours(10);
    wrongly_listed_neighbours(11);
}

#[test]
fn streams_that_inflate_twice_are_not_decoded_for_a_rewrite() {
    let twice = deflate(&deflate(&vec![0u8; 100 << 20]));
    let mut pdf = TestPdf::with_pages(1);
    for n in 20..30 {
        pdf.stream_with(n, "/Filter [/FlateDecode /FlateDecode]", &twice, None, true);
    }
    let started = Instant::now();
    let file = TestPdf::replace(
        &pdf.finish(),
        "/Contents 4 0 R",
        "/Contents [20 0 R 21 0 R 22 0 R 23 0 R 24 0 R 25 0 R 26 0 R 27 0 R 28 0 R 29 0 R]",
    );
    let mut document = open(file);
    let saved = document.save_rewrite(&lazy_pdf_syntax::SaveOptions {
        use_object_streams: false,
        document_id: None,
    });
    must_finish_within(Duration::from_secs(2), started);
    let found = saved
        .bytes
        .windows(twice.len())
        .filter(|w| *w == twice.as_slice())
        .count();
    assert_eq!(found, 10);
}

#[test]
fn a_rebuild_does_not_decode_every_object_stream_in_full() {
    let mut data = b"900 0 ".to_vec();
    data.resize(24 << 20, b' ');
    let packed = deflate(&data);
    let mut pdf = TestPdf::with_pages(1);
    for n in 20..70 {
        pdf.stream_with(
            n,
            "/Type /ObjStm /N 1 /First 6 /Filter /FlateDecode",
            &packed,
            None,
            true,
        );
    }
    let started = Instant::now();
    let mut document = open(pdf.finish_without_xref());
    assert!(document.xref_rebuilt());
    assert_eq!(document.pages().len(), 1);
    assert!(
        document
            .repairs()
            .iter()
            .any(|r| r.starts_with("Object stream") && r.contains("not")),
        "{:?}",
        document.repairs().len()
    );
    must_finish_within(Duration::from_secs(20), started);
}

// Objects listed a few bytes off, read after the parse budget was spent by unclosed strings.
fn misplaced(count: u32, shift: i64) -> (usize, Duration) {
    let mut out: Vec<u8> = b"%PDF-1.4
"
    .to_vec();
    let mut at: Vec<(u32, usize)> = Vec::new();
    let mut put = |out: &mut Vec<u8>, num: u32, body: &str| {
        at.push((num, out.len()));
        out.extend_from_slice(
            format!(
                "{num} 0 obj
{body}
endobj
"
            )
            .as_bytes(),
        );
    };
    put(&mut out, 1, "<</Type /Catalog /Pages 2 0 R>>");
    put(&mut out, 2, "<</Type /Pages /Kids [3 0 R] /Count 1>>");
    put(&mut out, 3, "<</Type /Page /Parent 2 0 R>>");
    for k in 0..80 {
        put(&mut out, 100 + k, "(");
    }
    for k in 0..count {
        put(&mut out, 100_000 + k, "<</V 1>>");
    }
    let xref = out.len();
    let max = 100_000 + count;
    let mut table = vec![None; max as usize + 1];
    for (num, pos) in &at {
        let pos = if *num >= 100_000 {
            *pos as i64 + shift
        } else {
            *pos as i64
        };
        table[*num as usize] = Some(pos);
    }
    out.extend_from_slice(
        format!(
            "xref
0 {}
",
            max + 1
        )
        .as_bytes(),
    );
    for (i, entry) in table.iter().enumerate() {
        match entry {
            Some(pos) => out.extend_from_slice(
                format!(
                    "{pos:010} 00000 n
"
                )
                .as_bytes(),
            ),
            None if i == 0 => out.extend_from_slice(
                b"0000000000 65535 f
",
            ),
            None => out.extend_from_slice(
                b"0000000000 00000 f
",
            ),
        }
    }
    out.extend_from_slice(
        format!(
            "trailer
<</Size {} /Root 1 0 R>>
startxref
{xref}
%%EOF
",
            max + 1
        )
        .as_bytes(),
    );
    let mut document = open(out);
    for k in 0..80 {
        document.get(100 + k);
    }
    let started = Instant::now();
    let found = (0..count)
        .filter(|k| {
            document
                .get(100_000 + k)
                .is_some_and(|o| o.as_dict().is_some())
        })
        .count();
    (found, started.elapsed())
}

#[test]
fn objects_listed_early_are_found_in_linear_time_once_the_parse_budget_is_spent() {
    let (found, took) = misplaced(40_000, -2);
    assert_eq!(found, 40_000);
    must_finish_within(Duration::from_secs(20), Instant::now() - took);
}

#[test]
fn objects_listed_late_are_found_once_the_parse_budget_is_spent() {
    let (found, _) = misplaced(10_000, 1);
    assert_eq!(found, 10_000);
}
