#![no_main]

use lazy_pdf_filters::dct;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = dct::has_adobe_marker(data);
    let _ = dct::read_header(data);
});
