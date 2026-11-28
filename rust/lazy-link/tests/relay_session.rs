#![cfg(feature = "relay")]
//! The session a request carries, claiming a group and the reasons the relay refuses with, against
//! a stand-in that records what it was asked.

mod common;

use common::{answer, stand};
use lazy_link::relay::{GroupPlatform, Relay, RelayError, random_id, room_id};

const STATUS: &str = r#"{"week":"w","used":3,"cap":5000000000,"paidUntil":0,"paid":false,"capped":false,"accountToken":"tok-1"}"#;

#[tokio::test]
async fn every_request_carries_the_session() {
    let (url, heads) = stand(vec![answer("200 OK", None, STATUS)]).await;
    let relay =
        Relay::new(&url, &random_id(), &random_id(), common::tcp()).with_session("acct.sess");
    let status = relay.status().await.unwrap();
    assert_eq!(status.account_token, "tok-1");
    assert_eq!(status.used, 3);
    assert!(
        heads.lock().unwrap()[0].contains("Authorization: Bearer acct.sess\r\n"),
        "{:?}",
        heads.lock().unwrap()
    );
}

#[tokio::test]
async fn a_room_handshake_carries_the_session_too() {
    let (url, heads) = stand(vec![answer("401 Unauthorized", Some("sign-in"), "sign in")]).await;
    let relay =
        Relay::new(&url, &random_id(), &random_id(), common::tcp()).with_session("acct.sess");
    assert!(matches!(
        relay.connect(&room_id(b"r")).await,
        Err(RelayError::SignIn)
    ));
    let head = heads.lock().unwrap()[0].to_lowercase();
    assert!(head.contains("authorization: bearer acct.sess"), "{head}");
    assert!(head.contains("upgrade: websocket"), "{head}");
}

#[tokio::test]
async fn claiming_puts_the_platform() {
    let (url, heads) = stand(vec![answer("200 OK", None, STATUS)]).await;
    let group = random_id();
    let relay = Relay::new(&url, &group, &random_id(), common::tcp()).with_session("s");
    relay.claim(GroupPlatform::Apple).await.unwrap();
    let head = heads.lock().unwrap()[0].clone();
    assert!(
        head.starts_with(&format!("PUT /relay/v1/group/{group} HTTP/1.1")),
        "{head}"
    );
}

#[tokio::test]
async fn the_relay_says_why_it_refuses() {
    let (url, _) = stand(vec![
        answer("401 Unauthorized", Some("sign-in"), "sign in"),
        answer("403 Forbidden", Some("wrong-account"), "no"),
        answer("403 Forbidden", Some("wrong-device"), "no"),
        answer("403 Forbidden", Some("attestation-required"), "no"),
        answer("409 Conflict", None, "too many groups"),
    ])
    .await;
    let relay = Relay::new(&url, &random_id(), &random_id(), common::tcp()).with_session("s");
    assert!(matches!(relay.status().await, Err(RelayError::SignIn)));
    assert!(matches!(
        relay.status().await,
        Err(RelayError::WrongAccount)
    ));
    assert!(matches!(relay.status().await, Err(RelayError::WrongDevice)));
    assert!(matches!(
        relay.claim(GroupPlatform::Desktop).await,
        Err(RelayError::AttestationRequired)
    ));
    assert!(matches!(
        relay.claim(GroupPlatform::Desktop).await,
        Err(RelayError::Refused(409))
    ));
}
