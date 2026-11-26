//! The wasm-bindgen surface of lazy-link for the browser and Node.
//! Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)

use js_sys::Uint8Array;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

fn error_name(error: lazy_link::PairingUriError) -> &'static str {
    match error {
        lazy_link::PairingUriError::NotPairingCode => "notPairingCode",
        lazy_link::PairingUriError::NewerVersion => "newerVersion",
        lazy_link::PairingUriError::Damaged => "damaged",
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
        let inner = lazy_link::PairingSession::new(opened_ms as i64, token, code_entropy)
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

    /// Returns accepted, wrongToken, expired, alreadyUsed or closed.
    pub fn redeem(&self, token: Option<String>, now_ms: f64, peer: Option<String>) -> String {
        use lazy_link::RedeemResult::*;
        let peer = peer.and_then(|p| p.parse().ok());
        match self.inner.redeem(token.as_deref(), now_ms as i64, peer) {
            Accepted => "accepted",
            WrongToken => "wrongToken",
            Expired => "expired",
            AlreadyUsed => "alreadyUsed",
            Closed => "closed",
        }
        .to_owned()
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

#[wasm_bindgen]
extern "C" {
    /// An object with `sign(hash: Uint8Array): Uint8Array`, backed by whatever key store the page has.
    pub type Signer;

    #[wasm_bindgen(method, catch)]
    fn sign(this: &Signer, hash: &[u8]) -> Result<Uint8Array, JsValue>;

    /// The same with `sign` returning a promise.
    pub type AsyncSigner;

    #[wasm_bindgen(method, catch, js_name = sign)]
    async fn sign_later(this: &AsyncSigner, hash: &[u8]) -> Result<JsValue, JsValue>;
}

#[wasm_bindgen(js_name = signData)]
pub fn sign_data(signer: &Signer, data: &[u8]) -> Result<Vec<u8>, JsValue> {
    Ok(signer.sign(&Sha256::digest(data))?.to_vec())
}

#[wasm_bindgen(js_name = signDataAsync)]
pub async fn sign_data_async(signer: &AsyncSigner, data: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let signature = signer.sign_later(&Sha256::digest(&data)).await?;
    Ok(Uint8Array::new(&signature).to_vec())
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
pub fn pattern_bytes(len: u32) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}
