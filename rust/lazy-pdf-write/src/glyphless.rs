//! The font of a PDF's invisible text layer: a tiny TrueType program whose only glyph draws nothing and is half an em wide,
//! embedded as a two-byte (Identity-H) composite font in which every character code shows that glyph. A character of the
//! Basic Multilingual Plane is written as its own UTF-16 code unit; the few characters beyond it get a code of the
//! surrogate range, which never stands for a character on its own. Everything is built from code, deterministically, the
//! same bytes as Lazy Scan's Swift and C# writers.

use std::collections::HashMap;
use std::sync::OnceLock;

use lazy_pdf_filters::encode::adler32;

pub const FONT_NAME: &str = "GlyphLessFont";
pub const UNITS_PER_EM: i32 = 1000;
/// The advance of every glyph, in 1/1000 em: the PDF's /DW and the program's hmtx agree.
pub const GLYPH_WIDTH: i32 = 500;
/// Character codes: 0 to this (two bytes).
pub const CODE_COUNT: usize = 0x10000;

/// How wide `code_count` codes are set at `font_size`: half the size each.
pub fn text_width(code_count: usize, font_size: f64) -> f64 {
    code_count as f64 * (GLYPH_WIDTH as f64 / 1000.0) * font_size
}

/// The TrueType program (/FontFile2), built once.
pub fn program() -> &'static [u8] {
    static PROGRAM: OnceLock<Vec<u8>> = OnceLock::new();
    PROGRAM.get_or_init(build)
}

/// The /CIDToGIDMap stream's data, zlib-compressed: every code to glyph 1, so no code ever shows the .notdef glyph.
pub fn cid_to_gid_map() -> &'static [u8] {
    static MAP: OnceLock<Vec<u8>> = OnceLock::new();
    MAP.get_or_init(build_cid_to_gid_map)
}

#[derive(Default)]
struct BigEndian(Vec<u8>);

impl BigEndian {
    fn u16(&mut self, value: u32) {
        self.0.extend((value as u16).to_be_bytes());
    }
    fn i16(&mut self, value: i32) {
        self.0.extend((value as i16).to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend(value.to_be_bytes());
    }
    fn bytes(&mut self, data: &[u8]) {
        self.0.extend(data);
    }
}

fn padded(length: usize) -> usize {
    (length + 3) & !3
}

/// The TrueType table checksum: the sum of its big-endian 32-bit words, zero-padded.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A TrueType font with two glyphs, both empty: .notdef and glyph 1, each 500 units wide on a 1000-unit em, ascent 1000
/// and descent 0 (so a line's text fills its box from the baseline up).
fn build() -> Vec<u8> {
    let tables: Vec<(&str, Vec<u8>)> = vec![
        ("OS/2", os2()),
        ("cmap", cmap()),
        ("glyf", vec![0; 4]), // both glyphs are empty (loca gives them no bytes); non-empty for strict readers
        ("head", head()),
        ("hhea", hhea()),
        ("hmtx", hmtx()),
        ("loca", loca()),
        ("maxp", maxp()),
        ("name", name()),
        ("post", post()),
    ];
    let count = tables.len() as u32;
    let (mut search_power, mut entry_selector) = (1u32, 0u32);
    while search_power * 2 <= count {
        search_power *= 2;
        entry_selector += 1;
    }
    let mut font = BigEndian::default();
    font.u32(0x0001_0000);
    font.u16(count);
    font.u16(search_power * 16);
    font.u16(entry_selector);
    font.u16(count * 16 - search_power * 16);
    let mut offset = 12 + tables.len() * 16;
    let mut head_offset = 0;
    for (tag, data) in &tables {
        font.bytes(tag.as_bytes());
        font.u32(checksum(data));
        font.u32(offset as u32);
        font.u32(data.len() as u32);
        if *tag == "head" {
            head_offset = offset;
        }
        offset += padded(data.len());
    }
    for (_, data) in &tables {
        font.bytes(data);
        font.bytes(&vec![0; padded(data.len()) - data.len()]);
    }
    let mut bytes = font.0;
    // head.checkSumAdjustment (bytes 8-11 of the table) makes the whole file sum to 0xB1B0AFBA.
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&bytes));
    bytes[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    bytes
}

