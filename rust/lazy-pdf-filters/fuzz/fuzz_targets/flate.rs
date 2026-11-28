#![no_main]

use lazy_pdf_filters::{Filter, Params, Value, decode};
use libfuzzer_sys::fuzz_target;

const CAP: u64 = 1 << 24;

// The first bytes pick the predictor and its shape; the rest is the zlib or raw deflate data.
fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_at_checked(4) else {
        return;
    };
    let mut parms = Params::new();
    parms
        .set(
            "Predictor",
            Value::Int([1, 2, 10, 12, 15, 3][head[0] as usize % 6]),
        )
        .set("Colors", Value::Int(head[1] as i64 % 5))
        .set(
            "BitsPerComponent",
            Value::Int([1, 2, 4, 8, 16, 3][head[2] as usize % 6]),
        )
        .set("Columns", Value::Int(head[3] as i64 * 7));
    let filter = [Filter::new("FlateDecode", Some(parms))];
    if let Ok(out) = decode(rest, &filter, CAP) {
        assert!(out.len() as u64 <= CAP);
    }
});
