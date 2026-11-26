mod common;

use common::open;
use common::pdf::TestPdf;
use lazy_pdf_syntax::{Document, Object};

fn set(document: &mut Document, number: u32, key: &str, value: Object) {
    let Some(Object::Dict(dict)) = document.get_mut(number) else {
        panic!("not a dictionary")
    };
    dict.insert(key, value);
}

#[test]
fn a_page_reads_its_dictionary_as_it_is_now() {
    let mut document = open(TestPdf::simple(1));
    let pages = document.pages();
    let page = &pages[0];
    assert_eq!(page.rotate(&mut document), 0);
    set(
        &mut document,
        page.reference.number,
        "Rotate",
        Object::int(90),
    );
    let media = Object::Array(vec![
        Object::int(0),
        Object::int(0),
        Object::int(10),
        Object::int(20),
    ]);
    set(&mut document, page.reference.number, "MediaBox", media);
    assert_eq!(page.rotate(&mut document), 90);
    assert_eq!(page.media_box(&mut document).width(), 10.0);
    let fresh = document.pages();
    assert_eq!(fresh[0].rotate(&mut document), 90);
    assert_eq!(
        fresh[0].dict(&mut document).get("Rotate"),
        Some(&Object::int(90))
    );
}

#[test]
fn the_other_page_boxes_fall_back_to_the_crop_box() {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
        .obj(2, "<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>")
        .obj(
            3,
            "<</Type /Page /MediaBox [0 0 100 100] /CropBox [10 10 90 90] /TrimBox [5 5 15 25]>>",
        )
        .obj(4, "<</Type /Page /MediaBox [0 0 50 60]>>");
    let mut document = open(pdf.finish());
    let pages = document.pages();
    let trim = pages[0].trim_box(&mut document);
    assert_eq!(
        (trim.left, trim.bottom, trim.right, trim.top),
        (5.0, 5.0, 15.0, 25.0)
    );
    let crop = pages[0].crop_box(&mut document);
    assert_eq!(pages[0].bleed_box(&mut document), crop);
    assert_eq!(pages[0].art_box(&mut document).width(), 80.0);
    assert_eq!(pages[1].trim_box(&mut document).height(), 60.0);
}