fn os2() -> Vec<u8> {
    let mut t = BigEndian::default();
    t.u16(4); // version
    t.i16(GLYPH_WIDTH); // xAvgCharWidth
    t.u16(400); // usWeightClass: regular
    t.u16(5); // usWidthClass: medium
    t.u16(0); // fsType: installable, no embedding restrictions
    for v in [650, 600, 0, 75] {
        t.i16(v); // subscript x size, y size, x offset, y offset
    }
    for v in [650, 600, 0, 350] {
        t.i16(v); // superscript x size, y size, x offset, y offset
    }
    t.i16(50);
    t.i16(250); // strikeout size, position
    t.i16(0); // sFamilyClass
    t.bytes(&[0; 10]); // panose: any
    for _ in 0..4 {
        t.u32(0); // ulUnicodeRange1-4
    }
    t.bytes(b"NONE"); // achVendID
    t.u16(0x0040); // fsSelection: regular
    t.u16(0x0020); // usFirstCharIndex
    t.u16(0x0020); // usLastCharIndex
    t.i16(UNITS_PER_EM);
    t.i16(0);
    t.i16(0); // typo ascender, descender, line gap
    t.u16(UNITS_PER_EM as u32);
    t.u16(0); // win ascent, descent
    t.u32(1);
    t.u32(0); // ulCodePageRange1-2: Latin 1
    t.i16(500);
    t.i16(UNITS_PER_EM); // sxHeight, sCapHeight
    t.u16(0);
    t.u16(0x0020);
    t.u16(1); // usDefaultChar, usBreakChar, usMaxContext
    t.0
}

fn cmap() -> Vec<u8> {
    // One Windows Unicode (3, 1) subtable, format 4: U+0020 to glyph 1, then the required 0xFFFF segment. A PDF reaches
    // the glyph through the CIDToGIDMap, never through this.
    let mut t = BigEndian::default();
    t.u16(0); // version
    t.u16(1); // one encoding record
    t.u16(3);
    t.u16(1);
    t.u32(12);
    let seg_count = 2;
    t.u16(4); // format
    t.u16(16 + seg_count * 8); // length
    t.u16(0); // language
    t.u16(seg_count * 2); // segCountX2
    t.u16(2 * 2); // searchRange: 2 x 2^floor(log2(segCount))
    t.u16(1); // entrySelector
    t.u16(0); // rangeShift
    t.u16(0x0020);
    t.u16(0xFFFF); // endCode
    t.u16(0); // reservedPad
    t.u16(0x0020);
    t.u16(0xFFFF); // startCode
    t.i16(1 - 0x0020);
    t.i16(1); // idDelta
    t.u16(0);
    t.u16(0); // idRangeOffset
    t.0
}

fn head() -> Vec<u8> {
    let mut t = BigEndian::default();
    t.u32(0x0001_0000); // version 1.0
    t.u32(0x0001_0000); // fontRevision 1.0
    t.u32(0); // checkSumAdjustment: set once the whole font is laid out
    t.u32(0x5F0F_3CF5); // magicNumber
    t.u16(0x000B); // flags: baseline at y = 0, left sidebearing at x = 0, integer scaling
    t.u16(UNITS_PER_EM as u32);
    t.u32(0);
    t.u32(0); // created: fixed, so the bytes never change
    t.u32(0);
    t.u32(0); // modified
    t.i16(0);
    t.i16(0);
    t.i16(GLYPH_WIDTH);
    t.i16(UNITS_PER_EM); // xMin, yMin, xMax, yMax
    t.u16(0); // macStyle
    t.u16(8); // lowestRecPPEM
    t.i16(2); // fontDirectionHint
    t.i16(0); // indexToLocFormat: short offsets
    t.i16(0); // glyphDataFormat
    t.0
}

fn hhea() -> Vec<u8> {
    let mut t = BigEndian::default();
    t.u32(0x0001_0000);
    t.i16(UNITS_PER_EM); // ascender
    t.i16(0); // descender
    t.i16(0); // lineGap
    t.u16(GLYPH_WIDTH as u32); // advanceWidthMax
    t.i16(0);
    t.i16(0); // minLeftSideBearing, minRightSideBearing
    t.i16(0); // xMaxExtent
    t.i16(1);
    t.i16(0);
    t.i16(0); // caretSlopeRise, caretSlopeRun, caretOffset
    for _ in 0..4 {
        t.i16(0); // reserved
    }
    t.i16(0); // metricDataFormat
    t.u16(2); // numberOfHMetrics
    t.0
}

