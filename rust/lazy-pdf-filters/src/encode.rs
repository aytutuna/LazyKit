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
