#![no_main]

use lazy_pdf_fonts::CMap;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let map = CMap::parse(data, None);
    let mut at = 0;
    while at < data.len().min(64) {
        let (code, length) = map.next_code(&data[at..]);
        let _ = map.cid_of(code, length);
        let _ = map.unicode_of(code, length);
        at += length.max(1);
    }
});
