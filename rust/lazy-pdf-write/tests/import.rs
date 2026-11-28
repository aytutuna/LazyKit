use lazy_pdf_filters::flate::inflate;
use lazy_pdf_syntax::{DecodeError, Document, Filters, Object, Parm};
use lazy_pdf_write::Collector;
use lazy_pdf_write::import::{Copier, ImportError, geometry, import_page};

struct Inflate;

impl Filters for Inflate {
    fn decode(
        &self,
        filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        match filter {
            b"FlateDecode" | b"Fl" => {
                inflate(data, max_size as u64, strict).map_err(|e| DecodeError::Damaged(e.message))
            }
            other => Err(DecodeError::Unsupported(
                String::from_utf8_lossy(other).into_owned(),
            )),
        }
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        lazy_pdf_filters::encode::flate(data, 6)
    }
}

/// A file of these objects (numbers from 1, bodies as written between "obj" and "endobj"), with a correct table.
fn file(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let start = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

fn stream(entries: &str, data: &str) -> String {
    format!(
        "<< {entries} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// A collector holding the catalog and an empty page tree, so that what it writes opens.
fn new_out() -> Collector {
    let mut out = Collector::new();
    out.alloc();
    out.alloc();
    out.set(1, "<< /Type /Catalog /Pages 2 0 R >>");
    out.set(2, "<< /Type /Pages /Kids [] /Count 0 >>");
    out
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, Box::new(Inflate)).unwrap()
}

fn catalog_and_pages(kids: &str, count: usize) -> [String; 2] {
    [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>"),
    ]
}

/// rotated.pdf of the Print fixtures: a green page with a yellow band along its top edge, turned 90 degrees.
fn rotated() -> Vec<u8> {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let contents = stream(
        "",
        "0 0.6 0 rg 0 0 612 792 re f\n1 1 0 rg 0 692 612 100 re f",
    );
    file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate 90 /Contents 4 0 R >>",
        &contents,
    ])
}

fn annotated() -> Vec<u8> {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let appearance = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 200 200]",
        "1 0 0 rg 0 0 200 200 re f",
    );
    file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Square /Rect [100 400 300 600] /F 4 /AP << /N 5 0 R >> >>",
        &appearance,
    ])
}

fn content_of(form: &lazy_pdf_write::import::Form, out: Collector) -> (String, String) {
    let pdf = out.finish("1.4", "/Root 1 0 R");
    let mut document = open(pdf);
    let object = document.get(form.object as u32).unwrap();
    let Object::Stream(stream) = &*object else {
        panic!("the form is not a stream");
    };
    let mut dict = Vec::new();
    stream.dict.write_to(&mut dict);
    let data = document.stream_data(stream).unwrap();
    (
        String::from_utf8(dict).unwrap(),
        String::from_utf8(data).unwrap(),
    )
}

#[test]
fn a_page_becomes_a_form_with_its_content() {
    let mut document = open(rotated());
    let mut out = new_out();
    let mut copier = Copier::new();
    let form = import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    assert_eq!((form.width, form.height), (792.0, 612.0));
    let (dict, content) = content_of(&form, out);
    assert!(dict.contains("/Subtype /Form"), "{dict}");
    assert!(dict.contains("/BBox [0 0 612 792]"), "{dict}");
    assert!(content.contains("0 0.6 0 rg 0 0 612 792 re f"), "{content}");
    assert!(
        content.trim()
            == "0 0.6 0 rg 0 0 612 792 re f
1 1 0 rg 0 692 612 100 re f",
        "{content}"
    );
}

