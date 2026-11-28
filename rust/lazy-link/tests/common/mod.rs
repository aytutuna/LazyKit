//! What the integration tests supply in place of an app: a transport over plain TCP.
#![allow(dead_code)]

use async_trait::async_trait;
use lazy_link::transport::{Connection, Transport, TransportError};
use std::sync::Arc;
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
