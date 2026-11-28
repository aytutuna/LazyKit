//! A font dictionary understood (ISO 32000-1 9): Type1, MMType1, TrueType, Type3 and Type0 over CIDFontType0 and
//! CIDFontType2. It turns the bytes of a shown string into characters with a width and a Unicode value. A broken
//! font never fails: widths fall back to the Standard 14 metrics, then the descriptor's /MissingWidth, and text
//! that cannot be traced is none. Everything the dictionary holds is read when the font is made, so a font
//! outlives the document it came from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use lazy_pdf_syntax::{Dict, Document, Object, Rect};

use crate::cmap::CMap;
use crate::encodings::{Encoding, encoding_by_name, standard, unicode_of_glyph_name, win_ansi};
use crate::program::FontProgram;
use crate::standard14::Standard14;
use crate::truetype::TrueTypeFont;

const MAX_DIFFERENCES: usize = 4096;
const MAX_CACHED_CHARS: usize = 100_000;
const MAX_CID: f64 = 0xFFFF as f64;
const MAX_WIDTH_ENTRIES: usize = 300_000;
const MAP_LIMIT: usize = 16 << 20;

/// One character of a shown string. Widths and vertical metrics are in 1/1000 em (for a Type 3 font, in glyph
/// space scaled the same way by its /FontMatrix).
#[derive(Debug, Clone, PartialEq)]
pub struct FontChar {
    /// The code as read from the string.
    pub code: i64,
    /// How many bytes it took.
    pub length: usize,
    /// The CID of a composite font; the code for a simple one.
    pub cid: i64,
    /// The horizontal advance.
    pub width: f64,
    /// The text it stands for, none when nothing could be worked out.
    pub unicode: Option<String>,
    /// A single-byte code 32, the one word spacing applies to (9.3.3).
    pub is_space: bool,
    /// The advance when the font writes vertically (usually negative).
    pub vertical_advance: f64,
    /// Where a vertical font puts the glyph origin from the horizontal one, x.
    pub origin_x: f64,
    /// The same, y.
    pub origin_y: f64,
}

pub struct PdfFont {
    /// Type1, MMType1, TrueType, Type3 or Type0.
    pub subtype: String,
    /// /BaseFont as written, with any subset tag.
    pub base_font: String,
    /// /BaseFont without the six-letter subset tag (9.6.4).
    pub name: String,
    /// The font writes top to bottom (/WMode 1 in its CMap).
    pub is_vertical: bool,
    pub is_embedded: bool,
    /// The /FontDescriptor /Flags; for a Standard 14 font without a descriptor, the flags its metrics imply.
    pub flags: i64,
    pub is_italic: bool,
    pub is_bold: bool,
    /// In 1/1000 em; the descent is negative.
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    /// The glyph space to text space matrix: 0.001 scale, or the /FontMatrix of a Type 3 font.
    pub font_matrix: [f64; 6],
    /// The /FontBBox in glyph space, when the font or its descriptor gives one.
    pub font_bbox: Option<Rect>,
    std14: Option<&'static Standard14>,
    program: Option<FontProgram>,
    to_unicode: Option<CMap>,
    // Simple fonts
    widths: Option<Vec<f64>>,
    first_char: i64,
    missing_width: f64,
    names: Vec<Option<String>>,
    // Composite fonts
    cmap: Arc<CMap>,
    ordering: String,
    default_width: f64,
    default_origin_y: f64,
    default_advance: f64,
    cid_widths: HashMap<i64, f64>,
    cid_vertical: HashMap<i64, (f64, f64, f64)>,
    cid_to_gid: Option<Vec<u16>>,
    // Characters already read, as the reference keeps them: all 256 of a simple font, up to MAX_CACHED_CHARS of a
    // composite one.
    simple_chars: Mutex<Vec<Option<FontChar>>>,
    composite_chars: Mutex<HashMap<i64, FontChar>>,
}

fn sat(v: f64) -> i64 {
    v as i64
}

fn sat32(v: f64) -> i64 {
    sat(v).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
}

fn finite(doc: &mut Document, value: &Object) -> Option<f64> {
    let v = doc.resolve(value)?.as_number()?.value();
    v.is_finite().then_some(v)
}

fn array_of(doc: &mut Document, dict: &Dict, key: &str) -> Option<Vec<Object>> {
    doc.lookup(dict, key)?.as_array().map(<[Object]>::to_vec)
}

