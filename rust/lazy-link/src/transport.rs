//! How the relay client reaches the network: a [`Transport`] opens a connection and Rust runs the
//! protocol over its bytes. The real one is [`crate::tls::TlsTransport`]; tests bring plain or
//! canned ones.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use async_trait::async_trait;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};

const CHUNK: usize = 16 * 1024;
/// A connection that has delivered nothing for this long is dead: the client pings every 30
/// seconds and the relay answers each ping, so a live one never stays quiet this long.
const IDLE_READ: Duration = Duration::from_secs(90);

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

/// A connection as a byte stream. The end of the connection reads as the end of the stream; a
/// connection that broke (a TLS error, a read that timed out) reads as an error.
#[derive(Debug)]
pub struct TransportStream {
    pipe: DuplexStream,
    failure: Arc<Mutex<Option<String>>>,
}

impl AsyncRead for TransportStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.pipe).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before => {
                match self.failure.lock().unwrap().take() {
                    Some(why) => Poll::Ready(Err(io::Error::other(why))),
                    None => Poll::Ready(Ok(())),
                }
            }
            other => other,
        }
    }
}

impl AsyncWrite for TransportStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.pipe).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_shutdown(cx)
    }
}

/// Connects and returns the connection as a byte stream.
pub async fn open(
    transport: &dyn Transport,
    host: &str,
    port: u16,
    server_name: &str,
) -> Result<TransportStream, TransportError> {
    Ok(pump(transport.connect(host, port, server_name).await?))
}

// TODO: the phone link needs half-close (a request body ends with a write shutdown while the
// reply is still coming); today the end of the pipe's write side closes the whole connection.
// Two tasks carry bytes between the connection and the pipe's far end; they end when either side does.
fn pump(conn: Arc<dyn Connection>) -> TransportStream {
    let (near, far) = tokio::io::duplex(CHUNK * 4);
    let (mut from_pipe, mut to_pipe) = tokio::io::split(far);
    let reading = conn.clone();
    let failure = Arc::new(Mutex::new(None));
    let broke = failure.clone();
    tokio::spawn(async move {
        loop {
            match tokio::time::timeout(IDLE_READ, reading.read(CHUNK)).await {
                Ok(Ok(bytes)) if !bytes.is_empty() => {
                    if to_pipe.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Ok(Ok(_)) => break,
                Ok(Err(e)) => {
                    *broke.lock().unwrap() = Some(e.0);
                    break;
                }
                Err(_) => {
                    *broke.lock().unwrap() = Some("the connection went quiet".into());
                    break;
                }
            }
        }
        // The pipe ends first, so the reader sees the error at once even if closing the connection
        // has to wait behind a write that is stuck; closing then frees a connection nobody uses.
        let _ = to_pipe.shutdown().await;
        reading.close().await;
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
    TransportStream {
        pipe: near,
        failure,
    }
}
