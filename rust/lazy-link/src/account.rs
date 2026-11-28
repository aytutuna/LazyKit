//! The client of the Lazy account service: one call per route, with the service's refusals as
//! their own errors. The screens (password, code, Apple, Google, the bot check) belong to the app;
//! this only speaks the protocol.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::base64url;
use crate::relay::Attestation;
use crate::tls::TlsTransport;
use crate::transport::Transport;
use crate::wire::{Reply, Wire, token_chars};
use serde_json::{Value, json};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// The account service, up to the version.
pub const BASE: &str = "https://alazy.app/account/v1";

/// Who is signing in: which Lazy app, which of its devices (32 lowercase hex characters, the id the
/// relay calls need to match) and an optional name for the session list.
#[derive(Debug, Clone)]
pub struct Login {
    pub app: String,
    pub device: String,
    pub label: Option<String>,
}

/// A session the service opened. `token` goes in every later request, here and at the relay.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub token: String,
    pub session: String,
    /// The account as `me` shows it: its address, whether it has a password, its linked identities.
    pub account: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignIn {
    Done(Session),
    /// The identity is already an account's under another sign-in: a code was sent to the account's
    /// address, and `confirm_link` with it finishes the sign-in.
    LinkRequired,
}

/// The proof that a request comes from a person's app, for the routes that send mail.
#[derive(Debug, Clone)]
pub enum Bot {
    /// A Turnstile token, made on a page with the widget for this kind of request.
    Turnstile(String),
    /// An App Attest or Play Integrity proof made for a `nonce` from [`Account::nonce`].
    Attest { nonce: String, proof: Attestation },
}

