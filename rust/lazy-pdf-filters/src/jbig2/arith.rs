/* Copyright 2012 Mozilla Foundation
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// Ported from pdf.js (src/core/arithmetic_decoder.js and the integer procedures of src/core/jbig2.js) by way
// of Lazy PDF's C# decoder, and changed: the contexts are plain byte arrays kept with the decoder, bytes
// past the end of the data read as 0xFF, and every value is range checked.

use super::budget::Budget;
use crate::error::{FilterError, Result};

#[rustfmt::skip]
const QE: [i32; 47] = [
    0x5601, 0x3401, 0x1801, 0x0ac1, 0x0521, 0x0221, 0x5601, 0x5401, 0x4801, 0x3801, 0x3001, 0x2401, 0x1c01, 0x1601, 0x5601,
    0x5401, 0x5101, 0x4801, 0x3801, 0x3401, 0x3001, 0x2801, 0x2401, 0x2201, 0x1c01, 0x1801, 0x1601, 0x1401, 0x1201, 0x1101,
    0x0ac1, 0x09c1, 0x08a1, 0x0521, 0x0441, 0x02a1, 0x0221, 0x0141, 0x0111, 0x0085, 0x0049, 0x0025, 0x0015, 0x0009, 0x0005,
    0x0001, 0x5601,
];

#[rustfmt::skip]
const NMPS: [u8; 47] = [
    1, 2, 3, 4, 5, 38, 7, 8, 9, 10, 11, 12, 13, 29, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 45, 46,
];

#[rustfmt::skip]
const NLPS: [u8; 47] = [
    1, 6, 9, 12, 29, 33, 6, 14, 14, 14, 17, 18, 20, 21, 14, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 46,
];

/// The states where the less probable symbol swaps with the more probable one.
fn switches(index: usize) -> bool {
    matches!(index, 0 | 6 | 14)
}

pub(crate) const IADH: usize = 0;
pub(crate) const IADW: usize = 1;
pub(crate) const IAEX: usize = 2;
pub(crate) const IAAI: usize = 3;
pub(crate) const IADT: usize = 4;
pub(crate) const IAFS: usize = 5;
pub(crate) const IADS: usize = 6;
pub(crate) const IAIT: usize = 7;
pub(crate) const IARI: usize = 8;
pub(crate) const IARDW: usize = 9;
pub(crate) const IARDH: usize = 10;
pub(crate) const IARDX: usize = 11;
pub(crate) const IARDY: usize = 12;
const INTEGER_COUNT: usize = 13;

/// The MQ arithmetic decoder of T.88 Annex E (the QM coder of JPEG 2000 Annex C) with the context sets
/// one segment needs: the generic and refinement region contexts and the integer procedures of Annex A.
/// A context is one byte, the state index in the upper seven bits and the more probable symbol in bit 0.
pub(crate) struct Arith<'a> {
    data: &'a [u8],
    end: usize,
    bp: usize,
    chigh: i32,
    clow: i32,
    a: i32,
    ct: i32,
    iaid: Vec<u8>,
    integers: [Option<Vec<u8>>; INTEGER_COUNT],
    /// Generic region contexts (the largest template has 16 bits).
    pub gb: Vec<u8>,
    /// Generic refinement region contexts (the largest template has 13 bits).
    pub gr: Vec<u8>,
}

impl<'a> Arith<'a> {
    /// E.3.5 INITDEC: starts decoding the bytes from `start` up to `end`. A decoder given
    /// `refinement_contexts` only refines (Huffman coded blocks, one for each symbol) and has no use for
    /// the other contexts.
    pub fn new(
        data: &'a [u8],
        start: usize,
        end: usize,
        refinement_contexts: Option<Vec<u8>>,
        budget: Option<&mut Budget>,
    ) -> Result<Self> {
        if let Some(budget) = budget {
            budget.fixed(if refinement_contexts.is_none() {
                80_000
            } else {
                0
            })?;
        }
        let gb = if refinement_contexts.is_none() {
            vec![0; 1 << 16]
        } else {
            Vec::new()
        };
        let mut arith = Self {
            data,
            end: end.min(data.len()),
            bp: start,
            chigh: 0,
            clow: 0,
            a: 0x8000,
            ct: 0,
            iaid: Vec::new(),
            integers: Default::default(),
            gb,
            gr: refinement_contexts.unwrap_or_else(|| vec![0; 1 << 13]),
        };
        arith.chigh = arith.at(start);
        arith.byte_in();
        arith.chigh = ((arith.chigh << 7) & 0xffff) | ((arith.clow >> 9) & 0x7f);
        arith.clow = (arith.clow << 7) & 0xffff;
        arith.ct -= 7;
        Ok(arith)
    }

    fn at(&self, index: usize) -> i32 {
        if index < self.end {
            self.data[index] as i32
        } else {
            0xff
        }
    }

    /// E.3.4 BYTEIN
    fn byte_in(&mut self) {
        if self.at(self.bp) == 0xff {
            if self.at(self.bp + 1) > 0x8f {
                self.clow += 0xff00;
                self.ct = 8;
            } else {
                self.bp += 1;
                self.clow += self.at(self.bp) << 9;
                self.ct = 7;
            }
        } else {
            self.bp += 1;
            self.clow += if self.bp < self.end {
                (self.data[self.bp] as i32) << 8
            } else {
                0xff00
            };
            self.ct = 8;
        }
        if self.clow > 0xffff {
            self.chigh += self.clow >> 16;
            self.clow &= 0xffff;
        }
    }

    /// E.3.2 DECODE: one bit under the context at `pos` of `contexts`.
    pub fn bit(&mut self, contexts: &mut [u8], pos: usize) -> i32 {
        let mut index = (contexts[pos] >> 1) as usize;
        let mut mps = (contexts[pos] & 1) as i32;
        let qe = QE[index];
        let d;
        let mut a = self.a - qe;
        if self.chigh < qe {
            if a < qe {
                a = qe;
                d = mps;
                index = NMPS[index] as usize;
            } else {
                a = qe;
                d = 1 ^ mps;
                if switches(index) {
                    mps = d;
                }
                index = NLPS[index] as usize;
            }
        } else {
            self.chigh -= qe;
            if a & 0x8000 != 0 {
                self.a = a;
                return mps;
            }
            if a < qe {
                d = 1 ^ mps;
                if switches(index) {
                    mps = d;
                }
                index = NLPS[index] as usize;
            } else {
                d = mps;
                index = NMPS[index] as usize;
            }
        }
        // E.3.3 RENORMD
        loop {
            if self.ct == 0 {
                self.byte_in();
            }
            a <<= 1;
            self.chigh = ((self.chigh << 1) & 0xffff) | ((self.clow >> 15) & 1);
            self.clow = (self.clow << 1) & 0xffff;
            self.ct -= 1;
            if a & 0x8000 != 0 {
                break;
            }
        }
        self.a = a;
        contexts[pos] = ((index << 1) as u8) | mps as u8;
        d
    }

    /// A.2: one integer of the procedure whose contexts are `procedure` (`IADH` and so on). None for
    /// the out-of-band value.
    pub fn integer(&mut self, procedure: usize) -> Result<Option<i32>> {
        let mut contexts = self.integers[procedure]
            .take()
            .unwrap_or_else(|| vec![0; 512]);
        let value = self.integer_with(&mut contexts);
        self.integers[procedure] = Some(contexts);
        value
    }

    fn integer_with(&mut self, contexts: &mut [u8]) -> Result<Option<i32>> {
        let mut prev = 1usize;
        let sign = self.read_bits(contexts, &mut prev, 1);
        let mut v: i64 = if self.read_bits(contexts, &mut prev, 1) == 0 {
            self.read_bits(contexts, &mut prev, 2)
        } else if self.read_bits(contexts, &mut prev, 1) == 0 {
            self.read_bits(contexts, &mut prev, 4) + 4
        } else if self.read_bits(contexts, &mut prev, 1) == 0 {
            self.read_bits(contexts, &mut prev, 6) + 20
        } else if self.read_bits(contexts, &mut prev, 1) == 0 {
            self.read_bits(contexts, &mut prev, 8) + 84
        } else if self.read_bits(contexts, &mut prev, 1) == 0 {
            self.read_bits(contexts, &mut prev, 12) + 340
        } else {
            self.read_bits(contexts, &mut prev, 32) + 4436
        };
        if sign != 0 && v == 0 {
            return Ok(None);
        }
        if sign != 0 {
            v = -v;
        }
        i32::try_from(v)
            .map(Some)
            .map_err(|_| FilterError::damaged("A JBIG2 integer is out of range."))
    }

    fn read_bits(&mut self, contexts: &mut [u8], prev: &mut usize, length: u32) -> i64 {
        let mut v = 0i64;
        for _ in 0..length {
            let bit = self.bit(contexts, *prev) as usize;
            *prev = if *prev < 256 {
                (*prev << 1) | bit
            } else {
                (((*prev << 1) | bit) & 511) | 256
            };
            v = (v << 1) | bit as i64;
        }
        v
    }

    /// A.3: a symbol id of `code_length` bits.
    pub fn iaid(&mut self, code_length: u32) -> usize {
        let size = 1usize << (code_length + 1);
        if self.iaid.len() < size {
            self.iaid = vec![0; size];
        }
        let mut contexts = std::mem::take(&mut self.iaid);
        let mut prev = 1usize;
        for _ in 0..code_length {
            prev = (prev << 1) | self.bit(&mut contexts, prev) as usize;
        }
        self.iaid = contexts;
        prev & ((1usize << code_length) - 1)
    }
}
