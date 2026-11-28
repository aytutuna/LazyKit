//! A Compact Font Format program (Adobe TN 5176), bare as in a PDF's FontFile3 or taken from the 'CFF ' table of
//! an OpenType font. Name-keyed and CID-keyed fonts are both read; charstring type 2 only. A font that breaks the
//! format does not parse.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::bytes::{Span, fmax, fmin, parse_double, to_int32};
use crate::cff_interp::Interpreter;
use crate::names::{cff_strings, standard_encoding_sids};
use crate::outline::{Bounds, Outline};

const MAX_GLYPHS: i64 = 65535;
const MAX_FONT_DICTS: i64 = 256;
pub(crate) const MAX_DICT_SIZE: i64 = 1 << 16;

pub struct CffFont {
    data: Vec<u8>,
    strings: Index,
    char_strings: Index,
    charset: Vec<i64>,
    encoding: [i64; 256],
    name_to_glyph: OnceLock<HashMap<String, i64>>,
    id_to_glyph: OnceLock<HashMap<i64, i64>>,
    pub(crate) global_subrs: Index,
    pub(crate) privates: Vec<Private>,
    fd_select: Option<Vec<u8>>,
    /// The font name from the Name INDEX.
    pub name: String,
    pub is_cid: bool,
    /// The top dictionary's FontMatrix, 0.001 0 0 0.001 0 0 by default; a CID font's first font DICT adds its own,
    /// multiplied in.
    pub font_matrix: [f64; 6],
    pub font_bounds: Option<Bounds>,
}

pub(crate) struct Glyph {
    pub outline: Outline,
    pub width: f64,
}

#[derive(Clone, Default)]
pub(crate) struct Private {
    pub subrs: Index,
    pub default_width: f64,
    pub nominal_width: f64,
}

impl CffFont {
    pub fn glyph_count(&self) -> i64 {
        self.char_strings.count
    }

