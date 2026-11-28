//! Big-endian reads that give 0 past the end, so a truncated table reads as zeros. Offsets are signed on purpose:
//! the reference arithmetic can go negative on damaged data and reads there are 0 as well.

#[derive(Clone, Copy)]
pub struct Span<'a>(pub &'a [u8]);

impl<'a> Span<'a> {
    pub fn len(self) -> i64 {
        self.0.len() as i64
    }

    /// The window `count` bytes long from `offset`, cut to what the span holds.
    pub fn slice(self, offset: i64, count: i64) -> Span<'a> {
        let o = offset.clamp(0, self.len());
        let n = count.clamp(0, self.len() - o);
        Span(&self.0[o as usize..(o + n) as usize])
    }

    pub fn from(self, offset: i64) -> Span<'a> {
        self.slice(offset, self.len() - offset.clamp(0, self.len()))
    }

    pub fn u8(self, o: i64) -> i64 {
        if o >= 0 && o < self.len() {
            self.0[o as usize] as i64
        } else {
            0
        }
    }

    pub fn u16(self, o: i64) -> i64 {
        if o >= 0 && o <= self.len() - 2 {
            ((self.0[o as usize] as i64) << 8) | self.0[o as usize + 1] as i64
        } else {
            0
        }
    }

    pub fn i16(self, o: i64) -> i64 {
        self.u16(o) as u16 as i16 as i64
    }

    pub fn u24(self, o: i64) -> i64 {
        if o >= 0 && o <= self.len() - 3 {
            let o = o as usize;
            ((self.0[o] as i64) << 16) | ((self.0[o + 1] as i64) << 8) | self.0[o + 2] as i64
        } else {
            0
        }
    }

    pub fn u32(self, o: i64) -> u32 {
        if o >= 0 && o <= self.len() - 4 {
            let o = o as usize;
            u32::from_be_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
        } else {
            0
        }
    }

    /// A 32-bit offset or length, 0 when it is larger than an i32.
    pub fn offset32(self, o: i64) -> i64 {
        let v = self.u32(o);
        if v > i32::MAX as u32 { 0 } else { v as i64 }
    }

    pub fn tag(self, o: i64) -> String {
        if o < 0 || o > self.len() - 4 {
            return String::new();
        }
        lazy_pdf_syntax::latin1(&self.0[o as usize..o as usize + 4])
    }

    /// The sum of the 32-bit words of the span, the last word padded with zeros (OpenType table checksums).
    pub fn checksum(self) -> u32 {
        let mut sum = 0u32;
        let mut i = 0;
        while i + 4 <= self.len() {
            sum = sum.wrapping_add(self.u32(i));
            i += 4;
        }
        if i < self.len() {
            let mut last = 0u32;
            for k in 0..4 {
                last = (last << 8)
                    | if i + k < self.len() {
                        self.0[(i + k) as usize] as u32
                    } else {
                        0
                    };
            }
            sum = sum.wrapping_add(last);
        }
        sum
    }
}

pub fn put_u16(d: &mut [u8], o: usize, v: i64) {
    d[o] = (v >> 8) as u8;
    d[o + 1] = v as u8;
}

pub fn put_u32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// C# `(int)x` on a double: saturating, NaN as 0.
pub fn to_int32(v: f64) -> i64 {
    v as i32 as i64
}

/// `double.TryParse` for the characters a number is written with.
pub fn parse_double(text: &str) -> Option<f64> {
    let body = text.trim_matches(|c: char| c == ' ' || ('\t'..='\r').contains(&c));
    if body.is_empty() || !body.chars().all(|c| "+-.0123456789eE".contains(c)) {
        return None;
    }
    body.parse().ok()
}

/// Swift min and max on doubles: a NaN first argument stays.
pub fn fmin(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

pub fn fmax(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}
