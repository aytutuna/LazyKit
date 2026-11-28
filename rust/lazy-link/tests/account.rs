#![cfg(feature = "relay")]
//! The account client against a stand-in: what it sends and how it reads the service's refusals.

mod common;

use common::{answer, stand};
use lazy_link::account::{Account, AccountError, Bot, Login, Provider, Session, SignIn};
use lazy_link::relay::{Attestation, random_id};
use std::time::Duration;

fn login() -> Login {
    Login {
        app: "lazysend".into(),
        device: random_id(),
        label: Some("Test".into()),
    }
}

fn account(url: &str) -> Account {
    Account::new(&url.replace("/relay/", "/account/"), common::tcp())
}

const SESSION: &str = r#"{"token":"acct.sess","session":"sess","account":"acct"}"#;

fn body_of(head: &str) -> serde_json::Value {
    serde_json::from_str(head.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[tokio::test]
async fn signing_up_sends_the_bot_check_and_reads_the_session() {
    let (url, heads) = stand(vec![
        answer("202 Accepted", None, r#"{"sent":true}"#),
        answer("200 OK", None, SESSION),
    ])
    .await;
    let a = account(&url);
    a.signup("ada@example.com", &Bot::Turnstile("dev".into()))
        .await
        .unwrap();
    let got = a
        .confirm_signup("ada@example.com", "123456", "correct horse", &login())
        .await
        .unwrap();
    assert_eq!(
        got,
        Session {
            token: "acct.sess".into(),
            session: "sess".into(),
            account: "acct".into()
        }
    );
    let heads = heads.lock().unwrap();
    assert!(
        heads[0].starts_with("POST /account/v1/signup HTTP/1.1"),
        "{}",
        heads[0]
    );
    assert_eq!(body_of(&heads[0])["turnstile"], "dev");
    let confirm = body_of(&heads[1]);
    assert_eq!(confirm["code"], "123456");
    assert_eq!(confirm["app"], "lazysend");
    assert_eq!(confirm["label"], "Test");
}

#[tokio::test]
async fn an_attestation_goes_in_as_the_bot_check() {
    let (url, heads) = stand(vec![answer("202 Accepted", None, r#"{"sent":true}"#)]).await;
    let bot = Bot::Attest {
        nonce: "n1".into(),
        proof: Attestation::Google {
            token: "tok".into(),
        },
    };
    account(&url).code("ada@example.com", &bot).await.unwrap();
    let body = body_of(&heads.lock().unwrap()[0]);
    assert_eq!(body["attest"]["nonce"], "n1");
    assert_eq!(body["attest"]["platform"], "google");
    assert_eq!(body["attest"]["token"], "tok");
}

#[tokio::test]
async fn a_session_goes_with_the_calls_that_need_one() {
    let (url, heads) = stand(vec![answer(
        "200 OK",
        None,
        r#"{"email":"ada@example.com"}"#,
    )])
    .await;
    let me = account(&url).with_session("acct.sess").me().await.unwrap();
    assert_eq!(me["email"], "ada@example.com");
    let head = heads.lock().unwrap()[0].clone();
    assert!(head.starts_with("GET /account/v1/me HTTP/1.1"), "{head}");
    assert!(
        head.contains("Authorization: Bearer acct.sess\r\n"),
        "{head}"
    );
}

#[tokio::test]
async fn a_provider_sign_in_may_ask_for_a_link_code() {
    let (url, _) = stand(vec![
        answer("202 Accepted", None, r#"{"linkRequired":true}"#),
        answer("200 OK", None, SESSION),
    ])
    .await;
    let a = account(&url);
    assert_eq!(
        a.social(Provider::Apple, "jwt", "n", &login())
            .await
            .unwrap(),
        SignIn::LinkRequired
    );
    assert!(matches!(
        a.confirm_link("ada@example.com", "123456", &login()).await,
        Ok(SignIn::Done(_))
    ));
}

#[tokio::test]
async fn the_service_says_why_it_refuses() {
    fn json(status: &str, message: &str, retry: Option<u64>) -> String {
        let reply = answer(status, None, &format!(r#"{{"error":"{message}"}}"#));
        match retry {
            Some(secs) => reply.replacen("\r\n", &format!("\r\nRetry-After: {secs}\r\n"), 1),
            None => reply,
        }
    }
    let (url, _) = stand(vec![
        json("401 Unauthorized", "sign in", None),
        json("401 Unauthorized", "wrong email, password or code", None),
        json("401 Unauthorized", "token refused", None),
        json("403 Forbidden", "sign in again first", None),
        json("403 Forbidden", "bot check failed", None),
        json(
            "429 Too Many Requests",
            "too many tries, try again later",
            Some(900),
        ),
        json(
            "429 Too Many Requests",
            "too many codes, try again later",
            Some(600),
        ),
        json(
            "503 Service Unavailable",
            "mail is busy, try again later",
            Some(3600),
        ),
        json("503 Service Unavailable", "slow down", Some(600)),
        json(
            "503 Service Unavailable",
            "the sign-in provider could not be reached",
            Some(30),
        ),
        json("501 Not Implemented", "not configured", None),
        json(
            "422 Unprocessable Entity",
            "the provider gave no verified email",
            None,
        ),
        json("400 Bad Request", "bad-email", None),
    ])
    .await;
    let a = account(&url).with_session("s");
    let r = |e: Result<_, AccountError>| e.map(|_: serde_json::Value| ()).unwrap_err();
    assert!(matches!(r(a.me().await), AccountError::SignIn));
    assert!(matches!(r(a.me().await), AccountError::WrongCredentials));
    assert!(matches!(r(a.me().await), AccountError::TokenRefused));
    assert!(matches!(r(a.me().await), AccountError::NeedFresh));
    assert!(matches!(r(a.me().await), AccountError::BotCheck));
    assert!(matches!(
        r(a.me().await),
        AccountError::Locked { retry_after } if retry_after == Duration::from_secs(900)
    ));
    assert!(matches!(
        r(a.me().await),
        AccountError::Limited { retry_after } if retry_after == Duration::from_secs(600)
    ));
    assert!(matches!(
        r(a.me().await),
        AccountError::Busy { retry_after } if retry_after == Duration::from_secs(3600)
    ));
    assert!(matches!(r(a.me().await), AccountError::SlowDown { .. }));
    assert!(matches!(r(a.me().await), AccountError::Unavailable { .. }));
    assert!(matches!(r(a.me().await), AccountError::NotConfigured));
    assert!(matches!(r(a.me().await), AccountError::NoEmail));
    assert!(matches!(r(a.me().await), AccountError::BadInput(m) if m == "bad-email"));
}
