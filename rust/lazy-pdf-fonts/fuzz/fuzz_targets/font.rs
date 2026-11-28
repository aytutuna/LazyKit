#![no_main]

use lazy_pdf_fonts::{PdfFont, StandardFilters};
use lazy_pdf_syntax::{Document, Object};
use libfuzzer_sys::fuzz_target;

// The data is a whole PDF file; every dictionary that says it is a font is read as one and made to decode.
fuzz_target!(|data: &[u8]| {
    let Ok(mut doc) = Document::open(data.to_vec(), Box::new(StandardFilters)) else {
        return;
    };
    for number in doc.object_numbers().into_iter().take(64) {
        let Some(object) = doc.get(number) else {
            continue;
        };
        let Object::Dict(dict) = &*object else {
            continue;
        };
        if dict
            .get("Type")
            .and_then(Object::as_name)
            .is_some_and(|n| n.is("Font"))
        {
            let font = PdfFont::new(&mut doc, dict);
            let _ = font.to_text(b"Hello \x00\x01\xff\x80\x41");
        }
    }
});
