use crate::base64url;
use crate::fingerprint::FINGERPRINT_LENGTH;
use std::collections::HashMap;

pub const VERSION: u32 = 1;
const SCHEME: &str = "lazysend:";
const PREFIX: &str = "lazysend://pair?";
/// The longest code read, in UTF-8 bytes.
const MAX_LENGTH: usize = 2048;
const MAX_ADDRESSES: usize = 8;
const MAX_ADDRESS_LENGTH: usize = 64;
const MAX_NAME_BYTES: usize = 128;
pub const TOKEN_LENGTH: usize = 16;

/// Why a scanned code is not a pairing code this app can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingUriError {
    /// Not a Lazy Send pairing code at all (another app's QR code, a web address).
    NotPairingCode,
    /// A pairing code of a newer protocol version than this app speaks.
    NewerVersion,
    /// A Lazy Send pairing code with a missing or malformed part.
    Damaged,
}

/// What the desktop's QR code carries so a phone can pair with it: the desktop's id and name, its
/// certificate's fingerprint (base64url), the addresses it listens on, its port and the pairing
/// session's one-time token (base64url of 16 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingUri {
    pub desktop_id: String,
    pub name: String,
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub token: String,
}

impl PairingUri {
    /// The fingerprint's bytes; None when it is not 32 bytes of base64url.
    pub fn fingerprint_bytes(&self) -> Option<Vec<u8>> {
        base64url::decode(&self.fingerprint).filter(|b| b.len() == FINGERPRINT_LENGTH)
    }

    /// The code as text; None for a code that `parse_pairing_uri` would refuse.
    pub fn format(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let mut text = format!(
            "{PREFIX}v={VERSION}&id={}&n={}&fp={}",
            percent_encode(&self.desktop_id),
            percent_encode(&self.name),
            percent_encode(&self.fingerprint)
        );
        if !self.addresses.is_empty() {
            let list: Vec<String> = self.addresses.iter().map(|a| percent_encode(a)).collect();
            text.push_str("&a=");
            text.push_str(&list.join(","));
        }
        text.push_str(&format!(
            "&p={}&t={}",
            self.port,
            percent_encode(&self.token)
        ));
        Some(text)
    }

    fn is_valid(&self) -> bool {
        is_valid_id(&self.desktop_id)
            && !self.name.is_empty()
            && self.name.len() <= MAX_NAME_BYTES
            && !self.name.chars().any(is_control)
            && self.fingerprint_bytes().is_some()
            && self.addresses.len() <= MAX_ADDRESSES
            && self.addresses.iter().all(|a| is_valid_address(a))
            && self.port >= 1
            && base64url::decode(&self.token).is_some_and(|t| t.len() == TOKEN_LENGTH)
    }
}

/// The pairing code in `text`, or why it cannot be used.
pub fn parse_pairing_uri(text: &str) -> Result<PairingUri, PairingUriError> {
    use PairingUriError::*;
    if text.len() > MAX_LENGTH {
        return Err(Damaged);
    }
    if !starts_with_ignore_case(text, SCHEME) {
        return Err(NotPairingCode);
    }
    // Only printable ASCII: percent-encoding covers everything else.
    if !starts_with_ignore_case(text, PREFIX)
        || !text.bytes().all(|b| b > 0x20 && b < 0x7F)
        || text.contains('#')
    {
        return Err(Damaged);
    }
    // Each parameter as written (still percent-encoded): the address list is split at its
    // commas before its addresses are decoded.
    let mut raw: HashMap<&str, &str> = HashMap::new();
    for part in text[PREFIX.len()..].split('&') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if key.is_empty() || percent_decode(value).is_none() || raw.insert(key, value).is_some() {
            return Err(Damaged);
        }
    }
    let value = |key: &str| raw.get(key).and_then(|v| percent_decode(v));
    // The version first: a newer code may have other parts this version would call damaged.
    let version_text = value("v").ok_or(Damaged)?;
    if !is_digits(&version_text, 9) {
        return Err(Damaged);
    }
    let version: u32 = version_text.parse().map_err(|_| Damaged)?;
    if version > VERSION {
        return Err(NewerVersion);
    }
    if version < 1 {
        return Err(Damaged);
    }
    let (Some(desktop_id), Some(name), Some(fingerprint), Some(port_text), Some(token)) =
        (value("id"), value("n"), value("fp"), value("p"), value("t"))
    else {
        return Err(Damaged);
    };
    let mut addresses = Vec::new();
    if let Some(list) = raw.get("a").filter(|l| !l.is_empty()) {
        for encoded in list.split(',') {
            addresses.push(percent_decode(encoded).ok_or(Damaged)?);
        }
    }
    if !is_digits(&port_text, 5) {
        return Err(Damaged);
    }
    let port: u16 = port_text.parse().map_err(|_| Damaged)?;
    let uri = PairingUri {
        desktop_id,
        name,
        fingerprint,
        addresses,
        port,
        token,
    };
    if uri.is_valid() {
        Ok(uri)
    } else {
        Err(Damaged)
    }
}

