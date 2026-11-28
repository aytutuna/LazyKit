//! An sfnt font: TrueType outlines (glyf) or OpenType with CFF outlines. Parses over the caller's bytes and
//! tolerates broken tables: a missing or truncated table gives zeros and empty outlines.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::bytes::{Span, to_int32};
use crate::cff::CffFont;
use crate::names::mac_glyphs;
use crate::outline::{Bounds, Outline};

const MAX_COMPOSITE_DEPTH: i64 = 8;
const MAX_COMPONENTS: i64 = 512;
const MAX_POINTS: i64 = 100_000;

struct CmapSubtable {
    platform: i64,
    encoding: i64,
    offset: i64,
}

pub struct TrueTypeFont {
    data: Vec<u8>,
    tables: HashMap<String, (i64, i64)>,
    loca_format: i64,
    h_metric_count: i64,
    pub units_per_em: i64,
    pub glyph_count: i64,
    /// Typographic ascent in font units (hhea, else OS/2), positive.
    pub ascent: i64,
    /// Descent in font units, negative below the baseline.
    pub descent: i64,
    pub line_gap: i64,
    pub cap_height: i64,
    pub x_height: i64,
    /// Box around all glyphs, from head.
    pub font_bounds: Bounds,
    pub italic_angle: f64,
    pub is_fixed_pitch: bool,
    pub is_bold_style: bool,
    /// usWeightClass of OS/2, 400 when the table is absent.
    pub weight_class: i64,
    /// The OS/2 licensing bits, 0 (installable) when the font has no OS/2 table.
    pub fs_type: i64,
    /// The CFF outlines of an OpenType/CFF font, none for TrueType outlines.
    pub cff: Option<CffFont>,
    cmaps: Vec<CmapSubtable>,
    unicode: i64,
    symbol: i64,
    glyph_names: OnceLock<Vec<Option<String>>>,
    name_to_glyph: OnceLock<HashMap<String, i64>>,
}

#[derive(Clone, Copy)]
struct Pt {
    x: f64,
    y: f64,
    on: bool,
}

#[derive(Default)]
struct Contours {
    points: Vec<Pt>,
    ends: Vec<i64>,
}

fn is_sfnt_tag(d: Span) -> bool {
    let v = d.u32(0);
    v == 0x0001_0000 || v == 0x4F54_544F || v == 0x7472_7565
}

impl TrueTypeFont {
    /// 1 for a plain font file, the number of fonts in a collection (.ttc), 0 when the bytes are no sfnt.
    pub fn collection_count(data: &[u8]) -> i64 {
        if data.len() < 12 {
            return 0;
        }
        let d = Span(data);
        if d.tag(0) == "ttcf" {
            return i64::from(d.u32(8).min(4096));
        }
        i64::from(is_sfnt_tag(d))
    }

    /// Opens the font at `collection_index` (0 for a plain file), or none when it has no usable head and maxp.
    pub fn parse(data: &[u8], collection_index: i64) -> Option<TrueTypeFont> {
        let count = Self::collection_count(data);
        if count == 0 || collection_index < 0 || collection_index >= count {
            return None;
        }

        let all = Span(data);
        let len = data.len() as i64;
        let mut dir = 0;
        if all.tag(0) == "ttcf" {
            dir = all.offset32(12 + 4 * collection_index);
            if dir <= 0 || dir > len - 12 || !is_sfnt_tag(all.from(dir)) {
                return None;
            }
        }

        let num_tables = all.u16(dir + 4).min((len - dir - 12) / 16);
        let mut tables: HashMap<String, (i64, i64)> = HashMap::new();
        let mut i = 0;
        while i < num_tables {
            let e = dir + 12 + 16 * i;
            i += 1;
            let offset = all.offset32(e + 8);
            let mut length = all.offset32(e + 12);
            if offset <= 0 || offset >= len {
                continue;
            }
            length = length.min(len - offset);
            tables.entry(all.tag(e)).or_insert((offset, length));
        }
        if tables.get("head")?.1 < 54 || tables.get("maxp")?.1 < 6 {
            return None;
        }
        Some(Self::new(data.to_vec(), tables))
    }

