#![cfg(feature = "relay")]
//! The relay client against small stand-in servers, so no relay has to run.

use futures_util::{SinkExt, StreamExt};
use lazy_link::relay::{MAX_FRAME, Relay, RelayError, random_id, room_id};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

async fn listener() -> (TcpListener, Relay) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = Relay::new(
        &format!("http://{}/relay/v1", l.local_addr().unwrap()),
        &random_id(),
        &random_id(),
    );
    (l, relay)
}

fn close(code: u16) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    }))
}

#[test]
fn ids_have_the_shape_the_relay_expects() {
    let id = random_id();
    assert_eq!(id.len(), 32);
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_ne!(id, random_id());
    let room = room_id(b"secret");
    assert_eq!(room.len(), 64);
    assert_eq!(room, room_id(b"secret"));
    assert_ne!(room, room_id(b"secreT"));
}

#[tokio::test]
async fn writes_go_out_in_frames_the_relay_accepts() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        let mut got = Vec::new();
        let mut biggest = 0;
        while got.len() < 300_000 {
            let Some(Ok(Message::Binary(b))) = ws.next().await else {
                panic!("expected a frame")
            };
            biggest = biggest.max(b.len());
            got.extend_from_slice(&b);
        }
        (got, biggest)
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    s.write_all(&data).await.unwrap();
    let (got, biggest) = server.await.unwrap();
    assert_eq!(got, data);
    assert!(biggest <= MAX_FRAME);
}

#[tokio::test]
async fn frames_arrive_as_a_byte_stream() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(Message::binary(vec![1, 2, 3])).await.unwrap();
        ws.send(Message::binary(vec![4, 5])).await.unwrap();
        ws.send(close(1000)).await.unwrap();
        let _ = ws.next().await;
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert_eq!(got, [1, 2, 3, 4, 5]);
    assert_eq!(s.close_code(), Some(1000));
}

#[tokio::test]
async fn a_cut_off_stream_reports_why_and_stops_writes() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(close(1013)).await.unwrap();
        let _ = ws.next().await;
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert!(got.is_empty());
    assert_eq!(s.close_code(), Some(1013));
    let chunk = vec![0u8; MAX_FRAME];
    let failed = timeout(Duration::from_secs(5), async {
        while s.write_all(&chunk).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        failed.is_ok(),
        "writes kept succeeding after the relay closed the stream"
    );
}

#[tokio::test]
async fn a_writer_waits_for_a_peer_that_does_not_read() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let _ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let chunk = vec![7u8; MAX_FRAME];
    let mut written = 0usize;
    // Memory stays bounded: the socket buffers and the stream's own window fill, then a write
    // waits instead of queueing without end.
    while let Ok(res) = timeout(Duration::from_millis(500), s.write_all(&chunk)).await {
        res.unwrap();
        written += chunk.len();
        assert!(written < 512 * 1024 * 1024, "writes never waited");
    }
}

// Answers a websocket upgrade with a plain HTTP status, as the relay does when it refuses.
async fn refuse(l: TcpListener, status: &str, extra: &str) {
    let (mut tcp, _) = l.accept().await.unwrap();
    let mut buf = [0u8; 2048];
    let _ = tcp.read(&mut buf).await.unwrap();
    let reply =
        format!("HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n");
    tcp.write_all(reply.as_bytes()).await.unwrap();
    tcp.shutdown().await.unwrap();
}

async fn refused_with(status: &'static str, extra: &'static str) -> RelayError {
    let (l, relay) = listener().await;
    tokio::spawn(refuse(l, status, extra));
    relay
        .connect(&room_id(b"r"))
        .await
        .err()
        .expect("the connection was refused")
}

#[tokio::test]
async fn refusals_become_their_own_errors() {
    assert!(matches!(
        refused_with("404 Not Found", "").await,
        RelayError::NoHost { woken: false }
    ));
    assert!(matches!(
        refused_with("404 Not Found", "x-relay-woken: 1\r\n").await,
        RelayError::NoHost { woken: true }
    ));
    assert!(matches!(
        refused_with("402 Payment Required", "").await,
        RelayError::Capped
    ));
    assert!(matches!(
        refused_with("403 Forbidden", "").await,
        RelayError::WrongGroup
    ));
    assert!(matches!(
        refused_with("429 Too Many Requests", "").await,
        RelayError::TooManyStreams
    ));
    assert!(matches!(
        refused_with("500 Internal Server Error", "").await,
        RelayError::Refused(500)
    ));
}

#[tokio::test]
async fn a_woken_host_is_waited_for() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        refuse_once(&l).await;
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(Message::binary(vec![9])).await.unwrap();
        let _ = ws.next().await;
    });
    let mut s = relay
        .connect_waiting(&room_id(b"r"), Duration::from_secs(10))
        .await
        .unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(b, [9]);
}

async fn refuse_once(l: &TcpListener) {
    let (mut tcp, _): (TcpStream, _) = l.accept().await.unwrap();
    let mut buf = [0u8; 2048];
    let _ = tcp.read(&mut buf).await.unwrap();
    tcp.write_all(b"HTTP/1.1 404 Not Found\r\nx-relay-woken: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tcp.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_host_that_is_replaced_says_so() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(close(1012)).await.unwrap();
        let _ = ws.next().await;
    });
    let mut host = relay.host(&room_id(b"r")).await.unwrap();
    assert!(matches!(host.accept().await, Err(RelayError::Closed(1012))));
}
