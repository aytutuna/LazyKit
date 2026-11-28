//! The embedded font file of a font descriptor, whichever of the three formats it holds.

use std::collections::HashMap;
use std::sync::OnceLock;

use lazy_pdf_syntax::{Dict, Document, Object};

use crate::cff::CffFont;
use crate::encodings::unicode_of_glyph_name;
use crate::truetype::TrueTypeFont;
use crate::type1::Type1Font;

const MAX_SIZE: usize = 64 << 20;

pub(crate) struct FontProgram {
    pub true_type: Option<TrueTypeFont>,
    pub bare_cff: Option<CffFont>,
    pub type1: Option<Type1Font>,
    reverse: OnceLock<HashMap<i64, i64>>,
}

impl FontProgram {
    fn with(
        true_type: Option<TrueTypeFont>,
        bare_cff: Option<CffFont>,
        type1: Option<Type1Font>,
    ) -> Self {
        FontProgram {
            true_type,
            bare_cff,
            type1,
            reverse: OnceLock::new(),
        }
    }

    /// The CFF outlines of a bare CFF or an OpenType font.
    pub fn cff(&self) -> Option<&CffFont> {
        self.bare_cff
            .as_ref()
            .or_else(|| self.true_type.as_ref().and_then(|t| t.cff.as_ref()))
    }

    pub fn load(doc: &mut Document, descriptor: Option<&Dict>) -> Option<FontProgram> {
        let descriptor = descriptor?;
        for key in ["FontFile2", "FontFile3", "FontFile"] {
            let Some(stream) = doc.lookup(descriptor, key).and_then(|v| match &*v {
                Object::Stream(s) => Some(s.clone()),
                _ => None,
            }) else {
                continue;
            };
            let Ok(data) = doc.stream_data_limited(&stream, MAX_SIZE, false) else {
                continue;
            };
            if let Some(program) = parse(&data) {
                return Some(program);
            }
        }
        None
    }

    /// The glyph name the program's own encoding gives a code.
    pub fn built_in_name(&self, code: i64) -> Option<String> {
        if let Some(t1) = &self.type1 {
            return t1.built_in_encoding.get(code as usize).cloned().flatten();
        }
        let cff = self.cff()?;
        let gid = cff.glyph_for_code(code);
        if gid > 0 { cff.glyph_name(gid) } else { None }
    }

    /// The advance of a glyph in 1/1000 em, by name where the program has names, else by code.
    pub fn width(&self, code: i64, name: Option<&str>) -> Option<f64> {
        if let Some(t1) = &self.type1 {
            let own = t1.built_in_encoding.get(code as usize).cloned().flatten();
            let name = name.map(str::to_string).or(own)?;
            if !t1.has_glyph(&name) {
                return None;
            }
            return Some(t1.width(&name) * t1.font_matrix[0] * 1000.0);
        }
        if let Some(cff) = self.cff()
            && self.true_type.is_none()
        {
            let mut gid = name.map_or(0, |n| cff.glyph_for_name(n));
            if gid == 0 {
                gid = cff.glyph_for_code(code);
            }
            return (gid > 0).then(|| cff.width(gid) * cff.font_matrix[0] * 1000.0);
        }
        None
    }

    /// The text of a TrueType glyph: the cmap read backwards, then the glyph's name in the post table.
    pub fn unicode_of(&self, glyph: i64) -> Option<String> {
        let tt = self.true_type.as_ref()?;
        let table = self.reverse.get_or_init(|| reverse_cmap(tt));
        if let Some(&code_point) = table.get(&glyph) {
            return u32::try_from(code_point)
                .ok()
                .and_then(char::from_u32)
                .map(String::from);
        }
        tt.glyph_name(glyph).and_then(|n| unicode_of_glyph_name(&n))
    }
}

// The key a file sits under is no guide: sniff the bytes.
fn parse(data: &[u8]) -> Option<FontProgram> {
    if data.len() < 4 {
        return None;
    }
    let head = &data[..4];
    if head == [0, 1, 0, 0] || head == b"true" || head == b"OTTO" || head == b"ttcf" {
        return TrueTypeFont::parse(data, 0).map(|t| FontProgram::with(Some(t), None, None));
    }
    if data[0] == 1 && data[1] == 0 {
        return CffFont::parse(data).map(|c| FontProgram::with(None, Some(c), None));
    }
    Type1Font::parse(data).map(|t| FontProgram::with(None, None, Some(t)))
}

// The private use area is where symbol fonts park their glyphs; as text it is noise.
fn reverse_cmap(tt: &TrueTypeFont) -> HashMap<i64, i64> {
    let mut map = HashMap::new();
    for cp in 0x20..=0xFFFF {
        if (0xD800..=0xF8FF).contains(&cp) {
            continue;
        }
        let glyph = tt.glyph_for_code_point(cp);
        if glyph > 0 {
            map.entry(glyph).or_insert(cp);
        }
    }
    map
}
