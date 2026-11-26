#![allow(clippy::needless_range_loop)] // index loops kept as in the Swift and C# twins, so the three are easy to compare

use crate::background::percentile;
use crate::gray_image::{ANALYSIS_DPI, GrayImage};
use lazy_image::Gray;

/// The six patch codes of document scanners: four parallel bars, two wide and two narrow, in an order that names the code.
/// Separator sheets carry one (Patch T by convention). A sheet fed the other way round reads its code backwards (`reversed`),
/// so every code counts as a separator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchCode {
    Patch1,
    Patch2,
    Patch3,
    Patch4,
    Patch6,
    PatchT,
}

impl PatchCode {
    pub const ALL: [PatchCode; 6] = [
        PatchCode::Patch1,
        PatchCode::Patch2,
        PatchCode::Patch3,
        PatchCode::Patch4,
        PatchCode::Patch6,
        PatchCode::PatchT,
    ];

    /// "Patch 1" to "Patch 6", "Patch T".
    pub fn label(self) -> &'static str {
        match self {
            PatchCode::Patch1 => "Patch 1",
            PatchCode::Patch2 => "Patch 2",
            PatchCode::Patch3 => "Patch 3",
            PatchCode::Patch4 => "Patch 4",
            PatchCode::Patch6 => "Patch 6",
            PatchCode::PatchT => "Patch T",
        }
    }

    /// The bars in reading order (top to bottom, or left to right), true for a wide bar: 1 = wide wide narrow narrow,
    /// 2 = wide narrow narrow wide, 3 = wide narrow wide narrow, 4 = narrow wide wide narrow, 6 = narrow narrow wide wide,
    /// T = narrow wide narrow wide.
    pub fn bars(self) -> [bool; 4] {
        match self {
            PatchCode::Patch1 => [true, true, false, false],
            PatchCode::Patch2 => [true, false, false, true],
            PatchCode::Patch3 => [true, false, true, false],
            PatchCode::Patch4 => [false, true, true, false],
            PatchCode::Patch6 => [false, false, true, true],
            PatchCode::PatchT => [false, true, false, true],
        }
    }

    /// The code of four bars in reading order (true: wide); `None` unless exactly two are wide.
    pub fn from_bars(bars: [bool; 4]) -> Option<PatchCode> {
        Self::ALL.into_iter().find(|c| c.bars() == bars)
    }

    /// The code read from the other end: 1 and 6, 3 and T swap; 2 and 4 read the same.
    pub fn reversed(self) -> PatchCode {
        let b = self.bars();
        Self::from_bars([b[3], b[2], b[1], b[0]]).unwrap_or(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// A patch code found on a page: the code as read, whether its bars run down the page (the sheet lay sideways) rather than
/// across it, where its bars are (analysis pixels, top-left origin) and how far along the bars it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchCodeHit {
    pub code: PatchCode,
    pub vertical_bars: bool,
    pub bounds: Bounds,
    pub length: usize,
}

// Sizes in hundredths of an inch (whole numbers, so every platform computes the same pixels).
pub const NARROW_BAR_HUNDREDTHS: i64 = 8;
pub const WIDE_BAR_HUNDREDTHS: i64 = 20;
pub const GAP_HUNDREDTHS: i64 = 8;
pub const MIN_NARROW_HUNDREDTHS: i64 = 5;
pub const MAX_NARROW_HUNDREDTHS: i64 = 12;
pub const MIN_WIDE_HUNDREDTHS: i64 = 15;
pub const MAX_WIDE_HUNDREDTHS: i64 = 26;
pub const MIN_GAP_HUNDREDTHS: i64 = 5;
pub const MAX_GAP_HUNDREDTHS: i64 = 12;
pub const QUIET_ZONE_HUNDREDTHS: i64 = 30;
pub const MIN_LENGTH_HUNDREDTHS: i64 = 150;
pub const MAX_STEP_HUNDREDTHS: i64 = 2;
pub const MAX_BREAK_HUNDREDTHS: i64 = 6;
pub const MIN_COVERAGE: f64 = 0.5;
pub const SHEET_BAR_LENGTH_HUNDREDTHS: i64 = 600;
pub const SHEET_MARGIN_HUNDREDTHS: i64 = 100;

fn floor_pixels(hundredths: i64, dpi: f64) -> i64 {
    (hundredths as f64 * dpi / 100.0).floor() as i64
}

fn ceiling_pixels(hundredths: i64, dpi: f64) -> i64 {
    (hundredths as f64 * dpi / 100.0).ceil() as i64
}

fn round_pixels(hundredths: i64, dpi: f64) -> i64 {
    (hundredths as f64 * dpi / 100.0).round() as i64
}

// The detector sizes in pixels at one resolution.
struct Limits {
    min_narrow: i64,
    max_narrow: i64,
    min_wide: i64,
    max_wide: i64,
    min_gap: i64,
    max_gap: i64,
    quiet: i64,
    min_length: i64,
    max_step: i64,
    max_break: i64,
}

impl Limits {
    fn new(dpi: f64) -> Self {
        Limits {
            min_narrow: floor_pixels(MIN_NARROW_HUNDREDTHS, dpi),
            max_narrow: ceiling_pixels(MAX_NARROW_HUNDREDTHS, dpi),
            min_wide: floor_pixels(MIN_WIDE_HUNDREDTHS, dpi),
            max_wide: ceiling_pixels(MAX_WIDE_HUNDREDTHS, dpi),
            min_gap: floor_pixels(MIN_GAP_HUNDREDTHS, dpi),
            max_gap: ceiling_pixels(MAX_GAP_HUNDREDTHS, dpi),
            quiet: round_pixels(QUIET_ZONE_HUNDREDTHS, dpi),
            min_length: round_pixels(MIN_LENGTH_HUNDREDTHS, dpi).max(1),
            max_step: round_pixels(MAX_STEP_HUNDREDTHS, dpi).max(1),
            max_break: round_pixels(MAX_BREAK_HUNDREDTHS, dpi).max(1),
        }
    }
}

// A code read on neighbouring lines.
struct Chain {
    code: PatchCode,
    first_line: usize,
    last_line: usize,
    last_start: i64,
    min_start: i64,
    max_end: i64,
    count: usize,
}

impl Chain {
    fn length(&self) -> usize {
        self.last_line - self.first_line + 1
    }
}

/// The patch code on the page, read at the analysis resolution (a finer image is reduced to it first); `None` when there is
/// none. A patch code is read like a one-dimensional barcode, along lines that cross its bars (the columns of the page for
/// bars running across it, the rows for bars running down it): the ink (darker than half the paper, the page 90th
/// percentile) along a line gives runs, and four runs in a row match when each is narrow (0.05 to 0.12 in) or wide (0.15 to
/// 0.26 in), exactly two are wide, the gaps between them are 0.05 to 0.12 in and nothing is printed for 0.3 in before and
/// after them. Matches on neighbouring lines that agree on the code and move by at most 0.02 in from one line to the next are
/// one code; it counts when it is read over at least 1.5 in of its length, on at least half the lines it spans. Bars across
/// the page are tried first; the longer of the two readings wins.
pub fn detect(gray: &GrayImage) -> Option<PatchCodeHit> {
    let reduced;
    let g = if gray.dpi > ANALYSIS_DPI as f64 {
        reduced = gray.at_resolution(ANALYSIS_DPI);
        &reduced
    } else {
        gray
    };
    let (w, h) = (g.width(), g.height());
    if w < 2 || h < 2 {
        return None;
    }
    let mut histogram = [0i32; 256];
    for &v in &g.gray.data {
        histogram[v as usize] += 1;
    }
    let paper = percentile(&histogram, g.gray.data.len(), 0.9);
    let ink: Vec<bool> = g
        .gray
        .data
        .iter()
        .map(|&v| (v as usize) * 2 < paper)
        .collect();

    let limits = Limits::new(g.dpi);
    let mut best = read(&ink, w, h, false, &limits);
    let mut vertical = false;
    if let Some(d) = read(&ink, w, h, true, &limits)
        && best.as_ref().is_none_or(|b| d.length() > b.length())
    {
        best = Some(d);
        vertical = true;
    }
    let chain = best?;
    // lines are columns for bars across the page (positions are rows), rows for bars down it
    let span = (chain.max_end - chain.min_start) as usize;
    let bounds = if vertical {
        Bounds {
            x: chain.min_start as usize,
            y: chain.first_line,
            width: span,
            height: chain.length(),
        }
    } else {
        Bounds {
            x: chain.first_line,
            y: chain.min_start as usize,
            width: chain.length(),
            height: span,
        }
    };
    Some(PatchCodeHit {
        code: chain.code,
        vertical_bars: vertical,
        bounds,
        length: chain.length(),
    })
}

// The longest code read along the columns (`line_is_row` false: bars across the page) or the rows (true: bars down it); `None`
// when none is long and dense enough.
fn read(ink: &[bool], w: usize, h: usize, line_is_row: bool, limits: &Limits) -> Option<Chain> {
    let (lines, length) = if line_is_row { (h, w) } else { (w, h) };
    let mut chains: Vec<Chain> = Vec::new();
    let mut starts: Vec<i64> = Vec::new();
    let mut ends: Vec<i64> = Vec::new();
    for line in 0..lines {
        starts.clear();
        ends.clear();
        let mut run_start: i64 = -1;
        for pos in 0..length {
            let is_ink = if line_is_row {
                ink[line * w + pos]
            } else {
                ink[pos * w + line]
            };
            if is_ink && run_start < 0 {
                run_start = pos as i64;
            } else if !is_ink && run_start >= 0 {
                starts.push(run_start);
                ends.push(pos as i64);
                run_start = -1;
            }
        }
        if run_start >= 0 {
            starts.push(run_start);
            ends.push(length as i64);
        }
        for i in 0..starts.len().saturating_sub(3) {
            let Some(code) = match_runs(&starts, &ends, i, limits) else {
                continue;
            };
            let (start, end) = (starts[i], ends[i + 3]);
            let found = chains.iter().position(|chain| {
                chain.code == code
                    && chain.last_line < line
                    && (line - chain.last_line) as i64 <= limits.max_break + 1
                    && (start - chain.last_start).abs() <= limits.max_step
            });
            match found {
                None => chains.push(Chain {
                    code,
                    first_line: line,
                    last_line: line,
                    last_start: start,
                    min_start: start,
                    max_end: end,
                    count: 1,
                }),
                Some(k) => {
                    let chain = &mut chains[k];
                    chain.last_line = line;
                    chain.last_start = start;
                    chain.min_start = chain.min_start.min(start);
                    chain.max_end = chain.max_end.max(end);
                    chain.count += 1;
                }
            }
        }
    }
    let mut best: Option<Chain> = None;
    for chain in chains {
        if (chain.length() as i64) < limits.min_length
            || (chain.count as f64) < MIN_COVERAGE * chain.length() as f64
        {
            continue;
        }
        if best.as_ref().is_none_or(|b| chain.length() > b.length()) {
            best = Some(chain);
        }
    }
    best
}

// The code of the four runs from run `i` on, when they read as one; else `None`.
fn match_runs(starts: &[i64], ends: &[i64], i: usize, limits: &Limits) -> Option<PatchCode> {
    let mut wide = [false; 4];
    for k in 0..4 {
        let width = ends[i + k] - starts[i + k];
        if width >= limits.min_wide && width <= limits.max_wide {
            wide[k] = true;
        } else if width < limits.min_narrow || width > limits.max_narrow {
            return None;
        }
        if k > 0 {
            let gap = starts[i + k] - ends[i + k - 1];
            if gap < limits.min_gap || gap > limits.max_gap {
                return None;
            }
        }
    }
    // nothing printed before the first bar and after the last (the page edge counts as quiet)
    let before = if i > 0 {
        starts[i] - ends[i - 1]
    } else {
        i64::MAX
    };
    let after = if i + 4 < starts.len() {
        starts[i + 4] - ends[i + 3]
    } else {
        i64::MAX
    };
    if before < limits.quiet || after < limits.quiet {
        return None;
    }
    PatchCode::from_bars(wide)
}

/// The separator sheet to print, `width_inches` x `height_inches` at `dpi`: white, with `code` in black bars across the sheet
/// 1 in from the top and again 1 in from the bottom, turned half a turn there, so the sheet reads the same code fed either
/// way round; and in the middle "SEPARATOR SHEET", the code and how to use and print it. Printed at real size, its bars have
/// the standard widths.
pub fn render_sheet(code: PatchCode, width_inches: f64, height_inches: f64, dpi: u32) -> GrayImage {
    let d = dpi as f64;
    let w = ((width_inches * d).round() as i64).max(1);
    let h = ((height_inches * d).round() as i64).max(1);
    let mut pixels = vec![255u8; (w * h) as usize];

    let narrow = round_pixels(NARROW_BAR_HUNDREDTHS, d).max(1);
    let wide = round_pixels(WIDE_BAR_HUNDREDTHS, d).max(1);
    let gap = round_pixels(GAP_HUNDREDTHS, d).max(1);
    let margin = round_pixels(SHEET_MARGIN_HUNDREDTHS, d);
    let bar_length = round_pixels(SHEET_BAR_LENGTH_HUNDREDTHS, d)
        .min(w - 2 * round_pixels(50, d))
        .max(1);
    let x0 = (w - bar_length) / 2;
    let bars = code.bars();

    // near the top, read top to bottom
    let mut y = margin;
    for is_wide in bars {
        let thickness = if is_wide { wide } else { narrow };
        fill_rect(&mut pixels, w, h, [x0, y, bar_length, thickness], 0);
        y += thickness + gap;
    }
    // near the bottom, half a turn round: read bottom to top
    y = h - margin;
    for is_wide in bars {
        let thickness = if is_wide { wide } else { narrow };
        fill_rect(
            &mut pixels,
            w,
            h,
            [x0, y - thickness, bar_length, thickness],
            0,
        );
        y -= thickness + gap;
    }

    // the words, centred, around the middle of the sheet
    let middle = h / 2;
    let title = round_pixels(5, d).max(1);
    let body = round_pixels(2, d).max(1);
    let label = code.label().to_uppercase();
    let texts = [
        ("SEPARATOR SHEET", middle - round_pixels(80, d), title),
        (label.as_str(), middle - round_pixels(20, d), title),
        (
            "PUT THIS SHEET IN FRONT OF EACH DOCUMENT.",
            middle + round_pixels(50, d),
            body,
        ),
        (
            "PRINT AT ACTUAL SIZE (100%). DO NOT SHRINK.",
            middle + round_pixels(80, d),
            body,
        ),
    ];
    for (text, top, cell) in texts {
        draw_text(&mut pixels, w, h, text, top, cell);
    }
    GrayImage::new(
        Gray {
            width: w as usize,
            height: h as usize,
            data: pixels,
        },
        d,
    )
}

// Fills the rectangle [x, y, width, height] with `value`, clipped to the sheet.
fn fill_rect(pixels: &mut [u8], w: i64, h: i64, rect: [i64; 4], value: u8) {
    let [x, y, rw, rh] = rect;
    let (y0, y1) = (y.max(0), h.min(y + rh));
    let (x0, x1) = (x.max(0), w.min(x + rw));
    if y0 >= y1 || x0 >= x1 {
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            pixels[(py * w + px) as usize] = value;
        }
    }
}

