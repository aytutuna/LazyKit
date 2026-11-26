use lazy_pdf_filters::{Filter, Params, Problem, Value, decode};

fn one(name: &str) -> [Filter; 1] {
    [Filter::new(name, None)]
}

fn too_big(result: lazy_pdf_filters::Result<Vec<u8>>) {
    let e = result.unwrap_err();
    assert_eq!(e.problem, Problem::Unsupported);
    assert_eq!(e.message, "A stream expands past the size limit.");
}

#[test]
fn flate_bomb_stops_at_the_cap() {
    let zeros = vec![0u8; 1 << 20];
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&zeros, 6);
    assert_eq!(
        decode(&packed, &one("FlateDecode"), 1 << 20).unwrap(),
        zeros
    );
    too_big(decode(&packed, &one("FlateDecode"), (1 << 20) - 1));
}

#[test]
fn ascii_filters_stop_at_the_cap() {
    too_big(decode(b"zzzz~>", &one("A85"), 15));
    assert_eq!(decode(b"zzzz~>", &one("A85"), 16).unwrap().len(), 16);
    too_big(decode(b"\x81a\x81b", &one("RL"), 255));
    too_big(decode(b"00000000>", &one("AHx"), 3));
}

#[test]
fn lzw_stops_at_the_cap() {
    // Clear, then 256 literal 'a' codes of nine bits is too long to write out; two codes are enough.
    let data = [0x80, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01];
    let all = decode(&data, &one("LZW"), 1 << 20).unwrap();
    assert!(!all.is_empty());
    too_big(decode(&data, &one("LZW"), all.len() as u64 - 1));
}

#[test]
fn predictor_row_is_bounded_by_the_data() {
    let mut parms = Params::new();
    parms
        .set("Predictor", Value::Int(12))
        .set("Columns", Value::Int(i32::MAX as i64));
    let filter = [Filter::new("LZWDecode", Some(parms))];
    assert!(decode(b"\x80\x0b\x60\x50\x22\x0c\x0c\x85\x01", &filter, 1 << 20).is_ok());
}
