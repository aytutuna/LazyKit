#![no_main]

use lazy_pdf_filters::{Filter, Params, Value, decode};
use libfuzzer_sys::fuzz_target;

const CAP: u64 = 1 << 24;

fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_at_checked(4) else {
        return;
    };
    let mut parms = Params::new();
    parms
        .set("K", Value::Int(head[0] as i64 % 5 - 2))
        .set("Columns", Value::Int(head[1] as i64 * 9 + 1))
        .set("Rows", Value::Int(if head[2] & 1 == 0 { 0 } else { head[3] as i64 }))
        .set("EncodedByteAlign", Value::Bool(head[2] & 2 != 0))
        .set("EndOfLine", Value::Bool(head[2] & 4 != 0))
        .set("BlackIs1", Value::Bool(head[2] & 8 != 0));
    let filter = [Filter::new("CCITTFaxDecode", Some(parms))];
    if let Ok(out) = decode(rest, &filter, CAP) {
        assert!(out.len() as u64 <= CAP);
    }
});
