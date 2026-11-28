//! The pieces of a Type0 font with Identity-H encoding (ISO 32000-1 9.7) for some text, from a font file. Plain
//! data; the caller writes the objects. The subset tag is made from a SHA-256 hash.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::bytes::to_int32;
use crate::subset::{SubsetStatus, subset};
use crate::truetype::TrueTypeFont;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedStatus {
    Ok,
    Unreadable,
    NoOutlines,
    /// OS/2 fsType restricts embedding. Nothing is built.
    EmbeddingForbidden,
}

/// Widths of consecutive CIDs starting at `first_cid`, in 1/1000 em: one "c [w1 w2 ...]" entry of /W.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidthRun {
    pub first_cid: i64,
    pub widths: Vec<i64>,
}

/// Values for a /FontDescriptor, in the 1/1000 em units of the PDF.
#[derive(Debug, Clone, PartialEq)]
pub struct FontDescriptorValues {
    pub flags: i64,
    pub font_bbox: [i64; 4],
    pub italic_angle: f64,
    pub ascent: i64,
    pub descent: i64,
    pub cap_height: i64,
    pub stem_v: i64,
}

/// The pieces of a Type0 font with Identity-H encoding for some text. A character code is two bytes,
/// big-endian, and is also the CID.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedFont {
    /// /BaseFont and /FontName, with a six-letter subset tag when `is_subset` (9.6.4).
    pub base_font: String,
    /// False: /CIDFontType2 with the program as /FontFile2 and /CIDToGIDMap /Identity. True: /CIDFontType0 with
    /// the whole OpenType font as /FontFile3 /Subtype /OpenType; no CIDToGIDMap.
    pub is_cff: bool,
    pub is_subset: bool,
    pub program: Vec<u8>,
    /// /DW. Every code in `widths` has its own entry.
    pub default_width: i64,
    pub widths: Vec<WidthRun>,
    pub descriptor: FontDescriptorValues,
    /// Unicode code point to character code for every character of the text the font has a glyph for.
    pub codes: BTreeMap<i64, i64>,
    /// Code points of the text the font has no glyph for; they encode as code 0 (.notdef).
    pub missing: Vec<i64>,
    /// Glyph ID of the source font to character code, for text that was shaped into glyphs.
    pub glyph_codes: BTreeMap<i64, i64>,
    /// Character code to the text it stands for, for the /ToUnicode CMap. One glyph may stand for several code
    /// points.
    pub unicode_of_code: BTreeMap<i64, String>,
}

impl EmbeddedFont {
    /// The string as two-byte codes for a show-text operator.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(text.len() * 2);
        for c in text.chars() {
            if (c as u32) < 0x20 {
                continue;
            }
            let code = self.codes.get(&i64::from(c as u32)).copied().unwrap_or(0);
            bytes.push((code >> 8) as u8);
            bytes.push(code as u8);
        }
        bytes
    }

    /// Character code to code point, for a /ToUnicode CMap. Where two characters share a glyph the lower code
    /// point wins.
    pub fn to_unicode(&self) -> BTreeMap<i64, i64> {
        let mut map = BTreeMap::new();
        for (&code_point, &code) in self.codes.iter().rev() {
            map.insert(code, code_point);
        }
        map
    }
}

struct Parts {
    program: Vec<u8>,
    whole: bool,
    glyph_codes: BTreeMap<i64, i64>,
    glyphs: Vec<i64>,
}

