//! The UniFFI surface of lazy-link for Swift, Kotlin and C#.
//! Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)

use std::net::IpAddr;
use std::sync::Arc;

uniffi::setup_scaffolding!();

/// Every call can fail, even the ones that only compute: Swift turns a panic into an error only
/// for a function that declares one, and aborts the app for any other.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LinkError {
    #[error("not a pairing code")]
    NotPairingCode,
    #[error("pairing code of a newer version")]
    NewerVersion,
    #[error("damaged pairing code")]
    Damaged,
    #[error("invalid input: {reason}")]
    Invalid { reason: String },
    #[error("signing failed: {reason}")]
    Signing { reason: String },
    #[error("unexpected error in native code: {reason}")]
    Unexpected { reason: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for LinkError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Unexpected {
            reason: error.reason,
        }
    }
}

impl From<lazy_link::PairingUriError> for LinkError {
    fn from(error: lazy_link::PairingUriError) -> Self {
        match error {
            lazy_link::PairingUriError::NotPairingCode => Self::NotPairingCode,
            lazy_link::PairingUriError::NewerVersion => Self::NewerVersion,
            lazy_link::PairingUriError::Damaged => Self::Damaged,
        }
    }
}

/// What a desktop's pairing code carries.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairingCode {
    pub desktop_id: String,
    pub name: String,
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub token: String,
}

impl From<lazy_link::PairingUri> for PairingCode {
    fn from(uri: lazy_link::PairingUri) -> Self {
        Self {
            desktop_id: uri.desktop_id,
            name: uri.name,
            fingerprint: uri.fingerprint,
            addresses: uri.addresses,
            port: uri.port,
            token: uri.token,
        }
    }
}

#[uniffi::export]
pub fn parse_pairing_code(text: String) -> Result<PairingCode, LinkError> {
    Ok(lazy_link::parse_pairing_uri(&text)?.into())
}

#[uniffi::export]
pub fn format_pairing_code(code: PairingCode) -> Result<String, LinkError> {
    let uri = lazy_link::PairingUri {
        desktop_id: code.desktop_id,
        name: code.name,
        fingerprint: code.fingerprint,
        addresses: code.addresses,
        port: code.port,
        token: code.token,
    };
    uri.format().ok_or(LinkError::Invalid {
        reason: "the pairing code has a part that cannot be written".into(),
    })
}

#[uniffi::export]
pub fn normalize_short_code(text: String) -> Result<Option<String>, LinkError> {
    Ok(lazy_link::normalize_short_code(&text))
}

/// The SHA-256 of a certificate's SubjectPublicKeyInfo.
#[uniffi::export]
pub fn spki_fingerprint(spki_der: &[u8]) -> Result<Vec<u8>, LinkError> {
    Ok(lazy_link::spki_fingerprint(spki_der).to_vec())
}

#[uniffi::export]
pub fn fingerprint_text(fingerprint: &[u8]) -> Result<String, LinkError> {
    Ok(lazy_link::fingerprint_text(fingerprint))
}

/// The six digits both screens show, spaced 3 and 3; empty for fewer than four bytes.
#[uniffi::export]
pub fn check_code(fingerprint: &[u8]) -> Result<String, LinkError> {
    Ok(lazy_link::spaced(&lazy_link::check_code(fingerprint)))
}

/// The six digits of a pairing by short code, spaced 3 and 3; empty unless the fingerprint is 32 bytes.
#[uniffi::export]
pub fn match_code(fingerprint: &[u8], short_code: String) -> Result<String, LinkError> {
    Ok(lazy_link::spaced(&lazy_link::match_code(
        fingerprint,
        &short_code,
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RedeemResult {
    Accepted,
    WrongToken,
    Expired,
    AlreadyUsed,
    Closed,
}

impl From<lazy_link::RedeemResult> for RedeemResult {
    fn from(result: lazy_link::RedeemResult) -> Self {
        match result {
            lazy_link::RedeemResult::Accepted => Self::Accepted,
            lazy_link::RedeemResult::WrongToken => Self::WrongToken,
            lazy_link::RedeemResult::Expired => Self::Expired,
            lazy_link::RedeemResult::AlreadyUsed => Self::AlreadyUsed,
            lazy_link::RedeemResult::Closed => Self::Closed,
        }
    }
}

#[derive(uniffi::Object)]
pub struct PairingSession {
    inner: lazy_link::PairingSession,
}

#[uniffi::export]
impl PairingSession {
    /// `token` is 16 random bytes and `code_entropy` at least 8, both from the system's random source.
    #[uniffi::constructor]
    pub fn new(
        opened_ms: i64,
        token: Vec<u8>,
        code_entropy: Vec<u8>,
    ) -> Result<Arc<Self>, LinkError> {
        let invalid = |reason: &str| LinkError::Invalid {
            reason: reason.into(),
        };
        let token = token
            .try_into()
            .map_err(|_| invalid("a pairing token has 16 bytes"))?;
        let inner = lazy_link::PairingSession::new(opened_ms, token, &code_entropy)
            .ok_or_else(|| invalid("the short code needs 8 bytes of randomness"))?;
        Ok(Arc::new(Self { inner }))
    }

    pub fn token(&self) -> String {
        self.inner.token()
    }

    pub fn short_code(&self) -> String {
        self.inner.short_code().to_owned()
    }

    /// `peer` is the address the token came from, or None when the caller does not know it.
    pub fn redeem(
        &self,
        token: Option<String>,
        now_ms: i64,
        peer: Option<String>,
    ) -> Result<RedeemResult, LinkError> {
        let peer = peer.and_then(|p| p.parse::<IpAddr>().ok());
        Ok(self.inner.redeem(token.as_deref(), now_ms, peer).into())
    }

    pub fn close(&self) {
        self.inner.close();
    }
}
