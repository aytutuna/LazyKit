use crate::base64url;
use crate::pairing_uri::TOKEN_LENGTH;
use crate::short_code::{ALPHABET, SHORT_CODE_LENGTH, normalize_short_code};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
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

/// Where a token came from: an address, with the zone of a link-local IPv6 address, so that
/// fe80::1%12 and fe80::1%13 are different peers as they are in the other implementations. An IPv4
/// address that arrived mapped into IPv6 is the same peer as the plain one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Peer {
    addr: IpAddr,
    zone: Option<String>,
}

impl From<IpAddr> for Peer {
    fn from(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V6(v6) => v6
                .to_ipv4_mapped()
                .map_or(Peer { addr, zone: None }, |v4| Peer {
                    addr: IpAddr::V4(v4),
                    zone: None,
                }),
            v4 => Peer {
                addr: v4,
                zone: None,
            },
        }
    }
}

/// An address as .NET, Java and Swift print it: `10.0.0.1`, `fe80::1`, `fe80::1%12`, `fe80::1%wlan0`.
impl FromStr for Peer {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let Some((address, zone)) = text.split_once('%') else {
            return text.parse::<IpAddr>().map(Peer::from).map_err(|_| ());
        };
        if zone.is_empty() || zone.contains('%') {
            return Err(());
        }
        let addr = address.parse::<Ipv6Addr>().map_err(|_| ())?;
        let mut peer = Peer::from(IpAddr::V6(addr));
        if peer.addr.is_ipv6() {
            peer.zone = Some(zone.to_owned());
        }
        Ok(peer)
    }
}

struct Inner {
    state: SessionState,
    failed_total: u32,
    failed_by_peer: HashMap<Option<Peer>, u32>,
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
            expires_ms: opened_ms.saturating_add(LIFETIME_MS),
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
        self.lock().failed_total
    }

    pub fn state_at(&self, now_ms: i64) -> SessionState {
        let inner = self.lock();
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
    pub fn redeem(&self, token: Option<&str>, now_ms: i64, peer: Option<Peer>) -> RedeemResult {
        let mut inner = self.lock();
        match inner.state {
            SessionState::Used => return RedeemResult::AlreadyUsed,
            SessionState::Closed => return RedeemResult::Closed,
            _ => {}
        }
        if now_ms >= self.expires_ms {
            inner.state = SessionState::Expired;
            return RedeemResult::Expired;
        }
        // A clock that steps back does not revive a session.
        if inner.state == SessionState::Expired {
            return RedeemResult::Expired;
        }
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

    /// The state is plain data, so a panic elsewhere cannot leave it half written.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How long the session stays open after `now_ms`, in milliseconds; zero once it is not open.
    pub fn remaining_ms(&self, now_ms: i64) -> i64 {
        match self.state_at(now_ms) {
            SessionState::Open => self.expires_ms.saturating_sub(now_ms).max(0),
            _ => 0,
        }
    }

    /// Closes the session: no token is accepted any more.
    pub fn close(&self) {
        let mut inner = self.lock();
        if inner.state == SessionState::Open {
            inner.state = SessionState::Closed;
        }
    }
}

fn equal(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: [u8; TOKEN_LENGTH] = [9; TOKEN_LENGTH];
    const ENTROPY: [u8; 8] = [7, 3, 21, 0, 9, 31, 12, 5];

    fn session() -> PairingSession {
        PairingSession::new(0, TOKEN, &ENTROPY).unwrap()
    }

    fn peer(text: &str) -> Option<Peer> {
        Some(text.parse().unwrap())
    }

    #[test]
    fn an_expired_session_stays_expired_when_the_clock_goes_back() {
        let s = session();
        assert_eq!(
            s.redeem(Some(&s.token()), LIFETIME_MS, None),
            RedeemResult::Expired
        );
        assert_eq!(
            s.redeem(Some(&s.token()), 1000, None),
            RedeemResult::Expired
        );
        assert_eq!(s.state_at(1000), SessionState::Expired);
    }

    #[test]
    fn a_huge_open_time_does_not_overflow() {
        let s = PairingSession::new(i64::MAX - 5, TOKEN, &ENTROPY).unwrap();
        assert_eq!(s.expires_ms(), i64::MAX);
        assert_eq!(s.remaining_ms(i64::MIN), i64::MAX);
    }

    #[test]
    fn zones_make_peers_different() {
        let s = session();
        for _ in 0..MAX_FAILED_ATTEMPTS {
            assert_eq!(
                s.redeem(Some("wrong"), 1, peer("fe80::1%12")),
                RedeemResult::WrongToken
            );
        }
        assert_eq!(
            s.redeem(Some("wrong"), 1, peer("fe80::1%12")),
            RedeemResult::Closed
        );
        assert_eq!(
            s.redeem(Some("wrong"), 1, peer("fe80::1%wlan0")),
            RedeemResult::WrongToken
        );
        assert_eq!(
            s.redeem(Some(&s.token()), 1, peer("fe80::1%13")),
            RedeemResult::Accepted
        );
    }

    #[test]
    fn peers_parse_as_the_platforms_print_them() {
        for good in [
            "10.0.0.1",
            "::1",
            "fe80::1%12",
            "fe80::1%wlan0",
            "::ffff:10.0.0.1",
        ] {
            assert!(good.parse::<Peer>().is_ok(), "{good}");
        }
        for bad in [
            "",
            "nope",
            "fe80::1%",
            "fe80::1%1%2",
            "10.0.0.1%12",
            "%12",
            "10.0.0",
        ] {
            assert!(bad.parse::<Peer>().is_err(), "{bad}");
        }
        assert_eq!(
            "::ffff:10.0.0.1".parse::<Peer>(),
            "10.0.0.1".parse::<Peer>()
        );
    }

    #[test]
    fn remaining_counts_down_and_stops() {
        let s = session();
        assert_eq!(s.remaining_ms(1000), LIFETIME_MS - 1000);
        assert_eq!(s.remaining_ms(LIFETIME_MS), 0);
        assert_eq!(s.failed_attempts(), 0);
    }
}
