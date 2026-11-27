#![cfg(feature = "relay")]
//! TLS to a real host with the system trust store, for the HTTP calls and the WebSocket. It
//! needs network and is ignored in the normal run (`cargo test -- --ignored`), so a failure here that is not a certificate error means the host is
//! unreachable. The relay need not be deployed: any answer from the server past the handshake,
//! a 404 included, shows the certificate was accepted.

use lazy_link::relay::{Relay, RelayError, random_id, room_id};

fn trusted(res: Result<impl Sized, RelayError>) -> bool {
    !matches!(res, Err(RelayError::Transport(_)))
}

#[tokio::test]
#[ignore = "needs the internet and the deployed relay; run on a schedule"]
async fn https_and_wss_accept_the_system_trust_store() {
    let relay = Relay::new("https://alazy.app/relay/v1", &random_id(), &random_id());
    let status = relay.status().await;
    assert!(trusted(status.map(|_| ())), "https call failed");
    let room = room_id(b"tls test");
    let socket = relay.connect(&room).await;
    assert!(trusted(socket.map(|_| ())), "wss connect failed");
}

#[tokio::test]
#[ignore = "needs the internet; run on a schedule"]
async fn an_untrusted_certificate_is_refused() {
    let relay = Relay::new(
        "https://self-signed.badssl.com/relay/v1",
        &random_id(),
        &random_id(),
    );
    // Any network failure is a Transport error too, so the text has to name the certificate.
    for res in [
        relay.connect(&room_id(b"tls test")).await.map(|_| ()),
        relay.status().await.map(|_| ()),
    ] {
        let Err(RelayError::Transport(e)) = res else {
            panic!("the connection was accepted")
        };
        assert!(e.contains("certificate"), "{e}");
    }
}
