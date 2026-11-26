use crate::error::{FilterError, Result};
use crate::output::Output;

/// PDF white space (ISO 32000-1 7.2.2).
fn is_white(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn decode_hex(data: &[u8], max_size: u64) -> Result<Vec<u8>> {
    let mut output = Output::new(max_size, data.len() / 2);
    let mut high = None;
    for &b in data {
        if b == b'>' {
            break;
        }
        if is_white(b) {
            continue;
        }
        let digit = hex_value(b).ok_or_else(|| {
            FilterError::damaged("ASCIIHexDecode data holds a character that is not a hex digit.")
        })?;
        match high.take() {
            None => high = Some(digit),
            Some(h) => output.push(h << 4 | digit)?,
        }
    }
    // An odd last digit stands for a digit and a zero.
    if let Some(h) = high {
        output.push(h << 4)?;
    }
    Ok(output.into_vec())
}

pub fn decode_85(data: &[u8], max_size: u64) -> Result<Vec<u8>> {
    let mut output = Output::new(max_size, data.len());
    let start = if data.starts_with(b"<~") { 2 } else { 0 };
    let mut group = [0u8; 5];
    let mut count = 0;
    for &b in &data[start..] {
        if b == b'~' {
            break;
        }
        if is_white(b) {
            continue;
        }
        if b == b'z' && count == 0 {
            output.fill(4, 0)?;
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(FilterError::damaged(
                "ASCII85Decode data holds a character outside ! to u.",
            ));
        }
        group[count] = b - b'!';
        count += 1;
        if count == 5 {
            emit(&group, 5, &mut output)?;
            count = 0;
        }
    }
    // A short last group of n characters gives n - 1 bytes; a lone character gives none.
    if count > 1 {
        group[count..].fill(84);
        emit(&group, count, &mut output)?;
    }
    Ok(output.into_vec())
}

fn emit(group: &[u8; 5], characters: usize, output: &mut Output) -> Result<()> {
    let value = group
        .iter()
        .fold(0u32, |v, &d| v.wrapping_mul(85).wrapping_add(d as u32));
    output.extend(&value.to_be_bytes()[..characters - 1])
}

pub fn decode_run_length(data: &[u8], max_size: u64) -> Result<Vec<u8>> {
    let mut output = Output::new(max_size, data.len().saturating_mul(2));
    let mut i = 0;
    while i < data.len() {
        let length = data[i] as usize;
        i += 1;
        if length == 128 {
            break;
        }
        if length < 128 {
            let count = (length + 1).min(data.len() - i);
            output.extend(&data[i..i + count])?;
            i += count;
        } else if i < data.len() {
            output.fill(257 - length, data[i])?;
            i += 1;
        }
    }
    Ok(output.into_vec())
}