    pub(crate) fn bytes(&self) -> Span<'_> {
        Span(&self.data)
    }

    pub(crate) fn char_string(&self, gid: i64) -> Span<'_> {
        self.char_strings.item(self.bytes(), gid)
    }

    /// Parses the CFF data at `offset`, `length` bytes long. The bytes are copied.
    pub fn parse_at(data: &[u8], offset: i64, length: i64) -> Option<CffFont> {
        if offset < 0 || length < 4 || length > data.len() as i64 - offset {
            return None;
        }
        read(data[offset as usize..(offset + length) as usize].to_vec())
    }

    pub fn parse(data: &[u8]) -> Option<CffFont> {
        Self::parse_at(data, 0, data.len() as i64)
    }

    fn read_encoding(&mut self, at: i64) {
        let d = Span(&self.data);
        let mut encoding = [0i64; 256];
        if at == 0 {
            let by_sid = sid_to_glyph(&self.charset);
            for (c, slot) in encoding.iter_mut().enumerate() {
                let sid = standard_encoding_sids()[c];
                if sid != 0
                    && let Some(&g) = by_sid.get(&(sid as i64))
                {
                    *slot = g;
                }
            }
            self.encoding = encoding;
            return;
        }
        if at == 1 {
            return;
        }

        let format = d.u8(at);
        let mut p = at + 1;
        if (format & 0x7F) == 0 {
            let n = d.u8(p);
            p += 1;
            let mut i = 0;
            while i < n && p < d.len() {
                encoding[d.u8(p) as usize] = i + 1;
                p += 1;
                i += 1;
            }
        } else if (format & 0x7F) == 1 {
            let ranges = d.u8(p);
            p += 1;
            let mut gid = 1;
            let mut r = 0;
            while r < ranges && p <= d.len() - 2 {
                let first = d.u8(p);
                let mut i = 0;
                while i <= d.u8(p + 1) {
                    if first + i < 256 {
                        encoding[(first + i) as usize] = gid;
                        gid += 1;
                    }
                    i += 1;
                }
                r += 1;
                p += 2;
            }
        }
        if (format & 0x80) != 0 {
            let n = d.u8(p);
            p += 1;
            let by_sid = sid_to_glyph(&self.charset);
            let mut i = 0;
            while i < n && p <= d.len() - 3 {
                if let Some(&g) = by_sid.get(&d.u16(p + 1)) {
                    encoding[d.u8(p) as usize] = g;
                }
                i += 1;
                p += 3;
            }
        }
        self.encoding = encoding;
    }

    fn sids(&self) -> &HashMap<i64, i64> {
        self.id_to_glyph.get_or_init(|| sid_to_glyph(&self.charset))
    }

    /// Glyph for a code of the font's built-in encoding (name-keyed fonts), 0 when unassigned.
    pub fn glyph_for_code(&self, code: i64) -> i64 {
        if (0..256).contains(&code) {
            self.encoding[code as usize]
        } else {
            0
        }
    }

    /// The glyph name, none for a CID-keyed font or a glyph out of range.
    pub fn glyph_name(&self, glyph_id: i64) -> Option<String> {
        if self.is_cid || glyph_id < 0 || glyph_id >= self.charset.len() as i64 {
            return None;
        }
        let sid = self.charset[glyph_id as usize];
        let standard = cff_strings();
        if sid < standard.len() as i64 {
            return Some(standard[sid as usize].to_string());
        }
        let s = self.strings.item(self.bytes(), sid - standard.len() as i64);
        Some(lazy_pdf_syntax::latin1(s.0))
    }

    /// The first glyph with this name, 0 when there is none or the font is CID-keyed.
    pub fn glyph_for_name(&self, name: &str) -> i64 {
        if self.is_cid {
            return 0;
        }
        let map = self.name_to_glyph.get_or_init(|| {
            let mut map = HashMap::new();
            for g in 0..self.charset.len() as i64 {
                if let Some(n) = self.glyph_name(g) {
                    map.entry(n).or_insert(g);
                }
            }
            map
        });
        map.get(name).copied().unwrap_or(0)
    }

    /// Glyph for a CID. A CID-keyed font maps through its charset; a name-keyed font used as a CIDFont takes the
    /// CID as the glyph index (ISO 32000-1 9.7.4.2). 0 when absent.
    pub fn glyph_for_cid(&self, cid: i64) -> i64 {
        if !self.is_cid {
            return if cid >= 0 && cid < self.glyph_count() {
                cid
            } else {
                0
            };
        }
        self.sids().get(&cid).copied().unwrap_or(0)
    }

    /// The CID of a glyph in a CID-keyed font; the glyph index itself otherwise.
    pub fn cid_for_glyph(&self, glyph_id: i64) -> i64 {
        if self.is_cid && glyph_id >= 0 && glyph_id < self.charset.len() as i64 {
            self.charset[glyph_id as usize]
        } else {
            glyph_id
        }
    }

    /// Advance width of the glyph in font units, taken from its charstring. 0 for a glyph out of range.
    pub fn width(&self, glyph_id: i64) -> f64 {
        if glyph_id < 0 || glyph_id >= self.glyph_count() {
            0.0
        } else {
            self.interpret(glyph_id, true, true).width
        }
    }

    /// The glyph's outline in font units (cubic curves), none for a glyph out of range. A broken charstring gives
    /// what was drawn before it broke.
    pub fn outline(&self, glyph_id: i64) -> Option<Outline> {
        if glyph_id < 0 || glyph_id >= self.glyph_count() {
            None
        } else {
            Some(self.interpret(glyph_id, false, true).outline)
        }
    }

    pub(crate) fn interpret(&self, glyph_id: i64, width_only: bool, allow_seac: bool) -> Glyph {
        let mut fd = 0usize;
        if let Some(select) = &self.fd_select
            && (glyph_id as usize) < select.len()
        {
            fd = select[glyph_id as usize] as usize;
        }
        let private = &self.privates[fd.min(self.privates.len() - 1)];
        Interpreter::new(self, private, width_only, allow_seac).run(glyph_id)
    }
}

