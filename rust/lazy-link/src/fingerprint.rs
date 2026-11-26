use crate::base64url;
use sha2::{Digest, Sha256};

pub const FINGERPRINT_LENGTH: usize = 32;

/// The fingerprint a phone pins a desktop's certificate by: the SHA-256 of the certificate's
/// SubjectPublicKeyInfo, so a renewed certificate with the same key keeps every pin.
pub fn spki_fingerprint(spki_der: &[u8]) -> [u8; FINGERPRINT_LENGTH] {
    Sha256::digest(spki_der).into()
}

/// The fingerprint as the pairing code carries it: base64url without padding (43 characters).
pub fn fingerprint_text(fingerprint: &[u8]) -> String {
    base64url::encode(fingerprint)
}

/// The six digits people compare on both screens: the first four bytes as a big-endian number
/// modulo one million, with leading zeros; empty for fewer than four bytes.
pub fn check_code(fingerprint: &[u8]) -> String {
    let Some(head) = fingerprint.first_chunk::<4>() else {
        return String::new();
    };
    format!("{:06}", u32::from_be_bytes(*head) % 1_000_000)
}

/// Six digits as people read them, three and three with a space.
pub fn spaced(digits: &str) -> String {
    if digits.len() == 6 && digits.is_ascii() {
        format!("{} {}", &digits[..3], &digits[3..])
    } else {
        digits.to_owned()
    }
}

/// The check code of a pairing by short code, from SHA-256 over the fingerprint and the canonical
/// short code (ASCII; anything else is hashed as '?', as .NET does). The short code is not known when the certificate is chosen, so a key cannot be
/// ground until the plain check code comes out right.
pub fn match_code(fingerprint: &[u8], short_code: &str) -> String {
    if fingerprint.len() != FINGERPRINT_LENGTH {
        return String::new();
    }
    let mut hash = Sha256::new();
    hash.update(fingerprint);
    // One byte per UTF-16 unit with '?' for anything outside ASCII, which is what .NET's
    // Encoding.ASCII makes of it, so every platform hashes the same bytes.
    let ascii: Vec<u8> = short_code
        .encode_utf16()
        .map(|u| u8::try_from(u).ok().filter(u8::is_ascii).unwrap_or(b'?'))
        .collect();
    hash.update(&ascii);
    check_code(&hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaced_leaves_non_ascii_alone() {
        assert_eq!(spaced("123456"), "123 456");
        assert_eq!(spaced("12345\u{e9}"), "12345\u{e9}");
        assert_eq!(spaced("\u{e9}2345"), "\u{e9}2345");
    }

    #[test]
    fn match_code_hashes_non_ascii_as_a_question_mark() {
        let fingerprint = [3u8; FINGERPRINT_LENGTH];
        assert_eq!(
            match_code(&fingerprint, "AB\u{e9}D"),
            match_code(&fingerprint, "AB?D")
        );
        // One '?' per UTF-16 unit: a character outside the BMP is two.
        assert_eq!(
            match_code(&fingerprint, "\u{1F600}"),
            match_code(&fingerprint, "??")
        );
    }
}