fn matrix_of(doc: &mut Document, items: &[Object]) -> Option<[f64; 6]> {
    if items.len() < 6 {
        return None;
    }
    let mut n = [0.0; 6];
    for (i, slot) in n.iter_mut().enumerate() {
        *slot = finite(doc, &items[i])?;
    }
    Some(n)
}

impl PdfFont {
    pub fn is_composite(&self) -> bool {
        self.subtype == "Type0"
    }

    pub fn is_type3(&self) -> bool {
        self.subtype == "Type3"
    }

    pub fn has_to_unicode(&self) -> bool {
        self.to_unicode.is_some()
    }

    pub fn is_fixed_pitch(&self) -> bool {
        self.flags & 1 != 0
    }

    pub fn is_serif(&self) -> bool {
        self.flags & 2 != 0
    }

    pub fn is_symbolic(&self) -> bool {
        self.flags & 4 != 0 && self.flags & 32 == 0
    }

    /// The font for a font dictionary of `doc`.
    pub fn new(doc: &mut Document, dictionary: &Dict) -> PdfFont {
        let base = doc
            .name_of(dictionary, "BaseFont")
            .map(|n| lazy_pdf_syntax::Name(n).text())
            .unwrap_or_default();
        let scalars: Vec<char> = base.chars().collect();
        let plain: String = if scalars.len() > 7
            && scalars[6] == '+'
            && scalars[..6].iter().all(|c| c.is_ascii_uppercase())
        {
            scalars[7..].iter().collect()
        } else {
            base.clone()
        };
        let declared = doc
            .name_of(dictionary, "Subtype")
            .map(|n| String::from_utf8_lossy(&n).into_owned());
        let subtype = match declared {
            Some(d) if ["Type1", "MMType1", "TrueType", "Type3", "Type0"].contains(&d.as_str()) => {
                d
            }
            _ if dictionary.contains_key("DescendantFonts") => "Type0".to_string(),
            _ => "Type1".to_string(),
        };
        let std14 = Standard14::find(&plain);
        let mut font = PdfFont {
            subtype,
            base_font: base,
            name: plain,
            is_vertical: false,
            is_embedded: false,
            flags: 0,
            is_italic: false,
            is_bold: false,
            ascent: 800.0,
            descent: -200.0,
            cap_height: 0.0,
            font_matrix: [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
            font_bbox: None,
            std14,
            program: None,
            to_unicode: None,
            widths: None,
            first_char: 0,
            missing_width: 0.0,
            names: vec![None; 256],
            cmap: CMap::predefined("Identity-H"),
            ordering: String::new(),
            default_width: 1000.0,
            default_origin_y: 880.0,
            default_advance: -1000.0,
            cid_widths: HashMap::new(),
            cid_vertical: HashMap::new(),
            cid_to_gid: None,
            simple_chars: Mutex::new(vec![None; 256]),
            composite_chars: Mutex::new(HashMap::new()),
        };
        font.initialize(doc, dictionary);
        font
    }

    fn initialize(&mut self, doc: &mut Document, dictionary: &Dict) {
        let mut holder = dictionary.clone();
        if self.is_composite()
            && let Some(first) =
                array_of(doc, dictionary, "DescendantFonts").and_then(|a| a.into_iter().next())
            && let Some(own) = doc.resolve(&first).and_then(|v| v.as_dict().cloned())
        {
            holder = own;
        }
        let descriptor = doc.dict_of(&holder, "FontDescriptor");
        self.is_embedded = descriptor.as_ref().is_some_and(|d| {
            d.contains_key("FontFile") || d.contains_key("FontFile2") || d.contains_key("FontFile3")
        });
        let empty = Dict::new();
        let d = descriptor.as_ref().unwrap_or(&empty);
        let has = descriptor.is_some();
        self.flags = match has.then(|| doc.integer_of(d, "Flags")).flatten() {
            Some(v) => i64::from(v as i32),
            None => self.std14.map_or(32, Standard14::flags),
        };
        let angle = has
            .then(|| doc.number_of(d, "ItalicAngle"))
            .flatten()
            .or_else(|| self.std14.map(|s| s.italic_angle))
            .unwrap_or(0.0);
        let lowered = self.name.to_lowercase();
        self.is_italic = self.flags & 64 != 0
            || angle != 0.0
            || lowered.contains("italic")
            || lowered.contains("oblique");
        self.is_bold = self.flags & (1 << 18) != 0
            || lowered.contains("bold")
            || has
                .then(|| doc.number_of(d, "FontWeight"))
                .flatten()
                .unwrap_or(0.0)
                >= 700.0
            || self.std14.is_some_and(|s| s.is_bold);
        let own_box = has
            .then(|| d.get("FontBBox").and_then(|v| doc.rect(v)))
            .flatten();
        self.font_bbox = own_box.or_else(|| {
            self.std14.map(|s| {
                Rect::new(
                    s.b_box.left as f64,
                    s.b_box.bottom as f64,
                    s.b_box.right as f64,
                    s.b_box.top as f64,
                )
            })
        });
        self.cap_height = has
            .then(|| doc.number_of(d, "CapHeight"))
            .flatten()
            .or_else(|| self.std14.map(|s| s.cap_height as f64))
            .unwrap_or(0.0);
        let mut ascent = has
            .then(|| doc.number_of(d, "Ascent"))
            .flatten()
            .or_else(|| self.std14.map(|s| s.ascent as f64))
            .unwrap_or(0.0);
        let mut descent = has
            .then(|| doc.number_of(d, "Descent"))
            .flatten()
            .or_else(|| self.std14.map(|s| s.descent as f64))
            .unwrap_or(0.0);
        if ascent == 0.0
            && descent == 0.0
            && let Some(b) = self.font_bbox
        {
            ascent = b.top;
            descent = b.bottom;
        }
        if ascent != 0.0 || descent != 0.0 {
            self.ascent = ascent;
            self.descent = if descent > 0.0 { -descent } else { descent };
        }
        if self.is_embedded {
            self.program = FontProgram::load(doc, descriptor.as_ref());
        }

        if self.is_composite() {
            self.initialize_composite(doc, dictionary, &holder);
        } else {
            self.initialize_simple(doc, dictionary, descriptor.as_ref());
        }
        // Apart from the metrics, so that a damaged map costs the text and nothing else.
        self.to_unicode = read_stream(doc, dictionary, "ToUnicode", MAP_LIMIT)
            .map(|data| CMap::parse(&data, None));
    }

    // A ToUnicode map is often written at another code width than the font reads, so a miss at the exact width
    // falls back to the numeric code at any width.
    fn mapped_text(&self, code: i64, length: usize) -> Option<String> {
        let map = self.to_unicode.as_ref()?;
        if let Some(exact) = map.unicode_of(code, length)
            && !exact.is_empty()
        {
            return Some(exact);
        }
        map.unicode_of(code, 0).filter(|any| !any.is_empty())
    }

    /// Reads the first character of `bytes`; returns the bytes it took (0 for empty input).
    pub fn read(&self, bytes: &[u8]) -> (usize, FontChar) {
        let Some(&first) = bytes.first() else {
            return (
                0,
                FontChar {
                    code: 0,
                    length: 0,
                    cid: 0,
                    width: 0.0,
                    unicode: None,
                    is_space: false,
                    vertical_advance: 0.0,
                    origin_x: 0.0,
                    origin_y: 0.0,
                },
            );
        };
        if self.is_composite() {
            self.read_composite(bytes)
        } else {
            (1, self.read_simple(first))
        }
    }

    pub fn decode(&self, bytes: &[u8]) -> Vec<FontChar> {
        let mut result = Vec::with_capacity(bytes.len());
        let mut rest = bytes;
        while !rest.is_empty() {
            let (used, c) = self.read(rest);
            result.push(c);
            rest = &rest[used..];
        }
        result
    }

    /// The string as text; characters that could not be traced are left out.
    pub fn to_text(&self, bytes: &[u8]) -> String {
        self.decode(bytes)
            .into_iter()
            .filter_map(|c| c.unicode)
            .collect()
    }

    /// The glyph name the encoding gives a code of a simple font.
    pub fn glyph_name_of(&self, code: i64) -> Option<&str> {
        if self.is_composite() || !(0..=255).contains(&code) {
            return None;
        }
        self.names[code as usize].as_deref()
    }

    /// The advance in 1/1000 em the font program gives a code of a simple font; none without a program or the
    /// glyph.
    pub fn program_width(&self, code: i64) -> Option<f64> {
        if self.is_composite() || !(0..=255).contains(&code) {
            return None;
        }
        let program = self.program.as_ref()?;
        if let Some(own) = program.width(code, self.names[code as usize].as_deref()) {
            return Some(own);
        }
        if let Some(tt) = &program.true_type
            && tt.cff.is_none()
        {
            let gid = self.true_type_glyph(tt, code);
            if gid > 0 {
                return Some(tt.advance(gid) as f64 * 1000.0 / tt.units_per_em as f64);
            }
        }
        None
    }

    /// The advance in 1/1000 em the font program gives a CID of a composite font; none without a program or the
    /// glyph.
    pub fn program_cid_width(&self, cid: i64) -> Option<f64> {
        if !self.is_composite() {
            return None;
        }
        let program = self.program.as_ref()?;
        if let Some(tt) = &program.true_type
            && tt.cff.is_none()
        {
            let gid = self.glyph_of(cid);
            return (gid >= 0 && gid < tt.glyph_count)
                .then(|| tt.advance(gid) as f64 * 1000.0 / tt.units_per_em as f64);
        }
        if let Some(cff) = program.cff() {
            let gid = if cff.is_cid {
                cff.glyph_for_cid(cid)
            } else {
                cid
            };
            return (gid > 0 || cid == 0)
                .then(|| cff.width(gid.max(0)) * cff.font_matrix[0] * 1000.0);
        }
        None
    }

    /// Whether the embedded program has an outline for the character. True for a font with no program of its own
    /// (the viewer draws it), and for white space, whose glyph is empty.
    pub fn can_draw(&self, ch: &FontChar) -> bool {
        let Some(program) = self.program.as_ref().filter(|_| self.is_embedded) else {
            return true;
        };
        let blank = ch
            .unicode
            .as_ref()
            .is_some_and(|u| !u.is_empty() && u.chars().all(char::is_whitespace));
        if self.is_composite() {
            if let Some(tt) = &program.true_type
                && tt.cff.is_none()
            {
                let gid = self.glyph_of(ch.cid);
                return if gid > 0 {
                    tt.bounds(gid).is_some() || blank
                } else {
                    blank
                };
            }
            if let Some(cff) = program.cff() {
                let gid = if cff.is_cid {
                    cff.glyph_for_cid(ch.cid)
                } else {
                    ch.cid
                };
                return if gid > 0 {
                    cff.outline(gid).is_some_and(|o| !o.is_empty()) || blank
                } else {
                    blank
                };
            }
            return true;
        }
        let code = ch.code;
        if !(0..=255).contains(&code) {
            return blank;
        }
        if let Some(simple) = &program.true_type
            && simple.cff.is_none()
        {
            let gid = self.true_type_glyph(simple, code);
            return if gid > 0 {
                simple.bounds(gid).is_some() || blank
            } else {
                blank
            };
        }
        let name = self.names[code as usize].as_deref();
        if let Some(t1) = &program.type1 {
            return name.is_some_and(|n| t1.has_glyph(n));
        }
        if let Some(bare) = program.cff() {
            let mut gid = name.map_or(0, |n| bare.glyph_for_name(n));
            if gid == 0 {
                gid = bare.glyph_for_code(code);
            }
            return if gid > 0 {
                bare.outline(gid).is_some_and(|o| !o.is_empty()) || blank
            } else {
                blank
            };
        }
        true
    }

    // Simple fonts, one byte a character: the encoding gives each code a glyph name (9.6.6).

    fn initialize_simple(
        &mut self,
        doc: &mut Document,
        dictionary: &Dict,
        descriptor: Option<&Dict>,
    ) {
        let mut scale = 1.0;
        if self.is_type3() {
            if let Some(items) = array_of(doc, dictionary, "FontMatrix")
                && let Some(m) = matrix_of(doc, &items)
                && m[0] != 0.0
            {
                self.font_matrix = m;
            }
            scale = self.font_matrix[0] * 1000.0;
            self.font_bbox = dictionary.get("FontBBox").and_then(|v| doc.rect(v));
            if let Some(b) = self.font_bbox {
                self.ascent = b.top * self.font_matrix[3] * 1000.0;
                self.descent = b.bottom * self.font_matrix[3] * 1000.0;
            }
        }
        self.missing_width = descriptor
            .and_then(|d| doc.number_of(d, "MissingWidth"))
            .unwrap_or(0.0)
            * scale;
        self.first_char = doc
            .integer_of(dictionary, "FirstChar")
            .unwrap_or(0)
            .clamp(0, 255);
        if let Some(array) = array_of(doc, dictionary, "Widths") {
            let mut list = vec![0.0; array.len().min(256)];
            for (i, slot) in list.iter_mut().enumerate() {
                if let Some(v) = finite(doc, &array[i]) {
                    *slot = v * scale;
                }
            }
            self.widths = Some(list);
        }
        self.names = self.build_names(doc, dictionary);
    }

    fn read_simple(&self, code: u8) -> FontChar {
        if let Some(known) = &self.simple_chars.lock().unwrap()[usize::from(code)] {
            return known.clone();
        }
        let made = self.make_simple(code);
        self.simple_chars.lock().unwrap()[usize::from(code)] = Some(made.clone());
        made
    }

    fn make_simple(&self, code: u8) -> FontChar {
        let index = i64::from(code);
        let width = self.simple_width(index);
        FontChar {
            code: index,
            length: 1,
            cid: index,
            width,
            unicode: self.simple_unicode(index),
            is_space: index == 32,
            vertical_advance: -1000.0,
            origin_x: width / 2.0,
            origin_y: 880.0,
        }
    }

    fn simple_width(&self, code: i64) -> f64 {
        if let Some(widths) = &self.widths {
            let index = code - self.first_char;
            return if index >= 0 && (index as usize) < widths.len() {
                widths[index as usize]
            } else {
                self.missing_width
            };
        }
        if self.is_type3() {
            return self.missing_width;
        }
        let name = self.names[code as usize].as_deref();
        if let Some(name) = name
            && let Some(standard) = self.std14.and_then(|s| s.width_of(name))
        {
            return standard as f64;
        }
        if let Some(program) = &self.program {
            if let Some(own) = program.width(code, name) {
                return own;
            }
            if let Some(tt) = &program.true_type
                && tt.cff.is_none()
            {
                let gid = self.true_type_glyph(tt, code);
                if gid > 0 {
                    return tt.advance(gid) as f64 * 1000.0 / tt.units_per_em as f64;
                }
            }
        }
        // No widths and no font to ask: the metrics a viewer would substitute.
        if self.std14.is_none()
            && let Some(name) = name
            && let Some(substitute) = self.substitute().and_then(|s| s.width_of(name))
        {
            return substitute as f64;
        }
        self.missing_width
    }

    fn substitute(&self) -> Option<&'static Standard14> {
        let family = if self.is_fixed_pitch() {
            "Courier"
        } else if self.is_serif() {
            "Times"
        } else {
            "Helvetica"
        };
        Standard14::find(&if self.is_bold {
            format!("{family}-Bold")
        } else if family == "Times" {
            "Times-Roman".to_string()
        } else {
            family.to_string()
        })
    }

