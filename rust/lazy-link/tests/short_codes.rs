mod data;

use lazy_link::{ALPHABET, PairingSession, normalize_short_code};

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

#[test]
fn sessions_redeem_as_the_data_says() {
    let data = data::load("short-codes.json");
    let token: [u8; 16] = data["tokenBytes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    for case in data["redeem"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let session = PairingSession::new(0, token, &[7, 3, 21, 0, 9, 31, 12, 5]).unwrap();
        let typed = format!(
            "{}-{}",
            &session.short_code()[..4],
            &session.short_code()[4..]
        )
        .to_lowercase();
        for (step, attempt) in case["attempts"].as_array().unwrap().iter().enumerate() {
            let presented = match attempt["token"].as_str() {
                Some("$token") => Some(session.token()),
                Some("$shortCode") => Some(session.short_code().to_owned()),
                Some("$shortCodeTyped") => Some(typed.clone()),
                other => other.map(str::to_owned),
            };
            let peer = attempt["peer"].as_str().map(|p| p.parse().unwrap());
            let at = attempt["at"].as_i64().unwrap() * 1000;
            let got = session.redeem(presented.as_deref(), at, peer);
            let expect = attempt["expect"].as_str().unwrap();
            assert_eq!(
                format!("{got:?}").to_lowercase(),
                expect.to_lowercase(),
                "{name}, attempt {}",
                step + 1
            );
        }
    }
}