/// A phone's, desktop's or item's id: 8 to 64 letters, digits, '-' and '_', so it is safe as a file name.
pub fn is_valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `address` is an IP address a code may carry: IPv4 in dotted decimal (no leading
/// zeros), or IPv6 (with "::", an embedded IPv4 tail and a zone after '%').
pub fn is_valid_address(address: &str) -> bool {
    if address.is_empty() || address.len() > MAX_ADDRESS_LENGTH {
        return false;
    }
    if address.contains(':') {
        is_ipv6(address)
    } else {
        is_ipv4(address)
    }
}

/// Everything but letters, digits and `-._~` as %XX of its UTF-8 bytes (uppercase hex).
pub fn percent_encode(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            text.push(b as char);
        } else {
            text.push_str(&format!("%{b:02X}"));
        }
    }
    text
}

/// The text of a percent-encoded value; None for a bad escape, a character that must be escaped,
/// or bytes that are not UTF-8.
pub fn percent_decode(value: &str) -> Option<String> {
    let input = value.as_bytes();
    let mut bytes = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        if c == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            bytes.push((hex_value(hex[0])? << 4) | hex_value(hex[1])?);
            i += 3;
        } else if c > 0x20 && c < 0x7F && !matches!(c, b'&' | b'=' | b'#' | b'?') {
            bytes.push(c);
            i += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Any C0 or C1 control character, or DEL.
fn is_control(c: char) -> bool {
    (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32))
}

fn is_digits(text: &str, max_length: usize) -> bool {
    !text.is_empty() && text.len() <= max_length && text.bytes().all(|b| b.is_ascii_digit())
}

fn is_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            is_digits(p, 3)
                && !(p.len() > 1 && p.starts_with('0'))
                && p.parse::<u32>().is_ok_and(|n| n <= 255)
        })
}

fn is_ipv6(text: &str) -> bool {
    let mut text = text;
    if let Some(percent) = text.find('%') {
        let zone = &text[percent + 1..];
        let zone_ok = !zone.is_empty()
            && zone.len() <= 32
            && zone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !zone_ok {
            return false;
        }
        text = &text[..percent];
    }
    let compressed = text.find("::");
    if let Some(at) = compressed
        && text[at + 1..].contains("::")
    {
        return false;
    }
    let (head, tail) = match compressed {
        Some(at) => (&text[..at], &text[at + 2..]),
        None => (text, ""),
    };
    let mut all: Vec<&str> = Vec::new();
    if !head.is_empty() {
        all.extend(head.split(':'));
    }
    if !tail.is_empty() {
        all.extend(tail.split(':'));
    }
    let mut groups = 0;
    for (i, group) in all.iter().enumerate() {
        if i == all.len() - 1 && group.contains('.') {
            if !is_ipv4(group) {
                return false;
            }
            groups += 2;
            continue;
        }
        if group.is_empty() || group.len() > 4 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        groups += 1;
    }
    if compressed.is_some() {
        groups <= 7
    } else {
        groups == 8
    }
}
