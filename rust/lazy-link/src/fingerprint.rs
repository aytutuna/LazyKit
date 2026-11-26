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
    if digits.len() == 6 {
        format!("{} {}", &digits[..3], &digits[3..])
    } else {
        digits.to_owned()
    }
}

/// The check code of a pairing by short code, from SHA-256 over the fingerprint and the canonical
/// short code. The short code is not known when the certificate is chosen, so a key cannot be
/// ground until the plain check code comes out right.
pub fn match_code(fingerprint: &[u8], short_code: &str) -> String {
    if fingerprint.len() != FINGERPRINT_LENGTH {
        return String::new();
    }
    let mut hash = Sha256::new();
    hash.update(fingerprint);
    hash.update(short_code.as_bytes());
    check_code(&hash.finalize())
}
