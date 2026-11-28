#![cfg(feature = "relay")]
//! The relay client against the real relay Worker, run with `wrangler dev` (relay/). These tests
//! need it listening, so they are ignored by default; the `relay-client` workflow starts it and
//! runs them with `--ignored`. RELAY_URL names the relay, default http://127.0.0.1:8787/relay/v1.

mod common;

use lazy_link::account::{Account, Bot, Login, Session};
use lazy_link::relay::{
    GroupPlatform, MAX_FRAME, MAX_STREAMS, Platform, Relay, RelayError, Store, WINDOW_FRAMES,
    random_id, room_id,
};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

fn url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "http://127.0.0.1:8787/relay/v1".into())
}

fn account_url() -> String {
    std::env::var("ACCOUNT_URL").unwrap_or_else(|_| url().replace("/relay/", "/account/"))
}

// Where the relay's dev run posts its mail (relay/scripts/dev-mail.mjs).
fn sink() -> String {
    std::env::var("MAIL_SINK").unwrap_or_else(|_| "127.0.0.1:8790".into())
}

const PASSWORD: &str = "correct horse battery";

// The code in the newest mail to `email`, asked of the sink.
async fn code_for(email: &str) -> String {
    for _ in 0..50 {
        let mut tcp = TcpStream::connect(sink()).await.unwrap();
        let request =
            format!("GET /code?to={email} HTTP/1.1\r\nHost: sink\r\nConnection: close\r\n\r\n");
        tcp.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        tcp.read_to_string(&mut answer).await.unwrap();
        if answer.starts_with("HTTP/1.1 200") {
            return answer.rsplit("\r\n\r\n").next().unwrap().trim().to_owned();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no mail came for {email}");
}

fn login(device: &str) -> Login {
    Login {
        app: "lazysend".into(),
        device: device.into(),
        label: None,
    }
}

// A new account, signed up through the service with the mail read from the dev sink; its first
// device's session.
async fn sign_up(device: &str) -> (String, Session) {
    let email = format!("{}@example.com", &random_id()[..12]);
    let account = Account::new(&account_url(), common::tcp());
    account
        .signup(&email, &Bot::Turnstile("dev".into()))
        .await
        .unwrap();
    let code = code_for(&email).await;
    let session = account
        .confirm_signup(&email, &code, PASSWORD, &login(device))
        .await
        .unwrap();
    (email, session)
}

async fn sign_in(email: &str, device: &str) -> Session {
    Account::new(&account_url(), common::tcp())
        .signin_password(email, PASSWORD, &login(device))
        .await
        .unwrap()
}

fn relay_of(group: &str, device: &str, session: &Session) -> Relay {
    Relay::new(&url(), group, device, common::tcp()).with_session(&session.token)
}

struct Person {
    email: String,
    device: String,
    session: Session,
    group: String,
    relay: Relay,
}

// A person with a group and one device that has claimed it.
async fn person() -> Person {
    let (device, group) = (random_id(), random_id());
    let (email, session) = sign_up(&device).await;
    let relay = relay_of(&group, &device, &session);
    relay.claim(GroupPlatform::Desktop).await.unwrap();
    Person {
        email,
        device,
        session,
        group,
        relay,
    }
}

// Two devices of one person: same account and group, different devices and sessions.
async fn pair() -> (Relay, Relay, String) {
    let p = person().await;
    let device = random_id();
    let b = relay_of(&p.group, &device, &sign_in(&p.email, &device).await);
    (p.relay, b, room_id(random_id().as_bytes()))
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn a_transfer_crosses_the_relay_between_two_cores() {
    let (a, b, room) = pair().await;
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
    let (a, b, room) = pair().await;
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
    let (a, b, room) = pair().await;
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
    let (a, _, room) = pair().await;
    let _host = a.host(&room).await.unwrap();
    let stranger = person().await.relay;
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
async fn the_relay_checks_who_is_asking() {
    let mine = person().await;
    let other = person().await;
    let room = room_id(random_id().as_bytes());
    // No session.
    let nobody = Relay::new(&url(), &mine.group, &mine.device, common::tcp());
    assert!(matches!(nobody.status().await, Err(RelayError::SignIn)));
    assert!(matches!(
        nobody.connect(&room).await,
        Err(RelayError::SignIn)
    ));
    // Another account's session on my group.
    let intruder = relay_of(&mine.group, &other.device, &other.session);
    assert!(matches!(
        intruder.status().await,
        Err(RelayError::WrongAccount)
    ));
    assert!(matches!(
        intruder.claim(GroupPlatform::Desktop).await,
        Err(RelayError::WrongAccount)
    ));
    assert!(matches!(
        intruder.host(&room).await,
        Err(RelayError::WrongAccount)
    ));
    // My session on a device it was not made for.
    let borrowed = relay_of(&mine.group, &random_id(), &mine.session);
    assert!(matches!(
        borrowed.host(&room).await,
        Err(RelayError::WrongDevice)
    ));
    // My own device is let in, and the account token is there for a purchase to carry.
    let status = mine.relay.status().await.unwrap();
    assert_eq!(status.account_token.len(), 36);
    assert!(mine.relay.host(&room).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the relay on RELAY_URL"]
async fn push_tokens_register_and_a_missing_host_is_reported() {
    let (a, b, room) = pair().await;
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
    let (a, _, _) = pair().await;
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
    let (a, b, room) = pair().await;
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
    let (a, b, room) = pair().await;
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
    let (a, b, room) = pair().await;
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
