// Ported from pdf.js (src/core/jbig2.js: Reader, HuffmanTable, decodeTablesSegment, getStandardTable) by way
// of Lazy PDF's C# decoder, and changed: codes are assigned by B.3 for the standard tables too and decoded
// by length instead of through a tree, and prefix lengths, range lengths and line counts are checked.
// See budget.rs for the licence.

use std::sync::{Arc, OnceLock};

use crate::error::{FilterError, Result};

/// Bits of a segment's data, most significant first; reading past its end is damage.
pub(crate) struct Reader<'a> {
    pub data: &'a [u8],
    /// The next byte to start on; with a byte only partly read it is the one after it.
    pub position: usize,
    pub end: usize,
    shift: i32,
    current: u32,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], start: usize, end: usize) -> Self {
        Self {
            data,
            position: start,
            end: end.min(data.len()),
            shift: -1,
            current: 0,
        }
    }

    pub fn read_bit(&mut self) -> Result<u32> {
        if self.shift < 0 {
            if self.position >= self.end {
                return Err(FilterError::damaged(
                    "A JBIG2 segment ends in the middle of its data.",
                ));
            }
            self.current = self.data[self.position] as u32;
            self.position += 1;
            self.shift = 7;
        }
        let bit = (self.current >> self.shift) & 1;
        self.shift -= 1;
        Ok(bit)
    }

    /// Up to 32 bits as one number.
    pub fn read_bits(&mut self, count: u32) -> Result<u64> {
        let mut result = 0u64;
        for _ in 0..count {
            result = (result << 1) | self.read_bit()? as u64;
        }
        Ok(result)
    }

    pub fn byte_align(&mut self) {
        self.shift = -1;
    }
}

const NORMAL: u8 = 0;
const LOWER: u8 = 1;
const OUT_OF_BAND: u8 = 3;
const MAX_PREFIX_LENGTH: usize = 32;
const MAX_CUSTOM_LINES: usize = 1 << 12;

/// A line of a table: range low, prefix length, range length and kind. A line with prefix length 0 is not used.
type Line = (i64, usize, u32, u8);

/// A prefix code table (Annex B) of values with a range of offset bits each, and maybe an out-of-band value.
pub(crate) struct Huffman {
    low: Vec<i64>,
    range_length: Vec<u32>,
    kind: Vec<u8>,
    by_length: Vec<usize>,
    first: Vec<i64>,
    count: Vec<usize>,
    start: Vec<usize>,
    longest: usize,
}

impl Huffman {
    pub fn new(lines: &[Line]) -> Result<Self> {
        let n = lines.len();
        let mut table = Self {
            low: Vec::with_capacity(n),
            range_length: Vec::with_capacity(n),
            kind: Vec::with_capacity(n),
            by_length: Vec::new(),
            first: vec![0; MAX_PREFIX_LENGTH + 1],
            count: vec![0; MAX_PREFIX_LENGTH + 1],
            start: vec![0; MAX_PREFIX_LENGTH + 1],
            longest: 0,
        };
        for &(low, prefix, range, kind) in lines {
            if prefix > MAX_PREFIX_LENGTH || range > 32 {
                return Err(FilterError::damaged(
                    "A JBIG2 Huffman table has a code length out of range.",
                ));
            }
            table.low.push(low);
            table.range_length.push(range);
            table.kind.push(kind);
            table.count[prefix] += 1;
            table.longest = table.longest.max(prefix);
        }
        table.count[0] = 0;
        // B.3: codes of one length are consecutive, in the order of the lines.
        let mut first = 0i64;
        let mut at = 0;
        for length in 1..=table.longest {
            first = (first + table.count[length - 1] as i64) << 1;
            if first + table.count[length] as i64 > 1i64 << length {
                return Err(FilterError::damaged(
                    "A JBIG2 Huffman table has more codes than fit.",
                ));
            }
            table.first[length] = first;
            table.start[length] = at;
            at += table.count[length];
        }
        table.by_length = vec![0; at];
        let mut next = vec![0usize; MAX_PREFIX_LENGTH + 1];
        for (i, line) in lines.iter().enumerate() {
            let length = line.1;
            if length == 0 {
                continue;
            }
            table.by_length[table.start[length] + next[length]] = i;
            next[length] += 1;
        }
        Ok(table)
    }

