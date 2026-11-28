mod common;

use common::pdf::TestPdf;
use lazy_pdf_fonts::{PdfFont, StandardFilters, TrueTypeFont};
use lazy_pdf_syntax::{Document, Object};

fn base() -> TestPdf {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        "<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<>>>>",
    );
    pdf.stream(4, "", b"");
    pdf
}

fn document(objects: &[&str]) -> Document {
    let mut pdf = base();
    for (i, body) in objects.iter().enumerate() {
        pdf.obj(5 + i as u32, body);
    }
    Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap()
}

fn font(doc: &mut Document, number: u32) -> PdfFont {
    let object = doc.get(number).unwrap();
    let Object::Dict(dict) = &*object else {
        panic!("object {number} is no dictionary");
    };
    PdfFont::new(doc, dict)
}

fn widths(chars: &[lazy_pdf_fonts::FontChar]) -> Vec<f64> {
    chars.iter().map(|c| c.width).collect()
}

#[test]
fn standard_font_widths_and_text_without_a_widths_array() {
    let mut d = document(&["<</Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold>>"]);
    let f = font(&mut d, 5);
    let chars = f.decode(b"A b");
    assert_eq!(widths(&chars), [722.0, 278.0, 611.0]);
    assert_eq!(f.to_text(b"A b"), "A b");
    assert!(f.is_bold);
    assert!(chars[1].is_space);
    assert!(!f.is_embedded);
}

#[test]
fn windows_names_map_to_the_standard_fonts() {
    let mut d = document(&[
        "<</Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TimesNewRomanPS-BoldMT /Encoding /WinAnsiEncoding>>",
    ]);
    let f = font(&mut d, 5);
    assert_eq!(f.name, "TimesNewRomanPS-BoldMT");
    assert_eq!(f.decode(b"A")[0].width, 722.0);
}

#[test]
fn widths_array_missing_width_and_out_of_range_codes() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 65 /LastChar 66 /Widths [500 600] /FontDescriptor 6 0 R>>",
        "<</Type /FontDescriptor /FontName /Foo /Flags 32 /MissingWidth 300 /Ascent 900 /Descent 250 /CapHeight 700>>",
    ]);
    let f = font(&mut d, 5);
    assert_eq!(widths(&f.decode(b"ABCa")), [500.0, 600.0, 300.0, 300.0]);
    assert_eq!(f.ascent, 900.0);
    assert_eq!(f.descent, -250.0);
}

#[test]
fn an_unknown_font_without_widths_borrows_helvetica_metrics() {
    let mut d = document(&["<</Type /Font /Subtype /Type1 /BaseFont /Foo>>"]);
    assert_eq!(font(&mut d, 5).decode(b"A")[0].width, 667.0);
}

#[test]
fn differences_rename_codes() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding <</Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [65 /Euro /ccedilla 200 /uni011F]>>>>",
    ]);
    let f = font(&mut d, 5);
    assert_eq!(f.to_text(b"AB"), "\u{20AC}\u{E7}");
    assert_eq!(f.to_text(&[200]), "\u{11F}");
    assert_eq!(f.to_text(b"C"), "C");
}

#[test]
fn win_ansi_and_mac_roman_differ_in_the_upper_half() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding>>",
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /MacRomanEncoding>>",
    ]);
    assert_eq!(font(&mut d, 5).to_text(&[0xE9]), "\u{E9}");
    assert_eq!(font(&mut d, 6).to_text(&[0x8E]), "\u{E9}");
}

#[test]
fn to_unicode_wins_over_the_encoding() {
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <005A> <42> <00660069> endbfchar endcmap";
    let mut pdf = base();
    pdf.obj(
        5,
        "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R>>",
    );
    pdf.stream(6, "", cmap);
    let mut d = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let f = font(&mut d, 5);
    assert!(f.has_to_unicode());
    assert_eq!(f.to_text(b"AB"), "Zfi");
    assert_eq!(f.to_text(b"C"), "C");
}

#[test]
fn type0_with_identity_h_reads_widths_from_w() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [6 0 R]>>",
        "<</Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo <</Registry (Adobe) /Ordering (Identity) /Supplement 0>> /DW 800 /W [1 [500 600] 10 20 700]>>",
    ]);
    let f = font(&mut d, 5);
    let chars = f.decode(&[0, 1, 0, 2, 0, 15, 0, 50]);
    assert_eq!(widths(&chars), [500.0, 600.0, 700.0, 800.0]);
    assert_eq!(
        chars.iter().map(|c| c.cid).collect::<Vec<_>>(),
        [1, 2, 15, 50]
    );
    assert!(chars.iter().all(|c| c.length == 2));
    assert!(f.is_composite());
    assert!(!f.is_vertical);
}

#[test]
fn vertical_metrics_from_dw2_and_w2() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-V /DescendantFonts [6 0 R]>>",
        "<</Type /Font /Subtype /CIDFontType0 /BaseFont /X /CIDSystemInfo <</Registry (Adobe) /Ordering (Identity) /Supplement 0>> /DW 1000 /DW2 [900 -950] /W2 [5 [-500 100 800] 7 8 -600 200 700]>>",
    ]);
    let f = font(&mut d, 5);
    assert!(f.is_vertical);
    let c = f.decode(&[0, 1, 0, 5, 0, 8]);
    let v = |i: usize| [c[i].vertical_advance, c[i].origin_x, c[i].origin_y];
    assert_eq!(v(0), [-950.0, 500.0, 900.0]);
    assert_eq!(v(1), [-500.0, 100.0, 800.0]);
    assert_eq!(v(2), [-600.0, 200.0, 700.0]);
}

