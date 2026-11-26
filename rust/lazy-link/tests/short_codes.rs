mod data;

use lazy_link::{ALPHABET, normalize_short_code};

#[test]
fn typed_codes_are_normalized_as_the_data_says() {
    let data = data::load("short-codes.json");
    assert_eq!(data["alphabet"], ALPHABET);
    for case in data["normalize"].as_array().unwrap() {
        // A null typed value is nothing typed at all, which the callers leave out.
        let Some(typed) = case["typed"].as_str() else {
            continue;
        };
        let expected = case["canonical"].as_str().map(str::to_owned);
        assert_eq!(normalize_short_code(typed), expected, "typed {typed:?}");
    }
}
