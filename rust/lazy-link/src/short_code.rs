pub const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const SHORT_CODE_LENGTH: usize = 8;

/// A short code as typed (any case, with dashes or spaces) in its canonical form; None when it is not
/// 8 characters of the alphabet, which also tells it from a 22-character token.
pub fn normalize_short_code(text: &str) -> Option<String> {
    if text.encode_utf16().count() > 32 {
        return None;
    }
    let mut code = String::with_capacity(SHORT_CODE_LENGTH);
    for c in text.chars() {
        if c == '-' || c == ' ' {
            continue;
        }
        let upper = c.to_ascii_uppercase();
        if !ALPHABET.contains(upper) {
            return None;
        }
        code.push(upper);
    }
    (code.len() == SHORT_CODE_LENGTH).then_some(code)
}
