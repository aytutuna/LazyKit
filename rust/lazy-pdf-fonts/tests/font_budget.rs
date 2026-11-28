// A font dictionary that names one big array over and over must not cost time for every name.

mod common;

use std::time::Instant;

use common::pdf::TestPdf;
use lazy_pdf_fonts::{PdfFont, StandardFilters, TrueTypeFont};
use lazy_pdf_syntax::{Document, Object};

fn read_font(widths: &str) -> std::time::Duration {
    let refs = 20_000usize;
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    let big = "1 ".repeat(100_000);
    pdf.obj(3, &format!("[{big}]"));
    let entries = widths.repeat(refs);
    pdf.obj(
        4,
        &format!(
            "<</Type /Font /Subtype /CIDFontType2 /BaseFont /X /W [{entries}] /W2 [{entries}]>>"
        ),
    );
    pdf.obj(
        5,
        "<</Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [4 0 R]>>",
    );
    let mut doc = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let object = doc.get(5).unwrap();
    let Object::Dict(dict) = &*object else {
        panic!("no font dictionary")
    };
    let start = Instant::now();
    let _ = PdfFont::new(&mut doc, dict);
    start.elapsed()
}

#[test]
fn shared_width_array_past_the_last_cid() {
    assert!(read_font("70000 3 0 R ").as_secs() < 5);
}

#[test]
fn shared_width_array_in_range() {
    assert!(read_font("0 3 0 R ").as_secs() < 5);
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    be32(&mut out, 0x00010000);
    be16(&mut out, tables.len() as u16);
    be16(&mut out, 0);
    be16(&mut out, 0);
    be16(&mut out, 0);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0);
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out
}

#[test]
fn composite_glyphs_stop_at_the_point_budget() {
    let glyphs = 64u16;
    // glyph 1: one contour of 65535 points, flags repeated, no coordinate bytes
    let mut simple = Vec::new();
    be16(&mut simple, 1);
    for _ in 0..4 {
        be16(&mut simple, 0);
    }
    be16(&mut simple, 65534);
    be16(&mut simple, 0);
    let mut left = 65535u32;
    while left > 0 {
        let n = left.min(256);
        simple.push(0x01 | 0x08 | 0x10 | 0x20);
        simple.push((n - 1) as u8);
        left -= n;
    }
    while simple.len() % 4 != 0 {
        simple.push(0);
    }
    // glyph 2: 512 components of glyph 1
    let mut comp = Vec::new();
    comp.extend_from_slice(&(-1i16).to_be_bytes());
    for _ in 0..4 {
        be16(&mut comp, 0);
    }
    for i in 0..512 {
        be16(&mut comp, if i < 511 { 0x22 } else { 0x02 });
        be16(&mut comp, 1);
        comp.push(0);
        comp.push(0);
    }
    while comp.len() % 4 != 0 {
        comp.push(0);
    }
    // loca must be monotone per glyph: start(g)=loca[g], end=loca[g+1]; give each composite glyph its own copy
    let mut glyf2 = simple.clone();
    let mut loca2 = Vec::new();
    be32(&mut loca2, 0);
    be32(&mut loca2, 0);
    be32(&mut loca2, simple.len() as u32);
    for _ in 2..glyphs {
        glyf2.extend(&comp);
        be32(&mut loca2, glyf2.len() as u32);
    }
    let mut head = vec![0u8; 54];
    head[18] = 0x04; // upem 1024
    head[51] = 1; // long loca
    let mut maxp = vec![0u8; 6];
    maxp[0..4].copy_from_slice(&0x00005000u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&glyphs.to_be_bytes());
    let font = sfnt(&[
        (b"glyf", glyf2),
        (b"head", head),
        (b"loca", loca2),
        (b"maxp", maxp),
    ]);
    let f = TrueTypeFont::parse(&font, 0).unwrap();
    let t = Instant::now();
    let _ = f.outline(2);
    for g in 0..glyphs as i64 {
        let _ = f.outline(g);
    }
    assert!(t.elapsed().as_secs() < 10);
}
