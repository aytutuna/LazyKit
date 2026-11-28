//! CCITTFaxDecode (ISO 32000-1 7.4.6; ITU-T T.4 and T.6): K below zero is Group 4, zero is Group 3
//! one-dimensional, above zero is Group 3 with a tag bit after each EOL choosing one or two
//! dimensions for the next row. EOLs are accepted whether or not /EndOfLine says they are there.

use crate::ccitt_codes;
use crate::error::{FilterError, Result};
use crate::output::Output;

pub struct Options {
    pub k: i32,
    pub columns: usize,
    pub rows: usize,
    pub byte_align: bool,
    pub end_of_line: bool,
    pub black_is_1: bool,
}

/// Rows of packed pixels, each starting on a byte boundary, white 1 unless `black_is_1`. A bad code
/// ends a Group 4 stream; Group 3 skips to the next EOL. With `rows` known, missing rows are white.
pub fn decode(data: &[u8], o: &Options, max_size: u64) -> Result<Vec<u8>> {
    let (k, columns, rows) = (o.k, o.columns, o.rows);
    let stride = columns.div_ceil(8);
    // The size is a usize below, so a limit past what the target addresses must not let it wrap.
    if rows > 0 && rows as u64 * stride as u64 > max_size.min(usize::MAX as u64) {
        return Err(FilterError::too_big());
    }
    let mut output = Output::new(
        max_size,
        if rows > 0 {
            rows * stride
        } else {
            data.len().saturating_mul(4)
        },
    );
    let white = if o.black_is_1 { 0 } else { 0xff };
    let mut row_bytes = vec![0u8; stride];
    let mut reference = vec![0i32; columns + 2];
    let mut coding = vec![0i32; columns + 2];
    let (mut reference_count, mut done) = (0, 0);
    let mut bits = BitReader::new(data);
    let mut resynced = false;

    while rows == 0 || done < rows {
        // With EOLs the fill bits that align the row sit in front of the EOL, which then ends on the boundary.
        let aligned = o.byte_align && !resynced;
        if aligned && !o.end_of_line {
            bits.align();
        }
        resynced = false;
        let mut eols = 0;
        let mut tag = -1;
        while bits.try_eol() {
            eols += 1;
            if k > 0 {
                tag = bits.read1();
            }
        }
        if eols == 0 && aligned && o.end_of_line {
            bits.align();
        }
        if bits.at_end() || eols >= 2 || (k < 0 && eols > 0) {
            break;
        }

        let two_d = k < 0 || (k > 0 && (if tag >= 0 { tag == 0 } else { done > 0 }));
        let start = bits.position;
        let mut count = 0;
        let ok = if two_d {
            decode_row_2d(
                &mut bits,
                &reference,
                reference_count,
                &mut coding,
                &mut count,
                columns,
            )
        } else {
            decode_row_1d(&mut bits, &mut coding, &mut count, columns)
        };
        // Padding bits after the last row are not a row.
        if !ok && bits.position == start && bits.rest_is_zero() {
            break;
        }

        row_bytes.fill(white);
        for i in (0..count).step_by(2) {
            let to = if i + 1 < count {
                coding[i + 1]
            } else {
                columns as i32
            };
            fill_black(&mut row_bytes, coding[i], to, o.black_is_1);
        }
        output.extend(&row_bytes)?;
        done += 1;
        std::mem::swap(&mut reference, &mut coding);
        reference_count = count;

        if ok {
            continue;
        }
        if k < 0 {
            break;
        }
        bits.skip_to_eol();
        resynced = true;
        // Nothing sound to predict the next row from.
        reference_count = 0;
    }
    if rows > done {
        output.fill((rows - done) * stride, white)?;
    }
    Ok(output.into_vec())
}