/// TrueType fonts are cut down to the glyphs of `texts` (the subsetter), codes are the new glyph IDs. A
/// TrueType font whose OS/2 forbids subsetting, and any OpenType font with CFF outlines, is embedded whole with
/// the glyph ID as code; there is no CFF subsetter. A font that forbids embedding is refused. Each character
/// gets the glyph the cmap has for it; `build_glyphs` takes glyphs a shaper chose.
pub fn build(
    font: &[u8],
    texts: &[&str],
    collection_index: i64,
) -> (Option<EmbeddedFont>, EmbedStatus) {
    let (opened, status) = open(font, collection_index);
    let Some(f) = opened else {
        return (None, status);
    };

    let mut glyph_of: BTreeMap<i64, i64> = BTreeMap::new();
    let mut missing: BTreeSet<i64> = BTreeSet::new();
    for text in texts {
        for c in text.chars() {
            let cp = i64::from(c as u32);
            if cp < 0x20 || glyph_of.contains_key(&cp) || missing.contains(&cp) {
                continue;
            }
            let gid = f.glyph_for_code_point(cp);
            if gid == 0 || gid >= f.glyph_count {
                missing.insert(cp);
            } else {
                glyph_of.insert(cp, gid);
            }
        }
    }
    let code_points: Vec<i64> = glyph_of.keys().copied().collect();

    let glyphs: Vec<i64> = code_points.iter().map(|cp| glyph_of[cp]).collect();
    let Some(parts) = assemble(font, &f, glyphs, collection_index) else {
        return (None, EmbedStatus::Unreadable);
    };
    let mut codes = BTreeMap::new();
    for (&cp, gid) in &glyph_of {
        codes.insert(cp, parts.glyph_codes.get(gid).copied().unwrap_or(0));
    }
    let mut unicode = BTreeMap::new();
    for cp in code_points.iter().rev() {
        if let Some(c) = char::from_u32(*cp as u32)
            && let Some(&code) = codes.get(cp)
        {
            unicode.insert(code, c.to_string());
        }
    }
    let missing = missing.into_iter().collect();
    (
        Some(finish(&f, parts, codes, missing, unicode)),
        EmbedStatus::Ok,
    )
}

/// Like `build` for glyphs a shaper chose. `unicode_of_glyph` maps each glyph ID used to the text it stands for
/// (several code points for a ligature); `missing` are the code points that came out as .notdef. `codes` holds
/// the glyphs that stand for a single code point.
pub fn build_glyphs(
    font: &[u8],
    unicode_of_glyph: &BTreeMap<i64, String>,
    missing: &[i64],
    collection_index: i64,
) -> (Option<EmbeddedFont>, EmbedStatus) {
    let (opened, status) = open(font, collection_index);
    let Some(f) = opened else {
        return (None, status);
    };

    // The order only feeds the subset tag: the subsetter numbers glyphs by ID whatever the order.
    let mut listed: Vec<(i64, &String)> = unicode_of_glyph
        .iter()
        .filter(|(g, _)| **g > 0 && **g < f.glyph_count)
        .map(|(g, t)| (*g, t))
        .collect();
    let first = |t: &String| t.chars().next().map_or(i64::MAX, |c| i64::from(c as u32));
    listed.sort_by(|a, b| first(a.1).cmp(&first(b.1)).then(a.0.cmp(&b.0)));
    let glyphs: Vec<i64> = listed.iter().map(|(g, _)| *g).collect();
    let Some(parts) = assemble(font, &f, glyphs.clone(), collection_index) else {
        return (None, EmbedStatus::Unreadable);
    };
    let mut codes = BTreeMap::new();
    let mut unicode = BTreeMap::new();
    for gid in &glyphs {
        let text = unicode_of_glyph.get(gid).cloned().unwrap_or_default();
        let code = parts.glyph_codes.get(gid).copied().unwrap_or(0);
        let mut chars = text.chars();
        if let (Some(only), None) = (chars.next(), chars.next()) {
            codes.insert(i64::from(only as u32), code);
        }
        unicode.insert(code, text);
    }
    let missing: Vec<i64> = missing
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (
        Some(finish(&f, parts, codes, missing, unicode)),
        EmbedStatus::Ok,
    )
}

fn open(font: &[u8], collection_index: i64) -> (Option<TrueTypeFont>, EmbedStatus) {
    let Some(f) = TrueTypeFont::parse(font, collection_index) else {
        return (None, EmbedStatus::Unreadable);
    };
    if !f.has_outlines() {
        return (None, EmbedStatus::NoOutlines);
    }
    if !f.embedding_allowed() {
        return (None, EmbedStatus::EmbeddingForbidden);
    }
    (Some(f), EmbedStatus::Ok)
}

