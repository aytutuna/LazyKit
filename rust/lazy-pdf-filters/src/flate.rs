use zlib_rs::{Inflate, InflateFlush, Status};

use crate::error::{FilterError, Result};
use crate::params::{self, Params};

const MAX_COLORS: i32 = 1 << 16;

#[derive(PartialEq)]
enum End {
    Clean,
    RanDry,
    Bad,
}

/// zlib data inflated; raw deflate data (no zlib header) is accepted too. Data that breaks off or
/// fails its checksum gives what decoded before the break; nothing decoded is damage. With `strict`
/// a stream that breaks off after some output is damage as well.
pub fn inflate(data: &[u8], max_size: u64, strict: bool) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    // The deflate stream is read bare, past a valid zlib header: the Adler-32 check would throw away
    // the last block of a file whose checksum is wrong (ISO 32000-1 7.4.4).
    let header = if data.len() >= 2
        && data[0] & 0x0f == 8
        && (data[0] as u32 * 256 + data[1] as u32).is_multiple_of(31)
    {
        2
    } else {
        0
    };
    let (mut output, mut end) = inflate_from(&data[header..], max_size, strict)?;
    if output.is_empty() && end != End::Clean && header > 0 {
        (output, end) = inflate_from(data, max_size, strict)?;
    }
    if output.is_empty() && end != End::Clean {
        return Err(FilterError::damaged(
            "A compressed stream does not inflate.",
        ));
    }
    Ok(output)
}

/// A raw deflate stream (a zip entry) inflated; a stream that breaks off or does not inflate is damage.
pub fn inflate_raw(data: &[u8], max_size: u64) -> Result<Vec<u8>> {
    let (output, end) = inflate_from(data, max_size, true)?;
    if end != End::Clean {
        return Err(FilterError::damaged(
            "A compressed stream does not inflate.",
        ));
    }
    Ok(output)
}

// The reference reads Flate through .NET's DeflateStream, which hands its output back after each 8 KB
// of input or 64 KB of output, and a data error throws away the stretch in progress. A damaged file
// has to give the same bytes on every platform, so the input goes to the inflater in the same pieces
// and the output is cut back to where the stretch began.
const INPUT_CHUNK: usize = 8192;
const OUTPUT_CALL: usize = 1 << 16;

fn inflate_from(data: &[u8], max_size: u64, strict: bool) -> Result<(Vec<u8>, End)> {
    // The buffer never holds more than one byte past the cap, which is how a stream over it is seen.
    let cap = usize::try_from(max_size.saturating_add(1)).unwrap_or(usize::MAX);
    let mut buffer = vec![0u8; data.len().saturating_mul(4).clamp(256, 1 << 22).min(cap)];
    // zlib's own inflater, so incomplete and over-subscribed codes are refused where the reference
    // refuses them.
    let mut state = Inflate::new(false, 15);
    let (mut offset, mut at, mut call_start) = (0, 0, 0);
    let end = loop {
        let chunk_end = (offset / INPUT_CHUNK + 1) * INPUT_CHUNK;
        let input = &data[offset..data.len().min(chunk_end)];
        // A call that ended exactly full is still the one in progress until another byte is written.
        let room = OUTPUT_CALL - (at - call_start);
        let limit = room.max(1).min(cap - at);
        if limit == 0 {
            return Err(FilterError::too_big());
        }
        if buffer.len() < at + limit {
            buffer.resize((at + limit).max(buffer.len().saturating_mul(2)).min(cap), 0);
        }
        let (in_before, out_before) = (state.total_in(), state.total_out());
        let status = state.decompress(input, &mut buffer[at..at + limit], InflateFlush::NoFlush);
        let used = (state.total_in() - in_before) as usize;
        let written = (state.total_out() - out_before) as usize;
        if room == 0 && written > 0 || offset % INPUT_CHUNK == 0 && used > 0 {
            call_start = at;
        }
        offset += used;
        at += written;
        if at as u64 > max_size {
            return Err(FilterError::too_big());
        }
        match status {
            Ok(Status::StreamEnd) => break End::Clean,
            Ok(Status::Ok) if written == limit => {}
            Ok(Status::Ok) => call_start = at,
            Ok(Status::BufError) => break End::RanDry,
            Err(_) => break End::Bad,
        }
    };
    if end == End::Bad {
        at = call_start;
    }
    buffer.truncate(at);
    if strict && at > 0 && end != End::Clean {
        return Err(FilterError::damaged("A compressed stream breaks off."));
    }
    Ok((buffer, end))
}

