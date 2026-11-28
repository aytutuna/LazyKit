#![no_main]

use lazy_pdf_filters::{Filter, decode};
use libfuzzer_sys::fuzz_target;

const CAP: u64 = 1 << 20;

// ASCIIHex, ASCII85 and RunLength are small enough to share a target.
fuzz_target!(|data: &[u8]| {
    let Some((name, rest)) = data.split_first() else {
        return;
    };
    let name = ["ASCIIHexDecode", "ASCII85Decode", "RunLengthDecode"][*name as usize % 3];
    if let Ok(out) = decode(rest, &[Filter::new(name, None)], CAP) {
        assert!(out.len() as u64 <= CAP);
    }
});
