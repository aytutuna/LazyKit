//! The Lazy phone link: the parts of the protocol that are plain functions of their input.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

#[cfg(feature = "relay")]
pub mod account;
mod base64url;
mod fingerprint;
#[cfg(feature = "relay")]
mod http;
mod pairing_uri;
#[cfg(feature = "relay")]
pub mod relay;
mod session;
mod short_code;
#[cfg(feature = "relay")]
pub mod tls;
#[cfg(feature = "relay")]
pub mod transport;
#[cfg(feature = "relay")]
mod wire;

pub use fingerprint::{
    FINGERPRINT_LENGTH, check_code, fingerprint_text, match_code, spaced, spki_fingerprint,
};

pub use pairing_uri::{
    PairingUri, PairingUriError, TOKEN_LENGTH, VERSION, is_valid_address, is_valid_id,
    parse_pairing_uri, percent_decode, percent_encode,
};
pub use session::{
    LIFETIME_MS, MAX_FAILED_ATTEMPTS, MAX_TOTAL_FAILED_ATTEMPTS, PairingSession, Peer,
    RedeemResult, SessionState,
};
pub use short_code::{ALPHABET, SHORT_CODE_LENGTH, normalize_short_code};
