#![no_main]

use lazy_pdf_syntax::Lexer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut lexer = Lexer::new(data, 0);
    // A lexer that stops moving would loop for ever, so every object has to use up some bytes or fail.
    while lexer.pos < data.len() {
        let before = lexer.pos;
        let _ = lexer.read_object_header();
        if lexer.read_object(true).is_err() {
            lexer.pos = before + 1;
        }
        assert!(lexer.pos > before || lexer.pos >= data.len());
    }
});
