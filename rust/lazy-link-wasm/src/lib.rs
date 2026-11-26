//! The wasm-bindgen surface of lazy-link for the browser and Node.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use js_sys::Uint8Array;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

const MAX_PATTERN_BYTES: u32 = 256 * 1024 * 1024;

fn error_name(error: lazy_link::PairingUriError) -> &'static str {
    match error {
        lazy_link::PairingUriError::NotPairingCode => "notPairingCode",
        lazy_link::PairingUriError::NewerVersion => "newerVersion",
        lazy_link::PairingUriError::Damaged => "damaged",
    }
}

/// Times are milliseconds since the epoch as JavaScript counts them: whole numbers a double holds exactly.
fn time(ms: f64) -> Result<i64, JsError> {
    const LIMIT: f64 = 9_007_199_254_740_991.0;
    if ms.is_finite() && ms.abs() <= LIMIT {
        Ok(ms as i64)
    } else {
        Err(JsError::new("a time is a finite number of milliseconds"))
    }
}

/// What a desktop's pairing code carries.
#[wasm_bindgen(getter_with_clone)]
pub struct PairingCode {
    #[wasm_bindgen(js_name = desktopId)]
    pub desktop_id: String,
    pub name: String,
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub token: String,
}

#[wasm_bindgen]
impl PairingCode {
    #[wasm_bindgen(constructor)]
    pub fn new(
        desktop_id: String,
        name: String,
        fingerprint: String,
        addresses: Vec<String>,
        port: u16,
        token: String,
    ) -> PairingCode {
        PairingCode {
            desktop_id,
            name,
            fingerprint,
            addresses,
            port,
            token,
        }
    }
}

/// Throws an Error whose message is notPairingCode, newerVersion or damaged.
#[wasm_bindgen(js_name = parsePairingCode)]
pub fn parse_pairing_code(text: &str) -> Result<PairingCode, JsError> {
    let uri = lazy_link::parse_pairing_uri(text).map_err(|e| JsError::new(error_name(e)))?;
    Ok(PairingCode {
        desktop_id: uri.desktop_id,
        name: uri.name,
        fingerprint: uri.fingerprint,
        addresses: uri.addresses,
        port: uri.port,
        token: uri.token,
    })
}

#[wasm_bindgen(js_name = formatPairingCode)]
pub fn format_pairing_code(code: &PairingCode) -> Result<String, JsError> {
    let uri = lazy_link::PairingUri {
        desktop_id: code.desktop_id.clone(),
        name: code.name.clone(),
        fingerprint: code.fingerprint.clone(),
        addresses: code.addresses.clone(),
        port: code.port,
        token: code.token.clone(),
    };
    uri.format()
        .ok_or_else(|| JsError::new("the pairing code has a part that cannot be written"))
}

#[wasm_bindgen(js_name = normalizeShortCode)]
pub fn normalize_short_code(text: &str) -> Option<String> {
    lazy_link::normalize_short_code(text)
}

#[wasm_bindgen(js_name = spkiFingerprint)]
pub fn spki_fingerprint(spki_der: &[u8]) -> Vec<u8> {
    lazy_link::spki_fingerprint(spki_der).to_vec()
}

#[wasm_bindgen(js_name = checkCode)]
pub fn check_code(fingerprint: &[u8]) -> String {
    lazy_link::spaced(&lazy_link::check_code(fingerprint))
}

#[wasm_bindgen(js_name = matchCode)]
pub fn match_code(fingerprint: &[u8], short_code: &str) -> String {
    lazy_link::spaced(&lazy_link::match_code(fingerprint, short_code))
}

#[wasm_bindgen]
pub struct PairingSession {
    inner: lazy_link::PairingSession,
}

#[wasm_bindgen]
impl PairingSession {
    #[wasm_bindgen(constructor)]
    pub fn new(
        opened_ms: f64,
        token: &[u8],
        code_entropy: &[u8],
    ) -> Result<PairingSession, JsError> {
        let token = token
            .try_into()
            .map_err(|_| JsError::new("a pairing token has 16 bytes"))?;
        let inner = lazy_link::PairingSession::new(time(opened_ms)?, token, code_entropy)
            .ok_or_else(|| JsError::new("the short code needs 8 bytes of randomness"))?;
        Ok(PairingSession { inner })
    }

