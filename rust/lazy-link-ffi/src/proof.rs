//! Calls that exist to prove the binding mechanics on every platform: a signer that stays on the
//! native side, an async call, a panic and big buffers.

use crate::LinkError;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Signs a digest with a key that never leaves the system's key store.
#[uniffi::export(with_foreign)]
pub trait Signer: Send + Sync {
    fn sign(&self, hash: Vec<u8>) -> Result<Vec<u8>, LinkError>;
}

/// The same, for a key store that answers later (a hardware token, a prompt).
#[uniffi::export(with_foreign)]
#[async_trait::async_trait]
pub trait AsyncSigner: Send + Sync {
    async fn sign(&self, hash: Vec<u8>) -> Result<Vec<u8>, LinkError>;
}

/// Hashes `data` and has `signer` sign the hash.
#[uniffi::export]
pub fn sign_data(signer: Arc<dyn Signer>, data: &[u8]) -> Result<Vec<u8>, LinkError> {
    signer.sign(Sha256::digest(data).to_vec())
}

#[uniffi::export]
pub async fn sign_data_async(
    signer: Arc<dyn AsyncSigner>,
    data: Vec<u8>,
) -> Result<Vec<u8>, LinkError> {
    signer.sign(Sha256::digest(&data).to_vec()).await
}

#[uniffi::export]
pub fn panic_now(message: String) -> Result<u32, LinkError> {
    panic!("{message}");
}

/// The SHA-256 of `data` as lower-case hex.
#[uniffi::export]
pub fn hash_bytes(data: &[u8]) -> Result<String, LinkError> {
    Ok(Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

const MAX_PATTERN_BYTES: u32 = 256 * 1024 * 1024;

/// `len` bytes of the pattern `i % 251`, the one the shared test data uses for large files.
#[uniffi::export]
pub fn pattern_bytes(len: u32) -> Result<Vec<u8>, LinkError> {
    if len > MAX_PATTERN_BYTES {
        return Err(LinkError::Invalid {
            reason: "at most 256 MiB".into(),
        });
    }
    Ok((0..len).map(|i| (i % 251) as u8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_bytes_has_a_cap() {
        assert_eq!(pattern_bytes(3).unwrap(), [0, 1, 2]);
        assert_eq!(pattern_bytes(252).unwrap()[251], 0);
        assert!(pattern_bytes(MAX_PATTERN_BYTES + 1).is_err());
        assert!(pattern_bytes(u32::MAX).is_err());
    }
}