fn hmtx() -> Vec<u8> {
    let mut t = BigEndian::default();
    for _ in 0..2 {
        t.u16(GLYPH_WIDTH as u32);
        t.i16(0);
    }
    t.0
}

fn loca() -> Vec<u8> {
    let mut t = BigEndian::default();
    for _ in 0..3 {
        t.u16(0); // short offsets (bytes / 2): both glyphs empty
    }
    t.0
}

fn maxp() -> Vec<u8> {
    let mut t = BigEndian::default();
    t.u32(0x0001_0000); // version 1.0 (TrueType outlines)
    t.u16(2); // numGlyphs
    for v in [0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        // maxPoints, maxContours, maxCompositePoints, maxCompositeContours, maxZones, twilight points, storage,
        // function defs, instruction defs, maxStackElements, maxSizeOfInstructions, maxComponentElements, depth
        t.u16(v);
    }
    t.0
}

fn name() -> Vec<u8> {
    let names: [(u32, &str); 4] = [
        (1, FONT_NAME),
        (2, "Regular"),
        (4, FONT_NAME),
        (6, FONT_NAME),
    ];
    let mut strings = BigEndian::default();
    let mut records = BigEndian::default();
    for (id, text) in names {
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        records.u16(3);
        records.u16(1);
        records.u16(0x0409);
        records.u16(id);
        records.u16(utf16.len() as u32);
        records.u16(strings.0.len() as u32);
        strings.bytes(&utf16);
    }
    let mut t = BigEndian::default();
    t.u16(0); // format
    t.u16(names.len() as u32);
    t.u16(6 + names.len() as u32 * 12); // string storage offset
    t.bytes(&records.0);
    t.bytes(&strings.0);
    t.0
}

fn post() -> Vec<u8> {
    let mut t = BigEndian::default();
    t.u32(0x0003_0000); // format 3: no glyph names
    t.u32(0); // italicAngle
    t.i16(-100);
    t.i16(50); // underlinePosition, underlineThickness
    t.u32(1); // isFixedPitch
    for _ in 0..4 {
        t.u32(0); // memory hints
    }
    t.0
}

/// The map's uncompressed data: two bytes per code, each the glyph 0x0001.
pub fn cid_to_gid_map_data() -> Vec<u8> {
    let mut data = vec![0u8; CODE_COUNT * 2];
    for i in (1..data.len()).step_by(2) {
        data[i] = 1;
    }
    data
}

/// The map zlib-compressed by hand: one fixed-Huffman deflate block holding the literals 00 01, then back-references of
/// 258 bytes at distance 2 (and one of 6) for the rest, the end of block, and the Adler-32 of the data. About 830 bytes
/// for 128 KiB, with no compression library involved.
fn build_cid_to_gid_map() -> Vec<u8> {
    let mut bits = DeflateBits::default();
    bits.bits(1, 1); // BFINAL
    bits.bits(1, 2); // BTYPE 01: fixed Huffman codes
    bits.code(0x30, 8); // literal 0x00
    bits.code(0x31, 8); // literal 0x01
    let remaining = CODE_COUNT * 2 - 2; // 131070 = 508 x 258 + 6
    for _ in 0..remaining / 258 {
        bits.matched(258);
    }
    if !remaining.is_multiple_of(258) {
        bits.matched(remaining % 258);
    }
    bits.code(0, 7); // end of block
    let mut output = vec![0x78, 0x01];
    output.extend(bits.finish());
    output.extend(adler32(&cid_to_gid_map_data()).to_be_bytes());
    output
}

/// Deflate's bit order: fields least significant bit first, Huffman codes most significant bit first.
#[derive(Default)]
struct DeflateBits {
    bytes: Vec<u8>,
    current: u32,
    count: u32,
}

impl DeflateBits {
    fn bits(&mut self, value: u32, count: u32) {
        for i in 0..count {
            self.current |= (value >> i & 1) << self.count;
            self.count += 1;
            if self.count == 8 {
                self.bytes.push(self.current as u8);
                self.current = 0;
                self.count = 0;
            }
        }
    }

    fn code(&mut self, code: u32, length: u32) {
        for i in (0..length).rev() {
            self.bits(code >> i & 1, 1);
        }
    }