#[test]
fn the_form_space_is_the_page_as_it_is_shown() {
    let mut document = open(rotated());
    let pages = document.pages();
    let geo = geometry(&mut document, &pages[0]);
    assert_eq!(
        (geo.rotate, geo.shown_width, geo.shown_height),
        (1, 792.0, 612.0)
    );
    // The band is on the right edge once the page is turned 90 degrees clockwise.
    let (sx, sy) = geo.to_shown(306.0, 742.0);
    assert_eq!((sx, sy), (742.0, 306.0));
    let (x, y) = geo.from_shown(sx, sy);
    assert_eq!((x, y), (306.0, 742.0));
    for rotate in 0..4 {
        let turned = lazy_pdf_write::import::PageGeometry { rotate, ..geo };
        let (sx, sy) = turned.to_shown(100.0, 200.0);
        let (x, y) = turned.from_shown(sx, sy);
        assert!(
            (x - 100.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9,
            "{rotate}"
        );
    }
}

#[test]
fn the_form_matrix_puts_the_band_where_the_page_shows_it() {
    let mut document = open(rotated());
    let mut out = new_out();
    let mut copier = Copier::new();
    let form = import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    let (dict, _) = content_of(&form, out);
    let matrix = dict
        .split("/Matrix [")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let m: Vec<f64> = matrix.split(' ').map(|n| n.parse().unwrap()).collect();
    // The band's middle, page space to form space (y up, from the bottom-left of the shown page).
    let (x, y) = (306.0, 742.0);
    let (fx, fy) = (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    assert_eq!((fx, fy), (742.0, 306.0));
}

#[test]
fn a_part_of_the_page_is_a_smaller_box() {
    let mut document = open(rotated());
    let mut out = new_out();
    let mut copier = Copier::new();
    // The right 100 points of the shown page, which hold the band.
    let part = (692.0, 0.0, 100.0, 612.0);
    let form = import_page(&mut document, &mut copier, 0, Some(part), &mut out).unwrap();
    assert_eq!((form.width, form.height), (100.0, 612.0));
    let (dict, _) = content_of(&form, out);
    assert!(dict.contains("/BBox [0 692 612 792]"), "{dict}");
}

#[test]
fn an_annotation_that_prints_is_drawn_into_the_form() {
    let mut document = open(annotated());
    let mut out = new_out();
    let mut copier = Copier::new();
    let form = import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    let (dict, content) = content_of(&form, out);
    assert!(
        content.contains("q 1 0 0 1 100 400 cm /LpAnnot0 Do Q"),
        "{content}"
    );
    assert!(dict.contains("/LpAnnot0"), "{dict}");
}

#[test]
fn an_annotation_that_does_not_print_is_left_out() {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let appearance = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 10 10]",
        "0 g 0 0 10 10 re f",
    );
    let pdf = file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 0 /AP << /N 5 0 R >> >>",
        &appearance,
        "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 6 /AP << /N 5 0 R >> >>",
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    let form = import_page(&mut document, &mut Copier::new(), 0, None, &mut out).unwrap();
    let (_, content) = content_of(&form, out);
    assert!(!content.contains("LpAnnot"), "{content}");
}

#[test]
fn resources_are_copied_once_however_many_forms_use_them() {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    let contents = stream("", "BT /F1 12 Tf 10 10 Td (Hi) Tj ET");
    let pdf = file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        &contents,
        font,
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    let mut copier = Copier::new();
    let first = import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    let after_first = out.count();
    let second = import_page(
        &mut document,
        &mut copier,
        0,
        Some((0.0, 0.0, 50.0, 50.0)),
        &mut out,
    )
    .unwrap();
    // One more object for the second form, the font is not copied again.
    assert_eq!(out.count(), after_first + 1);
    assert_ne!(first.object, second.object);
    let pdf = out.finish("1.4", "/Root 1 0 R");
    let text = String::from_utf8_lossy(&pdf).into_owned();
    assert_eq!(text.matches("/BaseFont /Helvetica").count(), 1);
    assert_eq!(text.matches("/Subtype /Form").count(), 2);
}

#[test]
fn a_missing_page_and_an_empty_part_are_refused() {
    let mut document = open(rotated());
    let mut out = Collector::new();
    assert_eq!(
        import_page(&mut document, &mut Copier::new(), 3, None, &mut out).unwrap_err(),
        ImportError::NoPage
    );
    assert_eq!(
        import_page(
            &mut document,
            &mut Copier::new(),
            0,
            Some((0.0, 0.0, 0.0, 10.0)),
            &mut out
        )
        .unwrap_err(),
        ImportError::NoPage
    );
}

#[test]
fn a_reference_loop_in_the_resources_is_copied_once() {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let pdf = file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /ExtGState << /G 4 0 R >> >> >>",
        "<< /Next 5 0 R >>",
        "<< /Next 4 0 R >>",
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    import_page(&mut document, &mut Copier::new(), 0, None, &mut out).unwrap();
    assert_eq!(out.count(), 5);
}

/// A file of binary objects (numbers from 1) with a correct table.
fn file_bytes(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    }
    let start = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

