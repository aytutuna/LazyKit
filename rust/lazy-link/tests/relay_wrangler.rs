#![cfg(feature = "relay")]
//! The relay client against the real relay Worker, run with `wrangler dev` (relay/). These tests
//! need it listening, so they are ignored by default; the `relay-client` workflow starts it and
//! runs them with `--ignored`. RELAY_URL names the relay, default http://127.0.0.1:8787/relay/v1.

mod common;

use lazy_link::relay::{
    MAX_FRAME, MAX_STREAMS, Platform, Relay, RelayError, Store, WINDOW_FRAMES, random_id, room_id,
};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

fn url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "http://127.0.0.1:8787/relay/v1".into())
}

// A device that proves itself to the relay the way a computer does.
fn device(group: &str) -> Relay {
    Relay::new(&url(), group, &random_id(), common::tcp())
}

// Two devices of one person: same group, different device ids.
fn pair() -> (Relay, Relay, String) {
    let group = random_id();
    (
        device(&group),
        device(&group),
        room_id(random_id().as_bytes()),
    )
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_transfer_crosses_the_relay_between_two_cores() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let up = pattern(3 * 1024 * 1024 + 123, 1);
    let down = pattern(2 * 1024 * 1024 + 7, 2);

    let (up_len, down_len) = (up.len(), down.len());
    let (up_hash, down_hash) = (Sha256::digest(&up), Sha256::digest(&down));
    let server = tokio::spawn(async move {
        let mut s = host.accept().await.unwrap();
        let mut got = vec![0u8; up_len];
        s.read_exact(&mut got).await.unwrap();
        s.write_all(&down).await.unwrap();
        s.flush().await.unwrap();
        // Hold the stream open until the guest has read everything.
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).await.unwrap();
        Sha256::digest(&got)
    });

    let s = b.connect(&room).await.unwrap();
    let (mut rd, mut wr) = tokio::io::split(s);
    let writer = tokio::spawn(async move {
        wr.write_all(&up).await.unwrap();
        wr.flush().await.unwrap();
        wr
    });
    let mut got = vec![0u8; down_len];
    rd.read_exact(&mut got).await.unwrap();
    let mut wr = writer.await.unwrap();
    wr.shutdown().await.unwrap();
    assert_eq!(Sha256::digest(&got), down_hash);
    assert_eq!(server.await.unwrap(), up_hash);

    // The relay counts what it forwarded against the group, a second after it was sent.
    let want = (up_len + down_len) as u64;
    let mut used = 0;
    for _ in 0..20 {
        used = b.status().await.unwrap().used;
        if used >= want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(used, want);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn the_room_holds_sixteen_streams_and_no_more() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let mut guests = Vec::new();
    let mut ends = Vec::new();
    for _ in 0..MAX_STREAMS {
        // A guest's connection is answered once the host has opened its end.
        let (guest, end) = tokio::join!(b.connect(&room), host.accept());
        guests.push(guest.unwrap());
        ends.push(end.unwrap());
    }
    assert!(matches!(
        b.connect(&room).await,
        Err(RelayError::TooManyStreams)
    ));
    drop(guests.pop());
    // The relay learns of the close from the guest's goodbye; its slot is free a moment later.
    let mut freed = false;
    for _ in 0..20 {
        let (guest, _) = tokio::join!(
            b.connect(&room),
            timeout(Duration::from_millis(500), host.accept())
        );
        if guest.is_ok() {
            freed = true;
            break;
        }
    }
    assert!(freed);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_reader_that_stops_makes_the_writer_wait() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let (guest, stalled) = tokio::join!(b.connect(&room), host.accept());
    let mut guest = guest.unwrap();
    let _stalled = stalled.unwrap();

    let chunk = vec![5u8; MAX_FRAME];
    let mut sent = 0usize;
    while let Ok(res) = timeout(Duration::from_secs(3), guest.write_all(&chunk)).await {
        res.unwrap();
        sent += chunk.len();
        assert!(sent < 64 * 1024 * 1024, "the writer never had to wait");
    }
    // The window of frames plus what the pipe in front of the socket holds.
    assert!(sent <= (WINDOW_FRAMES + 8) * MAX_FRAME, "{sent}");
    assert_eq!(guest.close_code(), None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn another_group_cannot_use_the_room() {
    let (a, _, room) = pair();
    let _host = a.host(&room).await.unwrap();
    let stranger = device(&random_id());
    assert!(matches!(
        stranger.connect(&room).await,
        Err(RelayError::WrongGroup)
    ));
    assert!(matches!(
        stranger.host(&room).await,
        Err(RelayError::WrongGroup)
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn push_tokens_register_and_a_missing_host_is_reported() {
    let (a, b, room) = pair();
    a.register_push(Platform::Fcm, "token").await.unwrap();
    drop(a.host(&room).await.unwrap());
    // The relay sees the host's socket close a moment after the drop.
    let mut res = b.connect(&room).await.map(drop);
    // Only the relay's answer for an empty room ends the wait; a slow close shows up as another
    // error or as a stream that opens and has nobody behind it.
    for _ in 0..20 {
        if matches!(res, Err(RelayError::NoHost { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        res = b.connect(&room).await.map(drop);
    }
    // No push keys are set on a local relay, so nobody is woken.
    assert!(matches!(res, Err(RelayError::NoHost { woken: false })));
    a.drop_push().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_group_starts_unpaid_and_receipts_are_not_checked_yet() {
    let (a, _, _) = pair();
    let status = a.status().await.unwrap();
    assert!(!status.paid && !status.capped);
    assert_eq!(status.used, 0);
    assert_eq!(status.cap, 5_000_000_000);
    assert!(matches!(
        a.pay(Store::Apple, "receipt").await,
        Err(RelayError::NotConfigured)
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_reply_still_arrives_after_the_request_is_shut_down() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let server = tokio::spawn(async move {
        let mut s = host.accept().await.unwrap();
        let mut request = Vec::new();
        s.read_to_end(&mut request).await.unwrap();
        s.write_all(b"reply").await.unwrap();
        s.flush().await.unwrap();
        request
    });
    let mut s = b.connect(&room).await.unwrap();
    s.write_all(b"request").await.unwrap();
    s.shutdown().await.unwrap();
    let mut reply = vec![0u8; 5];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, b"reply");
    assert_eq!(server.await.unwrap(), b"request");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_host_that_writes_and_drops_still_delivers_everything() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let body = pattern(WINDOW_FRAMES * MAX_FRAME + 256 * 1024 + 100_000, 3);
    let want = Sha256::digest(&body);
    let len = body.len();
    let server = tokio::spawn(async move {
        let mut s = host.accept().await.unwrap();
        s.write_all(&body).await.unwrap();
        s.shutdown().await.unwrap();
    });
    let mut s = b.connect(&room).await.unwrap();
    server.await.unwrap();
    // The host is gone; a slow reader still gets every byte and a normal end.
    let mut got = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = timeout(Duration::from_secs(20), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        if got.len() % (64 * 1024) < n {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    assert_eq!(got.len(), len);
    assert_eq!(Sha256::digest(&got), want);
    assert_eq!(s.close_code(), Some(1000));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_host_that_drops_without_shutdown_still_delivers_everything() {
    let (a, b, room) = pair();
    let mut host = a.host(&room).await.unwrap();
    let body = pattern(WINDOW_FRAMES * MAX_FRAME + 100_000, 5);
    let want = Sha256::digest(&body);
    let len = body.len();
    let server = tokio::spawn(async move {
        let mut s = host.accept().await.unwrap();
        s.write_all(&body).await.unwrap();
    });
    let mut s = b.connect(&room).await.unwrap();
    server.await.unwrap();
    let mut got = Vec::new();
    timeout(Duration::from_secs(20), s.read_to_end(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), len);
    assert_eq!(Sha256::digest(&got), want);
}