fn sid_to_glyph(charset: &[i64]) -> HashMap<i64, i64> {
    let mut map = HashMap::new();
    for g in (0..charset.len()).rev() {
        map.insert(charset[g], g as i64);
    }
    map
}

fn read(data: Vec<u8>) -> Option<CffFont> {
    let d = Span(&data);
    if d.u8(0) != 1 {
        return None;
    }
    let pos = d.u8(2);
    let names = Index::try_read(d, pos)?;
    let tops = Index::try_read(d, names.end)?;
    let strings = Index::try_read(d, tops.end)?;
    let gsubrs = Index::try_read(d, strings.end)?;
    if tops.count < 1 {
        return None;
    }

    let top = Dict::parse(tops.item(d, 0));
    let char_strings_at = to_int32(top.number(17, 0.0));
    if char_strings_at <= 0 {
        return None;
    }
    let char_strings = Index::try_read(d, char_strings_at)?;
    let glyph_count = char_strings.count.min(MAX_GLYPHS);

    let is_cid = top.has(1230);
    let mut privates = Vec::new();
    let mut fd_select = None;
    let mut cid_matrix = None;
    if is_cid {
        let fd_array_at = to_int32(top.number(1236, 0.0));
        if fd_array_at <= 0 {
            return None;
        }
        let fd_array = Index::try_read(d, fd_array_at)?;
        // Font DICTs may overlap, so each one is cut to a size no real one reaches.
        for i in 0..fd_array.count.min(MAX_FONT_DICTS) {
            let item = fd_array.item(d, i);
            let font_dict = Dict::parse(item.slice(0, item.len().min(MAX_DICT_SIZE)));
            if i == 0
                && let Some(m) = font_dict.numbers(1207)
                && m.len() == 6
            {
                cid_matrix = Some(m.to_vec());
            }
            privates.push(read_private(d, &font_dict));
        }
        fd_select = read_fd_select(d, to_int32(top.number(1237, 0.0)), glyph_count);
        if privates.is_empty() {
            return None;
        }
    } else {
        privates.push(read_private(d, &top));
    }

    let charset = read_charset(d, to_int32(top.number(15, 0.0)), glyph_count, is_cid);
    let top_matrix = top
        .numbers(1207)
        .filter(|m| m.len() == 6)
        .map(<[f64]>::to_vec);
    let font_matrix = combined_matrix(top_matrix, cid_matrix);
    let font_bounds = top.numbers(5).filter(|b| b.len() == 4).map(|b| Bounds {
        x_min: b[0],
        y_min: b[1],
        x_max: b[2],
        y_max: b[3],
    });
    let name = if names.count > 0 {
        lazy_pdf_syntax::latin1(names.item(d, 0).0)
    } else {
        String::new()
    };
    let encoding_at = to_int32(top.number(16, 0.0));

    let mut font = CffFont {
        data: Vec::new(),
        strings,
        char_strings,
        charset,
        encoding: [0; 256],
        name_to_glyph: OnceLock::new(),
        id_to_glyph: OnceLock::new(),
        global_subrs: gsubrs,
        privates,
        fd_select,
        name,
        is_cid,
        font_matrix,
        font_bounds,
    };
    font.data = data;
    if !is_cid {
        font.read_encoding(encoding_at);
    }
    Some(font)
}

// The top DICT's matrix and a font DICT's multiply; with only one of them that one stands alone (the top one
// defaults to 0.001, which a font DICT's matrix would otherwise scale twice).
fn combined_matrix(top: Option<Vec<f64>>, fd: Option<Vec<f64>>) -> [f64; 6] {
    let to_array = |v: Vec<f64>| [v[0], v[1], v[2], v[3], v[4], v[5]];
    let Some(fd) = fd else {
        return top.map_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0], to_array);
    };
    let Some(top) = top else { return to_array(fd) };
    [
        fd[0] * top[0] + fd[1] * top[2],
        fd[0] * top[1] + fd[1] * top[3],
        fd[2] * top[0] + fd[3] * top[2],
        fd[2] * top[1] + fd[3] * top[3],
        fd[4] * top[0] + fd[5] * top[2] + top[4],
        fd[4] * top[1] + fd[5] * top[3] + top[5],
    ]
}

