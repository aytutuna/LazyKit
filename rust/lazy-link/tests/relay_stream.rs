#![cfg(feature = "relay")]
#![allow(clippy::result_large_err)] // the handshake callback's error type is the library's
//! The relay client against small stand-in servers, so no relay has to run.

mod data;

use futures_util::{SinkExt, StreamExt};
use lazy_link::relay::{MAX_FRAME, Relay, RelayError, Store, WINDOW_FRAMES, random_id, room_id};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
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

#[test]
fn rooms_are_named_as_the_data_says() {
    let data = data::load("rooms.json");
    for case in data["rooms"].as_array().unwrap() {
        let hex = case["secretHex"].as_str().unwrap();
        let secret: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(
            room_id(&secret),
            case["room"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
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
        refused_with("503 Service Unavailable", "Retry-After: 60\r\n").await,
        RelayError::RateLimited { retry_after } if retry_after == Duration::from_secs(60)
    ));
    assert!(matches!(
        refused_with("503 Service Unavailable", "").await,
        RelayError::Refused(503)
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

#[tokio::test]
async fn a_rate_limited_request_says_when_to_retry() {
    let (l, relay) = listener().await;
    tokio::spawn(refuse(l, "503 Service Unavailable", "Retry-After: 30\r\n"));
    assert!(matches!(
        relay.status().await,
        Err(RelayError::RateLimited { retry_after }) if retry_after == Duration::from_secs(30)
    ));
}

#[tokio::test]
async fn shutdown_is_a_half_close_and_the_reply_still_arrives() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        let Some(Ok(Message::Binary(b))) = ws.next().await else {
            panic!("expected the request")
        };
        assert_eq!(&b[..], b"ask");
        let Some(Ok(Message::Text(t))) = ws.next().await else {
            panic!("expected the end of input")
        };
        assert_eq!(t.as_str(), "eof");
        ws.send(Message::binary(b"answer".to_vec())).await.unwrap();
        ws.send(Message::text("eof")).await.unwrap();
        // The client closed nothing, so the next frame is its goodbye when it is dropped.
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) if t.as_str() == "ack" => {}
                other => return matches!(other, Some(Ok(Message::Close(_))) | None),
            }
        }
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    s.write_all(b"ask").await.unwrap();
    s.shutdown().await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert_eq!(got, b"answer");
    assert_eq!(s.close_code(), None);
    drop(s);
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn the_other_ends_half_close_leaves_the_writer_working() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(Message::text("eof")).await.unwrap();
        let Some(Ok(Message::Binary(b))) = ws.next().await else {
            panic!("expected data after the half-close")
        };
        b.to_vec()
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert!(got.is_empty());
    s.write_all(b"still here").await.unwrap();
    assert_eq!(server.await.unwrap(), b"still here");
}

#[tokio::test]
async fn a_dropped_stream_that_is_given_up_on_closes_with_1011() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        while let Some(Ok(m)) = ws.next().await {
            match m {
                Message::Close(frame) => return frame.map(|f| u16::from(f.code)),
                Message::Text(t) if t.as_str() == "ping" => {
                    ws.send(Message::text("pong")).await.unwrap();
                }
                _ => {}
            }
        }
        None
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    s.write_all(&vec![1u8; WINDOW_FRAMES * MAX_FRAME + 256 * 1024])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(s);
    tokio::time::pause();
    assert_eq!(server.await.unwrap(), Some(1011));
}

#[tokio::test]
async fn a_stream_dropped_after_shutdown_still_says_goodbye() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        while let Some(Ok(m)) = ws.next().await {
            if let Message::Close(_) = m {
                return true;
            }
        }
        false
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    s.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(s);
    assert!(
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
    );
}

#[tokio::test]
async fn the_guest_names_its_own_device() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let device = random_id();
    let relay = Relay::new(
        &format!("http://{}/relay/v1", l.local_addr().unwrap()),
        &random_id(),
        &device,
    );
    let server = tokio::spawn(async move {
        let query = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let q = query.clone();
        let _ws = tokio_tungstenite::accept_hdr_async(
            l.accept().await.unwrap().0,
            move |req: &Request, res: Response| {
                *q.lock().unwrap() = req.uri().query().unwrap_or_default().to_string();
                Ok(res)
            },
        )
        .await
        .unwrap();
        query.lock().unwrap().clone()
    });
    let _s = relay.connect(&room_id(b"r")).await.unwrap();
    let query = server.await.unwrap();
    assert!(query.contains("role=guest"), "{query}");
    assert!(query.contains(&format!("device={device}")), "{query}");
}