/// Undoes a /Predictor (ISO 32000-1 7.4.4.4): 2 (TIFF) or 10-15 (PNG, each row with its own filter
/// byte), at 1, 2, 4, 8 or 16 bits per component. A last row cut short is kept as far as it goes.
pub fn predict(mut data: Vec<u8>, parms: Option<&Params>) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(data);
    }
    let Some((predictor, colors, bits, columns)) = check_predictor(parms)? else {
        return Ok(data);
    };
    // A row longer than the data is one short row; this keeps a bad /Columns from allocating.
    let row_bits = colors as u64 * bits as u64 * columns as u64;
    let row_length = (row_bits.div_ceil(8)).min(data.len() as u64) as usize;
    if predictor == 2 {
        predict_tiff(&mut data, row_length, colors, bits, columns);
        Ok(data)
    } else {
        let pixel = (colors * bits).div_ceil(8).min(row_length);
        Ok(predict_png(&data, row_length, pixel))
    }
}

/// The predictor, colors, bits per component and columns the parameters ask for, none when there is no
/// predictor, or an error when they cannot be used. Checked before a stream is decoded, so that bad parameters
/// do not cost an inflate.
pub fn check_predictor(parms: Option<&Params>) -> Result<Option<(i32, usize, usize, usize)>> {
    let predictor = params::int(parms, "Predictor", 1);
    if predictor <= 1 {
        return Ok(None);
    }
    let colors = params::int(parms, "Colors", 1).max(1);
    if colors > MAX_COLORS {
        return Err(FilterError::damaged(format!(
            "A predictor with {colors} colors."
        )));
    }
    let bits = params::int(parms, "BitsPerComponent", 8);
    let columns = params::int(parms, "Columns", 1).max(1);
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Err(FilterError::damaged(format!(
            "A predictor with {bits} bits per component."
        )));
    }
    if predictor != 2 && predictor < 10 {
        return Err(FilterError::unsupported(format!(
            "Predictor {predictor} is not supported."
        )));
    }
    Ok(Some((
        predictor,
        colors as usize,
        bits as usize,
        columns as usize,
    )))
}

fn predict_tiff(data: &mut [u8], row_length: usize, colors: usize, bits: usize, columns: usize) {
    for row in data.chunks_mut(row_length) {
        match bits {
            8 => {
                for i in colors..row.len() {
                    row[i] = row[i].wrapping_add(row[i - colors]);
                }
            }
            16 => {
                let step = colors * 2;
                let mut i = step;
                while i + 1 < row.len() {
                    let sum = u16::from_be_bytes([row[i], row[i + 1]])
                        .wrapping_add(u16::from_be_bytes([row[i - step], row[i - step + 1]]));
                    [row[i], row[i + 1]] = sum.to_be_bytes();
                    i += 2;
                }
            }
            _ => {
                let count =
                    ((columns as u64 * colors as u64).min((row.len() * 8 / bits) as u64)) as usize;
                let mask = (1usize << bits) - 1;
                for j in colors..count {
                    let sum = component(row, j, bits) + component(row, j - colors, bits);
                    let shift = 8 - bits - j * bits % 8;
                    let at = j * bits / 8;
                    row[at] =
                        ((row[at] as usize & !(mask << shift)) | ((sum & mask) << shift)) as u8;
                }
            }
        }
    }
}

fn component(row: &[u8], index: usize, bits: usize) -> usize {
    (row[index * bits / 8] as usize >> (8 - bits - index * bits % 8)) & ((1 << bits) - 1)
}

fn predict_png(data: &[u8], row_length: usize, pixel: usize) -> Vec<u8> {
    let pixel = pixel.max(1);
    let step = row_length + 1;
    let rem = data.len() % step;
    let mut output = vec![0u8; data.len() / step * row_length + rem.saturating_sub(1)];
    let zeros = vec![0u8; row_length];
    let (mut at, mut written, mut previous) = (0, 0, None);
    while at + 1 < data.len() {
        let kind = data[at];
        at += 1;
        let n = row_length.min(data.len() - at);
        let (done, rest) = output.split_at_mut(written);
        let row = &mut rest[..n];
        row.copy_from_slice(&data[at..at + n]);
        let up = match previous {
            Some(p) => &done[p..p + n],
            None => &zeros[..n],
        };
        // Other row filter bytes leave the row as it is, as other readers do.
        match kind {
            1 => {
                for i in pixel..n {
                    row[i] = row[i].wrapping_add(row[i - pixel]);
                }
            }
            2 => {
                for i in 0..n {
                    row[i] = row[i].wrapping_add(up[i]);
                }
            }
            3 => {
                for i in 0..n {
                    let left = if i >= pixel { row[i - pixel] as u32 } else { 0 };
                    row[i] = row[i].wrapping_add(((left + up[i] as u32) / 2) as u8);
                }
            }
            4 => {
                for i in 0..n {
                    let inside = i >= pixel;
                    let a = if inside { row[i - pixel] } else { 0 };
                    let c = if inside { up[i - pixel] } else { 0 };
                    row[i] = row[i].wrapping_add(paeth(a as i32, up[i] as i32, c as i32) as u8);
                }
            }
            _ => {}
        }
        previous = Some(written);
        written += n;
        at += n;
    }
    output
}

fn paeth(a: i32, b: i32, c: i32) -> i32 {
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