    fn new(data: Vec<u8>, tables: HashMap<String, (i64, i64)>) -> TrueTypeFont {
        let table = |tag: &str| -> Span {
            match tables.get(tag) {
                Some(&(offset, length)) => Span(&data).slice(offset, length),
                None => Span(&[]),
            }
        };

        let head = table("head");
        let mut upem = head.u16(18);
        if !(16..=16384).contains(&upem) {
            upem = 1000;
        }
        let font_bounds = Bounds {
            x_min: head.i16(36) as f64,
            y_min: head.i16(38) as f64,
            x_max: head.i16(40) as f64,
            y_max: head.i16(42) as f64,
        };
        let is_bold_style = (head.u16(44) & 1) != 0;
        let loca_format = head.i16(50);
        let glyph_count = table("maxp").u16(4);

        let hhea = table("hhea");
        let mut asc = hhea.i16(4);
        let mut desc = hhea.i16(6);
        let line_gap = hhea.i16(8);
        let h_metric_count = hhea.u16(34);

        let mut weight = 400;
        let mut fs = 0;
        let mut xh = 0;
        let mut cap = 0;
        let os2 = table("OS/2");
        if os2.len() >= 10 {
            weight = os2.u16(4);
            fs = os2.u16(8);
            if asc == 0 && desc == 0 {
                asc = os2.i16(68);
                desc = os2.i16(70);
                if asc == 0 && desc == 0 {
                    asc = os2.u16(74);
                    desc = -os2.u16(76);
                }
            }
            if os2.u16(0) >= 2 && os2.len() >= 90 {
                xh = os2.i16(86);
                cap = os2.i16(88);
            }
        }

        let post = table("post");
        let italic_angle = post.i16(4) as f64 + post.u16(6) as f64 / 65536.0;
        let is_fixed_pitch = post.u32(12) != 0;

        let cff = tables
            .get("CFF ")
            .and_then(|&(offset, length)| CffFont::parse_at(&data, offset, length));

        let mut font = TrueTypeFont {
            data: Vec::new(),
            tables: HashMap::new(),
            loca_format,
            h_metric_count,
            units_per_em: upem,
            glyph_count,
            ascent: asc,
            descent: desc,
            line_gap,
            cap_height: cap,
            x_height: xh,
            font_bounds,
            italic_angle,
            is_fixed_pitch,
            is_bold_style,
            weight_class: weight,
            fs_type: fs,
            cff,
            cmaps: Vec::new(),
            unicode: -1,
            symbol: -1,
            glyph_names: OnceLock::new(),
            name_to_glyph: OnceLock::new(),
        };
        font.data = data;
        font.tables = tables;
        font.read_cmap_directory();
        if font.cap_height == 0 {
            font.cap_height = font.guess_cap_height();
        }
        font
    }

    /// True when the font has outlines this class can read.
    pub fn has_outlines(&self) -> bool {
        self.cff.is_some() || (self.has_table("glyf") && self.has_table("loca"))
    }

    pub fn is_cff(&self) -> bool {
        self.cff.is_some()
    }

    /// False for fonts whose license restricts embedding (fsType 0x0002) or that allow only bitmaps (0x0200)
    /// (OpenType spec, OS/2 fsType). Preview and print, and editable embedding, are allowed.
    pub fn embedding_allowed(&self) -> bool {
        (self.fs_type & 0x000E) != 0x0002 && (self.fs_type & 0x0200) == 0
    }

    /// False when fsType 0x0100 says the whole font must be embedded.
    pub fn subsetting_allowed(&self) -> bool {
        (self.fs_type & 0x0100) == 0
    }

    pub fn has_table(&self, tag: &str) -> bool {
        self.tables.contains_key(tag)
    }

