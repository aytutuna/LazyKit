#![cfg(feature = "relay")]
//! The transport the app supplies: what the client asks of it, what it does with a pin and how
//! its failures come out. These run on every push, with no network.

mod common;

use async_trait::async_trait;
use lazy_link::relay::{Relay, RelayError, random_id, room_id};
use lazy_link::spki_fingerprint;
use lazy_link::transport::{Connection, Transport, TransportError, open};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// A connection that echoes, with a certificate key the test names.
struct Echo {
    spki: Option<Vec<u8>>,
    inbox: Mutex<Vec<u8>>,
    wake: tokio::sync::Notify,
    closed: Arc<AtomicBool>,
}

struct EchoTransport {
    spki: Option<Vec<u8>>,
    closed: Arc<AtomicBool>,
}

#[async_trait]
impl Transport for EchoTransport {
    async fn connect(
        &self,
        _: &str,
        _: u16,
        _: &str,
    ) -> Result<Arc<dyn Connection>, TransportError> {
        Ok(Arc::new(Echo {
            spki: self.spki.clone(),
            inbox: Mutex::default(),
            wake: tokio::sync::Notify::new(),
            closed: self.closed.clone(),
        }))
    }
}

#[async_trait]
impl Connection for Echo {
    async fn read(&self, max: usize) -> Result<Vec<u8>, TransportError> {
        loop {
            {
                let mut inbox = self.inbox.lock().unwrap();
                if !inbox.is_empty() {
                    let n = inbox.len().min(max);
                    return Ok(inbox.drain(..n).collect());
                }
                if self.closed.load(Ordering::SeqCst) {
                    return Ok(Vec::new());
                }
            }
            self.wake.notified().await;
        }
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        self.inbox.lock().unwrap().extend_from_slice(data);
        self.wake.notify_one();
        Ok(())
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn peer_spki(&self) -> Option<Vec<u8>> {
        self.spki.clone()
    }
}

fn echo(spki: Option<&[u8]>) -> (EchoTransport, Arc<AtomicBool>) {
    let closed = Arc::new(AtomicBool::new(false));
    (
        EchoTransport {
            spki: spki.map(<[u8]>::to_vec),
            closed: closed.clone(),
        },
        closed,
    )
}

#[tokio::test]
async fn bytes_go_both_ways_over_the_connection() {
    let (t, _) = echo(None);
    let s = open(&t, "relay.test", 443, "relay.test", None)
        .await
        .unwrap();
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let (mut rd, mut wr) = tokio::io::split(s);
    let send = data.clone();
    let writer = tokio::spawn(async move {
        wr.write_all(&send).await.unwrap();
    });
    let mut got = vec![0u8; data.len()];
    rd.read_exact(&mut got).await.unwrap();
    assert_eq!(got, data);
    writer.await.unwrap();
}

#[tokio::test]
async fn a_pinned_connection_needs_the_pinned_key() {
    let spki = b"a public key in DER".to_vec();
    let pin = spki_fingerprint(&spki);
    let (t, closed) = echo(Some(&spki));
    assert!(open(&t, "h", 1, "h", Some(&pin)).await.is_ok());
    assert!(!closed.load(Ordering::SeqCst));

    let mut wrong = pin;
    wrong[0] ^= 1;
    let err = open(&t, "h", 1, "h", Some(&wrong)).await.unwrap_err();
    assert!(err.0.contains("pinned"), "{err}");
    assert!(
        closed.load(Ordering::SeqCst),
        "a refused connection is closed"
    );

    let (t, _) = echo(None);
    let err = open(&t, "h", 1, "h", Some(&wrong)).await.unwrap_err();
    assert!(err.0.contains("no certificate"), "{err}");
    assert!(open(&t, "h", 1, "h", None).await.is_ok());
}

#[tokio::test]
async fn the_relay_asks_for_its_own_name_and_port() {
    let reply = "HTTP/1.1 404 Not Found
Content-Length: 0

";
    let t = common::Replies::new(&[reply, reply]);
    let relay = |base: &str| Relay::new(base, &random_id(), &random_id(), t.clone());
    let _ = relay("https://alazy.app/relay/v1")
        .connect(&room_id(b"r"))
        .await;
    let _ = relay("http://127.0.0.1:8787/relay/v1/").status().await;
    assert_eq!(
        *t.names.lock().unwrap(),
        [
            ("alazy.app".to_string(), 443, "alazy.app".to_string()),
            ("127.0.0.1".to_string(), 8787, "127.0.0.1".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_failed_connection_is_a_transport_error() {
    let relay = Relay::new(
        "https://alazy.app/relay/v1",
        &random_id(),
        &random_id(),
        common::Replies::new(&[]),
    );
    assert!(
        matches!(relay.status().await, Err(RelayError::Transport(e)) if e.contains("no more replies"))
    );
    assert!(matches!(
        relay.connect(&room_id(b"r")).await,
        Err(RelayError::Transport(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_connection_is_given_up_on() {
    struct Stall;
    #[async_trait]
    impl Transport for Stall {
        async fn connect(
            &self,
            _: &str,
            _: u16,
            _: &str,
        ) -> Result<Arc<dyn Connection>, TransportError> {
            std::future::pending().await
        }
    }
    let relay = Relay::new(
        "https://alazy.app/relay/v1",
        &random_id(),
        &random_id(),
        Arc::new(Stall),
    );
    let started = tokio::time::Instant::now();
    assert!(matches!(
        relay.status().await,
        Err(RelayError::Transport(_))
    ));
    assert!(started.elapsed() <= Duration::from_secs(16));
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_arrives_after_the_deadline_is_closed() {
    struct Late(EchoTransport);
    #[async_trait]
    impl Transport for Late {
        async fn connect(
            &self,
            host: &str,
            port: u16,
            name: &str,
        ) -> Result<Arc<dyn Connection>, TransportError> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            self.0.connect(host, port, name).await
        }
    }
    let (inner, closed) = echo(None);
    let relay = Relay::new(
        "https://alazy.app/relay/v1",
        &random_id(),
        &random_id(),
        Arc::new(Late(inner)),
    );
    assert!(matches!(
        relay.status().await,
        Err(RelayError::Transport(_))
    ));
    assert!(!closed.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(closed.load(Ordering::SeqCst), "the late connection was left open");
}