fn assemble(
    font: &[u8],
    f: &TrueTypeFont,
    glyphs: Vec<i64>,
    collection_index: i64,
) -> Option<Parts> {
    // A collection cannot be embedded as it is: its fonts share tables by offset.
    let whole = f.is_cff() || !f.subsetting_allowed();
    if whole && font.starts_with(b"ttcf") {
        return None;
    }

    let program;
    let mut code_of = BTreeMap::new();
    if whole {
        program = font.to_vec();
        // 9.7.4.2: the code of a CIDFontType0 is a CID, which a CID-keyed font maps through its charset.
        for &gid in &glyphs {
            code_of.insert(gid, f.cff.as_ref().map_or(gid, |c| c.cid_for_glyph(gid)));
        }
    } else {
        let unique: BTreeSet<i64> = glyphs.iter().copied().collect();
        let ids: Vec<i64> = unique.into_iter().collect();
        let result = subset(font, &ids, collection_index);
        if result.status != SubsetStatus::Ok {
            return None;
        }
        program = result.data;
        for &gid in &glyphs {
            code_of.insert(gid, result.glyph_map.get(&gid).copied().unwrap_or(0));
        }
    }
    Some(Parts {
        program,
        whole,
        glyph_codes: code_of,
        glyphs,
    })
}

fn finish(
    f: &TrueTypeFont,
    parts: Parts,
    codes: BTreeMap<i64, i64>,
    missing: Vec<i64>,
    unicode: BTreeMap<i64, String>,
) -> EmbeddedFont {
    let mut widths: BTreeMap<i64, i64> = BTreeMap::new();
    for (&gid, &code) in &parts.glyph_codes {
        widths.insert(code, scale(f.advance(gid) as f64, f.units_per_em));
    }

    // CID 0 has no glyph of the text, but Width measures a missing character by the .notdef advance.
    if !missing.is_empty() {
        widths.insert(0, scale(f.advance(0) as f64, f.units_per_em));
    }

    EmbeddedFont {
        base_font: base_font_name(f, !parts.whole, &parts.glyphs),
        is_cff: f.is_cff(),
        is_subset: !parts.whole,
        program: parts.program,
        default_width: 1000,
        widths: runs(&widths),
        descriptor: describe(f),
        codes,
        missing,
        glyph_codes: parts.glyph_codes,
        unicode_of_code: unicode,
    }
}

fn scale(units: f64, units_per_em: i64) -> i64 {
    to_int32((units * 1000.0 / units_per_em as f64).round_ties_even())
}

fn runs(widths: &BTreeMap<i64, i64>) -> Vec<WidthRun> {
    let mut runs = Vec::new();
    let mut current: Option<Vec<i64>> = None;
    let mut first = 0;
    let mut next = -1;
    for (&code, &width) in widths {
        if current.is_none() || code != next {
            if let Some(done) = current.take() {
                runs.push(WidthRun {
                    first_cid: first,
                    widths: done,
                });
            }
            current = Some(Vec::new());
            first = code;
        }
        if let Some(c) = current.as_mut() {
            c.push(width);
        }
        next = code + 1;
    }
    if let Some(done) = current {
        runs.push(WidthRun {
            first_cid: first,
            widths: done,
        });
    }
    runs
}

fn describe(f: &TrueTypeFont) -> FontDescriptorValues {
    let u = f.units_per_em;
    let mut flags = 32;
    if f.is_fixed_pitch {
        flags |= 1;
    }
    if f.italic_angle != 0.0 {
        flags |= 64;
    }
    let weight = if (100..=900).contains(&f.weight_class) {
        f.weight_class
    } else {
        400
    };
    let ratio = weight as f64 / 65.0;
    let stem_v = to_int32(50.0 + ratio * ratio);
    let b = f.font_bounds;
    FontDescriptorValues {
        flags,
        font_bbox: [
            scale(b.x_min, u),
            scale(b.y_min, u),
            scale(b.x_max, u),
            scale(b.y_max, u),
        ],
        italic_angle: f.italic_angle,
        ascent: scale(f.ascent as f64, u),
        descent: scale(f.descent as f64, u),
        cap_height: scale(f.cap_height as f64, u),
        stem_v,
    }
}

fn base_font_name(f: &TrueTypeFont, subset: bool, glyphs: &[i64]) -> String {
    let raw = f
        .post_script_name()
        .or_else(|| f.family_name())
        .unwrap_or_else(|| "Font".to_string());
    let mut name: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if name.is_empty() {
        name = "Font".into();
    }
    if !subset {
        return name;
    }

    let ids: Vec<String> = glyphs.iter().map(i64::to_string).collect();
    let seed = format!("{name}:{}", ids.join(","));
    let hash = Sha256::digest(seed.as_bytes());
    let tag: String = hash[..6].iter().map(|&h| (65 + h % 26) as char).collect();
    format!("{tag}+{name}")
}