    pub(crate) fn table(&self, tag: &str) -> Span<'_> {
        match self.tables.get(tag) {
            Some(&(offset, length)) => Span(&self.data).slice(offset, length),
            None => Span(&[]),
        }
    }

    /// Advance width of the glyph in font units, 0 for a glyph the font does not have.
    pub fn advance(&self, glyph_id: i64) -> i64 {
        if glyph_id < 0 || glyph_id >= self.glyph_count || self.h_metric_count == 0 {
            return 0;
        }
        self.table("hmtx")
            .u16(4 * glyph_id.min(self.h_metric_count - 1))
    }

    /// Left side bearing of the glyph from hmtx.
    pub fn left_side_bearing(&self, glyph_id: i64) -> i64 {
        if glyph_id < 0 || glyph_id >= self.glyph_count {
            return 0;
        }
        let hmtx = self.table("hmtx");
        if glyph_id < self.h_metric_count {
            hmtx.i16(4 * glyph_id + 2)
        } else {
            hmtx.i16(4 * self.h_metric_count + 2 * (glyph_id - self.h_metric_count))
        }
    }

    fn guess_cap_height(&self) -> i64 {
        let h = self.glyph_for_code_point(0x48);
        if h != 0
            && let Some(b) = self.bounds(h)
            && b.y_max > 0.0
        {
            return to_int32(b.y_max);
        }
        self.units_per_em * 7 / 10
    }

    /// Family name (name ID 1), none when the table has none.
    pub fn family_name(&self) -> Option<String> {
        self.read_name(1)
    }

    /// PostScript name (name ID 6), none when the table has none.
    pub fn post_script_name(&self) -> Option<String> {
        self.read_name(6)
    }

    fn read_name(&self, id: i64) -> Option<String> {
        let name = self.table("name");
        let count = name.u16(2).min((name.len() - 6) / 12);
        let storage = name.u16(4);
        let mut best = None;
        let mut best_rank = 0;
        let mut i = 0;
        while i < count {
            let r = 6 + 12 * i;
            i += 1;
            if name.u16(r + 6) != id {
                continue;
            }
            let platform = name.u16(r);
            let rank = match platform {
                3 if name.u16(r + 4) == 0x409 => 4,
                3 => 3,
                0 => 2,
                1 if name.u16(r + 4) == 0 => 1,
                _ => 0,
            };
            if rank <= best_rank {
                continue;
            }
            let length = name.u16(r + 8);
            let start = storage + name.u16(r + 10);
            if start > name.len() || length > name.len() - start {
                continue;
            }
            let raw = name.slice(start, length).0;
            best = Some(if platform == 1 {
                lazy_pdf_syntax::latin1(raw)
            } else {
                decode_big_endian_unicode(raw)
            });
            best_rank = rank;
        }
        best
    }

    /// Glyph name from 'post' (formats 1 and 2) or the CFF charset, none when the font has none for it.
    pub fn glyph_name(&self, glyph_id: i64) -> Option<String> {
        let names = self.glyph_names();
        if glyph_id >= 0 && glyph_id < names.len() as i64 {
            names[glyph_id as usize].clone()
        } else {
            None
        }
    }

    /// The first glyph with this name, 0 when there is none.
    pub fn glyph_for_name(&self, name: &str) -> i64 {
        let map = self.name_to_glyph.get_or_init(|| {
            let mut map = HashMap::new();
            for (g, n) in self.glyph_names().iter().enumerate() {
                if let Some(n) = n {
                    map.entry(n.clone()).or_insert(g as i64);
                }
            }
            map
        });
        map.get(name).copied().unwrap_or(0)
    }

    fn glyph_names(&self) -> &Vec<Option<String>> {
        self.glyph_names.get_or_init(|| self.read_glyph_names())
    }

    fn read_glyph_names(&self) -> Vec<Option<String>> {
        let post = self.table("post");
        let format = post.u32(0);
        let mut names: Vec<Option<String>> = vec![None; self.glyph_count.max(0) as usize];
        let standard = mac_glyphs();

        if format == 0x0001_0000 {
            for (g, slot) in names.iter_mut().enumerate().take(standard.len()) {
                *slot = Some(standard[g].to_string());
            }
            return names;
        }

        if format == 0x0002_0000 {
            let n = post.u16(32).min((post.len() - 34) / 2);
            let mut custom = Vec::new();
            let mut p = 34 + 2 * n;
            while p < post.len() {
                let len = post.u8(p);
                if p + 1 + len > post.len() {
                    break;
                }
                custom.push(lazy_pdf_syntax::latin1(post.slice(p + 1, len).0));
                p += 1 + len;
            }
            let mut g = 0;
            while g < n && g < names.len() as i64 {
                let idx = post.u16(34 + 2 * g);
                if idx < standard.len() as i64 {
                    names[g as usize] = Some(standard[idx as usize].to_string());
                } else if idx - 258 < custom.len() as i64 {
                    names[g as usize] = Some(custom[(idx - 258) as usize].clone());
                }
                g += 1;
            }
            return names;
        }

        if let Some(cff) = &self.cff {
            for (g, slot) in names.iter_mut().enumerate() {
                *slot = cff.glyph_name(g as i64);
            }
        }
        names
    }

    fn read_cmap_directory(&mut self) {
        let cmap = self.table("cmap");
        let count = cmap.u16(2).min((cmap.len() - 4) / 8);
        let mut list = Vec::new();
        let mut i = 0;
        while i < count {
            let offset = cmap.offset32(4 + 8 * i + 4);
            if offset > 0 && offset < cmap.len() - 4 {
                list.push(CmapSubtable {
                    platform: cmap.u16(4 + 8 * i),
                    encoding: cmap.u16(4 + 8 * i + 2),
                    offset,
                });
            }
            i += 1;
        }

        // Wide subtables first so code points above the BMP resolve.
        const UNICODE_ORDER: [(i64, i64); 8] = [
            (3, 10),
            (0, 6),
            (0, 4),
            (3, 1),
            (0, 3),
            (0, 2),
            (0, 1),
            (0, 0),
        ];
        let mut unicode = -1;
        for (p, e) in UNICODE_ORDER {
            let mut k = 0;
            while k < list.len() && unicode < 0 {
                if list[k].platform == p
                    && list[k].encoding == e
                    && is_lookup_format(cmap.u16(list[k].offset))
                {
                    unicode = k as i64;
                }
                k += 1;
            }
            if unicode >= 0 {
                break;
            }
        }
        self.symbol = list
            .iter()
            .position(|c| c.platform == 3 && c.encoding == 0)
            .map_or(-1, |i| i as i64);
        self.unicode = unicode;
        self.cmaps = list;
    }

    /// True when the font has a cmap subtable for the platform and encoding IDs.
    pub fn has_cmap(&self, platform_id: i64, encoding_id: i64) -> bool {
        self.cmaps
            .iter()
            .any(|c| c.platform == platform_id && c.encoding == encoding_id)
    }

    /// Glyph for a Unicode code point, 0 (.notdef) when the font has none. A font with only a (3,0) symbol
    /// subtable is asked for the code itself and for 0xF000 plus it, the way viewers do.
    pub fn glyph_for_code_point(&self, code_point: i64) -> i64 {
        let cmap = self.table("cmap");
        if self.unicode >= 0 {
            let g = lookup(cmap, self.cmaps[self.unicode as usize].offset, code_point);
            if g != 0 {
                return g;
            }
        }
        if self.unicode < 0 && self.symbol >= 0 && (0..256).contains(&code_point) {
            let offset = self.cmaps[self.symbol as usize].offset;
            let g = lookup(cmap, offset, code_point);
            return if g != 0 {
                g
            } else {
                lookup(cmap, offset, 0xF000 + code_point)
            };
        }
        0
    }

    /// Glyph for a code in one named cmap subtable, 0 when the subtable or the code is absent.
    pub fn glyph_for_cmap(&self, platform_id: i64, encoding_id: i64, code: i64) -> i64 {
        match self
            .cmaps
            .iter()
            .find(|c| c.platform == platform_id && c.encoding == encoding_id)
        {
            Some(c) => lookup(self.table("cmap"), c.offset, code),
            None => 0,
        }
    }

    /// Glyph for a base character followed by a variation selector, from the format 14 subtable (0,5). 0 when
    /// the font lists no such sequence; a default sequence gives the base character's glyph.
    pub fn glyph_for_variation(&self, code_point: i64, variation_selector: i64) -> i64 {
        let Some(c) = self
            .cmaps
            .iter()
            .find(|c| c.platform == 0 && c.encoding == 5)
        else {
            return 0;
        };
        let sub = self.table("cmap").from(c.offset);
        if sub.u16(0) != 14 {
            return 0;
        }

        let count = sub.offset32(6).min((sub.len() - 10) / 11);
        let mut lo = 0;
        let mut hi = count - 1;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let rec = 10 + 11 * mid;
            let vs = sub.u24(rec);
            if vs < variation_selector {
                lo = mid + 1;
            } else if vs > variation_selector {
                hi = mid - 1;
            } else {
                let non_default = sub.offset32(rec + 7);
                if non_default > 0 {
                    let n = sub
                        .offset32(non_default)
                        .min((sub.len() - non_default - 4) / 5);
                    let mut a = 0;
                    let mut b = n - 1;
                    while a <= b {
                        let m = (a + b) / 2;
                        let at = non_default + 4 + 5 * m;
                        let u = sub.u24(at);
                        if u < code_point {
                            a = m + 1;
                        } else if u > code_point {
                            b = m - 1;
                        } else {
                            return sub.u16(at + 3);
                        }
                    }
                }
                let def = sub.offset32(rec + 3);
                if def > 0 {
                    let n = sub.offset32(def).min((sub.len() - def - 4) / 4);
                    let mut a = 0;
                    let mut b = n - 1;
                    while a <= b {
                        let m = (a + b) / 2;
                        let at = def + 4 + 4 * m;
                        let start = sub.u24(at);
                        if code_point < start {
                            b = m - 1;
                        } else if code_point > start + sub.u8(at + 3) {
                            a = m + 1;
                        } else {
                            return self.glyph_for_code_point(code_point);
                        }
                    }
                }
                return 0;
            }
        }
        0
    }

    /// The glyph's data in 'glyf', empty for a blank glyph or one whose loca entry is out of range.
    pub(crate) fn glyph_data(&self, glyph_id: i64) -> Span<'_> {
        if glyph_id < 0 || glyph_id >= self.glyph_count {
            return Span(&[]);
        }
        let loca = self.table("loca");
        let glyf = self.table("glyf");
        let (start, mut end);
        if self.loca_format == 0 {
            start = loca.u16(2 * glyph_id) * 2;
            end = loca.u16(2 * glyph_id + 2) * 2;
        } else {
            start = loca.offset32(4 * glyph_id);
            end = loca.offset32(4 * glyph_id + 4);
        }
        end = end.min(glyf.len());
        if start < end {
            glyf.slice(start, end - start)
        } else {
            Span(&[])
        }
    }

    /// Box of the glyph in font units, none for an empty glyph. TrueType reads the glyph header, CFF measures the
    /// outline.
    pub fn bounds(&self, glyph_id: i64) -> Option<Bounds> {
        if let Some(cff) = &self.cff {
            return cff.outline(glyph_id)?.bounds();
        }
        let g = self.glyph_data(glyph_id);
        if g.len() < 10 {
            return None;
        }
        Some(Bounds {
            x_min: g.i16(2) as f64,
            y_min: g.i16(4) as f64,
            x_max: g.i16(6) as f64,
            y_max: g.i16(8) as f64,
        })
    }

    /// The outline of a glyph, composites resolved, in font units. Empty for a blank glyph, none when the glyph
    /// does not exist or the font has no outlines. Broken glyph data gives an empty outline.
    pub fn outline(&self, glyph_id: i64) -> Option<Outline> {
        if glyph_id < 0 || glyph_id >= self.glyph_count {
            return None;
        }
        if let Some(cff) = &self.cff {
            return cff.outline(glyph_id);
        }
        if !self.has_outlines() {
            return None;
        }

        let mut budget = MAX_COMPONENTS;
        let mut points = MAX_POINTS;
        let mut outline = Outline::default();
        if let Some(c) = self.load(glyph_id, 0, &mut budget, &mut points) {
            emit(&mut outline, &c);
        }
        Some(outline)
    }

    fn load(
        &self,
        glyph_id: i64,
        depth: i64,
        budget: &mut i64,
        points: &mut i64,
    ) -> Option<Contours> {
        let g = self.glyph_data(glyph_id);
        if g.len() < 10 {
            return Some(Contours::default());
        }
        let n = g.i16(0);
        if n < 0 {
            return Some(self.load_composite(g, depth, budget, points));
        }
        // A glyph that components repeat is counted each time: the points are built for each of them.
        if *points <= 0 {
            return None;
        }
        // The last end point gives the count before anything is allocated for it.
        if n > 0 && g.len() >= 10 + 2 * n && g.u16(10 + 2 * (n - 1)) + 1 > *points {
            return None;
        }
        let simple = load_simple(g, n)?;
        *points -= simple.points.len() as i64;
        if *points >= 0 { Some(simple) } else { None }
    }

    fn load_composite(&self, g: Span, depth: i64, budget: &mut i64, points: &mut i64) -> Contours {
        let mut result = Contours::default();
        if depth >= MAX_COMPOSITE_DEPTH {
            return result;
        }
        let mut p = 10;
        loop {
            if p > g.len() - 4 {
                return result;
            }
            *budget -= 1;
            if *budget < 0 {
                return result;
            }
            let flags = g.u16(p);
            let component = g.u16(p + 2);
            p += 4;

            let xy = (flags & 2) != 0;
            let (a1, a2);
            if (flags & 1) != 0 {
                a1 = if xy { g.i16(p) } else { g.u16(p) };
                a2 = if xy { g.i16(p + 2) } else { g.u16(p + 2) };
                p += 4;
            } else {
                a1 = if xy { g.u8(p) as i8 as i64 } else { g.u8(p) };
                a2 = if xy {
                    g.u8(p + 1) as i8 as i64
                } else {
                    g.u8(p + 1)
                };
                p += 2;
            }

            let (mut xx, mut xy01, mut yx10, mut yy) = (1.0, 0.0, 0.0, 1.0);
            if (flags & 0x8) != 0 {
                xx = f2dot14(g, p);
                yy = xx;
                p += 2;
            } else if (flags & 0x40) != 0 {
                xx = f2dot14(g, p);
                yy = f2dot14(g, p + 2);
                p += 4;
            } else if (flags & 0x80) != 0 {
                xx = f2dot14(g, p);
                xy01 = f2dot14(g, p + 2);
                yx10 = f2dot14(g, p + 4);
                yy = f2dot14(g, p + 6);
                p += 8;
            }

            if let Some(mut child) = self.load(component, depth + 1, budget, points) {
                for pt in &mut child.points {
                    *pt = Pt {
                        x: xx * pt.x + yx10 * pt.y,
                        y: xy01 * pt.x + yy * pt.y,
                        on: pt.on,
                    };
                }

                let (mut dx, mut dy) = (a1 as f64, a2 as f64);
                if !xy {
                    // Point matching: the child point a2 lands on the parent point a1.
                    if a1 < result.points.len() as i64 && a2 < child.points.len() as i64 {
                        dx = result.points[a1 as usize].x - child.points[a2 as usize].x;
                        dy = result.points[a1 as usize].y - child.points[a2 as usize].y;
                    } else {
                        dx = 0.0;
                        dy = 0.0;
                    }
                }

                let base = result.points.len() as i64;
                for pt in &child.points {
                    result.points.push(Pt {
                        x: pt.x + dx,
                        y: pt.y + dy,
                        on: pt.on,
                    });
                }
                for e in &child.ends {
                    result.ends.push(e + base);
                }
            }

            if (flags & 0x20) == 0 {
                return result;
            }
        }
    }
}

