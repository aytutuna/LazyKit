#![no_main]

use lazy_pdf_fonts::{TrueTypeFont, build, subset};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(font) = TrueTypeFont::parse(data, 0) {
        for g in 0..font.glyph_count.min(64) {
            let _ = font.outline(g);
            let _ = font.bounds(g);
            let _ = font.advance(g);
            let _ = font.glyph_name(g);
        }
        for cp in [0x20, 0x41, 0xC0, 0x4E2D, 0x1F600] {
            let _ = font.glyph_for_code_point(cp);
            let _ = font.glyph_for_variation(cp, 0xFE00);
        }
        let _ = font.family_name();
        let _ = font.post_script_name();
    }
    let _ = subset(data, &[1, 2, 3, 40], 0);
    let _ = build(data, &["Aé中"], 0);
});