/// The Group 4 data of a JBIG2 MMR bitmap (T.88 6.2.6): `rows` rows of packed pixels, black 1. A bad
/// code ends the rows and the rest is white. The second value is how many bytes were read, an EOFB
/// after the last row and the padding up to a byte included, for planes that follow each other in
/// one stream.
pub fn decode_mmr(data: &[u8], columns: usize, rows: usize) -> (Vec<u8>, usize) {
    let stride = columns.div_ceil(8);
    let mut output = vec![0u8; rows * stride];
    let mut row_bytes = vec![0u8; stride];
    // A row cannot change colour more often than the data has bits, however wide it is.
    let changes = columns.min(data.len() * 8) + 2;
    let mut reference = vec![0i32; changes];
    let mut coding = vec![0i32; changes];
    let mut reference_count = 0;
    let mut bits = BitReader::new(data);
    let mut done = 0;
    while done < rows && !bits.at_end() {
        let mut count = 0;
        let ok = decode_row_2d(
            &mut bits,
            &reference,
            reference_count,
            &mut coding,
            &mut count,
            columns,
        );
        row_bytes.fill(0);
        for i in (0..count).step_by(2) {
            let to = if i + 1 < count {
                coding[i + 1]
            } else {
                columns as i32
            };
            fill_black(&mut row_bytes, coding[i], to, true);
        }
        output[done * stride..(done + 1) * stride].copy_from_slice(&row_bytes);
        std::mem::swap(&mut reference, &mut coding);
        reference_count = count;
        done += 1;
        if !ok {
            break;
        }
    }
    if !bits.at_end() && bits.peek(24) == 0x001001 {
        bits.skip(24);
    }
    bits.align();
    let used = data.len().min((bits.position / 8) as usize);
    (output, used)
}

/// One-dimensional row: alternating white and black runs. The changing elements go to `changes`.
fn decode_row_1d(
    bits: &mut BitReader,
    changes: &mut [i32],
    count: &mut usize,
    columns: usize,
) -> bool {
    let mut position = 0;
    let mut black = false;
    while position < columns as i64 {
        let run = read_run(bits, black, columns);
        if run < 0 || *count >= changes.len() - 1 {
            return false;
        }
        position = (position + run).min(columns as i64);
        if position < columns as i64 {
            changes[*count] = position as i32;
            *count += 1;
        }
        black = !black;
    }
    true
}

/// Two-dimensional row (T.6): a0 is the last coded position, b1 the first changing element of the
/// reference row right of a0 and of the colour opposite a0's, b2 the one after it.
fn decode_row_2d(
    bits: &mut BitReader,
    reference: &[i32],
    reference_count: usize,
    changes: &mut [i32],
    count: &mut usize,
    columns: usize,
) -> bool {
    let columns = columns as i32;
    let (mut a0, mut ib) = (-1i32, 0usize);
    let mut black = false;
    while a0 < columns {
        while ib < reference_count && reference[ib] <= a0 {
            ib += 1;
        }
        let mut index = ib;
        if index < reference_count && (index % 2 == 0) == black {
            index += 1;
        }
        let b1 = if index < reference_count {
            reference[index]
        } else {
            columns
        };
        let b2 = if index + 1 < reference_count {
            reference[index + 1]
        } else {
            columns
        };

        let mode = bits.peek(7);
        let offset = if mode >> 6 == 1 {
            bits.skip(1);
            0
        } else if mode >> 4 == 0b011 {
            bits.skip(3);
            1
        } else if mode >> 4 == 0b010 {
            bits.skip(3);
            -1
        } else if mode >> 4 == 0b001 {
            bits.skip(3);
            let first = read_run(bits, black, columns as usize);
            let second = if first < 0 {
                -1
            } else {
                read_run(bits, !black, columns as usize)
            };
            if second < 0 || *count + 2 >= changes.len() {
                return false;
            }
            let a1 = (a0.max(0) as i64 + first).min(columns as i64);
            let a2 = (a1 + second).min(columns as i64);
            if a1 < columns as i64 {
                changes[*count] = a1 as i32;
                *count += 1;
            }
            if a2 < columns as i64 {
                changes[*count] = a2 as i32;
                *count += 1;
            }
            a0 = a2 as i32;
            continue;
        } else if mode >> 3 == 0b0001 {
            bits.skip(4);
            a0 = b2;
            continue;
        } else if mode >> 1 == 0b000011 {
            bits.skip(6);
            2
        } else if mode >> 1 == 0b000010 {
            bits.skip(6);
            -2
        } else if mode == 0b0000011 {
            bits.skip(7);
            3
        } else if mode == 0b0000010 {
            bits.skip(7);
            -3
        } else {
            return false;
        };

        let a1 = b1 + offset;
        if a1 > columns || a1 < a0.max(0) || (a1 == a0 && a0 >= 0) {
            return false;
        }
        if a1 < columns {
            if *count + 2 >= changes.len() {
                return false;
            }
            changes[*count] = a1;
            *count += 1;
        }
        a0 = a1;
        black = !black;
    }
    true
}

