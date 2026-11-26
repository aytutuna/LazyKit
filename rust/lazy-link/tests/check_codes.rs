mod data;

use lazy_link::{check_code, match_code, spaced, spki_fingerprint};

#[test]
fn check_and_match_codes_are_as_the_data_says() {
    let data = data::load("check-codes.json");
    for case in data["cases"].as_array().unwrap() {
        let fingerprint: Vec<u8> = case["fingerprint"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(spaced(&check_code(&fingerprint)), case["checkCode"]);
        let short_code = case["shortCode"].as_str().unwrap();
        assert_eq!(
            spaced(&match_code(&fingerprint, short_code)),
            case["matchCode"]
        );
    }
}

#[test]
fn short_input_has_no_check_code() {
    assert_eq!(check_code(&[1, 2, 3]), "");
    assert_eq!(match_code(&[0; 31], "7KQ2M9XD"), "");
}

#[test]
fn spki_fingerprint_is_sha256() {
    let hash = spki_fingerprint(b"abc");
    assert_eq!(hash[..4], [0xba, 0x78, 0x16, 0xbf]);
}