fn decode_big_endian_unicode(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    let mut text = String::from_utf16_lossy(&units);
    if raw.len() % 2 == 1 {
        text.push('\u{FFFD}');
    }
    text
}

fn is_lookup_format(format: i64) -> bool {
    matches!(format, 0 | 4 | 6 | 12 | 13)
}

fn lookup(cmap: Span, offset: i64, code: i64) -> i64 {
    if code < 0 {
        return 0;
    }
    let t = cmap.from(offset);
    match t.u16(0) {
        0 => {
            if code < 256 {
                t.u8(6 + code)
            } else {
                0
            }
        }
        4 => lookup_format4(t, code),
        6 => {
            let first = t.u16(6);
            let n = t.u16(8);
            if code >= first && code - first < n {
                t.u16(10 + 2 * (code - first))
            } else {
                0
            }
        }
        12 | 13 => {
            let n = t.offset32(12).min((t.len() - 16) / 12);
            let mut lo = 0;
            let mut hi = n - 1;
            while lo <= hi {
                let mid = (lo + hi) / 2;
                let g = 16 + 12 * mid;
                let start = i64::from(t.u32(g));
                if code < start {
                    hi = mid - 1;
                } else if code > i64::from(t.u32(g + 4)) {
                    lo = mid + 1;
                } else {
                    let mut gid = t.u32(g + 8);
                    // uint arithmetic: the sum wraps.
                    if t.u16(0) != 13 {
                        gid = gid.wrapping_add(code as u32).wrapping_sub(start as u32);
                    }
                    return if gid > 0xFFFF { 0 } else { i64::from(gid) };
                }
            }
            0
        }
        _ => 0,
    }
}

