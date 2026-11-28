//! What the integration tests supply in place of an app: a transport over plain TCP.
#![allow(dead_code)]

use async_trait::async_trait;
use lazy_link::transport::{Connection, Transport, TransportError};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
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

// Answers each connection with the next canned reply and keeps the request heads it read.
pub async fn stand(replies: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/relay/v1", listener.local_addr().unwrap());
    let heads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = heads.clone();
    tokio::spawn(async move {
        for reply in replies {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = 0;
            loop {
                let n = tcp.read(&mut buf[got..]).await.unwrap();
                got += n;
                if n == 0 || buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..got]).into_owned());
            tcp.write_all(reply.as_bytes()).await.unwrap();
            let _ = tcp.shutdown().await;
        }
    });
    (url, heads)
}

pub fn answer(status: &str, reason: Option<&str>, body: &str) -> String {
    let reason = reason.map_or(String::new(), |r| format!("x-relay-reason: {r}\r\n"));
    format!(
        "HTTP/1.1 {status}\r\n{reason}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
