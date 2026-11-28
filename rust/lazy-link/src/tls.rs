//! TLS for the relay client and, pinned, for the phone link: rustls on aws-lc-rs over a TCP
//! socket. The relay's certificate is judged by the system (rustls-platform-verifier); a phone's
//! self-made one by its public key's fingerprint.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::spki_fingerprint;
use crate::transport::{Connection, Transport, TransportError};
use async_trait::async_trait;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, aws_lc_rs};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use x509_cert::der::{Decode, Encode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// For a host with a certificate the system trusts, such as the relay: Windows' certificate
    /// store, the Apple keychain, Android's trust manager. On Android the app has to call the
    /// library's init once first, or the handshake fails.
    pub fn system() -> Result<Self, TransportError> {
        let config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(failed)?
            .with_platform_verifier()
            .map_err(failed)?
            .with_no_client_auth();
        Ok(Self::with_config(config))
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
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| failed("connecting timed out"))?
            .map_err(failed)?;
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