#[test]
fn mixed_length_cmaps_cut_the_string_by_codespace() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /90ms-RKSJ-H /DescendantFonts [6 0 R]>>",
        "<</Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo <</Registry (Adobe) /Ordering (Japan1) /Supplement 2>> /DW 1000>>",
    ]);
    let chars = font(&mut d, 5).decode(&[0x41, 0x82, 0xA0, 0x42]);
    assert_eq!(
        chars.iter().map(|c| c.length).collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert_eq!(chars[0].unicode.as_deref(), Some("A"));
    assert_eq!(chars[1].unicode.as_deref(), Some("\u{3042}"));
}

#[test]
fn an_adobe_collection_gives_unicode_without_a_to_unicode_map() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [6 0 R]>>",
        "<</Type /Font /Subtype /CIDFontType0 /BaseFont /X /CIDSystemInfo <</Registry (Adobe) /Ordering (Japan1) /Supplement 2>>>>",
    ]);
    assert_eq!(font(&mut d, 5).to_text(&[0, 34]), "A");
}

#[test]
fn an_embedded_cmap_stream_is_read() {
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Custom def /WMode 0 def 1 begincodespacerange <00> <FF> endcodespacerange 1 begincidrange <20> <7E> 100 endcidrange endcmap";
    let mut pdf = base();
    pdf.obj(
        5,
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding 7 0 R /DescendantFonts [6 0 R]>>",
    );
    pdf.obj(
        6,
        "<</Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 500 /W [100 [250]]>>",
    );
    pdf.stream(7, "/Type /CMap /CMapName /Custom", cmap);
    let mut d = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let c = font(&mut d, 5).decode(b"  ");
    assert_eq!(c.iter().map(|c| c.length).collect::<Vec<_>>(), [1, 1]);
    assert_eq!(c[0].cid, 100);
    assert_eq!(c[0].width, 250.0);
}

#[test]
fn broken_fonts_never_fail() {
    let mut d = document(&[
        "<<>>",
        "<</Type /Font /Subtype /Type0>>",
        "<</Type /Font /Subtype /Type0 /DescendantFonts [<<>>] /Encoding /NoSuchCMap>>",
        "<</Type /Font /Subtype /TrueType /Widths [/a (b) 3 [4]] /FirstChar -5 /Encoding 3>>",
        "<</Type /Font /Subtype /Type3 /FontMatrix [0 0 0 0 0 0] /Widths 5>>",
        "<</Type /Font /Subtype /Type1 /Encoding <</Differences [/a 99999 /b -4 (x)]>> /ToUnicode 5 0 R>>",
        "<</Type /Font /Subtype /Type0 /DescendantFonts [<</W [1 2 3 [4] 70000 80000 5 -3 /x]>>]>>",
    ]);
    for n in 5..=11 {
        let chars = font(&mut d, n).decode(&[0, 1, 2, 65, 200, 255, 0, 3]);
        assert!(!chars.is_empty());
        for c in &chars {
            assert!(c.length >= 1 && c.width.is_finite(), "font {n}");
        }
    }
}

#[test]
fn read_always_makes_progress() {
    let mut d = document(&[
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /UniJIS-UTF16-H /DescendantFonts [<<>>]>>",
    ]);
    let f = font(&mut d, 5);
    let mut bytes: &[u8] = &[0xD8, 0x00, 0xFF];
    let mut total = 0;
    while !bytes.is_empty() {
        let (used, _) = f.read(bytes);
        assert!(used > 0);
        bytes = &bytes[used..];
        total += 1;
    }
    assert!(total >= 1);
    assert_eq!(f.read(&[]).0, 0);
}

#[test]
fn the_subtype_is_guessed_when_missing() {
    let mut d = document(&["<</BaseFont /Helvetica>>", "<</DescendantFonts [<<>>]>>"]);
    assert_eq!(font(&mut d, 5).subtype, "Type1");
    assert_eq!(font(&mut d, 6).subtype, "Type0");
}

#[test]
fn a_whole_true_type_file_gives_unicode_from_its_cmap() {
    let bytes = common::golden_bytes("programs/synthetic.ttf");
    let tt = TrueTypeFont::parse(&bytes, 0).unwrap();
    let gid = tt.glyph_for_code_point(0x11F);
    let mut pdf = base();
    pdf.obj(5, "<</Type /Font /Subtype /Type0 /BaseFont /Arial /Encoding /Identity-H /DescendantFonts [6 0 R]>>");
    pdf.obj(6, "<</Type /Font /Subtype /CIDFontType2 /BaseFont /Arial /CIDSystemInfo <</Registry (Adobe) /Ordering (Identity) /Supplement 0>> /FontDescriptor 7 0 R /CIDToGIDMap /Identity>>");
    pdf.obj(
        7,
        "<</Type /FontDescriptor /FontName /Arial /Flags 32 /FontFile2 8 0 R>>",
    );
    pdf.stream(8, "", &bytes);
    let mut d = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let f = font(&mut d, 5);
    assert!(f.is_embedded);
    assert_eq!(f.to_text(&[(gid >> 8) as u8, gid as u8]), "\u{11F}");
}

#[test]
fn an_embedded_simple_true_type_without_encoding_reads_its_cmap_back() {
    let bytes = common::golden_bytes("programs/synthetic.ttf");
    let mut pdf = base();
    pdf.obj(
        5,
        "<</Type /Font /Subtype /TrueType /BaseFont /Arial /FontDescriptor 6 0 R>>",
    );
    pdf.obj(
        6,
        "<</Type /FontDescriptor /FontName /Arial /Flags 4 /FontFile2 7 0 R>>",
    );
    pdf.stream(7, "", &bytes);
    let mut d = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let f = font(&mut d, 5);
    assert_eq!(f.to_text(b"AB"), "AB");
    assert!(f.decode(b"A")[0].width > 0.0);
}
