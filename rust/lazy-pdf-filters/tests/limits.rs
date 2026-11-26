use lazy_pdf_filters::{Filter, Params, Problem, Value, dct, decode, decode_to_codec};

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
fn ccitt_rows_are_checked_against_the_cap_before_allocating() {
    let mut parms = Params::new();
    parms
        .set("Columns", Value::Int(1 << 20))
        .set("Rows", Value::Int(1 << 20));
    let filter = [Filter::new("CCITTFaxDecode", Some(parms))];
    too_big(decode(b"\x00", &filter, 1 << 20));
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

#[test]
fn codec_stops_the_chain_and_keeps_its_parameters() {
    let mut parms = Params::new();
    parms.set("ColorTransform", Value::Int(0));
    let chain = [
        Filter::new("AHx", None),
        Filter::new("DCT", Some(parms.clone())),
    ];
    let decoded = decode_to_codec(b"4142>", &chain, 1 << 20).unwrap();
    assert_eq!(decoded.data, b"AB");
    assert_eq!(decoded.codec.as_deref(), Some("DCTDecode"));
    assert_eq!(decoded.parms, Some(parms));
    assert_eq!(
        decode(b"4142>", &chain, 1 << 20).unwrap_err().message,
        "The /DCT filter is an image codec."
    );
}

#[test]
fn jpeg_header_and_adobe_marker() {
    let jpeg = [
        0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0,
        0xff, 0xc0, 0x00, 0x11, 8, 0x00, 0x20, 0x00, 0x30, 4, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0,
    ];
    assert!(dct::has_adobe_marker(&jpeg));
    let header = dct::read_header(&jpeg).unwrap();
    assert_eq!(
        (header.width, header.height, header.components),
        (0x30, 0x20, 4)
    );
    assert!(header.adobe);
    assert!(!dct::has_adobe_marker(&jpeg[..2]));
    assert_eq!(dct::read_header(b"not a jpeg"), None);
}
