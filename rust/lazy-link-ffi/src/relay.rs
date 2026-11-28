//! The relay client for Swift, Kotlin and C#. The app brings the encrypted connection (a
//! [`SecureTransport`] built on the system's TLS) and, once, the proof that it is the real app
//! (an [`AppAttester`]); everything else runs in Rust.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::LinkError;
use async_trait::async_trait;
use lazy_link::relay::{self, Attestation};
use lazy_link::transport::{self, Connection, Transport, TransportError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Mutex, Notify};

/// Opens connections with the system's TLS and checks the certificate against its trust store
/// for `server_name`. Each connection is named by a number the transport makes up. A read and
/// a write on one connection may be in flight together. (UniFFI cannot yet hand a foreign
/// object back from a foreign async call, so the connections stay on the app's side.)
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait SecureTransport: Send + Sync {
    async fn connect(&self, host: String, port: u16, server_name: String)
    -> Result<u64, LinkError>;
    /// Up to `max` bytes; an empty answer means the other end closed.
    async fn read(&self, connection: u64, max: u32) -> Result<Vec<u8>, LinkError>;
    /// Sends all of `data`.
    async fn write(&self, connection: u64, data: Vec<u8>) -> Result<(), LinkError>;
    /// Closes the connection and forgets its number.
    async fn close(&self, connection: u64);
    /// The server certificate's SubjectPublicKeyInfo in DER, if the system gives it out.
    fn peer_spki(&self, connection: u64) -> Option<Vec<u8>>;
}

/// What the app proves to the relay about itself.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum AppProof {
    /// App Attest: the key id and the attestation object.
    Apple {
        key_id: Vec<u8>,
        attestation: Vec<u8>,
    },
    /// A Play Integrity token whose nonce is the challenge text.
    Google { token: String },
    /// The install's P-256 public key (65 bytes, uncompressed) and its 64-byte signature (r then
    /// s) over `lazy relay install v1`, a zero byte, the group, a zero byte and the challenge.
    Desktop {
        public_key: Vec<u8>,
        signature: Vec<u8>,
    },
}

/// The app's way to prove itself, asked once when the relay meets a new group.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait AppAttester: Send + Sync {
    /// App Attest takes the UTF-8 bytes of `challenge` as its client data and Play Integrity the
    /// text as its nonce.
    async fn attest(&self, group: String, challenge: String) -> Result<AppProof, LinkError>;
}

struct Foreign(Arc<dyn SecureTransport>);
struct ForeignConnection {
    transport: Arc<dyn SecureTransport>,
    id: u64,
}
struct ForeignAttester(Arc<dyn AppAttester>);

fn failed(e: LinkError) -> TransportError {
    TransportError(e.to_string())
}

#[async_trait]
impl Transport for Foreign {
    async fn connect(
        &self,
        host: &str,
        port: u16,
        server_name: &str,
    ) -> Result<Arc<dyn Connection>, TransportError> {
        let id = self
            .0
            .connect(host.into(), port, server_name.into())
            .await
            .map_err(failed)?;
        Ok(Arc::new(ForeignConnection {
            transport: self.0.clone(),
            id,
        }))
    }
}

#[async_trait]
impl Connection for ForeignConnection {
    async fn read(&self, max: usize) -> Result<Vec<u8>, TransportError> {
        self.transport
            .read(self.id, max.try_into().unwrap_or(u32::MAX))
            .await
            .map_err(failed)
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        self.transport
            .write(self.id, data.to_vec())
            .await
            .map_err(failed)
    }

    async fn close(&self) {
        self.transport.close(self.id).await;
    }

    fn peer_spki(&self) -> Option<Vec<u8>> {
        self.transport.peer_spki(self.id)
    }
}

