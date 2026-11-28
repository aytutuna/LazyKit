//! How the relay client reaches the network: a [`Transport`] opens a connection and Rust runs the
//! protocol over its bytes. The real one is [`crate::tls::TlsTransport`]; tests bring plain or
//! canned ones.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;
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

/// Opens connections. `server_name` is what the certificate must be valid for.
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
}

/// Connects and returns the connection as a byte stream.
pub async fn open(
    transport: &dyn Transport,
    host: &str,
    port: u16,
    server_name: &str,
) -> Result<DuplexStream, TransportError> {
    Ok(pump(transport.connect(host, port, server_name).await?))
}

// TODO: the phone link needs half-close (a request body ends with a write shutdown while the
// reply is still coming); today the end of the pipe's write side closes the whole connection.
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