// `text` in the sheet type of 5 x 7 dots, each dot `cell` pixels, a dot of space between letters, centred across the sheet
// with its top at `top`. Characters the type does not have are spaces.
fn draw_text(pixels: &mut [u8], w: i64, h: i64, text: &str, top: i64, cell: i64) {
    let count = text.chars().count() as i64;
    let width = (count * 6 - 1) * cell;
    let mut x = (w - width) / 2;
    for c in text.chars() {
        if let Some(glyph) = glyph(c) {
            let dots = glyph.as_bytes();
            for row in 0..7 {
                for col in 0..5 {
                    if dots[row * 5 + col] == b'1' {
                        let at = [x + col as i64 * cell, top + row as i64 * cell, cell, cell];
                        fill_rect(pixels, w, h, at, 0);
                    }
                }
            }
        }
        x += 6 * cell;
    }
}

// The sheet type: 5 x 7 dots per character, rows top to bottom.
fn glyph(c: char) -> Option<&'static str> {
    Some(match c {
        'A' => "01110100011000111111100011000110001",
        'B' => "11110100011000111110100011000111110",
        'C' => "01110100011000010000100001000101110",
        'D' => "11110100011000110001100011000111110",
        'E' => "11111100001000011110100001000011111",
        'F' => "11111100001000011110100001000010000",
        'G' => "01110100011000010111100011000101111",
        'H' => "10001100011000111111100011000110001",
        'I' => "01110001000010000100001000010001110",
        'J' => "00111000100001000010000101001001100",
        'K' => "10001100101010011000101001001010001",
        'L' => "10000100001000010000100001000011111",
        'M' => "10001110111010110101100011000110001",
        'N' => "10001100011100110101100111000110001",
        'O' => "01110100011000110001100011000101110",
        'P' => "11110100011000111110100001000010000",
        'Q' => "01110100011000110001101011001001101",
        'R' => "11110100011000111110101001001010001",
        'S' => "01111100001000001110000010000111110",
        'T' => "11111001000010000100001000010000100",
        'U' => "10001100011000110001100011000101110",
        'V' => "10001100011000110001100010101000100",
        'W' => "10001100011000110101101011010101010",
        'X' => "10001100010101000100010101000110001",
        'Y' => "10001100010101000100001000010000100",
        'Z' => "11111000010001000100010001000011111",
        '0' => "01110100011001110101110011000101110",
        '1' => "00100011000010000100001000010001110",
        '2' => "01110100010000100010001000100011111",
        '3' => "11111000100010000010000011000101110",
        '4' => "00010001100101010010111110001000010",
        '5' => "11111100001111000001000011000101110",
        '6' => "00110010001000011110100011000101110",
        '7' => "11111000010001000100010000100001000",
        '8' => "01110100011000101110100011000101110",
        '9' => "01110100011000101111000010001001100",
        '.' => "00000000000000000000000000110001100",
        '(' => "00010001000100001000010000010000010",
        ')' => "01000001000001000010000100010001000",
        '%' => "11000110010001000100010001001100011",
        '-' => "00000000000000011111000000000000000",
        '?' => "01110100010000100010001000000000100",
        ':' => "00000011000110000000011000110000000",
        '/' => "00000000010001000100010001000000000",
        '_' => "00000000000000000000000000000011111",
        '#' => "01010010101111101010111110101001010",
        '+' => "00000001000010011111001000010000000",
        ',' => "00000000000000000000011000010001000",
        '&' => "01100100101010001000101011001001101",
        '\'' => "01100001000100000000000000000000000",
        '!' => "00100001000010000100001000000000100",
        _ => return None,
    })
}