    /// A value, or None for the out-of-band value.
    pub fn decode(&self, reader: &mut Reader) -> Result<Option<i32>> {
        let mut code = 0i64;
        for length in 1..=self.longest {
            code = (code << 1) | reader.read_bit()? as i64;
            let index = code - self.first[length];
            if index < 0 || index >= self.count[length] as i64 {
                continue;
            }
            let line = self.by_length[self.start[length] + index as usize];
            if self.kind[line] == OUT_OF_BAND {
                return Ok(None);
            }
            let offset = reader.read_bits(self.range_length[line])? as i64;
            let result = if self.kind[line] == LOWER {
                self.low[line] - offset
            } else {
                self.low[line] + offset
            };
            return i32::try_from(result)
                .map(Some)
                .map_err(|_| FilterError::damaged("A JBIG2 Huffman value is out of range."));
        }
        Err(FilterError::damaged(
            "A JBIG2 Huffman code is not in its table.",
        ))
    }

    /// B.2: the table a tables segment holds.
    pub fn custom(data: &[u8], start: usize, end: usize) -> Result<Self> {
        if end.saturating_sub(start) < 9 || end > data.len() {
            return Err(FilterError::damaged("A JBIG2 tables segment is cut off."));
        }
        let flags = data[start] as u32;
        let be = |at: usize| {
            i32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as i64
        };
        let (lowest, highest) = (be(start + 1), be(start + 5));
        let mut reader = Reader::new(data, start + 9, end);
        let prefix_bits = ((flags >> 1) & 7) + 1;
        let range_bits = ((flags >> 4) & 7) + 1;
        let mut lines: Vec<Line> = Vec::new();
        let mut current = lowest;
        loop {
            let prefix = reader.read_bits(prefix_bits)? as usize;
            let range = reader.read_bits(range_bits)? as u32;
            if range > 32 {
                return Err(FilterError::damaged(
                    "A JBIG2 Huffman table has a range out of range.",
                ));
            }
            lines.push((current, prefix, range, NORMAL));
            if lines.len() > MAX_CUSTOM_LINES {
                return Err(FilterError::too_big());
            }
            current += 1i64 << range;
            if current >= highest {
                break;
            }
        }
        lines.push((
            lowest - 1,
            reader.read_bits(prefix_bits)? as usize,
            32,
            LOWER,
        ));
        lines.push((highest, reader.read_bits(prefix_bits)? as usize, 32, NORMAL));
        if flags & 1 != 0 {
            lines.push((0, reader.read_bits(prefix_bits)? as usize, 0, OUT_OF_BAND));
        }
        Self::new(&lines)
    }

    /// The table of the symbol ids of a text region (7.4.3.1.7): symbol i has a code of `lengths[i]` bits.
    pub fn for_lengths(lengths: &[usize]) -> Result<Self> {
        let lines: Vec<Line> = lengths
            .iter()
            .enumerate()
            .map(|(i, &length)| (i as i64, length, 0, NORMAL))
            .collect();
        Self::new(&lines)
    }

    /// Table B.`number`, 1 to 15.
    pub fn standard(number: usize) -> Arc<Huffman> {
        static TABLES: OnceLock<Vec<Arc<Huffman>>> = OnceLock::new();
        TABLES.get_or_init(|| {
            STANDARD_LINES
                .iter()
                .map(|lines| {
                    Arc::new(Huffman::new(lines).expect("the standard tables are consistent"))
                })
                .collect()
        })[number - 1]
            .clone()
    }
}

