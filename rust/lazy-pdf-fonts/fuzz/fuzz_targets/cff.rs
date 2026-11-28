#![no_main]

use lazy_pdf_fonts::CffFont;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(font) = CffFont::parse(data) {
        for g in 0..font.glyph_count().min(64) {
            let _ = font.outline(g);
            let _ = font.width(g);
            let _ = font.glyph_name(g);
            let _ = font.cid_for_glyph(g);
        }
        let _ = font.glyph_for_name("A");
        let _ = font.glyph_for_cid(500);
        let _ = font.glyph_for_code(65);
    }
});