/// A run of one colour: make-up codes then a terminating code. -1 for a code that does not exist.
fn read_run(bits: &mut BitReader, black: bool, columns: usize) -> i64 {
    let table = if black {
        ccitt_codes::black()
    } else {
        ccitt_codes::white()
    };
    let mut total = 0i64;
    loop {
        let entry = table[bits.peek(13) as usize];
        if entry == 0 {
            return -1;
        }
        bits.skip((entry & 15) as u64);
        let run = (entry >> 4) as i64;
        total += run;
        if run < 64 {
            return total;
        }
        if total > columns as i64 {
            return -1;
        }
    }
}

fn fill_black(row: &mut [u8], from: i32, to: i32, black_is_1: bool) {
    let mut x = from.max(0) as usize;
    let to = (to.max(0) as usize).min(row.len() * 8);
    while x < to {
        if x & 7 == 0 && to - x >= 8 {
            row[x >> 3] = if black_is_1 { 0xff } else { 0 };
            x += 8;
            continue;
        }
        let bit = 0x80u8 >> (x & 7);
        if black_is_1 {
            row[x >> 3] |= bit;
        } else {
            row[x >> 3] &= !bit;
        }
        x += 1;
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    position: u64,
    end: u64,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            end: data.len() as u64 * 8,
        }
    }

    fn at_end(&self) -> bool {
        self.position >= self.end
    }

    fn skip(&mut self, count: u64) {
        self.position += count;
    }

    fn align(&mut self) {
        self.position = (self.position + 7) & !7;
    }

    fn bit(&self, at: u64) -> u8 {
        (self.data[(at >> 3) as usize] >> (7 - (at & 7))) & 1
    }

    /// The next `count` bits (24 at most); zeros past the end.
    fn peek(&self, count: u32) -> u32 {
        let index = (self.position >> 3) as usize;
        let mut value = 0u32;
        for i in 0..4 {
            value = value << 8 | self.data.get(index + i).copied().unwrap_or(0) as u32;
        }
        (value >> (32 - (self.position & 7) as u32 - count)) & ((1u32 << count) - 1)
    }

    fn read1(&mut self) -> i32 {
        let bit = self.peek(1) as i32;
        self.position += 1;
        bit
    }

    /// Takes an EOL: at least 11 zeros, fill bits included, then a one. Anything else is left alone.
    fn try_eol(&mut self) -> bool {
        let mut at = self.position;
        while at < self.end && self.bit(at) == 0 {
            at += 1;
        }
        if at - self.position < 11 {
            return false;
        }
        if at >= self.end {
            self.position = self.end;
            return false;
        }
        self.position = at + 1;
        true
    }

    fn rest_is_zero(&self) -> bool {
        (self.position..self.end).all(|at| self.bit(at) == 0)
    }

    /// Moves to the next EOL, or the end.
    fn skip_to_eol(&mut self) {
        while !self.at_end() && self.peek(12) != 1 {
            self.position += 1;
        }
    }
}
