mod data;

use lazy_link::{PairingUriError, check_code, parse_pairing_uri, spaced};

fn error_name(error: PairingUriError) -> &'static str {
    match error {
        PairingUriError::NotPairingCode => "notPairingCode",
        PairingUriError::NewerVersion => "newerVersion",
        PairingUriError::Damaged => "damaged",
    }
}

#[test]
fn pairing_codes_are_read_as_the_data_says() {
    let data = data::load("pairing-uris.json");
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let result = case["result"].as_str().unwrap();
        let parsed = parse_pairing_uri(case["uri"].as_str().unwrap());
        if result != "ok" {
            assert_eq!(parsed.map_err(error_name), Err(result), "{name}");
            continue;
        }
        let uri = parsed.unwrap_or_else(|e| panic!("{name}: refused with {e:?}"));
        let want = &case["pairing"];
        assert_eq!(
            uri.desktop_id,
            want["desktopId"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(uri.name, want["name"].as_str().unwrap(), "{name}");
        assert_eq!(
            uri.fingerprint,
            want["fingerprint"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(uri.port as u64, want["port"].as_u64().unwrap(), "{name}");
        assert_eq!(uri.token, want["token"].as_str().unwrap(), "{name}");
        let addresses: Vec<&str> = want["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(uri.addresses, addresses, "{name}");
        let code = spaced(&check_code(&uri.fingerprint_bytes().unwrap()));
        assert_eq!(code, want["checkCode"].as_str().unwrap(), "{name}");
        assert_eq!(
            parse_pairing_uri(&uri.format().unwrap()),
            Ok(uri),
            "{name}: format and read again"
        );
    }
}
