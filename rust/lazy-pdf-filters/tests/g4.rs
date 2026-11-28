use lazy_pdf_filters::ccitt::{self, Options};
use lazy_pdf_filters::g4::{self, Code, encode_g4, stride};
mod common;

use common::data;
use sha2::{Digest, Sha256};

/// The bit strings in order, then zero bits up to a byte boundary.
fn bits(parts: &[&str]) -> Vec<u8> {
    let all: String = parts.concat();
    let mut bytes = vec![0u8; all.len().div_ceil(8)];
    for (i, c) in all.bytes().enumerate() {
        if c == b'1' {
            bytes[i / 8] |= 0x80 >> (i % 8);
        }
    }
    bytes
}

const EOFB: &str = "000000000001000000000001";

fn packed(width: usize, height: usize, mut black: impl FnMut(usize, usize) -> bool) -> Vec<u8> {
    let stride = stride(width);
    let mut packed = vec![0xFFu8; stride * height];
    for y in 0..height {
        for x in 0..width {
            if black(x, y) {
                packed[y * stride + (x >> 3)] &= !(0x80 >> (x & 7));
            }
        }
    }
    packed
}

fn rows(rows: &[&str]) -> Vec<u8> {
    let grid: Vec<&[u8]> = rows.iter().map(|r| r.as_bytes()).collect();
    packed(grid[0].len(), grid.len(), |x, y| grid[y][x] == b'1')
}

/// xorshift32 (13, 17, 5), the generator of the reference tests.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Rng {
        Rng(if seed == 0 { 1 } else { seed })
    }

    fn below(&mut self, n: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 % n
    }
}

fn decode(g4: &[u8], width: usize, height: usize) -> Vec<u8> {
    let options = Options {
        k: -1,
        columns: width,
        rows: height,
        byte_align: false,
        end_of_line: false,
        black_is_1: false,
    };
    ccitt::decode(g4, &options, 1 << 30).unwrap()
}

fn assert_round_trip(packed: &[u8], width: usize, height: usize) -> Vec<u8> {
    let g4 = encode_g4(packed, width, height).unwrap();
    let decoded = decode(&g4, width, height);
    let stride = stride(width);
    for y in 0..height {
        for x in 0..width {
            let at = y * stride + (x >> 3);
            let mask = 0x80 >> (x & 7);
            assert_eq!(
                packed[at] & mask,
                decoded[at] & mask,
                "{width} x {height}: pixel ({x}, {y}) differs after decoding"
            );
        }
    }
    g4
}

fn text(code: Code) -> String {
    (0..code.length)
        .rev()
        .map(|i| if code.code >> i & 1 == 1 { '1' } else { '0' })
        .collect()
}

fn texts(tables: &[&[Code]]) -> Vec<String> {
    tables
        .iter()
        .flat_map(|t| t.iter().map(|&c| text(c)))
        .collect()
}

fn assert_prefix_free(codes: &[String], what: &str) {
    for (i, a) in codes.iter().enumerate() {
        for (j, b) in codes.iter().enumerate() {
            assert!(
                i == j || !b.starts_with(a.as_str()),
                "{what}: {a} is a prefix of {b}"
            );
        }
    }
}

#[test]
fn code_tables_cover_every_run_and_are_prefix_free_per_color() {
    let t = g4::tables();
    assert_eq!(t.white_terminating.len(), 64);
    assert_eq!(t.black_terminating.len(), 64);
    assert_eq!(t.white_make_up.len(), 27);
    assert_eq!(t.black_make_up.len(), 27);
    assert_eq!(t.extended_make_up.len(), 13);
    assert_eq!(t.vertical.len(), 7);

    let eol = "000000000001".to_string();
    let mut white = texts(&[&t.white_terminating, &t.white_make_up, &t.extended_make_up]);
    white.push(eol.clone());
    assert_prefix_free(&white, "white");
    let mut black = texts(&[&t.black_terminating, &t.black_make_up, &t.extended_make_up]);
    black.push(eol.clone());
    assert_prefix_free(&black, "black");
    let mut modes = texts(&[&t.vertical]);
    modes.push(text(g4::PASS));
    modes.push(text(g4::HORIZONTAL));
    modes.push(eol);
    assert_prefix_free(&modes, "modes");

    assert_eq!(text(t.white_terminating[0]), "00110101");
    assert_eq!(text(t.black_terminating[0]), "0000110111");
    assert_eq!(text(t.white_terminating[63]), "00110100");
    assert_eq!(text(t.black_terminating[63]), "000001100111");
    assert_eq!(text(t.white_make_up[26]), "010011011");
    assert_eq!(text(t.black_make_up[26]), "0000001100101");
    assert_eq!(text(t.extended_make_up[0]), "00000001000");
    assert_eq!(text(t.extended_make_up[12]), "000000011111");
}

#[test]
fn all_white_is_one_v0_per_row_then_eofb() {
    let g4 = encode_g4(&packed(1728, 10, |_, _| false), 1728, 10).unwrap();
    assert_eq!(g4, bits(&["1111111111", EOFB]));
    assert_eq!(g4, [0xFF, 0xC0, 0x04, 0x00, 0x40]);
}

#[test]
fn all_black_is_one_horizontal_row_then_two_v0_per_row() {
    let g4 = encode_g4(&packed(1728, 10, |_, _| true), 1728, 10).unwrap();
    let later = "11".repeat(9);
    assert_eq!(
        g4,
        bits(&[
            "001",
            "00110101",
            "0000001100101",
            "0000110111",
            &later,
            EOFB
        ])
    );
    assert_eq!(g4.len(), 10);
}

