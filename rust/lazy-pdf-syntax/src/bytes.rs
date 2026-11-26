/// The first place `pattern` starts at or after `from`.
pub fn find(data: &[u8], pattern: &[u8], from: usize) -> Option<usize> {
    let first = *pattern.first()?;
    let mut at = from;
    while at + pattern.len() <= data.len() {
        let skip = data[at..=data.len() - pattern.len()]
            .iter()
            .position(|b| *b == first)?;
        at += skip;
        if data[at..].starts_with(pattern) {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// The last place `pattern` starts.
pub fn rfind(data: &[u8], pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() || data.len() < pattern.len() {
        return None;
    }
    (0..=data.len() - pattern.len())
        .rev()
        .find(|&at| data[at..].starts_with(pattern))
}
