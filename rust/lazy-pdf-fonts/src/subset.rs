//! Cuts a TrueType font down to some glyphs. Glyph IDs are renumbered densely in the order of the original IDs
//! (.notdef stays 0), so a PDF font made from the result uses the new ID as the character code and an identity
//! CIDToGIDMap. Composite glyphs pull in their components. Hinting is kept as is: the glyph programs are copied
//! verbatim, so cvt, fpgm and prep stay with them. The output has head, hhea, maxp, hmtx, loca, glyf, post (no
//! names), name, OS/2 and the hinting tables; no cmap, which a CIDFontType2 does not use.

use std::collections::{BTreeMap, BTreeSet};

use crate::bytes::{Span, put_u16, put_u32};
use crate::truetype::TrueTypeFont;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubsetStatus {
    Ok,
    Unreadable,
    /// CFF outlines, or no glyf and loca: nothing this can cut down.
    NotTrueType,
    /// OS/2 fsType restricts embedding.
    EmbeddingForbidden,
    /// OS/2 fsType says the whole font must be embedded.
    SubsettingForbidden,
}

/// `status` is Ok, or why no font was made; the other members are empty unless it is Ok. `data` is a TrueType
/// font for FontFile2. `glyph_map` is the original glyph ID to the glyph ID in `data`, for every glyph kept.
pub struct SubsetResult {
    pub status: SubsetStatus,
    pub data: Vec<u8>,
    pub glyph_map: BTreeMap<i64, i64>,
}

impl SubsetResult {
    fn failed(status: SubsetStatus) -> Self {
        SubsetResult {
            status,
            data: Vec::new(),
            glyph_map: BTreeMap::new(),
        }
    }
}

const MAX_COMPONENTS: usize = 512;

pub fn subset(font: &[u8], glyph_ids: &[i64], collection_index: i64) -> SubsetResult {
    let Some(f) = TrueTypeFont::parse(font, collection_index) else {
        return SubsetResult::failed(SubsetStatus::Unreadable);
    };
    if f.is_cff() || !f.has_table("glyf") || !f.has_table("loca") {
        return SubsetResult::failed(SubsetStatus::NotTrueType);
    }
    if !f.embedding_allowed() {
        return SubsetResult::failed(SubsetStatus::EmbeddingForbidden);
    }
    if !f.subsetting_allowed() {
        return SubsetResult::failed(SubsetStatus::SubsettingForbidden);
    }

    let kept = close(&f, glyph_ids);
    let mut map = BTreeMap::new();
    for &old in &kept {
        let next = map.len() as i64;
        map.insert(old, next);
    }

    let Some((glyf, loca, long_loca)) = build_glyphs(&f, &kept, &map) else {
        return SubsetResult::failed(SubsetStatus::Unreadable);
    };
    let mut tables: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    tables.insert("glyf".into(), glyf);
    tables.insert("loca".into(), loca);
    tables.insert("hmtx".into(), build_hmtx(&f, &kept));
    tables.insert("head".into(), build_head(&f, long_loca));
    tables.insert("hhea".into(), build_hhea(&f, kept.len() as i64));
    tables.insert("maxp".into(), build_maxp(&f, kept.len() as i64));
    tables.insert("post".into(), build_post(&f));
    for t in ["OS/2", "cvt ", "fpgm", "prep"] {
        if f.has_table(t) {
            tables.insert(t.into(), f.table(t).0.to_vec());
        }
    }
    if let Some(name) = build_name(&f) {
        tables.insert("name".into(), name);
    }

    SubsetResult {
        status: SubsetStatus::Ok,
        data: assemble(&tables),
        glyph_map: map,
    }
}

fn close(f: &TrueTypeFont, requested: &[i64]) -> Vec<i64> {
    let mut kept = BTreeSet::from([0i64]);
    let mut pending = Vec::new();
    for &g in requested {
        if g >= 0 && g < f.glyph_count && kept.insert(g) {
            pending.push(g);
        }
    }
    pending.push(0);

    while let Some(next) = pending.pop() {
        let data = f.glyph_data(next);
        for at in component_offsets(data) {
            let component = data.u16(at);
            if component < f.glyph_count && kept.insert(component) {
                pending.push(component);
            }
        }
    }
    kept.into_iter().collect()
}

/// Positions of the component glyph IDs inside a composite glyph, none for a simple one.
fn component_offsets(g: Span) -> Vec<i64> {
    let mut at = Vec::new();
    if g.len() < 10 || g.i16(0) >= 0 {
        return at;
    }
    let mut p = 10;
    while p <= g.len() - 4 && at.len() < MAX_COMPONENTS {
        let flags = g.u16(p);
        at.push(p + 2);
        p += 4 + if (flags & 1) != 0 { 4 } else { 2 };
        if (flags & 0x8) != 0 {
            p += 2;
        } else if (flags & 0x40) != 0 {
            p += 4;
        } else if (flags & 0x80) != 0 {
            p += 8;
        }
        if (flags & 0x20) == 0 {
            break;
        }
    }
    at
}

