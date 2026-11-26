mod common;

use common::*;
use lazy_pdf_filters::{DEFAULT_MAX_SIZE, Filter, Params, Value, decode};

// jbig2 <file> <globals> <flips> <length> <fit>: fit is the image's /Width and /Height as WxH.
fn op(op: &str, words: &[&str]) -> String {
    assert_eq!(op, "jbig2");
    let mut data = read(words[0]);
    flips(words[2], &mut data);
    if words[3] != "-" {
        data.truncate(words[3].parse().unwrap());
    }
    let mut parms = Params::new();
    if words[1] != "-" {
        parms.set("JBIG2Globals", Value::Data(read(words[1])));
    }
    if words[4] != "-" {
        let (w, h) = words[4].split_once('x').unwrap();
        parms
            .set("Width", Value::Int(w.parse().unwrap()))
            .set("Height", Value::Int(h.parse().unwrap()));
    }
    let filter = [Filter::new("JBIG2Decode", Some(parms))];
    match decode(&data, &filter, DEFAULT_MAX_SIZE) {
        Ok(page) => show(&page),
        Err(e) => show_error(&e),
    }
}

#[test]
fn jbig2_output_is_the_golden_file() {
    check(
        &read_text("jbig2-input.txt"),
        &read_text("jbig2-output.txt"),
        op,
        &[],
    );
}
