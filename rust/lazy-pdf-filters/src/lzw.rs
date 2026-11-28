use crate::error::Result;
use crate::output::Output;

pub(crate) const CLEAR: usize = 256;
pub(crate) const END: usize = 257;
pub(crate) const FIRST: usize = 258;
pub(crate) const TABLE_SIZE: usize = 4096;

struct Table {
    prefix: [u16; TABLE_SIZE],
    suffix: [u8; TABLE_SIZE],
    first: [u8; TABLE_SIZE],
    length: [u16; TABLE_SIZE],
}

impl Table {
    fn new() -> Self {
        let mut table = Self {
            prefix: [0; TABLE_SIZE],
            suffix: [0; TABLE_SIZE],
            first: [0; TABLE_SIZE],
            length: [0; TABLE_SIZE],
        };
        for i in 0..256 {
            table.suffix[i] = i as u8;
            table.first[i] = i as u8;
            table.length[i] = 1;
        }
        table
    }

    fn add(&mut self, index: usize, previous: usize, head: u8) {
        self.prefix[index] = previous as u16;
        self.suffix[index] = head;
        self.first[index] = self.first[previous];
        self.length[index] = self.length[previous] + 1;
    }

    fn write(&self, output: &mut Output, mut code: usize) -> Result<()> {
        let span = output.append(self.length[code] as usize)?;
        for slot in span.iter_mut().rev() {
            *slot = self.suffix[code];
            code = self.prefix[code] as usize;
        }
        Ok(())
    }
}

/// LZWDecode (ISO 32000-1 7.4.4): codes of 9 to 12 bits, most significant first, 256 clears the table,
/// 257 ends the data. Corrupt data (a code beyond the table) ends the output where it is.
pub fn decode(data: &[u8], early_change: bool, max_size: u64) -> Result<Vec<u8>> {
    let mut output = Output::new(max_size, data.len().saturating_mul(3));
    let mut table = Table::new();
    let early = early_change as usize;
    let (mut bits, mut next) = (9usize, FIRST);
    let mut previous: Option<usize> = None;
    let mut pending = 0u32;
    let mut pending_bits = 0usize;
    for &b in data {
        pending = pending << 8 | b as u32;
        pending_bits += 8;
        while pending_bits >= bits {
            let code = (pending >> (pending_bits - bits)) as usize & ((1 << bits) - 1);
            pending_bits -= bits;
            pending &= (1u32 << pending_bits) - 1;
            if code == CLEAR {
                bits = 9;
                next = FIRST;
                previous = None;
                continue;
            }
            if code == END {
                return Ok(output.into_vec());
            }
            let Some(prev) = previous else {
                if code > 255 {
                    return Ok(output.into_vec());
                }
                output.push(code as u8)?;
                previous = Some(code);
                continue;
            };
            let head;
            if code < next && !(256..FIRST).contains(&code) {
                head = table.first[code];
                table.write(&mut output, code)?;
            } else if code == next && next < TABLE_SIZE {
                head = table.first[prev];
                table.add(next, prev, head);
                table.write(&mut output, code)?;
                next += 1;
                previous = Some(code);
                if next + early >= 1 << bits && bits < 12 {
                    bits += 1;
                }
                continue;
            } else {
                return Ok(output.into_vec());
            }
            if next < TABLE_SIZE {
                table.add(next, prev, head);
                next += 1;
                if next + early >= 1 << bits && bits < 12 {
                    bits += 1;
                }
            }
            previous = Some(code);
        }
    }
    Ok(output.into_vec())
}
