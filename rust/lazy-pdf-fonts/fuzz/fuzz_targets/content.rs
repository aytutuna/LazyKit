#![no_main]

use lazy_pdf_fonts::parse_content;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for op in parse_content(data) {
        assert!(op.start <= op.end && op.end <= data.len());
        let _ = op.to_string();
        if let Some(image) = &op.image {
            let _ = image.expanded(None);
        }
    }
});