#[derive(Debug)]
pub enum AccountError {
    /// There is no session, or it has ended.
    SignIn,
    /// The email, password or code is wrong.
    WrongCredentials,
    /// A provider's token was refused (wrong audience, expired, or a nonce already used).
    TokenRefused,
    /// The session is too old for this change: sign in again first.
    NeedFresh,
    /// The bot check is missing or did not pass.
    BotCheck,
    /// Too many wrong tries; try again after `retry_after`.
    Locked {
        retry_after: Duration,
    },
    /// Too many codes asked for; try again after `retry_after`.
    Limited {
        retry_after: Duration,
    },
    /// The service's mail is used up for now.
    Busy {
        retry_after: Duration,
    },
    /// This address has made too many requests.
    SlowDown {
        retry_after: Duration,
    },
    /// The identity provider could not be reached.
    Unavailable {
        retry_after: Duration,
    },
    /// The service is not set up for this.
    NotConfigured,
    /// The provider gave no verified email.
    NoEmail,
    /// The request was malformed; the service says how.
    BadInput(String),
    Refused(u16),
    Transport(String),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = |d: &Duration| d.as_secs();
        match self {
            Self::SignIn => f.write_str("sign in first"),
            Self::WrongCredentials => f.write_str("wrong email, password or code"),
            Self::TokenRefused => f.write_str("the sign-in token was refused"),
            Self::NeedFresh => f.write_str("sign in again before this change"),
            Self::BotCheck => f.write_str("the bot check failed"),
            Self::Locked { retry_after } => {
                write!(f, "too many wrong tries, retry in {} s", secs(retry_after))
            }
            Self::Limited { retry_after } => {
                write!(f, "too many codes, retry in {} s", secs(retry_after))
            }
            Self::Busy { retry_after } => {
                write!(f, "mail is busy, retry in {} s", secs(retry_after))
            }
            Self::SlowDown { retry_after } => {
                write!(f, "too many requests, retry in {} s", secs(retry_after))
            }
            Self::Unavailable { retry_after } => write!(
                f,
                "the provider is unreachable, retry in {} s",
                secs(retry_after)
            ),
            Self::NotConfigured => f.write_str("the service is not set up for this"),
            Self::NoEmail => f.write_str("the provider gave no verified email"),
            Self::BadInput(e) => write!(f, "bad request: {e}"),
            Self::Refused(status) => write!(f, "the service answered {status}"),
            Self::Transport(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for AccountError {}

// The service answers `{"error": "..."}`; the message and the status say which refusal it is.
fn refusal(r: &Reply) -> AccountError {
    let message = serde_json::from_str::<Value>(&r.body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_owned))
        .unwrap_or_else(|| r.body.clone());
    let retry_after = r.retry_after.unwrap_or_default();
    match r.status {
        400 => AccountError::BadInput(message),
        401 if message == "sign in" => AccountError::SignIn,
        401 if message == "token refused" => AccountError::TokenRefused,
        401 => AccountError::WrongCredentials,
        403 if message == "sign in again first" => AccountError::NeedFresh,
        403 if message.starts_with("bot check") => AccountError::BotCheck,
        422 => AccountError::NoEmail,
        429 if message.starts_with("too many tries") => AccountError::Locked { retry_after },
        429 => AccountError::Limited { retry_after },
        501 => AccountError::NotConfigured,
        503 if message.starts_with("mail is busy") => AccountError::Busy { retry_after },
        503 if message.starts_with("the sign-in provider") => {
            AccountError::Unavailable { retry_after }
        }
        503 => AccountError::SlowDown { retry_after },
        status => AccountError::Refused(status),
    }
}

fn json_of(r: &Reply) -> Result<Value, AccountError> {
    serde_json::from_str(&r.body).map_err(|e| AccountError::Transport(e.to_string()))
}

fn session_of(r: &Reply) -> Result<Session, AccountError> {
    let v = json_of(r)?;
    let text = |k: &str| {
        v[k].as_str()
            .map(str::to_owned)
            .ok_or_else(|| AccountError::Transport(format!("the answer has no {k}")))
    };
    Ok(Session {
        token: text("token")?,
        session: text("session")?,
        account: v["account"].clone(),
    })
}

fn signed_in(r: &Reply) -> Result<SignIn, AccountError> {
    if r.status == 202 {
        return Ok(SignIn::LinkRequired);
    }
    session_of(r).map(SignIn::Done)
}

fn login_body(login: &Login) -> Value {
    let mut body = json!({ "app": login.app, "device": login.device });
    if let Some(label) = &login.label {
        body["label"] = label.as_str().into();
    }
    body
}

fn with(mut body: Value, extra: Value) -> Value {
    if let (Some(into), Some(from)) = (body.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    body
}

fn bot_body(bot: &Bot) -> Value {
    match bot {
        Bot::Turnstile(token) => json!({ "turnstile": token }),
        Bot::Attest { nonce, proof } => {
            let mut proof = match proof {
                Attestation::Apple {
                    key_id,
                    attestation,
                } => json!({
                    "platform": "apple",
                    "keyId": base64url::encode(key_id),
                    "attestation": base64url::encode(attestation),
                }),
                Attestation::Google { token } => json!({ "platform": "google", "token": token }),
            };
            proof["nonce"] = nonce.as_str().into();
            json!({ "attest": proof })
        }
    }
}

/// The identity providers that sign in with a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Apple,
    Google,
}

#[derive(Clone)]
pub struct Account {
    wire: Wire,
}

impl Account {
    /// The service at alazy.app over TLS that the system trusts.
    pub fn system() -> Result<Self, AccountError> {
        Self::system_at(BASE)
    }

    /// The same at another address, up to the version. Only the native tests (the `dev-roots`
    /// feature) reach a service that is not alazy.app's.
    #[cfg(feature = "dev-roots")]
    pub fn at(base: &str) -> Result<Self, AccountError> {
        Self::system_at(base)
    }

    fn system_at(base: &str) -> Result<Self, AccountError> {
        let transport = TlsTransport::system().map_err(|e| AccountError::Transport(e.0))?;
        Ok(Self::new(base, Arc::new(transport)))
    }

    pub fn new(base: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            wire: Wire::new(base, transport),
        }
    }

    /// Presents a session on the calls that need one.
    pub fn with_session(mut self, token: &str) -> Self {
        self.wire.set_session(token);
        self
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Reply, AccountError> {
        let body = body.map(|b| b.to_string());
        let reply = self
            .wire
            .call(method, &format!("/{path}"), body.as_deref())
            .await
            .map_err(AccountError::Transport)?;
        match reply.status {
            200 | 202 | 204 => Ok(reply),
            _ => Err(refusal(&reply)),
        }
    }

    /// A nonce for a provider token or an attestation, good for one use.
    pub async fn nonce(&self) -> Result<String, AccountError> {
        let r = self.call("POST", "nonce", Some(json!({}))).await?;
        json_of(&r)?["nonce"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| AccountError::Transport("the answer has no nonce".into()))
    }

    /// Mails a code for a new account to `email`.
    pub async fn signup(&self, email: &str, bot: &Bot) -> Result<(), AccountError> {
        let body = with(json!({ "email": email }), bot_body(bot));
        self.call("POST", "signup", Some(body)).await.map(drop)
    }

    /// Mails a sign-in code to `email`.
    pub async fn code(&self, email: &str, bot: &Bot) -> Result<(), AccountError> {
        let body = with(json!({ "email": email }), bot_body(bot));
        self.call("POST", "code", Some(body)).await.map(drop)
    }

    /// Finishes a sign-up with the mailed code and the password to set.
    pub async fn confirm_signup(
        &self,
        email: &str,
        code: &str,
        password: &str,
        login: &Login,
    ) -> Result<Session, AccountError> {
        let body = with(
            json!({ "email": email, "code": code, "password": password }),
            login_body(login),
        );
        session_of(&self.call("POST", "signup/confirm", Some(body)).await?)
    }

    pub async fn signin_password(
        &self,
        email: &str,
        password: &str,
        login: &Login,
    ) -> Result<Session, AccountError> {
        let body = with(
            json!({ "email": email, "password": password }),
            login_body(login),
        );
        session_of(&self.call("POST", "signin", Some(body)).await?)
    }

    pub async fn signin_code(
        &self,
        email: &str,
        code: &str,
        login: &Login,
    ) -> Result<Session, AccountError> {
        let body = with(json!({ "email": email, "code": code }), login_body(login));
        session_of(&self.call("POST", "signin/code", Some(body)).await?)
    }

    /// Signs in with a token from Apple or Google, made for a nonce from [`nonce`](Self::nonce).
    pub async fn social(
        &self,
        provider: Provider,
        id_token: &str,
        nonce: &str,
        login: &Login,
    ) -> Result<SignIn, AccountError> {
        let path = match provider {
            Provider::Apple => "apple",
            Provider::Google => "google",
        };
        let body = with(
            json!({ "idToken": id_token, "nonce": nonce }),
            login_body(login),
        );
        signed_in(&self.call("POST", path, Some(body)).await?)
    }

    /// Finishes linking a provider identity to an existing account with the code mailed to it.
    pub async fn confirm_link(
        &self,
        email: &str,
        code: &str,
        login: &Login,
    ) -> Result<SignIn, AccountError> {
        let body = with(json!({ "email": email, "code": code }), login_body(login));
        signed_in(&self.call("POST", "link/confirm", Some(body)).await?)
    }

    pub async fn signout(&self) -> Result<(), AccountError> {
        self.call("POST", "signout", Some(json!({})))
            .await
            .map(drop)
    }

    /// The account: its address, whether it has a password and its linked identities.
    pub async fn me(&self) -> Result<Value, AccountError> {
        json_of(&self.call("GET", "me", None).await?)
    }

    pub async fn sessions(&self) -> Result<Value, AccountError> {
        json_of(&self.call("GET", "sessions", None).await?)
    }

    /// Ends another session of this account, named by the 12 characters `sessions` lists.
    pub async fn revoke(&self, session: &str) -> Result<(), AccountError> {
        if session.len() != 12 || !token_chars(session) {
            return Err(AccountError::BadInput("not a session id".into()));
        }
        self.call("DELETE", &format!("sessions/{session}"), None)
            .await
            .map(drop)
    }

    /// Everything the service holds about the account.
    pub async fn export(&self) -> Result<Value, AccountError> {
        json_of(&self.call("GET", "export", None).await?)
    }

    /// Deletes the account and what hangs on it. An account with a password needs it.
    pub async fn delete(&self, password: Option<&str>) -> Result<Value, AccountError> {
        let mut body = json!({ "confirm": "delete" });
        if let Some(password) = password {
            body["password"] = password.into();
        }
        json_of(&self.call("DELETE", "me", Some(body)).await?)
    }

    /// Mails a code that lets an account without a password set one.
    pub async fn password_code(&self, bot: &Bot) -> Result<(), AccountError> {
        self.call("POST", "password/code", Some(bot_body(bot)))
            .await
            .map(drop)
    }

    /// Sets or changes the password: `current` for an account that has one, `code` (from
    /// `password_code`) for one that has not.
    pub async fn set_password(
        &self,
        password: &str,
        current: Option<&str>,
        code: Option<&str>,
    ) -> Result<(), AccountError> {
        let mut body = json!({ "password": password });
        if let Some(current) = current {
            body["current"] = current.into();
        }
        if let Some(code) = code {
            body["code"] = code.into();
        }
        self.call("POST", "password", Some(body)).await.map(drop)
    }

    /// Mails a code to a new address; `confirm_email` finishes the change.
    pub async fn change_email(
        &self,
        email: &str,
        password: &str,
        bot: &Bot,
    ) -> Result<(), AccountError> {
        let body = with(
            json!({ "email": email, "password": password }),
            bot_body(bot),
        );
        self.call("POST", "email", Some(body)).await.map(drop)
    }

    /// Finishes an address change with the code mailed to the new address and, when the service
    /// asked for it, the one mailed to the old.
    pub async fn confirm_email(
        &self,
        email: &str,
        code: &str,
        old_code: Option<&str>,
    ) -> Result<(), AccountError> {
        let mut body = json!({ "email": email, "code": code });
        if let Some(old) = old_code {
            body["oldCode"] = old.into();
        }
        self.call("POST", "email/confirm", Some(body))
            .await
            .map(drop)
    }
}