// The control socket announces two guests; the first one's stream is refused as expired.
#[tokio::test]
async fn accept_skips_a_guest_that_already_gave_up() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut control = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        control.send(Message::text("open gone")).await.unwrap();
        control.send(Message::text("open here")).await.unwrap();
        let mut streams = Vec::new();
        for _ in 0..2 {
            let ws = tokio_tungstenite::accept_hdr_async(
                l.accept().await.unwrap().0,
                |req: &Request, res: Response| {
                    if req.uri().query().unwrap_or_default().contains("id=gone") {
                        Err(tokio_tungstenite::tungstenite::http::Response::builder()
                            .status(404)
                            .body(None)
                            .unwrap())
                    } else {
                        Ok(res)
                    }
                },
            )
            .await;
            streams.push(ws);
        }
        let mut ok = streams.into_iter().find_map(Result::ok).unwrap();
        ok.send(Message::binary(vec![5])).await.unwrap();
        let _ = control.next().await;
    });
    let mut host = relay.host(&room_id(b"r")).await.unwrap();
    let mut s = host.accept().await.unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(b, [5]);
}

#[tokio::test]
async fn a_connect_that_stalls_is_given_up_on() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let (_tcp, _) = l.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let res = timeout(
        Duration::from_secs(5),
        relay.connect_waiting(&room_id(b"r"), Duration::from_secs(1)),
    )
    .await
    .expect("connect_waiting kept waiting past its own timeout");
    assert!(matches!(res, Err(RelayError::Transport(_))));
}

// The next two accept the upgrade and then never answer, as a path that has died does.
#[tokio::test]
async fn a_host_whose_connection_goes_quiet_is_told_it_is_closed() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let _ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let mut host = relay.host(&room_id(b"r")).await.unwrap();
    tokio::time::pause();
    assert!(matches!(host.accept().await, Err(RelayError::Closed(1006))));
}

#[tokio::test]
async fn a_stream_whose_connection_goes_quiet_ends_with_1006() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let _ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    tokio::time::pause();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert!(got.is_empty());
    assert_eq!(s.close_code(), Some(1006));
}

#[tokio::test]
async fn a_writer_stops_when_the_other_end_has_not_taken_its_frames() {
    let (l, relay) = listener().await;
    let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
    tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Binary(_))) => seen_tx.send(()).unwrap(),
                    Some(Ok(_)) => {}
                    _ => return,
                },
                Some(n) = ack_rx.recv() => {
                    for _ in 0..n {
                        ws.send(Message::text("ack")).await.unwrap();
                    }
                }
            }
        }
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let chunk = vec![1u8; MAX_FRAME];
    // The server reads everything at once, so only the missing acks can stop the writer.
    let writer = tokio::spawn(async move {
        for _ in 0..4 * WINDOW_FRAMES {
            s.write_all(&chunk).await.unwrap();
        }
        s
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut got = 0;
    while seen.try_recv().is_ok() {
        got += 1;
    }
    assert_eq!(got, WINDOW_FRAMES);
    assert!(!writer.is_finished());
    // Each batch answers only frames that were sent, as a real reader does.
    for _ in 0..3 {
        ack_tx.send(WINDOW_FRAMES).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _s = timeout(Duration::from_secs(10), writer)
        .await
        .expect("the writer did not go on after the acks")
        .unwrap();
}

#[tokio::test]
async fn an_ack_for_nothing_sent_ends_the_stream() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        for _ in 0..1000 {
            if ws.send(Message::text("ack")).await.is_err() {
                return;
            }
        }
        let _ = ws.next().await;
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let mut got = Vec::new();
    timeout(Duration::from_secs(10), s.read_to_end(&mut got))
        .await
        .expect("the stream was not ended")
        .unwrap();
    assert_eq!(s.close_code(), Some(1008));
}

#[tokio::test]
async fn a_frame_given_to_the_reader_is_acknowledged() {
    let (l, relay) = listener().await;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        for i in 0..3u8 {
            ws.send(Message::binary(vec![i; 10])).await.unwrap();
        }
        let mut acks = 0;
        while acks < 3 {
            match ws.next().await {
                Some(Ok(Message::Text(t))) if t.as_str() == "ack" => acks += 1,
                Some(Ok(_)) => {}
                _ => break,
            }
        }
        acks
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    let mut got = [0u8; 30];
    s.read_exact(&mut got).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn a_purchase_for_another_group_has_its_own_error() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let (mut tcp, _) = l.accept().await.unwrap();
        // Read the body too, or closing with it unread resets the connection.
        let mut got = Vec::new();
        let mut buf = [0u8; 2048];
        while !got.ends_with(b"}") {
            let n = tcp.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        let reply = b"HTTP/1.1 409 Conflict
Content-Length: 0
Connection: close

";
        tcp.write_all(reply).await.unwrap();
        tcp.shutdown().await.unwrap();
    });
    assert!(matches!(
        relay.pay(Store::Apple, "r").await,
        Err(RelayError::PurchaseUsed)
    ));
}

#[tokio::test]
async fn a_rate_limited_wait_is_kept_up_until_the_deadline() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let (mut tcp, _) = l.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = tcp.read(&mut buf).await.unwrap();
        tcp.write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        tcp.shutdown().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        ws.send(Message::binary(vec![4])).await.unwrap();
        let _ = ws.next().await;
    });
    let mut s = relay
        .connect_waiting(&room_id(b"r"), Duration::from_secs(10))
        .await
        .unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(b, [4]);
}

