//! CCITT Group 4 (ITU-T T.6) encoding of a black and white image, bit for bit what Lazy Scan's Swift and C# encoders
//! write: the rows coded as libtiff codes them, ending in EOFB and zero bits up to a byte.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use std::sync::OnceLock;

use crate::error::{FilterError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code {
    pub code: u32,
    pub length: u32,
}

/// The code tables, written as the bit strings of T.4 tables 2 and 3 and T.6 table 1.
pub struct Tables {
    pub vertical: Vec<Code>,
    pub white_terminating: Vec<Code>,
    pub black_terminating: Vec<Code>,
    pub white_make_up: Vec<Code>,
    pub black_make_up: Vec<Code>,
    pub extended_make_up: Vec<Code>,
}

pub const PASS: Code = Code {
    code: 0b0001,
    length: 4,
};
pub const HORIZONTAL: Code = Code {
    code: 0b001,
    length: 3,
};
const EOL: Code = Code {
    code: 1,
    length: 12,
};

fn codes(bits: &[&str]) -> Vec<Code> {
    bits.iter()
        .map(|text| Code {
            code: text.bytes().fold(0, |v, c| v * 2 + (c == b'1') as u32),
            length: text.len() as u32,
        })
        .collect()
}

pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        vertical: codes(&VERTICAL),
        white_terminating: codes(&WHITE_TERMINATING),
        black_terminating: codes(&BLACK_TERMINATING),
        white_make_up: codes(&WHITE_MAKE_UP),
        black_make_up: codes(&BLACK_MAKE_UP),
        extended_make_up: codes(&EXTENDED_MAKE_UP),
    })
}

/// Bytes in one packed row: bits from the most significant, 0 = black.
pub fn stride(width: usize) -> usize {
    width.div_ceil(8)
}

/// The image as G4 data, for a /CCITTFaxDecode stream with /K -1 or a TIFF strip with compression 4.
/// `packed` holds `stride(width)` bytes a row, 0 = black.
pub fn encode_g4(packed: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    if width < 1 {
        return Err(FilterError::unsupported(format!(
            "A G4 image must be at least 1 pixel wide, got {width}."
        )));
    }
    let row_bytes = stride(width);
    let needed = row_bytes
        .checked_mul(height)
        .ok_or_else(FilterError::too_big)?;
    if packed.len() < needed {
        return Err(FilterError::damaged(format!(
            "Packed buffer must be {needed} bytes, got {}.",
            packed.len()
        )));
    }
    let mut bits = Bits::new((needed / 8).max(64));
    // The changing elements of the reference and the coding row: where a pixel differs from the one before it
    // (white before the first). Even entries start black runs, odd ones white runs.
    let columns = width.checked_add(1).ok_or_else(FilterError::too_big)?;
    let mut reference = vec![0usize; columns];
    let mut coding = vec![0usize; columns];
    let mut reference_count = 0;
    for y in 0..height {
        let coding_count = changing_elements(&packed[y * row_bytes..], width, &mut coding);
        encode_row(
            &mut bits,
            &coding[..coding_count],
            &reference[..reference_count],
            width,
        );
        std::mem::swap(&mut reference, &mut coding);
        reference_count = coding_count;
    }
    bits.write(EOL);
    bits.write(EOL);
    Ok(bits.finish())
}

fn changing_elements(row: &[u8], width: usize, changes: &mut [usize]) -> usize {
    let mut count = 0;
    let mut black = false;
    for x in 0..width {
        let pixel = row[x >> 3] & (0x80 >> (x & 7)) == 0;
        if pixel != black {
            changes[count] = x;
            count += 1;
            black = pixel;
        }
    }
    count
}

fn encode_row(bits: &mut Bits, coding: &[usize], reference: &[usize], width: usize) {
    let t = tables();
    let mut a0: isize = -1;
    let mut black = false;
    let mut ia = 0;
    let mut ib = 0;
    loop {
        while ia < coding.len() && coding[ia] as isize <= a0 {
            ia += 1;
        }
        let a1 = coding.get(ia).copied().unwrap_or(width) as isize;
        // b1 is the first changing element right of a0 or the one after it, of the color opposite a0's. One of the
        // wrong color can be b1 again once a vertical mode has changed a0's color, so only those at or left of a0
        // are skipped for good.
        while ib < reference.len() && reference[ib] as isize <= a0 {
            ib += 1;
        }
        let mut k = ib;
        if k < reference.len() && (k % 2 == 0) == black {
            k += 1;
        }
        let b1 = reference.get(k).copied().unwrap_or(width) as isize;
        let b2 = reference.get(k + 1).copied().unwrap_or(width) as isize;
        if b2 < a1 {
            bits.write(PASS);
            a0 = b2;
        } else if (a1 - b1).abs() <= 3 {
            bits.write(t.vertical[(a1 - b1 + 3) as usize]);
            a0 = a1;
            black = !black;
        } else {
            let a2 = coding.get(ia + 1).copied().unwrap_or(width) as isize;
            let start = a0.max(0);
            bits.write(HORIZONTAL);
            write_run(bits, (a1 - start) as usize, black);
            write_run(bits, (a2 - a1) as usize, !black);
            a0 = a2;
        }
        if a0 >= width as isize {
            return;
        }
    }
}

