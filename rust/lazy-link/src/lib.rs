//! The Lazy phone link: the parts of the protocol that are plain functions of their input.
//! Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)

mod base64url;
mod fingerprint;
mod short_code;

pub use fingerprint::{
    FINGERPRINT_LENGTH, check_code, fingerprint_text, match_code, spaced, spki_fingerprint,
};

pub use short_code::{ALPHABET, SHORT_CODE_LENGTH, normalize_short_code};