/// A document of `pages` pages; page `i` has the resources of object `resources[i]`, a font dictionary naming `fonts[i]`
/// objects of its own (or shared, when two pages name the same resources object).
fn crowded(fonts_in_resources: &[usize], shared: bool) -> Vec<u8> {
    let pages = fonts_in_resources.len();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        stream_bytes("", "0 g"),
    ];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + i)).collect();
    objects[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {pages} >>",
        kids.join(" ")
    )
    .into_bytes();
    // Objects 4.. are the pages, then one resources object per page (one in all when shared), then the fonts.
    let resource_count = if shared { 1 } else { pages };
    let first_resources = 4 + pages;
    let mut next_font = first_resources + resource_count;
    for i in 0..pages {
        let resources = first_resources + if shared { 0 } else { i };
        objects.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources {resources} 0 R /Contents 3 0 R >>")
                .into_bytes(),
        );
    }
    let mut font_objects = Vec::new();
    for &count in fonts_in_resources.iter().take(resource_count) {
        let names: Vec<String> = (0..count)
            .map(|f| format!("/F{f} {} 0 R", next_font + f))
            .collect();
        objects.push(format!("<< /Font << {} >> >>", names.join(" ")).into_bytes());
        for _ in 0..count {
            font_objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        }
        next_font += count;
    }
    objects.extend(font_objects);
    file_bytes(&objects)
}

fn stream_bytes(entries: &str, data: &str) -> Vec<u8> {
    stream(entries, data).into_bytes()
}

#[test]
fn a_failed_import_leaves_nothing_dangling() {
    // Two pages share one resources object that reaches more than a page may.
    let mut document = open(crowded(&[100_500], true));
    let mut out = new_out();
    let mut copier = Copier::new();
    let before = out.count();
    for _ in 0..2 {
        let error = import_page(&mut document, &mut copier, 0, None, &mut out).unwrap_err();
        assert_eq!(error, ImportError::TooLarge);
        assert_eq!(
            out.count(),
            before,
            "what the failed import allocated is gone"
        );
    }
}

#[test]
fn the_limits_are_for_one_page_not_the_whole_sheet() {
    // Each page reaches 60,000 objects of its own: the second import alone is within the limit.
    let mut document = open(crowded(&[60_000, 60_000], false));
    let mut out = new_out();
    let mut copier = Copier::new();
    import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    import_page(&mut document, &mut copier, 1, None, &mut out).unwrap();
    assert!(out.count() > 120_000);
}

#[test]
fn a_single_content_stream_is_copied_as_stored() {
    let compressed = lazy_pdf_filters::encode::flate(b"0 0 1 rg 0 0 50 50 re f", 6);
    let mut stream_object = format!(
        "<< /Filter /FlateDecode /Length {} >>\nstream\n",
        compressed.len()
    )
    .into_bytes();
    stream_object.extend(&compressed);
    stream_object.extend(b"\nendstream");
    let pdf = file_bytes(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_object,
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    let form = import_page(&mut document, &mut Copier::new(), 0, None, &mut out).unwrap();
    let (dict, content) = content_of(&form, out);
    assert!(dict.contains("/Filter /FlateDecode"), "{dict}");
    assert_eq!(content, "0 0 1 rg 0 0 50 50 re f");
}

#[test]
fn pages_naming_one_flate_bomb_cost_its_expansion_once() {
    // 40 MB of blanks in a few kilobytes, named through an array so that it has to be decoded.
    let bomb = lazy_pdf_filters::encode::flate(&vec![b' '; 40 << 20], 6);
    let mut stream_object = format!(
        "<< /Filter /FlateDecode /Length {} >>\nstream\n",
        bomb.len()
    )
    .into_bytes();
    stream_object.extend(&bomb);
    stream_object.extend(b"\nendstream");
    let page =
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents [4 0 R] >>".to_vec();
    let pdf = file_bytes(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R 6 0 R] /Count 3 >>".to_vec(),
        page.clone(),
        stream_object,
        page.clone(),
        page,
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    let mut copier = Copier::new();
    import_page(&mut document, &mut copier, 0, None, &mut out).unwrap();
    for index in [1, 2] {
        let error = import_page(&mut document, &mut copier, index, None, &mut out).unwrap_err();
        assert_eq!(error, ImportError::TooLarge);
    }
}

#[test]
fn an_annotation_with_states_takes_the_first_when_none_is_chosen_and_names_do_not_collide() {
    let [catalog, pages] = catalog_and_pages("3 0 R", 1);
    let on = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 10 10]",
        "0 g 0 0 10 10 re f",
    );
    let pdf = file(&[
        &catalog,
        &pages,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /LpAnnot0 6 0 R >> >> /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /F 4 /AP << /N << /On 5 0 R >> >> >>",
        &on,
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream",
    ]);
    let mut document = open(pdf);
    let mut out = new_out();
    let form = import_page(&mut document, &mut Copier::new(), 0, None, &mut out).unwrap();
    let (dict, content) = content_of(&form, out);
    assert!(content.contains("/LpAnnot0_ Do"), "{content}");
    assert!(
        dict.contains("/LpAnnot0 ") && dict.contains("/LpAnnot0_ "),
        "{dict}"
    );
}