fn lookup_format4(t: Span, code: i64) -> i64 {
    if code > 0xFFFF {
        return 0;
    }
    let seg_count = (t.u16(6) / 2).min((t.len() - 16) / 8);
    let ends = 14;
    let starts = 16 + 2 * seg_count;
    let deltas = starts + 2 * seg_count;
    let ranges = deltas + 2 * seg_count;

    let mut lo = 0;
    let mut hi = seg_count - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if t.u16(ends + 2 * mid) < code {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= seg_count || t.u16(ends + 2 * lo) < code || t.u16(starts + 2 * lo) > code {
        return 0;
    }

    let delta = t.u16(deltas + 2 * lo);
    let range = t.u16(ranges + 2 * lo);
    if range == 0 {
        return (code + delta) & 0xFFFF;
    }
    let g = t.u16(ranges + 2 * lo + range + 2 * (code - t.u16(starts + 2 * lo)));
    if g == 0 { 0 } else { (g + delta) & 0xFFFF }
}

fn f2dot14(d: Span, o: i64) -> f64 {
    d.i16(o) as f64 / 16384.0
}

fn load_simple(g: Span, contour_count: i64) -> Option<Contours> {
    let mut result = Contours::default();
    if contour_count == 0 {
        return Some(result);
    }
    let mut p = 10;
    let mut last = -1;
    for _ in 0..contour_count {
        let end = g.u16(p);
        if end < last {
            return None;
        }
        result.ends.push(end);
        last = end;
        p += 2;
    }
    let count = (last + 1) as usize;
    p += 2 + g.u16(p);
    if p > g.len() {
        return None;
    }

    let mut flags = vec![0u8; count];
    let mut i = 0;
    while i < count {
        if p >= g.len() {
            return None;
        }
        let f = g.0[p as usize];
        p += 1;
        flags[i] = f;
        i += 1;
        if f & 8 == 0 {
            continue;
        }
        if p >= g.len() {
            return None;
        }
        let mut r = g.0[p as usize];
        p += 1;
        while r > 0 && i < count {
            flags[i] = f;
            i += 1;
            r -= 1;
        }
    }

    let mut xs = vec![0.0; count];
    let mut ys = vec![0.0; count];
    if !read_coords(g, &mut p, &flags, &mut xs, 0x02, 0x10)
        || !read_coords(g, &mut p, &flags, &mut ys, 0x04, 0x20)
    {
        return None;
    }

    result.points.reserve(count);
    for k in 0..count {
        result.points.push(Pt {
            x: xs[k],
            y: ys[k],
            on: flags[k] & 1 != 0,
        });
    }
    Some(result)
}

fn read_coords(
    g: Span,
    p: &mut i64,
    flags: &[u8],
    into: &mut [f64],
    short_bit: u8,
    same_bit: u8,
) -> bool {
    let mut v = 0i64;
    for (i, &f) in flags.iter().enumerate() {
        if f & short_bit != 0 {
            if *p >= g.len() {
                return false;
            }
            let d = i64::from(g.0[*p as usize]);
            *p += 1;
            v += if f & same_bit != 0 { d } else { -d };
        } else if f & same_bit == 0 {
            if *p > g.len() - 2 {
                return false;
            }
            v += g.i16(*p);
            *p += 2;
        }
        into[i] = v as f64;
    }
    true
}

fn emit(outline: &mut Outline, c: &Contours) {
    let mut first = 0i64;
    for &end in &c.ends {
        let last = end.min(c.points.len() as i64 - 1);
        if last - first >= 1 {
            emit_contour(outline, &c.points, first, last);
        }
        first = end + 1;
    }
}

fn emit_contour(o: &mut Outline, pts: &[Pt], s: i64, e: i64) {
    let (ps, pe) = (pts[s as usize], pts[e as usize]);
    let (start, from, to) = if ps.on {
        (ps, s + 1, e)
    } else if pe.on {
        (pe, s, e - 1)
    } else {
        (
            Pt {
                x: (ps.x + pe.x) / 2.0,
                y: (ps.y + pe.y) / 2.0,
                on: true,
            },
            s,
            e,
        )
    };

    o.move_to(start.x, start.y);
    let mut ctrl: Option<Pt> = None;
    let mut i = from;
    while i <= to {
        let p = pts[i as usize];
        i += 1;
        if p.on {
            match ctrl {
                Some(c) => o.quad_to(c.x, c.y, p.x, p.y),
                None => o.line_to(p.x, p.y),
            }
            ctrl = None;
        } else {
            if let Some(c) = ctrl {
                o.quad_to(c.x, c.y, (c.x + p.x) / 2.0, (c.y + p.y) / 2.0);
            }
            ctrl = Some(p);
        }
    }
    if let Some(last) = ctrl {
        o.quad_to(last.x, last.y, start.x, start.y);
    }
    o.close();
}
