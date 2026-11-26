use crate::base64url;
use crate::pairing_uri::TOKEN_LENGTH;
use crate::short_code::{ALPHABET, SHORT_CODE_LENGTH, normalize_short_code};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use subtle::ConstantTimeEq;

pub const LIFETIME_MS: i64 = 5 * 60 * 1000;
pub const MAX_FAILED_ATTEMPTS: u32 = 5;
pub const MAX_TOTAL_FAILED_ATTEMPTS: u32 = 25;

/// Where a pairing session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting for a phone to present the token.
    Open,
    /// A phone presented the token; it cannot be used again.
    Used,
    /// Its five minutes are over.
    Expired,
    /// Closed by the desktop, or after too many wrong tokens from all peers together.
    Closed,
}

/// The answer to a token presented to a pairing session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemResult {
    Accepted,
    WrongToken,
    Expired,
    AlreadyUsed,
    Closed,
}

struct Inner {
    state: SessionState,
    failed_total: u32,
    failed_by_peer: HashMap<Option<IpAddr>, u32>,
}

/// One pairing window on the desktop: a 128-bit token shown in the QR code, good for one pairing
/// within five minutes, and an 8-character short code for a guest that cannot scan. A peer that
/// sends five wrong ones is refused for the rest of the session, and 25 wrong ones from anyone
/// close it, so neither can be guessed. The clock is passed in, so tests control it.
pub struct PairingSession {
    token: [u8; TOKEN_LENGTH],
    short_code: String,
    expires_ms: i64,
    inner: Mutex<Inner>,
}

impl PairingSession {
    /// `token` is the 16 token bytes; `code_entropy` is at least 8 random bytes for the short code.
    /// Both come from the caller's system random source, so the core needs none of its own.
    pub fn new(opened_ms: i64, token: [u8; TOKEN_LENGTH], code_entropy: &[u8]) -> Option<Self> {
        if code_entropy.len() < SHORT_CODE_LENGTH {
            return None;
        }
        let alphabet = ALPHABET.as_bytes();
        // 256 is a multiple of 32, so masking a random byte is unbiased.
        let short_code = code_entropy[..SHORT_CODE_LENGTH]
            .iter()
            .map(|b| alphabet[(b & 31) as usize] as char)
            .collect();
        Some(Self {
            token,
            short_code,
            expires_ms: opened_ms + LIFETIME_MS,
            inner: Mutex::new(Inner {
                state: SessionState::Open,
                failed_total: 0,
                failed_by_peer: HashMap::new(),
            }),
        })
    }

    /// The token as the pairing code carries it (base64url, 22 characters).
    pub fn token(&self) -> String {
        base64url::encode(&self.token)
    }

    /// The short code: 8 characters of Crockford base32, no separator.
    pub fn short_code(&self) -> &str {
        &self.short_code
    }

    pub fn expires_ms(&self) -> i64 {
        self.expires_ms
    }

    pub fn failed_attempts(&self) -> u32 {
        self.inner.lock().unwrap().failed_total
    }

    pub fn state_at(&self, now_ms: i64) -> SessionState {
        let inner = self.inner.lock().unwrap();
        if inner.state == SessionState::Open && now_ms >= self.expires_ms {
            SessionState::Expired
        } else {
            inner.state
        }
    }

    /// Presents `token` (the 22-character token or a short code as typed) from `peer` at `now_ms`.
    /// The right one while the session is open is accepted once. A wrong one counts against its
    /// peer, after which that peer gets `Closed` for the session, and against the total, after
    /// which the session is closed. Callers that do not know the peer pass none, and count as one.
    pub fn redeem(&self, token: Option<&str>, now_ms: i64, peer: Option<IpAddr>) -> RedeemResult {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            SessionState::Used => return RedeemResult::AlreadyUsed,
            SessionState::Closed => return RedeemResult::Closed,
            _ => {}
        }
        if now_ms >= self.expires_ms {
            inner.state = SessionState::Expired;
            return RedeemResult::Expired;
        }
        let peer = peer.map(unmap);
        let by_peer = inner.failed_by_peer.get(&peer).copied().unwrap_or(0);
        if by_peer >= MAX_FAILED_ATTEMPTS {
            return RedeemResult::Closed;
        }
        let mut matches = token
            .and_then(base64url::decode)
            .is_some_and(|presented| equal(&presented, &self.token));
        if let Some(code) = token.and_then(normalize_short_code) {
            matches = equal(code.as_bytes(), self.short_code.as_bytes());
        }
        if !matches {
            inner.failed_total += 1;
            inner.failed_by_peer.insert(peer, by_peer + 1);
            if inner.failed_total >= MAX_TOTAL_FAILED_ATTEMPTS {
                inner.state = SessionState::Closed;
            }
            return RedeemResult::WrongToken;
        }
        inner.state = SessionState::Used;
        RedeemResult::Accepted
    }

    /// Closes the session: no token is accepted any more.
    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.state == SessionState::Open {
            inner.state = SessionState::Closed;
        }
    }
}

/// An IPv4 address that arrived mapped into IPv6 is the same peer as the plain one.
fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        v4 => v4,
    }
}

fn equal(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}