    pub fn token(&self) -> String {
        self.inner.token()
    }

    #[wasm_bindgen(js_name = shortCode)]
    pub fn short_code(&self) -> String {
        self.inner.short_code().to_owned()
    }

    /// When the session ends, in the same milliseconds as the time it was opened at.
    #[wasm_bindgen(js_name = expiresMs)]
    pub fn expires_ms(&self) -> f64 {
        self.inner.expires_ms() as f64
    }

    #[wasm_bindgen(js_name = failedAttempts)]
    pub fn failed_attempts(&self) -> u32 {
        self.inner.failed_attempts()
    }

    /// Returns open, used, expired or closed.
    #[wasm_bindgen(js_name = stateAt)]
    pub fn state_at(&self, now_ms: f64) -> Result<String, JsError> {
        use lazy_link::SessionState::*;
        let state = match self.inner.state_at(time(now_ms)?) {
            Open => "open",
            Used => "used",
            Expired => "expired",
            Closed => "closed",
        };
        Ok(state.to_owned())
    }

    /// Milliseconds the session stays open after `now_ms`; zero once it is not open.
    #[wasm_bindgen(js_name = remainingMs)]
    pub fn remaining_ms(&self, now_ms: f64) -> Result<f64, JsError> {
        Ok(self.inner.remaining_ms(time(now_ms)?) as f64)
    }

    /// Returns accepted, wrongToken, expired, alreadyUsed or closed. `peer` is an address as the
    /// platform prints it, an IPv6 one possibly with its zone (fe80::1%12); anything else throws.
    pub fn redeem(
        &self,
        token: Option<String>,
        now_ms: f64,
        peer: Option<String>,
    ) -> Result<String, JsError> {
        use lazy_link::RedeemResult::*;
        let peer = peer
            .map(|p| p.parse::<lazy_link::Peer>())
            .transpose()
            .map_err(|()| JsError::new("the peer is not an address"))?;
        let result = match self.inner.redeem(token.as_deref(), time(now_ms)?, peer) {
            Accepted => "accepted",
            WrongToken => "wrongToken",
            Expired => "expired",
            AlreadyUsed => "alreadyUsed",
            Closed => "closed",
        };
        Ok(result.to_owned())
    }

    pub fn end(&self) {
        self.inner.close();
    }
}

#[wasm_bindgen]
extern "C" {
    /// An object with `sign(hash: Uint8Array): Uint8Array`, backed by whatever key store the page has.
    pub type Signer;

    #[wasm_bindgen(method, catch)]
    fn sign(this: &Signer, hash: &[u8]) -> Result<JsValue, JsValue>;

    /// The same with `sign` returning a promise.
    pub type AsyncSigner;

    #[wasm_bindgen(method, catch, js_name = sign)]
    async fn sign_later(this: &AsyncSigner, hash: &[u8]) -> Result<JsValue, JsValue>;
}

fn signature(value: JsValue) -> Result<Vec<u8>, JsValue> {
    value
        .dyn_into::<Uint8Array>()
        .map(|bytes| bytes.to_vec())
        .map_err(|_| JsError::new("the signer must return a Uint8Array").into())
}

#[wasm_bindgen(js_name = signData)]
pub fn sign_data(signer: &Signer, data: &[u8]) -> Result<Vec<u8>, JsValue> {
    signature(signer.sign(&Sha256::digest(data))?)
}

#[wasm_bindgen(js_name = signDataAsync)]
pub async fn sign_data_async(signer: &AsyncSigner, data: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    signature(signer.sign_later(&Sha256::digest(&data)).await?)
}

/// A panic traps: the page sees a RuntimeError and the instance is not used again.
#[wasm_bindgen(js_name = panicNow)]
pub fn panic_now(message: &str) -> u32 {
    panic!("{message}");
}

#[wasm_bindgen(js_name = hashBytes)]
pub fn hash_bytes(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[wasm_bindgen(js_name = patternBytes)]
pub fn pattern_bytes(len: u32) -> Result<Vec<u8>, JsError> {
    if len > MAX_PATTERN_BYTES {
        return Err(JsError::new("at most 256 MiB"));
    }
    Ok((0..len).map(|i| (i % 251) as u8).collect())
}
