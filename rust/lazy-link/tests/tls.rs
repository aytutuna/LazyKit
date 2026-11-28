#![cfg(feature = "relay")]
//! The TLS layer against a server of its own on the loopback: the pin, the system verifier and
//! the relay's HTTP over it.

use lazy_link::relay::{Relay, RelayError, random_id};
use lazy_link::spki_fingerprint;
use lazy_link::tls::TlsTransport;
use lazy_link::transport::open;
use rcgen::PublicKeyData;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const STATUS: &str =
    r#"{"week":"2026-W48","used":7,"cap":5000000000,"paidUntil":0,"paid":false,"capped":false}"#;

struct Server {
    port: u16,
    cert: CertificateDer<'static>,
    spki: Vec<u8>,
}

// Echoes what it is sent, except when the first bytes are an HTTP request, which it answers with a
// group status.
async fn serve() -> Server {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert = made.cert.der().clone();
    let spki = made.signing_key.subject_public_key_info();
    let key = PrivateKeyDer::try_from(made.signing_key.serialize_der()).unwrap();
    let config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.clone()], key)
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                while let Ok(n) = tls.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let http = buf.starts_with(b"GET ");
                    let reply = if http {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{STATUS}",
                            STATUS.len()
                        )
                        .into_bytes()
                    } else {
                        buf[..n].to_vec()
                    };
                    if tls.write_all(&reply).await.is_err() || http {
                        let _ = tls.shutdown().await;
                        break;
                    }
                }
            });
        }
    });
    Server { port, cert, spki }
}

fn trusting(cert: &CertificateDer<'static>) -> TlsTransport {
    let mut roots = RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsTransport::with_config(config)
}

#[tokio::test]
async fn a_pinned_connection_carries_bytes_both_ways() {
    let server = serve().await;
    let pin = spki_fingerprint(&server.spki);
    let tls = TlsTransport::pinned(pin).unwrap();
    // The name is not the certificate's: a pin ignores it.
    let mut s = open(&tls, "127.0.0.1", server.port, "elsewhere.test")
        .await
        .unwrap();
    s.write_all(b"hello over a pin").await.unwrap();
    let mut got = [0u8; 16];
    s.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"hello over a pin");
}

#[tokio::test]
async fn a_pinned_connection_needs_the_pinned_key() {
    let server = serve().await;
    let mut pin = spki_fingerprint(&server.spki);
    pin[0] ^= 1;
    let tls = TlsTransport::pinned(pin).unwrap();
    let err = open(&tls, "127.0.0.1", server.port, "localhost")
        .await
        .expect_err("the wrong key was accepted");
    assert!(err.0.contains("pinned"), "{err}");
}

#[tokio::test]
async fn the_system_does_not_trust_a_certificate_nobody_vouches_for() {
    let server = serve().await;
    let tls = TlsTransport::system().unwrap();
    assert!(
        open(&tls, "127.0.0.1", server.port, "localhost")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_name_has_to_match_the_certificate() {
    let server = serve().await;
    let tls = trusting(&server.cert);
    assert!(
        open(&tls, "127.0.0.1", server.port, "localhost")
            .await
            .is_ok()
    );
    let err = open(&tls, "127.0.0.1", server.port, "relay.example")
        .await
        .expect_err("a certificate for another name was accepted");
    assert!(err.0.to_lowercase().contains("certificate"), "{err}");
}

#[tokio::test]
async fn the_relay_client_speaks_http_over_tls() {
    let server = serve().await;
    let relay = Relay::new(
        &format!("https://localhost:{}/relay/v1", server.port),
        &random_id(),
        &random_id(),
        Arc::new(trusting(&server.cert)),
    );
    let status = relay.status().await.unwrap();
    assert_eq!(status.used, 7);
    let nobody = Relay::new(
        &format!("https://localhost:{}/relay/v1", server.port),
        &random_id(),
        &random_id(),
        Arc::new(TlsTransport::system().unwrap()),
    );
    assert!(matches!(
        nobody.status().await,
        Err(RelayError::Transport(_))
    ));
}
