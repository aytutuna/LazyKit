//! The writing side of the filters: what a writer chooses from when it packs a stream.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use std::collections::HashMap;

use zlib_rs::{Deflate, DeflateFlush, Status};

use crate::lzw::{CLEAR, END, FIRST, TABLE_SIZE};

/// zlib data (RFC 1950) at a zlib level, 0 (stored) to 9 (smallest).
pub fn flate(data: &[u8], level: i32) -> Vec<u8> {
    let mut deflate = Deflate::new(level.clamp(0, 9), true, 15);
    let mut output = vec![0u8; data.len() / 2 + 64];
    loop {
        let used = deflate.total_in() as usize;
        let at = deflate.total_out() as usize;
        let status = deflate
            .compress(&data[used..], &mut output[at..], DeflateFlush::Finish)
            .expect("a deflate stream made here is valid");
        if status == Status::StreamEnd {
            output.truncate(deflate.total_out() as usize);
            return output;
        }
        output.resize(output.len() * 2, 0);
    }
}

/// Hex digits in upper case and the closing ">".
pub fn ascii_hex(data: &[u8]) -> Vec<u8> {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = Vec::with_capacity(data.len() * 2 + 1);
    for &b in data {
        output.push(DIGITS[(b >> 4) as usize]);
        output.push(DIGITS[(b & 15) as usize]);
    }
    output.push(b'>');
    output
}

/// Base-85 text with "z" for a group of four zero bytes and the closing "~>".
pub fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len() / 4 * 5 + 8);
    for group in data.chunks(4) {
        let mut bytes = [0u8; 4];
        bytes[..group.len()].copy_from_slice(group);
        let mut value = u32::from_be_bytes(bytes);
        if value == 0 && group.len() == 4 {
            output.push(b'z');
            continue;
        }
        let mut chars = [0u8; 5];
        for c in chars.iter_mut().rev() {
            *c = (value % 85) as u8 + b'!';
            value /= 85;
        }
        output.extend_from_slice(&chars[..group.len() + 1]);
    }
    output.extend_from_slice(b"~>");
    output
}

/// Runs of two or more equal bytes and stretches of literals, closed by the 128 end byte.
pub fn run_length(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len() + data.len() / 64 + 2);
    let mut i = 0;
    while i < data.len() {
        let mut run = 1;
        while i + run < data.len() && run < 128 && data[i + run] == data[i] {
            run += 1;
        }
        if run >= 2 {
            output.push((257 - run) as u8);
            output.push(data[i]);
            i += run;
            continue;
        }
        let mut end = i + 1;
        while end < data.len()
            && end - i < 128
            && !(end + 1 < data.len() && data[end] == data[end + 1])
        {
            end += 1;
        }
        output.push((end - i - 1) as u8);
        output.extend_from_slice(&data[i..end]);
        i = end;
    }
    output.push(128);
    output
}

/// LZWDecode's codes (ISO 32000-1 7.4.4): the table is cleared before it fills up.
pub fn lzw(data: &[u8], early_change: bool) -> Vec<u8> {
    let mut writer = Codes {
        output: Vec::with_capacity(data.len() / 2),
        pending: 0,
        pending_bits: 0,
        bits: 9,
    };
    let mut table: HashMap<u32, usize> = HashMap::new();
    let early = early_change as usize;
    let mut next = FIRST;
    // The encoder's table runs one entry ahead of the decoder's, so it widens one entry later.
    let grow = |next: &mut usize, bits: &mut u32| {
        *next += 1;
        if *next + early > 1 << *bits && *bits < 12 {
            *bits += 1;
        }
    };
    writer.put(CLEAR);
    let mut current: Option<usize> = None;
    for &b in data {
        let Some(c) = current else {
            current = Some(b as usize);
            continue;
        };
        let key = (c as u32) << 8 | b as u32;
        if let Some(&found) = table.get(&key) {
            current = Some(found);
            continue;
        }
        writer.put(c);
        table.insert(key, next);
        grow(&mut next, &mut writer.bits);
        if next >= TABLE_SIZE - 2 {
            writer.put(CLEAR);
            table.clear();
            next = FIRST;
            writer.bits = 9;
        }
        current = Some(b as usize);
    }
    if let Some(c) = current {
        writer.put(c);
        grow(&mut next, &mut writer.bits);
    }
    writer.put(END);
    if writer.pending_bits > 0 {
        writer
            .output
            .push((writer.pending << (8 - writer.pending_bits)) as u8);
    }
    writer.output
}