// Annex B.5: (range low, prefix length, range length, kind) for each line in the order of the standard, with
// the lower range line as kind 1 and the out-of-band line as kind 3. The last range of a table has 32 offset bits.
#[rustfmt::skip]
static STANDARD_LINES: [&[Line]; 15] = [
    // B.1
    &[(0, 1, 4, 0), (16, 2, 8, 0), (272, 3, 16, 0), (65808, 3, 32, 0)],
    // B.2
    &[(0, 1, 0, 0), (1, 2, 0, 0), (2, 3, 0, 0), (3, 4, 3, 0), (11, 5, 6, 0), (75, 6, 32, 0), (0, 6, 0, 3)],
    // B.3
    &[(-256, 8, 8, 0), (0, 1, 0, 0), (1, 2, 0, 0), (2, 3, 0, 0), (3, 4, 3, 0), (11, 5, 6, 0), (-257, 8, 32, 1),
      (75, 7, 32, 0), (0, 6, 0, 3)],
    // B.4
    &[(1, 1, 0, 0), (2, 2, 0, 0), (3, 3, 0, 0), (4, 4, 3, 0), (12, 5, 6, 0), (76, 5, 32, 0)],
    // B.5
    &[(-255, 7, 8, 0), (1, 1, 0, 0), (2, 2, 0, 0), (3, 3, 0, 0), (4, 4, 3, 0), (12, 5, 6, 0), (-256, 7, 32, 1),
      (76, 6, 32, 0)],
    // B.6
    &[(-2048, 5, 10, 0), (-1024, 4, 9, 0), (-512, 4, 8, 0), (-256, 4, 7, 0), (-128, 5, 6, 0), (-64, 5, 5, 0),
      (-32, 4, 5, 0), (0, 2, 7, 0), (128, 3, 7, 0), (256, 3, 8, 0), (512, 4, 9, 0), (1024, 4, 10, 0),
      (-2049, 6, 32, 1), (2048, 6, 32, 0)],
    // B.7
    &[(-1024, 4, 9, 0), (-512, 3, 8, 0), (-256, 4, 7, 0), (-128, 5, 6, 0), (-64, 5, 5, 0), (-32, 4, 5, 0),
      (0, 4, 5, 0), (32, 5, 5, 0), (64, 5, 6, 0), (128, 4, 7, 0), (256, 3, 8, 0), (512, 3, 9, 0),
      (1024, 3, 10, 0), (-1025, 5, 32, 1), (2048, 5, 32, 0)],
    // B.8
    &[(-15, 8, 3, 0), (-7, 9, 1, 0), (-5, 8, 1, 0), (-3, 9, 0, 0), (-2, 7, 0, 0), (-1, 4, 0, 0), (0, 2, 1, 0),
      (2, 5, 0, 0), (3, 6, 0, 0), (4, 3, 4, 0), (20, 6, 1, 0), (22, 4, 4, 0), (38, 4, 5, 0), (70, 5, 6, 0),
      (134, 5, 7, 0), (262, 6, 7, 0), (390, 7, 8, 0), (646, 6, 10, 0), (-16, 9, 32, 1), (1670, 9, 32, 0),
      (0, 2, 0, 3)],
    // B.9
    &[(-31, 8, 4, 0), (-15, 9, 2, 0), (-11, 8, 2, 0), (-7, 9, 1, 0), (-5, 7, 1, 0), (-3, 4, 1, 0),
      (-1, 3, 1, 0), (1, 3, 1, 0), (3, 5, 1, 0), (5, 6, 1, 0), (7, 3, 5, 0), (39, 6, 2, 0), (43, 4, 5, 0),
      (75, 4, 6, 0), (139, 5, 7, 0), (267, 5, 8, 0), (523, 6, 8, 0), (779, 7, 9, 0), (1291, 6, 11, 0),
      (-32, 9, 32, 1), (3339, 9, 32, 0), (0, 2, 0, 3)],
    // B.10
    &[(-21, 7, 4, 0), (-5, 8, 0, 0), (-4, 7, 0, 0), (-3, 5, 0, 0), (-2, 2, 2, 0), (2, 5, 0, 0), (3, 6, 0, 0),
      (4, 7, 0, 0), (5, 8, 0, 0), (6, 2, 6, 0), (70, 5, 5, 0), (102, 6, 5, 0), (134, 6, 6, 0), (198, 6, 7, 0),
      (326, 6, 8, 0), (582, 6, 9, 0), (1094, 6, 10, 0), (2118, 7, 11, 0), (-22, 8, 32, 1), (4166, 8, 32, 0),
      (0, 2, 0, 3)],
    // B.11
    &[(1, 1, 0, 0), (2, 2, 1, 0), (4, 4, 0, 0), (5, 4, 1, 0), (7, 5, 1, 0), (9, 5, 2, 0), (13, 6, 2, 0),
      (17, 7, 2, 0), (21, 7, 3, 0), (29, 7, 4, 0), (45, 7, 5, 0), (77, 7, 6, 0), (141, 7, 32, 0)],
    // B.12
    &[(1, 1, 0, 0), (2, 2, 0, 0), (3, 3, 1, 0), (5, 5, 0, 0), (6, 5, 1, 0), (8, 6, 1, 0), (10, 7, 0, 0),
      (11, 7, 1, 0), (13, 7, 2, 0), (17, 7, 3, 0), (25, 7, 4, 0), (41, 8, 5, 0), (73, 8, 32, 0)],
    // B.13
    &[(1, 1, 0, 0), (2, 3, 0, 0), (3, 4, 0, 0), (4, 5, 0, 0), (5, 4, 1, 0), (7, 3, 3, 0), (15, 6, 1, 0),
      (17, 6, 2, 0), (21, 6, 3, 0), (29, 6, 4, 0), (45, 6, 5, 0), (77, 7, 6, 0), (141, 7, 32, 0)],
    // B.14
    &[(-2, 3, 0, 0), (-1, 3, 0, 0), (0, 1, 0, 0), (1, 3, 0, 0), (2, 3, 0, 0)],
    // B.15
    &[(-24, 7, 4, 0), (-8, 6, 2, 0), (-4, 5, 1, 0), (-2, 4, 0, 0), (-1, 3, 0, 0), (0, 1, 0, 0), (1, 3, 0, 0),
      (2, 4, 0, 0), (3, 5, 1, 0), (5, 6, 2, 0), (9, 7, 4, 0), (-25, 7, 32, 1), (25, 7, 32, 0)],
];