/// None when the glyphs add up to more than an i32 holds: loca offsets are 32 bits, and a loca whose glyphs
/// overlap can make a font of a few hundred KB ask for gigabytes.
fn build_glyphs(
    f: &TrueTypeFont,
    kept: &[i64],
    map: &BTreeMap<i64, i64>,
) -> Option<(Vec<u8>, Vec<u8>, bool)> {
    let mut total = 0i64;
    for &old in kept {
        total += (f.glyph_data(old).len() + 3) & !3;
        if total > i64::from(i32::MAX) {
            return None;
        }
    }

    let mut offsets = vec![0usize; kept.len() + 1];
    let mut glyf: Vec<u8> = Vec::new();
    let mut i = 0;
    for &old in kept {
        offsets[i] = glyf.len();
        i += 1;
        let mut data = f.glyph_data(old).0.to_vec();
        let copy = data.clone();
        let span = Span(&copy);
        for at in component_offsets(span) {
            let new = map.get(&span.u16(at)).copied().unwrap_or(0);
            put_u16(&mut data, at as usize, new);
        }
        glyf.extend(&data);
        while !glyf.len().is_multiple_of(4) {
            glyf.push(0);
        }
    }
    offsets[i] = glyf.len();

    let long_loca = glyf.len() >= 0x20000;
    let mut loca = vec![0u8; offsets.len() * if long_loca { 4 } else { 2 }];
    for (k, &offset) in offsets.iter().enumerate() {
        if long_loca {
            put_u32(&mut loca, 4 * k, offset as u32);
        } else {
            put_u16(&mut loca, 2 * k, (offset / 2) as i64);
        }
    }
    Some((glyf, loca, long_loca))
}

fn build_hmtx(f: &TrueTypeFont, kept: &[i64]) -> Vec<u8> {
    let mut hmtx = vec![0u8; kept.len() * 4];
    for (i, &old) in kept.iter().enumerate() {
        put_u16(&mut hmtx, 4 * i, f.advance(old));
        put_u16(&mut hmtx, 4 * i + 2, f.left_side_bearing(old) & 0xFFFF);
    }
    hmtx
}

fn build_head(f: &TrueTypeFont, long_loca: bool) -> Vec<u8> {
    let mut head = f.table("head").slice(0, 54).0.to_vec();
    put_u32(&mut head, 8, 0);
    put_u16(&mut head, 50, i64::from(long_loca));
    head
}

fn build_hhea(f: &TrueTypeFont, count: i64) -> Vec<u8> {
    let src = f.table("hhea");
    let mut hhea = vec![0u8; 36];
    let n = (src.len() as usize).min(36);
    hhea[..n].copy_from_slice(&src.0[..n]);
    put_u16(&mut hhea, 34, count);
    hhea
}

fn build_maxp(f: &TrueTypeFont, count: i64) -> Vec<u8> {
    let src = f.table("maxp");
    let mut maxp = src.slice(0, src.len().min(32)).0.to_vec();
    put_u16(&mut maxp, 4, count);
    maxp
}

fn build_post(f: &TrueTypeFont) -> Vec<u8> {
    let src = f.table("post");
    let mut post = vec![0u8; 32];
    let n = (src.len() as usize).min(32);
    post[..n].copy_from_slice(&src.0[..n]);
    put_u32(&mut post, 0, 0x0003_0000);
    // memory hints are meaningless without the names
    post[16..32].fill(0);
    post
}

fn build_name(f: &TrueTypeFont) -> Option<Vec<u8>> {
    let ps = f.post_script_name()?;
    let records = [
        (1, f.family_name().unwrap_or_else(|| ps.clone())),
        (4, ps.clone()),
        (6, ps),
    ];
    let mut storage: Vec<u8> = Vec::new();
    let mut name = vec![0u8; 6 + 12 * records.len()];
    put_u16(&mut name, 2, records.len() as i64);
    let header = name.len() as i64;
    put_u16(&mut name, 4, header);
    for (i, (id, text)) in records.iter().enumerate() {
        let mut bytes = Vec::new();
        for unit in text.encode_utf16() {
            bytes.push((unit >> 8) as u8);
            bytes.push(unit as u8);
        }
        let r = 6 + 12 * i;
        put_u16(&mut name, r, 3);
        put_u16(&mut name, r + 2, 1);
        put_u16(&mut name, r + 4, 0x409);
        put_u16(&mut name, r + 6, *id);
        put_u16(&mut name, r + 8, bytes.len() as i64);
        put_u16(&mut name, r + 10, storage.len() as i64);
        storage.extend(bytes);
    }
    name.extend(storage);
    Some(name)
}

fn assemble(tables: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    // BTreeMap keys come in ordinal order, which is the order of the tags (ASCII).
    let count = tables.len();
    let entry_selector = usize::BITS as usize - 1 - count.leading_zeros() as usize;
    let search_range = (1usize << entry_selector) * 16;

    let mut size = 12 + 16 * count;
    for data in tables.values() {
        size += (data.len() + 3) & !3;
    }
    let mut file = vec![0u8; size];
    put_u32(&mut file, 0, 0x0001_0000);
    put_u16(&mut file, 4, count as i64);
    put_u16(&mut file, 6, search_range as i64);
    put_u16(&mut file, 8, entry_selector as i64);
    put_u16(&mut file, 10, (count * 16 - search_range) as i64);

    let mut dir = 12;
    let mut offset = 12 + 16 * count;
    let mut head_at = 0;
    for (tag, data) in tables {
        file[dir..dir + 4].copy_from_slice(&tag.as_bytes()[..4]);
        put_u32(&mut file, dir + 4, Span(data).checksum());
        put_u32(&mut file, dir + 8, offset as u32);
        put_u32(&mut file, dir + 12, data.len() as u32);
        file[offset..offset + data.len()].copy_from_slice(data);
        if tag == "head" {
            head_at = offset;
        }
        offset += (data.len() + 3) & !3;
        dir += 16;
    }
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(Span(&file).checksum());
    put_u32(&mut file, head_at + 8, adjust);
    file
}
