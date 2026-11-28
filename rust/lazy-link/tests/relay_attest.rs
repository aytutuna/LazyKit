#![cfg(feature = "relay")]
//! The app check from the client's side, against a stand-in relay, and the waiting for a woken
//! host. Nothing here needs a network.

mod common;

use async_trait::async_trait;
use lazy_link::relay::{Attestation, Attester, Platform, Relay, RelayError, random_id, room_id};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// A relay that wants every group verified once. `attest_status` is what it answers an attestation.
struct Stand {
    verified: AtomicBool,
    attest_status: u16,
    seen: Mutex<Vec<String>>,
    attests: Mutex<Vec<serde_json::Value>>,
}

impl Stand {
    fn new(attest_status: u16) -> Arc<Self> {
        Arc::new(Self {
            verified: AtomicBool::new(false),
            attest_status,
            seen: Mutex::default(),
            attests: Mutex::default(),
        })
    }
}

fn reply(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

const WANTED: &str = "x-relay-reason: attestation-required\r\n";

async fn serve(l: TcpListener, stand: Arc<Stand>) {
    loop {
        let (mut tcp, _) = l.accept().await.unwrap();
        let stand = stand.clone();
        tokio::spawn(async move {
            let mut peek = [0u8; 4096];
            let n = tcp.peek(&mut peek).await.unwrap();
            let line = String::from_utf8_lossy(&peek[..n])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            stand.seen.lock().unwrap().push(line.clone());
            if line.contains("role=") {
                if stand.verified.load(Ordering::SeqCst) {
                    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    use futures_util::StreamExt;
                    while ws.next().await.is_some() {}
                } else {
                    let _ = read_request(&mut tcp).await;
                    let _ = tcp
                        .write_all(reply("403 Forbidden", WANTED, "").as_bytes())
                        .await;
                }
                return;
            }
            let (_, body) = read_request(&mut tcp).await;
            let answer = if line.contains("/challenge") {
                reply("200 OK", "", "{\"challenge\":\"abc123\"}")
            } else if line.contains("/attest") {
                stand
                    .attests
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&body).unwrap());
                if stand.attest_status == 204 {
                    stand.verified.store(true, Ordering::SeqCst);
                }
                reply(&format!("{} X", stand.attest_status), "", "")
            } else if stand.verified.load(Ordering::SeqCst) {
                reply("204 No Content", "", "")
            } else {
                reply("403 Forbidden", WANTED, "")
            };
            let _ = tcp.write_all(answer.as_bytes()).await;
            let _ = tcp.shutdown().await;
        });
    }
}

async fn read_request(tcp: &mut TcpStream) -> (String, String) {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let n = tcp.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let want = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|v| v.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if body.len() >= want || n == 0 {
                return (head.to_string(), body.to_string());
            }
        }
        if n == 0 {
            return (String::new(), String::new());
        }
    }
}

async fn stand(status: u16) -> (Arc<Stand>, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/relay/v1", l.local_addr().unwrap());
    let stand = Stand::new(status);
    tokio::spawn(serve(l, stand.clone()));
    (stand, url)
}

struct Fake {
    calls: AtomicUsize,
    answer: Result<Attestation, String>,
    asked: Mutex<Vec<(String, String)>>,
}

impl Fake {
    fn new(answer: Result<Attestation, String>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            answer,
            asked: Mutex::default(),
        })
    }
}

#[async_trait]
impl Attester for Fake {
    async fn attest(&self, group: &str, challenge: &str) -> Result<Attestation, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.asked
            .lock()
            .unwrap()
            .push((group.into(), challenge.into()));
        self.answer.clone()
    }
}

fn apple() -> Result<Attestation, String> {
    Ok(Attestation::Apple {
        key_id: vec![1, 2, 3],
        attestation: vec![251, 255],
    })
}