    fn simple_unicode(&self, code: i64) -> Option<String> {
        if let Some(mapped) = self.mapped_text(code, 1) {
            return Some(mapped);
        }
        let name = self.names[code as usize].as_deref();
        if let Some(from_name) = name.and_then(unicode_of_glyph_name) {
            return Some(from_name);
        }
        let program = self.program.as_ref()?;
        if let Some(tt) = &program.true_type
            && tt.cff.is_none()
        {
            let gid = self.true_type_glyph(tt, code);
            return if gid > 0 {
                program.unicode_of(gid)
            } else {
                None
            };
        }
        program
            .built_in_name(code)
            .and_then(|n| unicode_of_glyph_name(&n))
    }

    // 9.6.6.4: a symbolic font's cmap is read by code, (3,0) at 0xF000 and up, or (1,0).
    fn true_type_glyph(&self, tt: &TrueTypeFont, code: i64) -> i64 {
        if let Some(name) = self.names.get(code as usize).and_then(|n| n.as_deref())
            && let Some(text) = unicode_of_glyph_name(name)
            && let Some(first) = text.chars().next()
        {
            let glyph = tt.glyph_for_code_point(i64::from(first as u32));
            if glyph > 0 {
                return glyph;
            }
        }
        let mut gid = tt.glyph_for_cmap(3, 0, code);
        if gid == 0 {
            gid = tt.glyph_for_cmap(3, 0, 0xF000 + code);
        }
        if gid == 0 {
            gid = tt.glyph_for_cmap(1, 0, code);
        }
        if gid == 0 {
            gid = tt.glyph_for_code_point(code);
        }
        gid
    }

