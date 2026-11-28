#![no_main]

use lazy_pdf_fonts::Type1Font;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(font) = Type1Font::parse(data) {
        for name in font.glyph_names.iter().take(64) {
            let _ = font.outline(name);
            let _ = font.width(name);
        }
    }
});
