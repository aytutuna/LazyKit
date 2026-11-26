#![no_main]

use lazy_pdf_syntax::{DecodeError, Document, Filters, Parm, SaveOptions};
use libfuzzer_sys::fuzz_target;

/// Flate and nothing else, with output capped far below the library's own limit so a bomb costs little.
struct Flate;

impl Filters for Flate {
    fn decode(
        &self,
        filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        _strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        match filter {
            b"FlateDecode" | b"Fl" => {
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, max_size.min(1 << 20))
                    .map_err(|e| DecodeError::Damaged(format!("{e:?}")))
            }
            _ => Err(DecodeError::Unsupported("filter".into())),
        }
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 1)
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(mut document) = Document::open(data.to_vec(), Box::new(Flate)) else {
        return;
    };
    let pages = document.pages();
    for page in pages.iter().take(50) {
        page.media_box(&mut document);
        page.crop_box(&mut document);
        page.rotate(&mut document);
        page.user_unit(&mut document);
        page.contents(&mut document);
    }
    for number in document.object_numbers().into_iter().take(500) {
        document.get(number);
    }
    document.is_signed();
    document.is_linearized();
    for packed in [false, true] {
        let saved = document.save_rewrite(&SaveOptions {
            use_object_streams: packed,
            document_id: None,
        });
        // What the writer made has to be something the reader accepts.
        assert!(Document::open(saved.bytes, Box::new(Flate)).is_ok());
    }
    if let Ok(saved) = document.save_incremental() {
        assert!(saved.len() >= data.len().min(document.length()));
    }
});
