// How the adapter maps the filter crate's problems onto the syntax crate's errors.

use lazy_pdf_fonts::StandardFilters;
use lazy_pdf_syntax::{DecodeError, Filters};

#[test]
fn output_past_the_limit_is_too_large() {
    let filters = StandardFilters;
    let packed = filters.flate(&vec![0u8; 10_000]);
    let result = filters.decode(b"FlateDecode", &[], &packed, 100, false);
    assert!(matches!(result, Err(DecodeError::TooLarge(_))));
    let result = filters.decode(b"FlateDecode", &[], &packed, 20_000, false);
    assert_eq!(result.unwrap().len(), 10_000);
}

#[test]
fn broken_and_unknown_data_keep_their_kind() {
    let filters = StandardFilters;
    let result = filters.decode(b"FlateDecode", &[], b"not zlib data at all", 1000, true);
    assert!(matches!(result, Err(DecodeError::Damaged(_))));
    let result = filters.decode(b"NoSuchDecode", &[], b"abc", 1000, false);
    assert!(matches!(result, Err(DecodeError::Unsupported(_))));
}
