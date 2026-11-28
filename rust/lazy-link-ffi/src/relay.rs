//! The relay client for Swift, Kotlin and C#. Rust does everything, TLS included; the app only
//! brings, once, the proof that it is the real app (an [`AppAttester`]).
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::LinkError;
use async_trait::async_trait;
use lazy_link::relay::{self, Attestation};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Mutex, Notify};

/// What the app proves to the service about itself: App Attest on Apple devices, Play Integrity on Android.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum AppProof {
    /// App Attest: the key id and the attestation object.
    Apple {
        key_id: Vec<u8>,
        attestation: Vec<u8>,
    },
    /// A Play Integrity token whose nonce is the challenge text.
    Google { token: String },
}

/// The app's way to prove itself, asked once when the relay meets a new group.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait AppAttester: Send + Sync {
    /// App Attest takes the UTF-8 bytes of `challenge` as its client data and Play Integrity the
    /// text as its nonce.
    async fn attest(&self, group: String, challenge: String) -> Result<AppProof, LinkError>;
}

struct ForeignAttester(Arc<dyn AppAttester>);

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
    #[error("sign in first")]
    SignIn,
    #[error("the group belongs to another account")]
    WrongAccount,
    #[error("the session is for another device")]
    WrongDevice,
    #[error("the app could not make its proof: {reason}")]
    Attester { reason: String },
    #[error("the relay answered {status}")]
    Refused { status: u16 },
    #[error("the relay closed the connection ({code})")]
    Closed { code: u16 },
    #[error("{reason}")]
    Transport { reason: String },
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
            E::SignIn => Self::SignIn,
            E::WrongAccount => Self::WrongAccount,
            E::WrongDevice => Self::WrongDevice,
            E::Attester(reason) => Self::Attester { reason },
            E::Refused(status) => Self::Refused { status },
            E::Closed(code) => Self::Closed { code },
            E::Transport(reason) => Self::Transport { reason },
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
    /// The account's token, which a store purchase carries so the relay knows whose it is.
    pub account_token: String,
}

/// The kind of device a group is made on: Apple and Google groups also need the app check.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum GroupPlatform {
    Apple,
    Google,
    Desktop,
}

fn status_of(s: relay::GroupStatus) -> GroupStatus {
    GroupStatus {
        used: s.used,
        cap: s.cap,
        paid_until: s.paid_until,
        paid: s.paid,
        capped: s.capped,
        account_token: s.account_token,
    }
}

#[derive(uniffi::Object)]
pub struct Relay {
    inner: relay::Relay,
}

// A relay at another address, up to the version. Only in the native test builds.
#[cfg(feature = "dev-roots")]
#[uniffi::export]
impl Relay {
    #[uniffi::constructor]
    pub fn new_at(
        base: String,
        group: String,
        device: String,
        session: String,
        attester: Option<Arc<dyn AppAttester>>,
    ) -> Result<Arc<Self>, RelayFailure> {
        Self::made(
            relay::Relay::at(&base, &group, &device)?,
            &session,
            attester,
        )
    }
}

impl Relay {
    fn made(
        inner: relay::Relay,
        session: &str,
        attester: Option<Arc<dyn AppAttester>>,
    ) -> Result<Arc<Self>, RelayFailure> {
        let mut inner = inner.with_session(session);
        if let Some(attester) = attester {
            inner = inner.with_attester(Arc::new(ForeignAttester(attester)));
        }
        Ok(Arc::new(Self { inner }))
    }
}

// Calls run on a Tokio runtime of their own: the stream tasks need one.
#[uniffi::export(async_runtime = "tokio")]
impl Relay {
    /// The relay at alazy.app. `session` is the token an account sign-in gave, made for this
    /// `device`.
    #[uniffi::constructor]
    pub fn new(
        group: String,
        device: String,
        session: String,
        attester: Option<Arc<dyn AppAttester>>,
    ) -> Result<Arc<Self>, RelayFailure> {
        Self::made(relay::Relay::system(&group, &device)?, &session, attester)
    }

    /// Makes the signed-in account the owner of this group, before its first use.
    pub async fn claim(&self, platform: GroupPlatform) -> Result<GroupStatus, RelayFailure> {
        let platform = match platform {
            GroupPlatform::Apple => relay::GroupPlatform::Apple,
            GroupPlatform::Google => relay::GroupPlatform::Google,
            GroupPlatform::Desktop => relay::GroupPlatform::Desktop,
        };
        Ok(status_of(self.inner.claim(platform).await?))
    }

    pub async fn status(&self) -> Result<GroupStatus, RelayFailure> {
        Ok(status_of(self.inner.status().await?))
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

    /// Ends the stream in both directions. What was written before is still sent. (Not `close`:
    /// Kotlin gives every object one.)
    pub async fn finish(&self) -> Result<(), LinkError> {
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
