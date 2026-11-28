//! The account service for Swift, Kotlin and C#: one call per route. The screens belong to the
//! app; the answers that are JSON documents (the account, its sessions, an export) come back as
//! JSON text.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::relay::AppProof;
use lazy_link::account::{self, AccountError};
use lazy_link::relay::Attestation;
use std::sync::Arc;

/// Who is signing in: the Lazy app (`lazysend`, `lazyprint`, ...), this device's id (32 lowercase
/// hex characters, the same the relay calls use) and an optional name for the session list.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Login {
    pub app: String,
    pub device: String,
    pub label: Option<String>,
}

/// A session the service opened. `token` goes to `Account.with_session` and `Relay.new`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Session {
    pub token: String,
    /// The session's short id, as `sessions` lists it and `revoke` takes it.
    pub id: String,
    /// The account as JSON: its address, whether it has a password, its linked identities.
    pub account_json: String,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum SignIn {
    Done {
        session: Session,
    },
    /// The identity belongs to an account that signed up another way: a code went to that
    /// account's address, and `confirm_link` with it finishes the sign-in.
    LinkRequired,
}

/// The proof that a request comes from a person's app, for the routes that send mail.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum Bot {
    /// A Turnstile token.
    Turnstile { token: String },
    /// An App Attest or Play Integrity proof made for a nonce from `Account.nonce`.
    Attest { nonce: String, proof: AppProof },
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum Provider {
    Apple,
    Google,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AccountFailure {
    #[error("sign in first")]
    SignIn,
    #[error("wrong email, password or code")]
    WrongCredentials,
    #[error("the sign-in token was refused")]
    TokenRefused,
    #[error("sign in again before this change")]
    NeedFresh,
    #[error("the bot check failed")]
    BotCheck,
    #[error("too many wrong tries, retry in {retry_after_ms} ms")]
    Locked { retry_after_ms: u64 },
    #[error("too many codes, retry in {retry_after_ms} ms")]
    Limited { retry_after_ms: u64 },
    #[error("mail is busy, retry in {retry_after_ms} ms")]
    Busy { retry_after_ms: u64 },
    #[error("too many requests, retry in {retry_after_ms} ms")]
    SlowDown { retry_after_ms: u64 },
    #[error("the provider is unreachable, retry in {retry_after_ms} ms")]
    Unavailable { retry_after_ms: u64 },
    #[error("the service is not set up for this")]
    NotConfigured,
    #[error("the provider gave no verified email")]
    NoEmail,
    #[error("bad request: {reason}")]
    BadInput { reason: String },
    #[error("the service answered {status}")]
    Refused { status: u16 },
    #[error("{reason}")]
    Transport { reason: String },
}

impl From<AccountError> for AccountFailure {
    fn from(e: AccountError) -> Self {
        use AccountError as E;
        let ms = |d: std::time::Duration| d.as_millis() as u64;
        match e {
            E::SignIn => Self::SignIn,
            E::WrongCredentials => Self::WrongCredentials,
            E::TokenRefused => Self::TokenRefused,
            E::NeedFresh => Self::NeedFresh,
            E::BotCheck => Self::BotCheck,
            E::Locked { retry_after } => Self::Locked {
                retry_after_ms: ms(retry_after),
            },
            E::Limited { retry_after } => Self::Limited {
                retry_after_ms: ms(retry_after),
            },
            E::Busy { retry_after } => Self::Busy {
                retry_after_ms: ms(retry_after),
            },
            E::SlowDown { retry_after } => Self::SlowDown {
                retry_after_ms: ms(retry_after),
            },
            E::Unavailable { retry_after } => Self::Unavailable {
                retry_after_ms: ms(retry_after),
            },
            E::NotConfigured => Self::NotConfigured,
            E::NoEmail => Self::NoEmail,
            E::BadInput(reason) => Self::BadInput { reason },
            E::Refused(status) => Self::Refused { status },
            E::Transport(reason) => Self::Transport { reason },
        }
    }
}

fn login(l: Login) -> account::Login {
    account::Login {
        app: l.app,
        device: l.device,
        label: l.label,
    }
}

fn bot(b: Bot) -> account::Bot {
    match b {
        Bot::Turnstile { token } => account::Bot::Turnstile(token),
        Bot::Attest { nonce, proof } => account::Bot::Attest {
            nonce,
            proof: match proof {
                AppProof::Apple {
                    key_id,
                    attestation,
                } => Attestation::Apple {
                    key_id,
                    attestation,
                },
                AppProof::Google { token } => Attestation::Google { token },
            },
        },
    }
}

fn session(s: account::Session) -> Session {
    Session {
        token: s.token,
        id: s.session,
        account_json: s.account.to_string(),
    }
}

fn signed_in(s: account::SignIn) -> SignIn {
    match s {
        account::SignIn::Done(s) => SignIn::Done {
            session: session(s),
        },
        account::SignIn::LinkRequired => SignIn::LinkRequired,
    }
}

#[derive(uniffi::Object)]
pub struct Account {
    inner: account::Account,
}

#[uniffi::export(async_runtime = "tokio")]
impl Account {
    /// `base` is the service's address up to the version, `https://alazy.app/account/v1`.
    /// `session` is a token from a sign-in, for the calls that need one; sign-in calls need none.
    #[uniffi::constructor]
    pub fn new(base: String, session: Option<String>) -> Result<Arc<Self>, AccountFailure> {
        let mut inner = account::Account::system(&base)?;
        if let Some(token) = session {
            inner = inner.with_session(&token);
        }
        Ok(Arc::new(Self { inner }))
    }

    pub async fn nonce(&self) -> Result<String, AccountFailure> {
        Ok(self.inner.nonce().await?)
    }

    pub async fn signup(&self, email: String, bot_check: Bot) -> Result<(), AccountFailure> {
        Ok(self.inner.signup(&email, &bot(bot_check)).await?)
    }

    pub async fn code(&self, email: String, bot_check: Bot) -> Result<(), AccountFailure> {
        Ok(self.inner.code(&email, &bot(bot_check)).await?)
    }

    pub async fn confirm_signup(
        &self,
        email: String,
        code: String,
        password: String,
        who: Login,
    ) -> Result<Session, AccountFailure> {
        let s = self
            .inner
            .confirm_signup(&email, &code, &password, &login(who))
            .await?;
        Ok(session(s))
    }

    pub async fn signin_password(
        &self,
        email: String,
        password: String,
        who: Login,
    ) -> Result<Session, AccountFailure> {
        let s = self
            .inner
            .signin_password(&email, &password, &login(who))
            .await?;
        Ok(session(s))
    }

    pub async fn signin_code(
        &self,
        email: String,
        code: String,
        who: Login,
    ) -> Result<Session, AccountFailure> {
        let s = self.inner.signin_code(&email, &code, &login(who)).await?;
        Ok(session(s))
    }

    pub async fn social(
        &self,
        provider: Provider,
        id_token: String,
        nonce: String,
        who: Login,
    ) -> Result<SignIn, AccountFailure> {
        let provider = match provider {
            Provider::Apple => account::Provider::Apple,
            Provider::Google => account::Provider::Google,
        };
        let s = self
            .inner
            .social(provider, &id_token, &nonce, &login(who))
            .await?;
        Ok(signed_in(s))
    }

    pub async fn confirm_link(
        &self,
        email: String,
        code: String,
        who: Login,
    ) -> Result<SignIn, AccountFailure> {
        let s = self.inner.confirm_link(&email, &code, &login(who)).await?;
        Ok(signed_in(s))
    }

    pub async fn signout(&self) -> Result<(), AccountFailure> {
        Ok(self.inner.signout().await?)
    }

    /// The account as JSON.
    pub async fn me(&self) -> Result<String, AccountFailure> {
        Ok(self.inner.me().await?.to_string())
    }

    /// The account's sessions as JSON.
    pub async fn sessions(&self) -> Result<String, AccountFailure> {
        Ok(self.inner.sessions().await?.to_string())
    }

    pub async fn revoke(&self, session: String) -> Result<(), AccountFailure> {
        Ok(self.inner.revoke(&session).await?)
    }

    /// Everything the service holds about the account, as JSON.
    pub async fn export(&self) -> Result<String, AccountFailure> {
        Ok(self.inner.export().await?.to_string())
    }

    /// Deletes the account; what was deleted comes back as JSON.
    pub async fn delete(&self, password: Option<String>) -> Result<String, AccountFailure> {
        Ok(self.inner.delete(password.as_deref()).await?.to_string())
    }

    pub async fn password_code(&self, bot_check: Bot) -> Result<(), AccountFailure> {
        Ok(self.inner.password_code(&bot(bot_check)).await?)
    }

    pub async fn set_password(
        &self,
        password: String,
        current: Option<String>,
        code: Option<String>,
    ) -> Result<(), AccountFailure> {
        Ok(self
            .inner
            .set_password(&password, current.as_deref(), code.as_deref())
            .await?)
    }

    pub async fn change_email(
        &self,
        email: String,
        password: String,
        bot_check: Bot,
    ) -> Result<(), AccountFailure> {
        Ok(self
            .inner
            .change_email(&email, &password, &bot(bot_check))
            .await?)
    }

    pub async fn confirm_email(
        &self,
        email: String,
        code: String,
        old_code: Option<String>,
    ) -> Result<(), AccountFailure> {
        Ok(self
            .inner
            .confirm_email(&email, &code, old_code.as_deref())
            .await?)
    }
}
