//! How the relay client reaches the network: the app opens the encrypted connection with its
//! system's own TLS and Rust runs the protocol over the bytes. The same traits will carry the
//! pinned phone link, where [`open`] also checks the certificate's fingerprint.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::spki_fingerprint;
use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

const CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

/// Opens connections with the system's TLS: SChannel on Windows, Network.framework on Apple
/// systems, Android's own sockets, the browser on the web. `server_name` is what the certificate
/// must be valid for; the system checks it against its trust store, unless the app is pinning.
///
/// The caller may give up on a `connect` (a timeout) without cancelling it: the call still runs
/// to its end and the connection it returns is closed on the caller's side. A transport should
/// bound its own connect time, and must not assume the future is dropped.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn connect(
        &self,
        host: &str,
        port: u16,
        server_name: &str,
    ) -> Result<Arc<dyn Connection>, TransportError>;
}

/// One encrypted connection. Reads and writes arrive at the same time from different tasks.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Up to `max` bytes; an empty answer means the other end closed.
    async fn read(&self, max: usize) -> Result<Vec<u8>, TransportError>;
    /// Sends all of `data`.
    async fn write(&self, data: &[u8]) -> Result<(), TransportError>;
    async fn close(&self);
    /// The server certificate's SubjectPublicKeyInfo in DER, when the system gives it out. It is
    /// what a pin is checked against.
    fn peer_spki(&self) -> Option<Vec<u8>>;
}

/// Connects and returns the connection as a byte stream. With `pin`, the connection is dropped
/// unless the server's public key hashes to it.
pub async fn open(
    transport: &dyn Transport,
    host: &str,
    port: u16,
    server_name: &str,
    pin: Option<&[u8]>,
) -> Result<DuplexStream, TransportError> {
    let conn = transport.connect(host, port, server_name).await?;
    if let Some(pin) = pin
        && let Err(e) = check_pin(conn.peer_spki().as_deref(), pin)
    {
        conn.close().await;
        return Err(e);
    }
    Ok(pump(conn))
}

/// Fails unless `spki`, a certificate's public key in DER, hashes to `pin`.
pub fn check_pin(spki: Option<&[u8]>, pin: &[u8]) -> Result<(), TransportError> {
    let spki = spki.ok_or_else(|| {
        TransportError("the system gave no certificate to check the pin against".into())
    })?;
    if bool::from(spki_fingerprint(spki).as_slice().ct_eq(pin)) {
        Ok(())
    } else {
        Err(TransportError(
            "the server's certificate is not the pinned one".into(),
        ))
    }
}

// Two tasks carry bytes between the connection and the pipe's far end; they end when either side does.
fn pump(conn: Arc<dyn Connection>) -> DuplexStream {
    let (near, far) = tokio::io::duplex(CHUNK * 4);
    let (mut from_pipe, mut to_pipe) = tokio::io::split(far);
    let reading = conn.clone();
    tokio::spawn(async move {
        loop {
            match reading.read(CHUNK).await {
                Ok(bytes) if !bytes.is_empty() => {
                    if to_pipe.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = to_pipe.shutdown().await;
    });
    tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match from_pipe.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    if conn.write(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        conn.close().await;
    });
    near
}