struct Codes {
    output: Vec<u8>,
    pending: u32,
    pending_bits: u32,
    bits: u32,
}

impl Codes {
    fn put(&mut self, code: usize) {
        self.pending = self.pending << self.bits | code as u32;
        self.pending_bits += self.bits;
        while self.pending_bits >= 8 {
            self.pending_bits -= 8;
            self.output.push((self.pending >> self.pending_bits) as u8);
        }
        self.pending &= (1u32 << self.pending_bits) - 1;
    }
}

const HASH_BITS: u32 = 15;
const WINDOW: usize = 32768;
const MAX_MATCH: usize = 258;

const LENGTH_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// The Adler-32 checksum of RFC 1950.
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

/// A zlib stream with no compression library behind it: one fixed-Huffman block whose matches come from a greedy LZ77
/// over a 32 KiB window, a table of the last position of each three-byte hash finding the candidate. Slower and
/// looser than zlib, but the same bytes as Lazy Scan's Swift and C# encoders, which the PDF writers need more than the
/// last few percent.
pub fn zlib_fixed(data: &[u8]) -> Vec<u8> {
    let mut bits = DeflateBits::default();
    bits.put(1, 1); // BFINAL
    bits.put(1, 2); // BTYPE 01: fixed Huffman codes
    let count = data.len();
    let mut head = vec![-1i64; 1 << HASH_BITS];
    let mut i = 0;
    while i < count {
        let (mut length, mut distance) = (0, 0);
        if i + 3 <= count {
            let h = hash(data, i);
            let candidate = head[h];
            head[h] = i as i64;
            if candidate >= 0 && i - candidate as usize <= WINDOW {
                let candidate = candidate as usize;
                let limit = MAX_MATCH.min(count - i);
                while length < limit && data[candidate + length] == data[i + length] {
                    length += 1;
                }
                distance = i - candidate;
            }
        }
        if length >= 3 {
            bits.matched(length, distance);
            for k in i + 1..i + length {
                if k + 3 <= count {
                    head[hash(data, k)] = k as i64;
                }
            }
            i += length;
        } else {
            bits.literal(data[i]);
            i += 1;
        }
    }
    bits.code(0, 7); // end of block
    let mut output = vec![0x78, 0x01];
    output.extend(bits.finish());
    output.extend(adler32(data).to_be_bytes());
    output
}

fn hash(data: &[u8], i: usize) -> usize {
    let value = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
    (value.wrapping_mul(2654435761) >> (32 - HASH_BITS)) as usize
}

/// Deflate's bit order: fields least significant bit first, Huffman codes most significant bit first.
#[derive(Default)]
struct DeflateBits {
    bytes: Vec<u8>,
    buffer: u64,
    filled: u32,
}

impl DeflateBits {
    fn put(&mut self, value: u32, count: u32) {
        self.buffer |= (value as u64) << self.filled;
        self.filled += count;
        while self.filled >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.filled -= 8;
        }
    }

    fn code(&mut self, code: u32, length: u32) {
        let reversed = code.reverse_bits() >> (32 - length);
        self.put(reversed, length);
    }

    fn literal(&mut self, value: u8) {
        if value < 144 {
            self.code(0x30 + value as u32, 8);
        } else {
            self.code(0x190 + value as u32 - 144, 9);
        }
    }

    fn matched(&mut self, length: usize, distance: usize) {
        let symbol = LENGTH_BASE.iter().rposition(|&b| b <= length).unwrap();
        if symbol < 23 {
            self.code(symbol as u32 + 1, 7); // symbols 257-279: 7-bit codes 1-23
        } else {
            self.code(0xC0 + symbol as u32 - 23, 8); // symbols 280-285: 8-bit codes 0xC0 on
        }
        self.put((length - LENGTH_BASE[symbol]) as u32, LENGTH_EXTRA[symbol]);
        let slot = DISTANCE_BASE.iter().rposition(|&b| b <= distance).unwrap();
        self.code(slot as u32, 5);
        self.put(
            (distance - DISTANCE_BASE[slot]) as u32,
            DISTANCE_EXTRA[slot],
        );
    }

    fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}
