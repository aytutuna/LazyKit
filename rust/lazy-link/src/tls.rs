//! TLS for the relay client and, pinned, for the phone link: rustls on aws-lc-rs over a TCP
//! socket. The relay's certificate is judged by the system on Apple and Windows
//! (rustls-platform-verifier) and by Mozilla's bundled roots on Android; a phone's self-made one
//! by its public key's fingerprint.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::spki_fingerprint;
use crate::transport::{Connection, Transport, TransportError};
use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, aws_lc_rs};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
#[cfg(not(target_os = "android"))]
use rustls_platform_verifier::{BuilderVerifierExt, Verifier};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use x509_cert::der::{Decode, Encode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an address gets before the next one is tried as well (RFC 8305 suggests 250 ms).
const STAGGER: Duration = Duration::from_millis(250);
/// How long one address may take on its own.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

// Roots an app adds for a relay of its own (development); empty in a shipped app.
static DEVELOPMENT_ROOTS: StdMutex<Vec<CertificateDer<'static>>> = StdMutex::new(Vec::new());

/// Trusts one more certificate authority, given as DER, for the relay connections made from now
/// on. A relay run on a developer's machine has a certificate of its own authority; an app never
/// calls this.
pub fn trust_development_root(der: &[u8]) {
    DEVELOPMENT_ROOTS
        .lock()
        .unwrap()
        .push(CertificateDer::from(der.to_vec()));
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(aws_lc_rs::default_provider())
}

fn failed(e: impl std::fmt::Display) -> TransportError {
    TransportError(e.to_string())
}

/// Connects over TCP and runs TLS with the config it was made with.
pub struct TlsTransport {
    config: Arc<ClientConfig>,
}

impl TlsTransport {
    /// For the relay: Windows' certificate store or the Apple keychain judge its certificate. On
    /// Android the system's trust is not used, since certificate authorities that a user or a
    /// device manager installed there would let the owner of one read the relay traffic (group and
    /// device ids, attestation answers, push tokens); Mozilla's roots, bundled at a pinned version
    /// of `webpki-roots`, judge it instead.
    pub fn system() -> Result<Self, TransportError> {
        let builder = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(failed)?;
        let extra = DEVELOPMENT_ROOTS.lock().unwrap().clone();
        Ok(Self::with_config(system_verified(builder, extra)?))
    }

    /// For a device with a self-made certificate: the connection is made only to the server whose
    /// public key hashes to `pin` (see [`spki_fingerprint`]); names and dates are not looked at.
    pub fn pinned(pin: [u8; 32]) -> Result<Self, TransportError> {
        let provider = provider();
        let verifier = Arc::new(Pinned {
            pin,
            algorithms: provider.signature_verification_algorithms,
        });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(failed)?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        Ok(Self::with_config(config))
    }

    /// For a config made elsewhere (tests with a certificate of their own).
    pub fn with_config(config: ClientConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

#[async_trait]
impl Transport for TlsTransport {
    async fn connect(
        &self,
        host: &str,
        port: u16,
        server_name: &str,
    ) -> Result<Arc<dyn Connection>, TransportError> {
        let name = ServerName::try_from(server_name.to_owned()).map_err(failed)?;
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, connect_tcp(host, port))
            .await
            .map_err(|_| failed("connecting timed out"))??;
        let _ = tcp.set_nodelay(true);
        let tls = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TlsConnector::from(self.config.clone()).connect(name, tcp),
        )
        .await
        .map_err(|_| failed("the TLS handshake timed out"))?
        .map_err(failed)?;
        let (read, write) = tokio::io::split(tls);
        Ok(Arc::new(TlsConnection {
            read: Mutex::new(read),
            write: Mutex::new(write),
        }))
    }
}

struct TlsConnection {
    read: Mutex<ReadHalf<TlsStream<TcpStream>>>,
    write: Mutex<WriteHalf<TlsStream<TcpStream>>>,
}

#[async_trait]
impl Connection for TlsConnection {
    async fn read(&self, max: usize) -> Result<Vec<u8>, TransportError> {
        let mut buf = vec![0u8; max];
        let n = self
            .read
            .lock()
            .await
            .read(&mut buf)
            .await
            .map_err(failed)?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        self.write
            .lock()
            .await
            .write_all(data)
            .await
            .map_err(failed)
    }

    async fn close(&self) {
        let _ = self.write.lock().await.shutdown().await;
    }
}

// Accepts the one certificate whose public key hashes to the pin. The handshake signatures are
// still checked, so only the holder of the key gets through.
#[derive(Debug)]
struct Pinned {
    pin: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

/// The SubjectPublicKeyInfo of a DER certificate, as the fingerprint is taken over.
fn spki_of(cert: &[u8]) -> Option<Vec<u8>> {
    let cert = x509_cert::Certificate::from_der(cert).ok()?;
    cert.tbs_certificate.subject_public_key_info.to_der().ok()
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let spki = spki_of(end_entity)
            .ok_or_else(|| Error::General("the certificate cannot be read".into()))?;
        if bool::from(spki_fingerprint(&spki).as_slice().ct_eq(&self.pin)) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "the server's certificate is not the pinned one".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(target_os = "android")]
fn system_verified(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier>,
    extra: Vec<CertificateDer<'static>>,
) -> Result<ClientConfig, TransportError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for root in extra {
        roots.add(root).map_err(failed)?;
    }
    Ok(builder.with_root_certificates(roots).with_no_client_auth())
}

#[cfg(not(target_os = "android"))]
fn system_verified(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier>,
    extra: Vec<CertificateDer<'static>>,
) -> Result<ClientConfig, TransportError> {
    if extra.is_empty() {
        return Ok(builder
            .with_platform_verifier()
            .map_err(failed)?
            .with_no_client_auth());
    }
    let verifier = Verifier::new_with_extra_roots(extra, provider()).map_err(failed)?;
    Ok(builder
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

// Resolves the name and connects the way RFC 8305 asks, so a network whose IPv6 (or IPv4) path
// drops packets costs a quarter of a second, not the whole connect time.
async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, TransportError> {
    let found: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(failed)?
        .collect();
    race(
        interleaved(found),
        STAGGER,
        ATTEMPT_TIMEOUT,
        TcpStream::connect,
    )
    .await
}

// IPv6 first, then the families alternate, as the RFC orders them.
fn interleaved(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let (mut v6, mut v4): (Vec<_>, Vec<_>) = addrs.into_iter().partition(SocketAddr::is_ipv6);
    v6.reverse();
    v4.reverse();
    let mut out = Vec::new();
    while !v6.is_empty() || !v4.is_empty() {
        out.extend(v6.pop());
        out.extend(v4.pop());
    }
    out
}

// Starts the first address, the next one after `stagger` or as soon as one fails, and so on; the
// first to connect wins and the others are dropped.
async fn race<T, F, Fut>(
    addrs: Vec<SocketAddr>,
    stagger: Duration,
    attempt: Duration,
    connect: F,
) -> Result<T, TransportError>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<T>>,
{
    let mut addrs = addrs.into_iter();
    let mut running = FuturesUnordered::new();
    let mut last = failed("the name has no address");
    let mut start = |running: &mut FuturesUnordered<_>| match addrs.next() {
        Some(addr) => {
            let attempt_future = connect(addr);
            running.push(async move {
                tokio::time::timeout(attempt, attempt_future)
                    .await
                    .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()))
            });
            true
        }
        None => false,
    };
    let mut more = start(&mut running);
    if running.is_empty() {
        return Err(last);
    }
    loop {
        let timer = tokio::time::sleep(stagger);
        tokio::select! {
            done = running.next(), if !running.is_empty() => match done {
                Some(Ok(connected)) => return Ok(connected),
                Some(Err(e)) => {
                    last = failed(e);
                    more = more && start(&mut running);
                    if running.is_empty() {
                        return Err(last);
                    }
                }
                None => return Err(last),
            },
            () = timer, if more => {
                more = start(&mut running);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn v6() -> SocketAddr {
        "[2001:db8::1]:443".parse().unwrap()
    }

    fn v4() -> SocketAddr {
        "192.0.2.1:443".parse().unwrap()
    }

    #[test]
    fn the_families_take_turns_with_ipv6_first() {
        let a: SocketAddr = "[2001:db8::2]:443".parse().unwrap();
        let b: SocketAddr = "192.0.2.2:443".parse().unwrap();
        assert_eq!(interleaved(vec![v4(), b, v6(), a]), [v6(), v4(), a, b]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_address_that_drops_packets_is_overtaken_after_the_stagger() {
        let started = tokio::time::Instant::now();
        let got = race(
            vec![v6(), v4()],
            STAGGER,
            ATTEMPT_TIMEOUT,
            |addr| async move {
                if addr.is_ipv6() {
                    std::future::pending().await
                } else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(addr)
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(got, v4());
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_moves_on_at_once() {
        let started = tokio::time::Instant::now();
        let got = race(
            vec![v6(), v4()],
            STAGGER,
            ATTEMPT_TIMEOUT,
            |addr| async move {
                if addr.is_ipv6() {
                    Err(Error::from(ErrorKind::ConnectionRefused))
                } else {
                    Ok(addr)
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(got, v4());
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[tokio::test(start_paused = true)]
    async fn every_address_failing_reports_the_last_error_and_a_silent_one_times_out() {
        let err = race(vec![v6(), v4()], STAGGER, ATTEMPT_TIMEOUT, |_| async {
            std::future::pending::<std::io::Result<()>>().await
        })
        .await
        .unwrap_err();
        assert!(err.0.to_lowercase().contains("timed out"), "{err}");
        let none = race(Vec::new(), STAGGER, ATTEMPT_TIMEOUT, |_| async {
            Ok::<(), Error>(())
        })
        .await
        .unwrap_err();
        assert!(none.0.contains("no address"), "{none}");
    }
}
