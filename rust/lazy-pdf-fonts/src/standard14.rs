//! One of the 14 standard fonts (ISO 32000-1 9.6.2.2) with the metrics of its Adobe AFM file. `find` also answers
//! to the names other programs give them: Arial, TimesNewRomanPS-BoldMT.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::data;
use crate::encodings::{Encoding, inflate_text, standard, symbol, zapf_dingbats};

/// A font's bounding box in glyph space (1000 units to the em).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontBox {
    pub left: i64,
    pub bottom: i64,
    pub right: i64,
    pub top: i64,
}

#[derive(Debug)]
pub struct Standard14 {
    /// The BaseFont name: "Helvetica-Bold", "Times-Roman", "Symbol".
    pub name: String,
    /// The AFM's Ascender and Descender (negative); the bounding box's top and bottom for Symbol and
    /// ZapfDingbats, which have none.
    pub ascent: i64,
    pub descent: i64,
    pub b_box: FontBox,
    /// Zero where the AFM gives none (Symbol, ZapfDingbats).
    pub cap_height: i64,
    pub x_height: i64,
    pub italic_angle: f64,
    pub stem_v: i64,
    pub is_fixed_pitch: bool,
    pub is_bold: bool,
    widths: HashMap<String, i64>,
}

impl Standard14 {
    fn new(f: &[&str], widths: HashMap<String, i64>) -> Standard14 {
        let int = |i: usize| -> i64 { f[i].parse().expect("Bad number in the Standard 14 table.") };
        Standard14 {
            name: f[1].to_string(),
            ascent: int(2),
            descent: int(3),
            b_box: FontBox {
                left: int(4),
                bottom: int(5),
                right: int(6),
                top: int(7),
            },
            cap_height: int(8),
            x_height: int(9),
            italic_angle: f[10].parse().expect("Bad number in the Standard 14 table."),
            stem_v: int(11),
            is_fixed_pitch: f[12] == "1",
            is_bold: f[13] == "1",
            widths,
        }
    }

    pub fn is_italic(&self) -> bool {
        self.italic_angle != 0.0
    }

    pub fn is_serif(&self) -> bool {
        self.name.starts_with("Times")
    }

    /// Symbol and ZapfDingbats, whose encoding is built in and not a Latin one.
    pub fn is_symbolic(&self) -> bool {
        self.name == "Symbol" || self.name == "ZapfDingbats"
    }

    /// The /Flags of a FontDescriptor for the font (ISO 32000-1 9.8.2).
    pub fn flags(&self) -> i64 {
        i64::from(self.is_fixed_pitch)
            | if self.is_serif() { 2 } else { 0 }
            | if self.is_symbolic() { 4 } else { 32 }
            | if self.is_italic() { 64 } else { 0 }
            | if self.is_bold { 1 << 18 } else { 0 }
    }

    /// What a code shows when the font has no /Encoding: Symbol's and ZapfDingbats' own, else StandardEncoding.
    pub fn built_in_encoding(&self) -> &'static Encoding {
        match self.name.as_str() {
            "Symbol" => symbol(),
            "ZapfDingbats" => zapf_dingbats(),
            _ => standard(),
        }
    }

    /// The advance of a glyph in 1/1000 em, or none when the font has no glyph of that name.
    pub fn width_of(&self, glyph_name: &str) -> Option<i64> {
        self.widths.get(glyph_name).copied()
    }

    pub fn all() -> &'static [Standard14] {
        static FONTS: OnceLock<Vec<Standard14>> = OnceLock::new();
        FONTS.get_or_init(load)
    }

    /// The standard font a BaseFont name stands for, or none. A subset tag (ABCDEF+) is ignored, so are case,
    /// spaces, and the "PS" and "MT" of the Windows names; "Arial Narrow" and "Arial Black" are other fonts and
    /// give none.
    pub fn find(base_font: &str) -> Option<&'static Standard14> {
        // Longest first: "TimesNewRoman" before "Times".
        const FAMILIES: [(&str, &str); 8] = [
            ("TimesNewRoman", "Times"),
            ("Times", "Times"),
            ("Arial", "Helvetica"),
            ("Helvetica", "Helvetica"),
            ("CourierNew", "Courier"),
            ("Courier", "Courier"),
            ("Symbol", "Symbol"),
            ("ZapfDingbats", "ZapfDingbats"),
        ];
        // UTF-16 units: the length test and the cuts count the same.
        let mut name: Vec<u16> = base_font.encode_utf16().collect();
        if name.len() > 7 && name[6] == 0x2B {
            name.drain(..7);
        }
        name.retain(|&u| u != 0x20);
        for (prefix, family) in FAMILIES {
            let prefix_units: Vec<u16> = prefix.encode_utf16().collect();
            if name.len() < prefix_units.len() || !matches(&name, 0, &prefix_units) {
                continue;
            }
            let mut rest: Vec<u16> = name[prefix_units.len()..]
                .iter()
                .copied()
                .filter(|&u| u != 0x2D && u != 0x2C)
                .collect();
            rest = strip(&strip(&rest, "PS"), "MT");
            if family == "Symbol" || family == "ZapfDingbats" {
                return if rest.is_empty() {
                    by_name(family)
                } else {
                    None
                };
            }

            let bold = contains(&rest, "Bold");
            let italic = contains(&rest, "Italic") || contains(&rest, "Oblique");
            for word in ["Bold", "Italic", "Oblique", "Roman", "Regular", "Normal"] {
                rest = strip(&rest, word);
            }
            if !rest.is_empty() {
                return None;
            }

            let style = match (bold, italic) {
                (false, false) if family == "Times" => "-Roman",
                (false, false) => "",
                (true, false) => "-Bold",
                (false, true) if family == "Times" => "-Italic",
                (false, true) => "-Oblique",
                (true, true) if family == "Times" => "-BoldItalic",
                (true, true) => "-BoldOblique",
            };
            return by_name(&format!("{family}{style}"));
        }
        None
    }
}

fn by_name(name: &str) -> Option<&'static Standard14> {
    Standard14::all().iter().find(|f| f.name == name)
}

// Ordinal, ignoring case: the words are ASCII, so folding A to Z is all there is to compare.
fn fold(unit: u16) -> u16 {
    if (0x41..=0x5A).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

fn matches(text: &[u16], start: usize, word: &[u16]) -> bool {
    start + word.len() <= text.len()
        && word
            .iter()
            .enumerate()
            .all(|(i, &w)| fold(text[start + i]) == fold(w))
}

fn contains(text: &[u16], word: &str) -> bool {
    let units: Vec<u16> = word.encode_utf16().collect();
    (0..text.len()).any(|i| matches(text, i, &units))
}

fn strip(text: &[u16], word: &str) -> Vec<u16> {
    let units: Vec<u16> = word.encode_utf16().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if matches(text, i, &units) {
            i += units.len();
        } else {
            result.push(text[i]);
            i += 1;
        }
    }
    result
}

fn load() -> Vec<Standard14> {
    let mut fonts = Vec::new();
    let text = inflate_text(data::STANDARD14_FONTS);
    let mut header: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.starts_with("F ") {
            header = line.split(' ').collect();
            continue;
        }
        let parts: Vec<&str> = line.split(' ').collect();
        let mut widths = HashMap::with_capacity((parts.len().max(1) - 1) / 2);
        let mut i = 1;
        while i + 1 < parts.len() {
            let width = parts[i + 1]
                .parse()
                .expect("Bad width in the Standard 14 table.");
            widths.insert(parts[i].to_string(), width);
            i += 2;
        }
        fonts.push(Standard14::new(&header, widths));
    }
    fonts
}
