#![no_main]

use lazy_pdf_filters::{Filter, Params, Value, decode};
use libfuzzer_sys::fuzz_target;

const CAP: u64 = 1 << 24;

fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_at_checked(2) else {
        return;
    };
    let mut parms = Params::new();
    parms
        .set("EarlyChange", Value::Int((head[0] & 1) as i64))
        .set("Predictor", Value::Int([1, 2, 12][head[1] as usize % 3]))
        .set("Columns", Value::Int(head[1] as i64 / 3 + 1));
    let filter = [Filter::new("LZWDecode", Some(parms))];
    if let Ok(out) = decode(rest, &filter, CAP) {
        assert!(out.len() as u64 <= CAP);
    }
});