#[test]
fn mode_codes_pass_horizontal_and_every_vertical_offset() {
    let image = rows(&[
        "00011000", "00011000", "00110000", "00000000", "00000011", "00000001", "01111111",
        "11111000", "00001110", "00000011", "11111111", "11100000", "00011111",
    ]);
    let g4 = encode_g4(&image, 8, 13).unwrap();
    let expected = [
        "001", "1000", "11", "1", "1", "1", "1", "010", "010", "1", "0001", "1", "000010", "1",
        "011", "1", "001", "000111", "00011", "010", "0000010", "1", "001", "1011", "10", "1",
        "000011", "011", "001", "00110101", "000101", "1", "001", "10", "1100", "0000011", "1",
        EOFB,
    ];
    assert_eq!(g4, bits(&expected));
    assert_round_trip(&image, 8, 13);
}

#[test]
fn long_runs_repeat_the_largest_extended_make_up_code() {
    // White 5200 = 2560 + 2560 + 64 + 16; black 2800 = 2560 + 192 + 48.
    let g4 = encode_g4(&packed(8000, 1, |x, _| x >= 5200), 8000, 1).unwrap();
    assert_eq!(
        g4,
        bits(&[
            "001",
            "000000011111",
            "000000011111",
            "11011",
            "101010",
            "000000011111",
            "000011001001",
            "000001100100",
            EOFB
        ])
    );
    // 2624 is the first run with two make-up codes (2560 + 64 + 0); 2623 is 2560 + 63.
    let g4 = encode_g4(&packed(5247, 1, |x, _| x >= 2624), 5247, 1).unwrap();
    assert_eq!(
        g4,
        bits(&[
            "001",
            "000000011111",
            "11011",
            "00110101",
            "000000011111",
            "000001100111",
            EOFB
        ])
    );
}

#[test]
fn no_rows_is_just_eofb() {
    assert_eq!(encode_g4(&[], 100, 0).unwrap(), [0x00, 0x10, 0x01]);
}

#[test]
fn single_pixels_and_one_pixel_lines() {
    assert_eq!(
        encode_g4(&rows(&["1"]), 1, 1).unwrap(),
        bits(&["010", "1", EOFB])
    );
    assert_eq!(encode_g4(&rows(&["0"]), 1, 1).unwrap(), bits(&["1", EOFB]));
    assert_round_trip(&packed(1, 17, |_, y| y % 3 != 1), 1, 17);
    assert_round_trip(&packed(997, 1, |x, _| x % 5 < 2 || x > 900), 997, 1);
}

#[test]
fn arguments_are_checked() {
    assert!(encode_g4(&[0; 4], 0, 1).is_err());
    assert!(encode_g4(&[0; 3], 9, 2).is_err()); // needs 4 bytes
}

#[test]
fn round_trip_run_lengths_of_both_colors() {
    let mut runs: Vec<usize> = (1..=130).collect();
    runs.extend((131..=2700).step_by(61));
    runs.extend([
        1727, 1728, 1729, 1791, 1792, 1793, 2559, 2560, 2561, 2623, 2624, 2625, 2700,
    ]);
    let width = 2710;
    let height = runs.len() * 4;
    let image = packed(width, height, |x, y| {
        let length = runs[y / 4];
        match y % 4 {
            1 => x >= length,
            3 => x < length,
            _ => false,
        }
    });
    assert_round_trip(&image, width, height);
}

#[test]
fn round_trip_odd_widths() {
    for width in [1usize, 2, 7, 8, 9, 15, 17, 1727, 1729] {
        let mut rng = Rng::new(0xB1E0 + width as u32);
        let image = packed(width, 24, |x, y| {
            if y < 8 {
                rng.below(3) == 0
            } else if y < 16 {
                (x + y) & 1 == 0
            } else {
                x + y - 15 >= width
            }
        });
        assert_round_trip(&image, width, 24);
    }
}

#[test]
fn round_trip_very_wide_rows() {
    let mut rng = Rng::new(0x5EED_0001);
    let image = packed(20000, 12, |x, y| match y {
        0 => true,
        1 => (3..15000).contains(&x),
        2 => x % 2629 == 0,
        3 => x >= 10000,
        4 => x < 2624 || (5184..5247).contains(&x),
        5 => rng.below(50) == 0,
        _ => (x / 97 + y) % 3 == 0,
    });
    assert_round_trip(&image, 20000, 12);
}

/// Inputs and outputs of the reference encoder (the C# one, whose bytes the Swift tests pin by hash).
fn golden(name: &str, width: usize, height: usize, count: usize, hash: &str) {
    let input = data(&format!("{name}.packed"));
    let expected = data(&format!("{name}.g4"));
    let g4 = assert_round_trip(&input, width, height);
    assert_eq!(g4, expected);
    assert_eq!(g4.len(), count);
    let digest: String = Sha256::digest(&g4)
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect();
    assert_eq!(digest, hash);
}

#[test]
fn golden_checkerboard() {
    golden(
        "g4-checker-37x23",
        37,
        23,
        335,
        "E8A046161C50C7D37EA4A44DC4A9F1DCE909940D388C27C8CCECCBE798DC43EC",
    );
}

#[test]
fn golden_noise() {
    let mut rng = Rng::new(0xC0FF_EE01);
    let generated = packed(517, 211, |_, _| rng.below(2) == 0);
    assert_eq!(generated, data("g4-noise-517x211.packed"));
    golden(
        "g4-noise-517x211",
        517,
        211,
        28704,
        "4255D22C831C890F8DAFAC2B936E2CAB95D2AB5FAAD06CC844C04E5AB8D215C0",
    );
}

#[test]
fn golden_text_page() {
    golden(
        "g4-text-850x1100",
        850,
        1100,
        17666,
        "DD8EC2973FEA77FBB02C0DBD9A9BC69C9771852D2AD18B70AEF0387D94EC4DFE",
    );
}