    fn build_names(&self, doc: &mut Document, dictionary: &Dict) -> Vec<Option<String>> {
        let mut result: Vec<Option<String>> = vec![None; 256];
        let mut table: Option<&'static Encoding> = None;
        let mut differences: Option<Vec<Object>> = None;
        let encoding = dictionary
            .get("Encoding")
            .and_then(|v| doc.resolve(v))
            .map(lazy_pdf_syntax::Value::into_shared);
        match encoding.as_deref() {
            Some(Object::Name(n)) => table = encoding_by_name(&n.text()),
            Some(Object::Dict(d)) => {
                if let Some(base) = doc.name_of(d, "BaseEncoding") {
                    table = encoding_by_name(&String::from_utf8_lossy(&base));
                }
                differences = array_of(doc, d, "Differences");
            }
            _ => {}
        }
        let table = table.or_else(|| self.default_encoding());
        if let Some(table) = table {
            for (slot, name) in result.iter_mut().zip(table.iter()) {
                *slot = name.map(str::to_string);
            }
        } else if let Some(own) = self.own_encoding() {
            result = own;
        }
        if let Some(differences) = differences {
            let mut code = 0i64;
            for item in differences.iter().take(MAX_DIFFERENCES) {
                let resolved = doc.resolve(item).map(lazy_pdf_syntax::Value::into_shared);
                match resolved.as_deref() {
                    Some(Object::Number(n)) => code = sat32(n.value().clamp(-1.0, 1000.0)),
                    Some(Object::Name(glyph)) => {
                        if (0..256).contains(&code) {
                            result[code as usize] = Some(glyph.text());
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }
        result
    }

    // 9.6.6.1: with no base encoding a symbolic or embedded font uses its own.
    fn default_encoding(&self) -> Option<&'static Encoding> {
        if let Some(standard14) = self.std14
            && standard14.is_symbolic()
        {
            return Some(standard14.built_in_encoding());
        }
        if self.is_embedded && self.subtype != "TrueType" && self.own_encoding().is_some() {
            return None;
        }
        if self.subtype == "TrueType" {
            return if self.is_symbolic() && self.is_embedded {
                None
            } else {
                Some(win_ansi())
            };
        }
        Some(standard())
    }

    // The embedded program's own encoding, when it assigns any code.
    fn own_encoding(&self) -> Option<Vec<Option<String>>> {
        let program = self.program.as_ref()?;
        if self.subtype == "TrueType" {
            return None;
        }
        let own: Vec<Option<String>> = (0..256).map(|i| program.built_in_name(i)).collect();
        own.iter().any(Option::is_some).then_some(own)
    }

    // Composite fonts: a CMap cuts the string into codes and gives each a CID, the descendant CIDFont gives the CID
    // a width and a glyph (9.7).

    fn initialize_composite(&mut self, doc: &mut Document, dictionary: &Dict, cid_font: &Dict) {
        let encoding = dictionary
            .get("Encoding")
            .and_then(|v| doc.resolve(v))
            .map(lazy_pdf_syntax::Value::into_shared);
        match encoding.as_deref() {
            Some(Object::Name(n)) => self.cmap = CMap::predefined(&n.text()),
            Some(Object::Stream(stream)) => {
                if let Some(map) = read_cmap(doc, stream) {
                    self.cmap = Arc::new(map);
                }
            }
            _ => {}
        }
        self.is_vertical = self.cmap.is_vertical;
        if let Some(info) = doc.dict_of(cid_font, "CIDSystemInfo")
            && let Some(value) = doc.lookup(&info, "Ordering")
            && let Object::String(s) = &*value
        {
            self.ordering = lazy_pdf_syntax::latin1(&s.bytes);
        }
        self.default_width = doc.number_of(cid_font, "DW").unwrap_or(1000.0);
        if let Some(dw2) = array_of(doc, cid_font, "DW2")
            && dw2.len() >= 2
            && let Some(vy) = doc
                .resolve(&dw2[0])
                .and_then(|v| v.as_number().map(|n| n.value()))
            && let Some(w1) = doc
                .resolve(&dw2[1])
                .and_then(|v| v.as_number().map(|n| n.value()))
        {
            self.default_origin_y = vy;
            self.default_advance = w1;
        }
        if let Some(w) = array_of(doc, cid_font, "W") {
            self.read_widths(doc, &w);
        }
        if let Some(w2) = array_of(doc, cid_font, "W2") {
            self.read_vertical_metrics(doc, &w2);
        }
        if let Some(data) = read_stream(doc, cid_font, "CIDToGIDMap", 1 << 18) {
            self.cid_to_gid = Some(
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect(),
            );
        }
    }

    // 9.7.4.3: "c [w1 w2 ...]" or "cfirst clast w".
    fn read_widths(&mut self, doc: &mut Document, items: &[Object]) {
        let mut budget = MAX_WIDTH_ENTRIES;
        let mut i = 0;
        while i < items.len() && budget > 0 {
            let Some(first) = finite(doc, &items[i]) else {
                i += 1;
                continue;
            };
            let shared = items
                .get(i + 1)
                .and_then(|o| doc.resolve(o))
                .map(|v| v.into_shared());
            if let Some(list) = shared.as_deref().and_then(Object::as_array) {
                if first > MAX_CID {
                    budget = budget.saturating_sub(list.len().min(budget));
                    i += 2;
                    continue;
                }
                let mut k = 0;
                while k < list.len() && first + k as f64 <= MAX_CID && budget > 0 {
                    budget -= 1;
                    if let Some(width) = finite(doc, &list[k]) {
                        self.cid_widths.insert(sat32(first) + k as i64, width);
                    }
                    k += 1;
                }
                i += 2;
            } else if i + 2 < items.len() {
                if let Some(last) = finite(doc, &items[i + 1])
                    && let Some(width) = finite(doc, &items[i + 2])
                {
                    let mut c = sat(first.max(0.0));
                    while c as f64 <= last.min(MAX_CID) && budget > 0 {
                        budget -= 1;
                        self.cid_widths.insert(c, width);
                        c += 1;
                    }
                }
                i += 3;
            } else {
                break;
            }
        }
    }

    // 9.7.4.3: "c [w1 vx vy ...]" or "cfirst clast w1 vx vy".
    fn read_vertical_metrics(&mut self, doc: &mut Document, items: &[Object]) {
        let mut budget = MAX_WIDTH_ENTRIES;
        let mut i = 0;
        while i < items.len() && budget > 0 {
            let Some(first) = finite(doc, &items[i]) else {
                i += 1;
                continue;
            };
            let shared = items
                .get(i + 1)
                .and_then(|o| doc.resolve(o))
                .map(|v| v.into_shared());
            if let Some(list) = shared.as_deref().and_then(Object::as_array) {
                if first > MAX_CID {
                    budget = budget.saturating_sub(list.len().min(budget));
                    i += 2;
                    continue;
                }
                let mut k = 0;
                while k + 2 < list.len() && first + (k / 3) as f64 <= MAX_CID && budget > 0 {
                    budget -= 1;
                    if let Some(a) = finite(doc, &list[k])
                        && let Some(x) = finite(doc, &list[k + 1])
                        && let Some(y) = finite(doc, &list[k + 2])
                    {
                        self.cid_vertical
                            .insert(sat32(first) + (k / 3) as i64, (a, x, y));
                    }
                    k += 3;
                }
                i += 2;
            } else if i + 4 < items.len() {
                if let Some(last) = finite(doc, &items[i + 1])
                    && let Some(a) = finite(doc, &items[i + 2])
                    && let Some(x) = finite(doc, &items[i + 3])
                    && let Some(y) = finite(doc, &items[i + 4])
                {
                    let mut c = sat(first.max(0.0));
                    while c as f64 <= last.min(MAX_CID) && budget > 0 {
                        budget -= 1;
                        self.cid_vertical.insert(c, (a, x, y));
                        c += 1;
                    }
                }
                i += 5;
            } else {
                break;
            }
        }
    }

    fn read_composite(&self, bytes: &[u8]) -> (usize, FontChar) {
        let (mut code, mut length) = self.cmap.next_code(bytes);
        if length == 0 || length > bytes.len() {
            length = 1;
            code = i64::from(bytes[0]);
        }
        let key = ((length as i64) << 32) | (code & 0xFFFF_FFFF);
        if let Some(known) = self.composite_chars.lock().unwrap().get(&key) {
            return (length, known.clone());
        }
        let cid = self.cmap.cid_of(code, length).unwrap_or(0);
        let width = self
            .cid_widths
            .get(&cid)
            .copied()
            .unwrap_or(self.default_width);
        let vertical = self.cid_vertical.get(&cid).copied().unwrap_or((
            self.default_advance,
            width / 2.0,
            self.default_origin_y,
        ));
        let character = FontChar {
            code,
            length,
            cid,
            width,
            unicode: self.composite_unicode(code, length, cid),
            is_space: length == 1 && code == 32,
            vertical_advance: vertical.0,
            origin_x: vertical.1,
            origin_y: vertical.2,
        };
        let mut cache = self.composite_chars.lock().unwrap();
        if cache.len() < MAX_CACHED_CHARS {
            cache.insert(key, character.clone());
        }
        (length, character)
    }

    fn composite_unicode(&self, code: i64, length: usize, cid: i64) -> Option<String> {
        if let Some(mapped) = self.mapped_text(code, length) {
            return Some(mapped);
        }
        if self
            .cmap
            .name
            .as_deref()
            .is_some_and(|n| n.starts_with("Uni"))
            && let Some(direct) = self.cmap.unicode_of(code, length)
            && !direct.is_empty()
        {
            return Some(direct);
        }
        if cid == 0 {
            return None;
        }
        if let Some(table) = cid_to_unicode(&self.ordering)
            && let Some(&code_point) = table.get(&cid)
        {
            return u32::try_from(code_point)
                .ok()
                .and_then(char::from_u32)
                .map(String::from);
        }
        let program = self.program.as_ref()?;
        if let Some(tt) = &program.true_type
            && tt.cff.is_none()
        {
            let gid = self.glyph_of(cid);
            return if gid > 0 {
                program.unicode_of(gid)
            } else {
                None
            };
        }
        if let Some(cff) = program.cff() {
            let gid = if cff.is_cid {
                cff.glyph_for_cid(cid)
            } else {
                cid
            };
            if gid > 0
                && let Some(glyph_name) = cff.glyph_name(gid)
            {
                return unicode_of_glyph_name(&glyph_name);
            }
        }
        None
    }

    // /CIDToGIDMap of a CIDFontType2: identity unless a stream says otherwise (9.7.4.2).
    fn glyph_of(&self, cid: i64) -> i64 {
        match &self.cid_to_gid {
            None => cid,
            // A negative CID can come out of a CMap range that wraps.
            Some(table) => {
                if cid >= 0 && (cid as usize) < table.len() {
                    i64::from(table[cid as usize])
                } else {
                    0
                }
            }
        }
    }
}

fn read_stream(doc: &mut Document, dict: &Dict, key: &str, limit: usize) -> Option<Vec<u8>> {
    let stream = match &*doc.lookup(dict, key)? {
        Object::Stream(s) => s.clone(),
        _ => return None,
    };
    doc.stream_data_limited(&stream, limit, false).ok()
}

fn read_cmap(doc: &mut Document, stream: &lazy_pdf_syntax::Stream) -> Option<CMap> {
    let parent = match doc
        .lookup(&stream.dict, "UseCMap")
        .map(lazy_pdf_syntax::Value::into_shared)
        .as_deref()
    {
        Some(Object::Name(n)) => Some(CMap::predefined(&n.text())),
        Some(Object::Stream(inner)) if inner.data_offset != stream.data_offset => {
            let data = doc.stream_data_limited(inner, MAP_LIMIT, false).ok()?;
            Some(Arc::new(CMap::parse(&data, None)))
        }
        _ => None,
    };
    let data = doc.stream_data_limited(stream, MAP_LIMIT, false).ok()?;
    Some(CMap::parse(&data, parent))
}

// Adobe's character collections, and the Unicode CMaps that reach their CIDs (the half-width one is where the
// single-byte codes of the Shift-JIS CMaps go). The vertical ones come last: they add the CIDs of the vertical
// forms, which the horizontal maps never reach.
fn unicode_cmaps(ordering: &str) -> Option<&'static [&'static str]> {
    Some(match ordering {
        "Japan1" => &["UniJIS-UCS2-H", "UniJIS-UCS2-HW-H", "UniJIS-UCS2-V"],
        "GB1" => &["UniGB-UCS2-H", "UniGB-UCS2-V"],
        "CNS1" => &["UniCNS-UCS2-H", "UniCNS-UCS2-V"],
        "Korea1" => &["UniKS-UCS2-H", "UniKS-UCS2-V"],
        _ => return None,
    })
}

type CidTable = Arc<HashMap<i64, i64>>;

fn cid_to_unicode(ordering: &str) -> Option<CidTable> {
    static TABLES: OnceLock<Mutex<HashMap<String, CidTable>>> = OnceLock::new();
    let names = unicode_cmaps(ordering)?;
    let mut tables = TABLES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()?;
    if let Some(known) = tables.get(ordering) {
        return Some(known.clone());
    }
    let mut map = HashMap::new();
    for name in names {
        let unicode_map = CMap::predefined(name);
        for code in 0x20..=0xFFFF {
            if let Some(cid) = unicode_map.cid_of(code, 2)
                && cid > 0
            {
                map.entry(cid).or_insert(code);
            }
        }
    }
    let built = Arc::new(map);
    tables.insert(ordering.to_string(), built.clone());
    Some(built)
}