#[async_trait]
impl relay::Attester for ForeignAttester {
    async fn attest(&self, group: &str, challenge: &str) -> Result<Attestation, String> {
        let proof = self
            .0
            .attest(group.into(), challenge.into())
            .await
            .map_err(|e| e.to_string())?;
        Ok(match proof {
            AppProof::Apple {
                key_id,
                attestation,
            } => Attestation::Apple {
                key_id,
                attestation,
            },
            AppProof::Google { token } => Attestation::Google { token },
            AppProof::Desktop {
                public_key,
                signature,
            } => Attestation::Desktop {
                public_key,
                signature,
            },
        })
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RelayFailure {
    #[error("nobody is waiting in the room (woken: {woken})")]
    NoHost { woken: bool },
    #[error("the group has used its weekly allowance")]
    Capped,
    #[error("too many streams in the room")]
    TooManyStreams,
    #[error("too many requests, retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("the room belongs to another group")]
    WrongGroup,
    #[error("the purchase pays for another group")]
    PurchaseUsed,
    #[error("the relay cannot check receipts of this store yet")]
    NotConfigured,
    #[error("the store rejected the receipt")]
    ReceiptRejected,
    #[error("the relay wants an app check first")]
    AttestationRequired,
    #[error("the relay refused the app check")]
    AttestationRejected,
    #[error("the relay cannot check this platform's app proof yet")]
    AttestationUnavailable,
    #[error("the app could not make its proof: {message}")]
    Attester { message: String },
    #[error("the relay answered {status}")]
    Refused { status: u16 },
    #[error("the relay closed the connection ({code})")]
    Closed { code: u16 },
    #[error("{message}")]
    Transport { message: String },
}

impl From<relay::RelayError> for RelayFailure {
    fn from(e: relay::RelayError) -> Self {
        use relay::RelayError as E;
        match e {
            E::NoHost { woken } => Self::NoHost { woken },
            E::Capped => Self::Capped,
            E::TooManyStreams => Self::TooManyStreams,
            E::RateLimited { retry_after } => Self::RateLimited {
                retry_after_ms: retry_after.as_millis() as u64,
            },
            E::WrongGroup => Self::WrongGroup,
            E::PurchaseUsed => Self::PurchaseUsed,
            E::NotConfigured => Self::NotConfigured,
            E::ReceiptRejected => Self::ReceiptRejected,
            E::AttestationRequired => Self::AttestationRequired,
            E::AttestationRejected => Self::AttestationRejected,
            E::AttestationUnavailable => Self::AttestationUnavailable,
            E::Attester(message) => Self::Attester { message },
            E::Refused(status) => Self::Refused { status },
            E::Closed(code) => Self::Closed { code },
            E::Transport(message) => Self::Transport { message },
        }
    }
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum PushPlatform {
    Apns,
    Fcm,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum PurchaseStore {
    Apple,
    Google,
    Microsoft,
}

/// What the relay has counted for a group this week.
#[derive(Debug, Clone, uniffi::Record)]
pub struct GroupStatus {
    pub used: u64,
    pub cap: u64,
    /// Milliseconds since the epoch; 0 for a group that never paid.
    pub paid_until: u64,
    pub paid: bool,
    pub capped: bool,
}

#[derive(uniffi::Object)]
pub struct Relay {
    inner: relay::Relay,
}

// Calls run on a Tokio runtime of their own: the stream tasks need one.
#[uniffi::export(async_runtime = "tokio")]
impl Relay {
    /// `base` is the relay's address up to the version, `https://alazy.app/relay/v1`.
    #[uniffi::constructor]
    pub fn new(
        base: String,
        group: String,
        device: String,
        transport: Arc<dyn SecureTransport>,
        attester: Option<Arc<dyn AppAttester>>,
    ) -> Arc<Self> {
        let mut inner = relay::Relay::new(&base, &group, &device, Arc::new(Foreign(transport)));
        if let Some(attester) = attester {
            inner = inner.with_attester(Arc::new(ForeignAttester(attester)));
        }
        Arc::new(Self { inner })
    }

    pub async fn status(&self) -> Result<GroupStatus, RelayFailure> {
        let s = self.inner.status().await?;
        Ok(GroupStatus {
            used: s.used,
            cap: s.cap,
            paid_until: s.paid_until,
            paid: s.paid,
            capped: s.capped,
        })
    }

    pub async fn register_push(
        &self,
        platform: PushPlatform,
        token: String,
    ) -> Result<(), RelayFailure> {
        let platform = match platform {
            PushPlatform::Apns => relay::Platform::Apns,
            PushPlatform::Fcm => relay::Platform::Fcm,
        };
        Ok(self.inner.register_push(platform, &token).await?)
    }

    pub async fn drop_push(&self) -> Result<(), RelayFailure> {
        Ok(self.inner.drop_push().await?)
    }

    pub async fn host(&self, room: String) -> Result<Arc<RelayHost>, RelayFailure> {
        Ok(Arc::new(RelayHost {
            inner: Mutex::new(self.inner.host(&room).await?),
        }))
    }

    pub async fn connect(&self, room: String) -> Result<Arc<RelayConnection>, RelayFailure> {
        Ok(RelayConnection::new(self.inner.connect(&room).await?))
    }

    /// Like `connect`, but waits up to `timeout_ms` for a closed host app that the relay woke.
    pub async fn connect_waiting(
        &self,
        room: String,
        timeout_ms: u64,
    ) -> Result<Arc<RelayConnection>, RelayFailure> {
        let stream = self
            .inner
            .connect_waiting(&room, Duration::from_millis(timeout_ms))
            .await?;
        Ok(RelayConnection::new(stream))
    }
}

/// The host's end of a room. Dropping it leaves the room.
#[derive(uniffi::Object)]
pub struct RelayHost {
    inner: Mutex<relay::RelayHost>,
}

#[uniffi::export(async_runtime = "tokio")]
impl RelayHost {
    /// Waits for the next guest. An error means the control connection is gone.
    pub async fn accept(&self) -> Result<Arc<RelayConnection>, RelayFailure> {
        let stream = self.inner.lock().await.accept().await?;
        Ok(RelayConnection::new(stream))
    }
}

/// One connection of the phone link through the relay, as a byte stream.
#[derive(uniffi::Object)]
pub struct RelayConnection {
    read: Mutex<Option<ReadHalf<relay::RelayStream>>>,
    write: Mutex<Option<WriteHalf<relay::RelayStream>>>,
    code: Arc<OnceLock<u16>>,
    closing: Notify,
}

impl RelayConnection {
    fn new(stream: relay::RelayStream) -> Arc<Self> {
        let code = stream.close_cell();
        let (read, write) = tokio::io::split(stream);
        Arc::new(Self {
            read: Mutex::new(Some(read)),
            write: Mutex::new(Some(write)),
            code,
            closing: Notify::new(),
        })
    }
}

fn io_failure(e: std::io::Error) -> LinkError {
    LinkError::Unexpected {
        reason: e.to_string(),
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl RelayConnection {
    /// Up to `max` bytes; empty once the other end has finished or the stream is closed. A close
    /// that was not a normal one is in `close_code`.
    pub async fn read(&self, max: u32) -> Result<Vec<u8>, LinkError> {
        let mut guard = self.read.lock().await;
        let Some(read) = guard.as_mut() else {
            return Ok(Vec::new());
        };
        let mut buf = vec![0u8; (max as usize).clamp(1, 1 << 20)];
        tokio::select! {
            n = read.read(&mut buf) => {
                let n = n.map_err(io_failure)?;
                buf.truncate(n);
                Ok(buf)
            }
            _ = self.closing.notified() => Ok(Vec::new()),
        }
    }

    pub async fn write(&self, data: Vec<u8>) -> Result<(), LinkError> {
        let mut guard = self.write.lock().await;
        let Some(write) = guard.as_mut() else {
            return Err(LinkError::Invalid {
                reason: "the stream is closed".into(),
            });
        };
        write.write_all(&data).await.map_err(io_failure)
    }

    /// Tells the other end nothing more is coming; the reply can still be read.
    pub async fn shutdown(&self) -> Result<(), LinkError> {
        if let Some(write) = self.write.lock().await.as_mut() {
            write.shutdown().await.map_err(io_failure)?;
        }
        Ok(())
    }

    /// Ends the stream in both directions. What was written before is still sent.
    pub async fn close(&self) -> Result<(), LinkError> {
        self.closing.notify_waiters();
        self.read.lock().await.take();
        self.write.lock().await.take();
        Ok(())
    }

    /// How the stream ended, once it has: 1000 for a normal close, 1013 when a reader fell
    /// behind, 4402 at the weekly cap, 1006 when the connection simply dropped.
    pub fn close_code(&self) -> Result<Option<u16>, LinkError> {
        Ok(self.code.get().copied())
    }
}

/// Connects through `transport`, checks the certificate against `pin` (the SHA-256 of its public
/// key) when one is given, and returns the key's SHA-256 as the Rust side computes it. It lets
/// each platform's transport prove that it hands over the right certificate.
#[uniffi::export(async_runtime = "tokio")]
pub async fn probe_connection(
    transport: Arc<dyn SecureTransport>,
    host: String,
    port: u16,
    server_name: String,
    pin: Option<Vec<u8>>,
) -> Result<Vec<u8>, LinkError> {
    let id = transport.connect(host, port, server_name).await?;
    let spki = transport.peer_spki(id);
    let checked = match &pin {
        Some(pin) => transport::check_pin(spki.as_deref(), pin).map_err(|e| LinkError::Invalid {
            reason: e.to_string(),
        }),
        None => Ok(()),
    };
    transport.close(id).await;
    checked?;
    let spki = spki.ok_or(LinkError::Invalid {
        reason: "the system gave no certificate".into(),
    })?;
    Ok(lazy_link::spki_fingerprint(&spki).to_vec())
}
