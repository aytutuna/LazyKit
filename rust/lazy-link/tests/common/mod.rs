//! What the integration tests supply in place of an app: a transport over plain TCP and an
//! attester that proves a computer the way the relay's desktop check asks.
#![allow(dead_code)]

use async_trait::async_trait;
use lazy_link::relay::{Attestation, Attester, random_id};
use lazy_link::transport::{Connection, Transport, TransportError};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

pub struct Tcp;

pub fn tcp() -> Arc<dyn Transport> {
    Arc::new(Tcp)
}

struct TcpConnection {
    read: Mutex<OwnedReadHalf>,
    write: Mutex<OwnedWriteHalf>,
}

#[async_trait]
impl Transport for Tcp {
    async fn connect(
        &self,
        host: &str,
        port: u16,
        _server_name: &str,
    ) -> Result<Arc<dyn Connection>, TransportError> {
        let (read, write) = TcpStream::connect((host, port))
            .await
            .map_err(|e| TransportError(e.to_string()))?
            .into_split();
        Ok(Arc::new(TcpConnection {
            read: Mutex::new(read),
            write: Mutex::new(write),
        }))
    }
}

#[async_trait]
impl Connection for TcpConnection {
    async fn read(&self, max: usize) -> Result<Vec<u8>, TransportError> {
        let mut buf = vec![0u8; max];
        let n = self
            .read
            .lock()
            .await
            .read(&mut buf)
            .await
            .map_err(|e| TransportError(e.to_string()))?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        self.write
            .lock()
            .await
            .write_all(data)
            .await
            .map_err(|e| TransportError(e.to_string()))
    }

    async fn close(&self) {
        let _ = self.write.lock().await.shutdown().await;
    }
}

/// A computer's install key, signing what the relay's challenge asks for.
pub struct Desktop {
    key: SigningKey,
    pub calls: AtomicUsize,
}

impl Desktop {
    pub fn new() -> Arc<Self> {
        let bytes = [random_id(), random_id()].concat();
        let raw: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&bytes[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        Arc::new(Self {
            key: SigningKey::from_slice(&raw).unwrap(),
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Attester for Desktop {
    async fn attest(&self, group: &str, challenge: &str) -> Result<Attestation, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let message = format!("lazy relay install v1\0{group}\0{challenge}");
        let signature: Signature = self.key.sign(message.as_bytes());
        Ok(Attestation::Desktop {
            public_key: self
                .key
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
            signature: signature.to_bytes().to_vec(),
        })
    }
}

/// A transport that answers every connection with the next canned reply, once the client has
/// written its request, and notes when each connection was made.
pub struct Replies {
    replies: std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>,
    pub connects: std::sync::Mutex<Vec<tokio::time::Instant>>,
    pub names: std::sync::Mutex<Vec<(String, u16, String)>>,
}

impl Replies {
    pub fn new(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: std::sync::Mutex::new(replies.iter().map(|r| r.as_bytes().to_vec()).collect()),
            connects: Default::default(),
            names: Default::default(),
        })
    }
}

struct Canned {
    reply: std::sync::Mutex<Option<Vec<u8>>>,
    written: tokio::sync::Notify,
}

#[async_trait]
impl Transport for Replies {
    async fn connect(
        &self,
        host: &str,
        port: u16,
        server_name: &str,
    ) -> Result<Arc<dyn Connection>, TransportError> {
        self.connects
            .lock()
            .unwrap()
            .push(tokio::time::Instant::now());
        self.names
            .lock()
            .unwrap()
            .push((host.into(), port, server_name.into()));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| TransportError("no more replies".into()))?;
        Ok(Arc::new(Canned {
            reply: std::sync::Mutex::new(Some(reply)),
            written: tokio::sync::Notify::new(),
        }))
    }
}

#[async_trait]
impl Connection for Canned {
    async fn read(&self, _max: usize) -> Result<Vec<u8>, TransportError> {
        if self.reply.lock().unwrap().is_none() {
            return Ok(Vec::new());
        }
        self.written.notified().await;
        Ok(self.reply.lock().unwrap().take().unwrap_or_default())
    }

    async fn write(&self, _data: &[u8]) -> Result<(), TransportError> {
        self.written.notify_one();
        Ok(())
    }

    async fn close(&self) {}
}