/// One run as Modified Huffman codes: 2560 make-up codes while 2624 or more remain, one make-up code for a remaining
/// multiple of 64, then the terminating code, as libtiff does.
fn write_run(bits: &mut Bits, length: usize, black: bool) {
    let t = tables();
    let (terminating, make_up) = if black {
        (&t.black_terminating, &t.black_make_up)
    } else {
        (&t.white_terminating, &t.white_make_up)
    };
    let largest = t.extended_make_up[t.extended_make_up.len() - 1];
    let mut run = length;
    while run >= 2624 {
        bits.write(largest);
        run -= 2560;
    }
    if run >= 64 {
        let index = run / 64 - 1;
        let code = if index < make_up.len() {
            make_up[index]
        } else {
            t.extended_make_up[index - make_up.len()]
        };
        bits.write(code);
        run -= (index + 1) * 64;
    }
    bits.write(terminating[run]);
}

/// Bits most significant first; the last byte is filled up with zero bits.
struct Bits {
    bytes: Vec<u8>,
    pending: u32,
    pending_bits: u32,
}

impl Bits {
    fn new(capacity: usize) -> Bits {
        Bits {
            bytes: Vec::with_capacity(capacity),
            pending: 0,
            pending_bits: 0,
        }
    }

    fn write(&mut self, code: Code) {
        // At most 13 bits come in and fewer than 8 wait.
        let mask = (1u32 << code.length) - 1;
        self.pending = self.pending << code.length | code.code & mask;
        self.pending_bits += code.length;
        while self.pending_bits >= 8 {
            self.pending_bits -= 8;
            self.bytes.push((self.pending >> self.pending_bits) as u8);
        }
        self.pending &= (1u32 << self.pending_bits) - 1;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.pending_bits > 0 {
            self.bytes
                .push((self.pending << (8 - self.pending_bits)) as u8);
        }
        self.bytes
    }
}

const VERTICAL: [&str; 7] = ["0000010", "000010", "010", "1", "011", "000011", "0000011"];

const WHITE_TERMINATING: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100",
    "00111", "01000", "001000", "000011", "110100", "110101", "101010", "101011", "0100111",
    "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011",
    "0100100", "0011000", "00000010", "00000011", "00011010", "00011011", "00010010", "00010011",
    "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011",
    "00101100", "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011",
    "01010100", "01010101", "00100100", "00100101", "01011000", "01011001", "01011010", "01011011",
    "01001010", "01001011", "00110010", "00110011", "00110100",
];

const BLACK_TERMINATING: [&str; 64] = [
    "0000110111",
    "010",
    "11",
    "10",
    "011",
    "0011",
    "0010",
    "00011",
    "000101",
    "000100",
    "0000100",
    "0000101",
    "0000111",
    "00000100",
    "00000111",
    "000011000",
    "0000010111",
    "0000011000",
    "0000001000",
    "00001100111",
    "00001101000",
    "00001101100",
    "00000110111",
    "00000101000",
    "00000010111",
    "00000011000",
    "000011001010",
    "000011001011",
    "000011001100",
    "000011001101",
    "000001101000",
    "000001101001",
    "000001101010",
    "000001101011",
    "000011010010",
    "000011010011",
    "000011010100",
    "000011010101",
    "000011010110",
    "000011010111",
    "000001101100",
    "000001101101",
    "000011011010",
    "000011011011",
    "000001010100",
    "000001010101",
    "000001010110",
    "000001010111",
    "000001100100",
    "000001100101",
    "000001010010",
    "000001010011",
    "000000100100",
    "000000110111",
    "000000111000",
    "000000100111",
    "000000101000",
    "000001011000",
    "000001011001",
    "000000101011",
    "000000101100",
    "000001011010",
    "000001100110",
    "000001100111",
];

const WHITE_MAKE_UP: [&str; 27] = [
    "11011",
    "10010",
    "010111",
    "0110111",
    "00110110",
    "00110111",
    "01100100",
    "01100101",
    "01101000",
    "01100111",
    "011001100",
    "011001101",
    "011010010",
    "011010011",
    "011010100",
    "011010101",
    "011010110",
    "011010111",
    "011011000",
    "011011001",
    "011011010",
    "011011011",
    "010011000",
    "010011001",
    "010011010",
    "011000",
    "010011011",
];

const BLACK_MAKE_UP: [&str; 27] = [
    "0000001111",
    "000011001000",
    "000011001001",
    "000001011011",
    "000000110011",
    "000000110100",
    "000000110101",
    "0000001101100",
    "0000001101101",
    "0000001001010",
    "0000001001011",
    "0000001001100",
    "0000001001101",
    "0000001110010",
    "0000001110011",
    "0000001110100",
    "0000001110101",
    "0000001110110",
    "0000001110111",
    "0000001010010",
    "0000001010011",
    "0000001010100",
    "0000001010101",
    "0000001011010",
    "0000001011011",
    "0000001100100",
    "0000001100101",
];

const EXTENDED_MAKE_UP: [&str; 13] = [
    "00000001000",
    "00000001100",
    "00000001101",
    "000000010010",
    "000000010011",
    "000000010100",
    "000000010101",
    "000000010110",
    "000000010111",
    "000000011100",
    "000000011101",
    "000000011110",
    "000000011111",
];
