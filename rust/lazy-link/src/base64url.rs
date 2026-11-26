use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

pub fn encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// The bytes of base64url text without padding; None for anything else, stray bits included.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}