#[tokio::test]
async fn a_new_group_proves_itself_and_is_let_in() {
    let (stand, url) = stand(204).await;
    let attester = Fake::new(apple());
    let group = random_id();
    let relay =
        Relay::new(&url, &group, &random_id(), common::tcp()).with_attester(attester.clone());
    let _stream = relay.connect(&room_id(b"r")).await.unwrap();
    assert_eq!(attester.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *attester.asked.lock().unwrap(),
        [(group, "abc123".to_string())]
    );
    assert_eq!(
        stand.attests.lock().unwrap()[0],
        serde_json::json!({
            "platform": "apple", "keyId": "AQID", "attestation": "-_8", "challenge": "abc123"
        })
    );

    // Verified groups are not asked again.
    let _again = relay.connect(&room_id(b"r")).await.unwrap();
    assert_eq!(attester.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_proof_is_sent_as_the_platform_names_it() {
    for (proof, platform) in [
        (
            Attestation::Google {
                token: "tok".into(),
            },
            "google",
        ),
        (
            Attestation::Desktop {
                public_key: vec![4; 65],
                signature: vec![9; 64],
            },
            "desktop",
        ),
    ] {
        let (stand, url) = stand(204).await;
        let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp())
            .with_attester(Fake::new(Ok(proof)));
        relay.connect(&room_id(b"r")).await.unwrap();
        assert_eq!(stand.attests.lock().unwrap()[0]["platform"], platform);
    }
}

#[tokio::test]
async fn push_registration_proves_itself_too() {
    let (stand, url) = stand(204).await;
    let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp())
        .with_attester(Fake::new(apple()));
    relay.register_push(Platform::Fcm, "token").await.unwrap();
    assert_eq!(stand.attests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn without_an_attester_the_relay_says_what_it_wants() {
    let (_stand, url) = stand(204).await;
    let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp());
    assert!(matches!(
        relay.connect(&room_id(b"r")).await,
        Err(RelayError::AttestationRequired)
    ));
    assert!(matches!(
        relay.register_push(Platform::Fcm, "t").await,
        Err(RelayError::AttestationRequired)
    ));
}

#[tokio::test]
async fn a_refused_proof_is_reported_as_such() {
    for (status, ok) in [(422u16, true), (501, false)] {
        let (_stand, url) = stand(status).await;
        let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp())
            .with_attester(Fake::new(apple()));
        let err = relay.connect(&room_id(b"r")).await.err().unwrap();
        if ok {
            assert!(matches!(err, RelayError::AttestationRejected), "{err}");
        } else {
            assert!(matches!(err, RelayError::AttestationUnavailable), "{err}");
        }
    }
    let (_stand, url) = stand(429).await;
    let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp())
        .with_attester(Fake::new(apple()));
    assert!(matches!(
        relay.connect(&room_id(b"r")).await,
        Err(RelayError::Refused(429))
    ));
}

#[tokio::test]
async fn an_app_that_cannot_make_its_proof_says_why() {
    let (_stand, url) = stand(204).await;
    let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp())
        .with_attester(Fake::new(Err("no key store".into())));
    assert!(matches!(
        relay.connect(&room_id(b"r")).await,
        Err(RelayError::Attester(e)) if e == "no key store"
    ));
}

#[tokio::test(start_paused = true)]
async fn an_app_that_never_answers_its_proof_is_given_up_on() {
    struct Silent;
    #[async_trait]
    impl Attester for Silent {
        async fn attest(&self, _: &str, _: &str) -> Result<Attestation, String> {
            std::future::pending().await
        }
    }
    let wanted = "HTTP/1.1 403 Forbidden\r\nx-relay-reason: attestation-required\r\nContent-Length: 0\r\n\r\n";
    let challenge = "HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"challenge\":\"c\"}";
    let t = common::Replies::new(&[wanted, challenge]);
    let relay = Relay::new("https://alazy.app/relay/v1", &random_id(), &random_id(), t)
        .with_attester(Arc::new(Silent));
    assert!(matches!(
        relay.register_push(Platform::Fcm, "t").await,
        Err(RelayError::Attester(_))
    ));
}

const NOT_FOUND_WOKEN: &str =
    "HTTP/1.1 404 Not Found\r\nx-relay-woken: 1\r\nContent-Length: 0\r\n\r\n";
const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";

#[tokio::test(start_paused = true)]
async fn a_woken_host_is_tried_after_a_second_and_then_every_four() {
    let t = common::Replies::new(&[NOT_FOUND_WOKEN; 8]);
    let relay = Relay::new(
        "https://alazy.app/relay/v1",
        &random_id(),
        &random_id(),
        t.clone(),
    );
    let start = tokio::time::Instant::now();
    let res = relay
        .connect_waiting(&room_id(b"r"), Duration::from_secs(12))
        .await;
    assert!(matches!(res, Err(RelayError::NoHost { woken: true })));
    let at: Vec<u64> = t
        .connects
        .lock()
        .unwrap()
        .iter()
        .map(|i| (*i - start).as_secs())
        .collect();
    assert_eq!(at, [0, 1, 5, 9]);
}

#[tokio::test(start_paused = true)]
async fn a_huge_timeout_neither_panics_nor_waits_for_nothing() {
    let t = common::Replies::new(&[NOT_FOUND_WOKEN, NOT_FOUND]);
    let relay = Relay::new("https://alazy.app/relay/v1", &random_id(), &random_id(), t);
    let res = relay.connect_waiting(&room_id(b"r"), Duration::MAX).await;
    assert!(matches!(res, Err(RelayError::NoHost { woken: false })));
}