#[tokio::test]
async fn a_slow_handshake_does_not_hold_up_the_next_guest() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut control = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        control.send(Message::text("open slow")).await.unwrap();
        control.send(Message::text("open quick")).await.unwrap();
        for _ in 0..2 {
            let tcp = l.accept().await.unwrap().0;
            tokio::spawn(async move {
                // The request line names the guest, and peeking leaves it for the handshake.
                let mut buf = [0u8; 512];
                let n = tcp.peek(&mut buf).await.unwrap();
                let slow = String::from_utf8_lossy(&buf[..n]).contains("id=slow");
                if slow {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                if let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await {
                    let _ = ws
                        .send(Message::binary(vec![if slow { 1 } else { 2 }]))
                        .await;
                    let _ = ws.next().await;
                }
            });
        }
        let _ = control.next().await;
    });
    let mut host = relay.host(&room_id(b"r")).await.unwrap();
    let mut s = timeout(Duration::from_secs(2), host.accept())
        .await
        .expect("the quick guest waited for the slow one")
        .unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(b, [2]);
}

// A host writes a reply and drops the stream at once, as it would with TCP. The server hands
// out acks slowly, so the credit and the pipe are both full when the stream goes.
#[tokio::test]
async fn data_written_before_a_drop_is_all_sent() {
    let (l, relay) = listener().await;
    let total = (WINDOW_FRAMES * MAX_FRAME + 256 * 1024 + 5000) * 2;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        let mut got = 0;
        let mut pending = 0;
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        got += b.len();
                        pending += 1;
                    }
                    Some(Ok(Message::Close(_))) => return (got, true),
                    Some(Ok(_)) => {}
                    _ => return (got, false),
                },
                _ = tokio::time::sleep(Duration::from_millis(20)), if pending > 0 => {
                    ws.send(Message::text("ack")).await.unwrap();
                    pending -= 1;
                }
            }
        }
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    s.write_all(&vec![3u8; total]).await.unwrap();
    s.shutdown().await.unwrap();
    drop(s);
    let (got, closed) = timeout(Duration::from_secs(30), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, total);
    assert!(closed);
}

// One side reads nothing until its own upload is done while the other sends more than the
// pipe holds; the upload still goes through because acks are not queued behind that data.
#[tokio::test]
async fn an_upload_goes_on_while_incoming_data_waits_to_be_read() {
    let (l, relay) = listener().await;
    let upload = 4 * WINDOW_FRAMES * MAX_FRAME;
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        for _ in 0..20 {
            ws.send(Message::binary(vec![8u8; MAX_FRAME]))
                .await
                .unwrap();
        }
        let mut got = 0;
        while got < upload {
            match ws.next().await {
                Some(Ok(Message::Binary(b))) => {
                    got += b.len();
                    ws.send(Message::text("ack")).await.unwrap();
                }
                Some(Ok(_)) => {}
                _ => break,
            }
        }
        got
    });
    let mut s = relay.connect(&room_id(b"r")).await.unwrap();
    timeout(Duration::from_secs(10), s.write_all(&vec![1u8; upload]))
        .await
        .expect("the upload stalled behind unread data")
        .unwrap();
    assert_eq!(server.await.unwrap(), upload);
}

#[tokio::test]
async fn a_huge_retry_after_does_not_panic() {
    let (l, relay) = listener().await;
    tokio::spawn(refuse(
        l,
        "503 Service Unavailable",
        "Retry-After: 18446744073709551615\r\n",
    ));
    let res = relay
        .connect_waiting(&room_id(b"r"), Duration::from_secs(2))
        .await;
    assert!(matches!(res, Err(RelayError::RateLimited { .. })));
}

#[tokio::test]
async fn accept_skips_a_join_the_relay_refuses() {
    let (l, relay) = listener().await;
    tokio::spawn(async move {
        let mut control = tokio_tungstenite::accept_async(l.accept().await.unwrap().0)
            .await
            .unwrap();
        control.send(Message::text("open limited")).await.unwrap();
        control.send(Message::text("open fine")).await.unwrap();
        let mut ok = None;
        for _ in 0..2 {
            let ws = tokio_tungstenite::accept_hdr_async(
                l.accept().await.unwrap().0,
                |req: &Request, res: Response| {
                    if req.uri().query().unwrap_or_default().contains("id=limited") {
                        Err(tokio_tungstenite::tungstenite::http::Response::builder()
                            .status(503)
                            .header("retry-after", "60")
                            .body(None)
                            .unwrap())
                    } else {
                        Ok(res)
                    }
                },
            )
            .await;
            if let Ok(ws) = ws {
                ok = Some(ws);
            }
        }
        let mut ok = ok.unwrap();
        ok.send(Message::binary(vec![6])).await.unwrap();
        let _ = control.next().await;
    });
    let mut host = relay.host(&room_id(b"r")).await.unwrap();
    let mut s = host.accept().await.unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(b, [6]);
}