fn read_charset(d: Span, at: i64, glyph_count: i64, is_cid: bool) -> Vec<i64> {
    let mut sids = vec![0i64; glyph_count as usize];
    if at <= 2 {
        // The predefined charsets are only exact for ISOAdobe; Expert ones fall back to identity.
        for g in 1..glyph_count {
            sids[g as usize] = if at == 0 && !is_cid { g.min(228) } else { g };
        }
        return sids;
    }

    let format = d.u8(at);
    let mut p = at + 1;
    let mut gid = 1;
    if format == 0 {
        while gid < glyph_count && p <= d.len() - 2 {
            sids[gid as usize] = d.u16(p);
            gid += 1;
            p += 2;
        }
    } else if format == 1 || format == 2 {
        let width = if format == 1 { 3 } else { 4 };
        while gid < glyph_count && p <= d.len() - width {
            let first = d.u16(p);
            let left = if format == 1 {
                d.u8(p + 2)
            } else {
                d.u16(p + 2)
            };
            p += width;
            let mut i = 0;
            while i <= left && gid < glyph_count {
                sids[gid as usize] = first + i;
                gid += 1;
                i += 1;
            }
        }
    }
    sids
}

fn read_fd_select(d: Span, at: i64, glyph_count: i64) -> Option<Vec<u8>> {
    if at <= 0 {
        return None;
    }
    let mut map = vec![0u8; glyph_count as usize];
    let format = d.u8(at);
    if format == 0 {
        for g in 0..glyph_count {
            map[g as usize] = d.u8(at + 1 + g) as u8;
        }
    } else if format == 3 {
        let ranges = d.u16(at + 1);
        let mut p = at + 3;
        let mut filled = 0;
        let mut r = 0;
        while r < ranges && p <= d.len() - 5 {
            let first = d.u16(p);
            let fd = d.u8(p + 2) as u8;
            let next = d.u16(p + 3).min(glyph_count);
            // Ranges ascend; one that starts inside the last is cut so that no glyph is written twice.
            let mut g = first.max(filled);
            while g < next {
                map[g as usize] = fd;
                g += 1;
            }
            filled = filled.max(next);
            r += 1;
            p += 3;
        }
    } else {
        return None;
    }
    Some(map)
}

/// Reads the Private DICT a font DICT points to, with its local subroutines.
fn read_private(d: Span, font_dict: &Dict) -> Private {
    let empty = Private::default();
    let Some(p) = font_dict.numbers(18) else {
        return empty;
    };
    if p.len() != 2 || p[1] <= 0.0 || p[1] >= d.len() as f64 {
        return empty;
    }
    let at = to_int32(p[1]);
    let size = to_int32(fmin(
        fmin(fmax(p[0], 0.0), (d.len() - at) as f64),
        MAX_DICT_SIZE as f64,
    ));
    let dict = Dict::parse(d.slice(at, size));
    let mut subrs = Index::default();
    let subrs_at = dict.number(19, 0.0);
    if subrs_at > 0.0 && (at as f64) + subrs_at < d.len() as f64 {
        subrs = Index::try_read(d, at + to_int32(subrs_at)).unwrap_or_default();
    }
    Private {
        subrs,
        default_width: dict.number(20, 0.0),
        nominal_width: dict.number(21, 0.0),
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Index {
    offsets_at: i64,
    off_size: i64,
    data_base: i64,
    pub count: i64,
    /// Position after the INDEX, where the next structure starts.
    pub end: i64,
}

impl Index {
    pub fn try_read(d: Span, pos: i64) -> Option<Index> {
        if pos < 0 || pos > d.len() - 2 {
            return None;
        }
        let count = d.u16(pos);
        if count == 0 {
            return Some(Index {
                count: 0,
                off_size: 1,
                offsets_at: pos + 2,
                data_base: pos + 2,
                end: pos + 2,
            });
        }
        let off_size = d.u8(pos + 2);
        if !(1..=4).contains(&off_size) {
            return None;
        }
        let offsets_at = pos + 3;
        let data_base = offsets_at + (count + 1) * off_size - 1;
        if data_base >= d.len() {
            return None;
        }
        let last = offset(d, offsets_at, off_size, count);
        if last < 1 || last > d.len() - data_base {
            return None;
        }
        Some(Index {
            count,
            off_size,
            offsets_at,
            data_base,
            end: data_base + last,
        })
    }

    pub fn item<'a>(&self, d: Span<'a>, i: i64) -> Span<'a> {
        if i < 0 || i >= self.count {
            return Span(&[]);
        }
        let start = offset(d, self.offsets_at, self.off_size, i);
        let stop = offset(d, self.offsets_at, self.off_size, i + 1);
        if start < 1 || stop < start || stop > self.end - self.data_base {
            return Span(&[]);
        }
        d.slice(self.data_base + start, stop - start)
    }
}