    /// A length of 3-10 or 258 (no extra bits) at distance 2 (distance code 1).
    fn matched(&mut self, length: usize) {
        if length == 258 {
            self.code(0xC0 + (285 - 280), 8); // symbols 280-287: 8-bit codes 0xC0 on
        } else if (3..=10).contains(&length) {
            self.code(length as u32 - 3 + 1, 7); // symbols 257-264: 7-bit codes 1-8
        } else {
            panic!("a match of {length} is not made here");
        }
        self.code(1, 5); // distance code 1: a distance of 2
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.current as u8);
        }
        self.bytes
    }
}

/// The part of every ToUnicode map that never changes: each code outside the surrogate range D800-DFFF maps to the same
/// UTF-16 code unit, as 248 ranges of 256 codes in blocks of at most 100, as CMaps require.
fn identity_ranges() -> &'static str {
    static RANGES: OnceLock<String> = OnceLock::new();
    RANGES.get_or_init(|| {
        let high_bytes: Vec<u32> = (0..256).filter(|h| !(0xD8..=0xDF).contains(h)).collect();
        let mut out = String::new();
        for block in high_bytes.chunks(100) {
            out.push_str(&format!("{} beginbfrange\n", block.len()));
            for high in block {
                out.push_str(&format!("<{high:02X}00> <{high:02X}FF> <{high:02X}00>\n"));
            }
            out.push_str("endbfrange\n");
        }
        out
    })
}

/// The character codes of one document's text layer. A character of the Basic Multilingual Plane is its own UTF-16 code
/// unit. Each character beyond it gets a code of its own from D800 up, in the order the document first uses it, since a
/// surrogate never stands for a character alone; the ToUnicode map gives that code back as the pair. A document with
/// more than 2048 such characters writes the rest as U+FFFD.
#[derive(Default)]
pub struct Codes {
    codes: HashMap<u32, u32>,
    supplementary: Vec<u32>,
}

pub const FIRST_SUPPLEMENTARY_CODE: u32 = 0xD800;
pub const SUPPLEMENTARY_CAPACITY: usize = 0x800;
pub const REPLACEMENT_CODE: u32 = 0xFFFD;

impl Codes {
    /// The codes for these texts: every character beyond the BMP in them, in order of first use.
    pub fn for_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Codes {
        let mut codes = Codes::default();
        for text in texts {
            for c in text.chars() {
                let point = c as u32;
                if point <= 0xFFFF
                    || codes.codes.contains_key(&point)
                    || codes.supplementary.len() == SUPPLEMENTARY_CAPACITY
                {
                    continue;
                }
                codes.codes.insert(
                    point,
                    FIRST_SUPPLEMENTARY_CODE + codes.supplementary.len() as u32,
                );
                codes.supplementary.push(point);
            }
        }
        codes
    }

    /// The characters beyond the BMP with a code, by code: the first is D800.
    pub fn supplementary(&self) -> &[u32] {
        &self.supplementary
    }

    /// The codes of `text`, one per character.
    pub fn codes(&self, text: &str) -> Vec<u32> {
        text.chars()
            .map(|c| {
                let point = c as u32;
                if point <= 0xFFFF {
                    point
                } else {
                    self.codes.get(&point).copied().unwrap_or(REPLACEMENT_CODE)
                }
            })
            .collect()
    }

    /// `text` as a PDF hex string of its codes, two bytes each ("<00480069>" for "Hi").
    pub fn hex_string(&self, text: &str) -> String {
        let mut out = String::from("<");
        for code in self.codes(text) {
            out.push_str(&format!("{code:04X}"));
        }
        out.push('>');
        out
    }

    /// The ToUnicode CMap: every code outside D800-DFFF is the UTF-16 code unit it was written from, and each code from
    /// D800 is its character's surrogate pair, in blocks of at most 100.
    pub fn to_unicode_cmap(&self) -> String {
        let mut out = String::new();
        out.push_str("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
        out.push_str("/CIDSystemInfo <</Registry (Adobe) /Ordering (UCS) /Supplement 0>> def\n");
        out.push_str("/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n");
        out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
        out.push_str(identity_ranges());
        for (block, chunk) in self.supplementary.chunks(100).enumerate() {
            out.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (i, &point) in chunk.iter().enumerate() {
                let mut pair = [0u16; 2];
                char::from_u32(point).unwrap().encode_utf16(&mut pair);
                out.push_str(&format!(
                    "<{:04X}> <{:04X}{:04X}>\n",
                    FIRST_SUPPLEMENTARY_CODE as usize + block * 100 + i,
                    pair[0],
                    pair[1]
                ));
            }
            out.push_str("endbfchar\n");
        }
        out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        out
    }
}