fn offset(d: Span, at: i64, size: i64, i: i64) -> i64 {
    let mut v = 0i64;
    for k in 0..size {
        v = (v << 8) | d.u8(at + i * size + k);
    }
    v.min(i32::MAX as i64)
}

/// A parsed DICT: the operands of each operator, a two-byte operator 12 n keyed as 1200 + n.
pub(crate) struct Dict {
    entries: HashMap<i64, Vec<f64>>,
}

const MAX_OPERANDS: usize = 48;

impl Dict {
    pub fn parse(d: Span) -> Dict {
        let mut entries = HashMap::new();
        let mut operands = [0f64; MAX_OPERANDS];
        let mut n = 0usize;
        let mut p = 0i64;
        while p < d.len() {
            let b = d.0[p as usize] as i64;
            p += 1;
            if b <= 21 {
                let mut op = b;
                if b == 12 {
                    op = 1200 + d.u8(p);
                    p += 1;
                }
                entries.insert(op, operands[..n].to_vec());
                n = 0;
                continue;
            }

            let v = if b == 28 {
                p += 2;
                d.i16(p - 2) as f64
            } else if b == 29 {
                p += 4;
                d.u32(p - 4) as i32 as f64
            } else if b == 30 {
                read_real(d, &mut p)
            } else if (32..=246).contains(&b) {
                (b - 139) as f64
            } else if (247..=250).contains(&b) {
                p += 1;
                ((b - 247) * 256 + d.u8(p - 1) + 108) as f64
            } else if (251..=254).contains(&b) {
                p += 1;
                (-(b - 251) * 256 - d.u8(p - 1) - 108) as f64
            } else {
                continue;
            };
            if n < MAX_OPERANDS {
                operands[n] = v;
                n += 1;
            }
        }
        Dict { entries }
    }

    pub fn has(&self, op: i64) -> bool {
        self.entries.contains_key(&op)
    }

    pub fn numbers(&self, op: i64) -> Option<&[f64]> {
        self.entries.get(&op).map(Vec::as_slice)
    }

    pub fn number(&self, op: i64, fallback: f64) -> f64 {
        match self.entries.get(&op) {
            Some(v) if !v.is_empty() => v[0],
            _ => fallback,
        }
    }
}

fn read_real(d: Span, p: &mut i64) -> f64 {
    let mut text = String::new();
    while *p < d.len() {
        let b = d.0[*p as usize] as i64;
        *p += 1;
        for half in 0..2 {
            let nibble = if half == 0 { b >> 4 } else { b & 15 };
            match nibble {
                0..=9 => text.push((b'0' + nibble as u8) as char),
                10 => text.push('.'),
                11 => text.push('E'),
                12 => text.push_str("E-"),
                14 => text.push('-'),
                15 => return parse_double(&text).unwrap_or(0.0),
                _ => {}
            }
        }
    }
    0.0
}
