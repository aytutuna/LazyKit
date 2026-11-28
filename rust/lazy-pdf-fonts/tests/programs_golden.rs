mod common;

use common::cff_builder::{self, Part};
use common::{b, check, golden, n, outline, s};
use lazy_pdf_fonts::{
    Bounds, CffFont, SubsetStatus, TrueTypeFont, cff_strings, mac_glyphs, standard_encoding,
    standard_encoding_sids, subset,
};

fn standard_names_dump() -> String {
    let mut out = String::new();
    for (i, name) in cff_strings().iter().enumerate() {
        out += &format!("cff {i} {name}\n");
    }
    for (i, name) in mac_glyphs().iter().enumerate() {
        out += &format!("mac {i} {name}\n");
    }
    for c in 0..256 {
        out += &format!(
            "std {c} {} {}\n",
            s(standard_encoding(c)),
            standard_encoding_sids()[c]
        );
    }
    out
}

#[test]
fn standard_names_are_the_golden_file() {
    check("programs/standard-names-output.txt", &standard_names_dump());
}

fn bounds(v: Option<Bounds>) -> String {
    match v {
        None => "null".into(),
        Some(v) => format!(
            "{} {} {} {}",
            n(v.x_min),
            n(v.y_min),
            n(v.x_max),
            n(v.y_max)
        ),
    }
}

fn join<T>(items: impl IntoIterator<Item = T>, f: impl Fn(T) -> String) -> String {
    items.into_iter().map(f).collect::<Vec<_>>().join(",")
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap_or(0))
        .collect()
}

fn cff(input: &str) -> String {
    let mut out = String::new();
    let fonts = [
        ("name", cff_builder::name_keyed(None)),
        ("cid", cff_builder::cid_keyed(None, None)),
        (
            "cidMatrix",
            cff_builder::cid_keyed(Some(&[3, 0, 0, 3, 0, 0]), Some(&[2, 0, 0, 2, 0, 0])),
        ),
    ];
    for (label, data) in &fonts {
        let Some(f) = CffFont::parse(data) else {
            return "unreadable
"
            .into();
        };
        out += &format!(
            "font {label} name {} cid {} glyphs {} matrix {} bounds {}
",
            f.name,
            b(f.is_cid),
            f.glyph_count(),
            join(f.font_matrix, n),
            bounds(f.font_bounds)
        );
        for g in -1..=f.glyph_count() {
            out += &format!(
                "  glyph {g} name {} width {} cid {} {}
",
                s(f.glyph_name(g).as_deref()),
                n(f.width(g)),
                f.cid_for_glyph(g),
                outline(f.outline(g).as_ref())
            );
        }
        out += &format!(
            "  names {}
",
            join(["A", "B", "custom", ".notdef", "nothing"], |x| f
                .glyph_for_name(x)
                .to_string())
        );
        out += &format!(
            "  codes {}
",
            join(60..70, |c| f.glyph_for_code(c).to_string())
        );
        out += &format!(
            "  cids {}
",
            join([-1, 0, 1, 2, 3, 10, 11, 12, 500, 501], |c| f
                .glyph_for_cid(c)
                .to_string())
        );
    }

    let mut count = 0;
    for line in input
        .split('\n')
        .filter(|l| !l.is_empty() && !l.starts_with("# "))
    {
        let mut parts: Vec<Part> = line
            .split(' ')
            .map(|token| {
                if let Ok(v) = token.parse::<i32>() {
                    Part::Number(i64::from(v))
                } else if let Some(hex) = token.strip_prefix("raw:") {
                    Part::Raw(unhex(hex))
                } else {
                    cff_builder::o(token)
                }
            })
            .collect();
        parts.push(cff_builder::o("return"));
        let Some(f) = CffFont::parse(&cff_builder::name_keyed(Some(cff_builder::code(&parts))))
        else {
            return "unreadable
"
            .into();
        };
        count += 1;
        out += &format!(
            "subr {count} {line}
"
        );
        out += &format!(
            "  width {} {}
",
            n(f.width(2)),
            outline(f.outline(2).as_ref())
        );
    }
    out
}

#[test]
fn cff_is_the_golden_file() {
    check(
        "programs/cff-output.txt",
        &cff(&golden("programs/cff-input.txt")),
    );
}

fn true_type(data: &[u8]) -> String {
    let mut out = String::new();
    let Some(f) = TrueTypeFont::parse(data, 0) else {
        return "unreadable\n".into();
    };
    let mut l = |text: String| {
        out += &text;
        out.push('\n');
    };
    l(format!(
        "collection {}",
        TrueTypeFont::collection_count(data)
    ));
    l(format!(
        "upem {} glyphs {} ascent {} descent {} lineGap {}",
        f.units_per_em, f.glyph_count, f.ascent, f.descent, f.line_gap
    ));
    l(format!(
        "cap {} x {} bounds {} italic {} fixed {} bold {}",
        f.cap_height,
        f.x_height,
        bounds(Some(f.font_bounds)),
        n(f.italic_angle),
        b(f.is_fixed_pitch),
        b(f.is_bold_style)
    ));
    l(format!(
        "weight {} fsType {} embedding {} subsetting {} cff {} outlines {}",
        f.weight_class,
        f.fs_type,
        b(f.embedding_allowed()),
        b(f.subsetting_allowed()),
        b(f.is_cff()),
        b(f.has_outlines())
    ));
    l(format!(
        "family {} ps {}",
        s(f.family_name().as_deref()),
        s(f.post_script_name().as_deref())
    ));
    for t in [
        "glyf", "loca", "cmap", "post", "name", "cvt ", "fpgm", "prep", "CFF ", "kern",
    ] {
        l(format!("table {t} {}", b(f.has_table(t))));
    }
    for cp in [
        0x20, 0x41, 0x42, 0x43, 0xC0, 0x11F, 0x130, 0x131, 0x1F600, 0x4E2D, -1, 0x10FFFF,
    ] {
        l(format!(
            "cp {cp} {} cmap31 {} cmap10 {} cmap310 {} cmap00 {}",
            f.glyph_for_code_point(cp),
            f.glyph_for_cmap(3, 1, cp),
            f.glyph_for_cmap(1, 0, cp),
            f.glyph_for_cmap(3, 10, cp),
            f.glyph_for_cmap(0, 0, cp)
        ));
    }
    l(format!(
        "hasCmap 31 {} 10 {} 310 {} 05 {} 30 {}",
        b(f.has_cmap(3, 1)),
        b(f.has_cmap(1, 0)),
        b(f.has_cmap(3, 10)),
        b(f.has_cmap(0, 5)),
        b(f.has_cmap(3, 0))
    ));
    for (cp, vs) in [
        (0x41, 0xFE00),
        (0x42, 0xFE00),
        (0x43, 0xFE00),
        (0x11F, 0xFE00),
        (0x11F, 0xFE01),
    ] {
        l(format!(
            "variation {cp} {vs} {}",
            f.glyph_for_variation(cp, vs)
        ));
    }
    for g in -1..=f.glyph_count {
        l(format!(
            "glyph {g} name {} advance {} lsb {} box {}",
            s(f.glyph_name(g).as_deref()),
            f.advance(g),
            f.left_side_bearing(g),
            bounds(f.bounds(g))
        ));
        l(format!("  {}", outline(f.outline(g).as_ref())));
    }
    for name in [".notdef", "A", "Alt", "acutecomb", "space", "nothing"] {
        l(format!("forName {name} {}", f.glyph_for_name(name)));
    }
    out
}

#[test]
fn true_type_is_the_golden_file() {
    let font = common::golden_bytes("programs/synthetic.ttf");
    check("programs/truetype-output.txt", &true_type(&font));
}

fn hex_lines(out: &mut String, data: &[u8]) {
    let digits = common::hex(data);
    let mut i = 0;
    while i < digits.len() {
        *out += &format!("  {}\n", &digits[i..(i + 128).min(digits.len())]);
        i += 128;
    }
}

fn pairs(map: &std::collections::BTreeMap<i64, i64>) -> String {
    map.iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn table_offset(font: &[u8], tag: &str) -> usize {
    let count = (usize::from(font[4]) << 8) | usize::from(font[5]);
    for i in 0..count {
        let at = 12 + 16 * i;
        if &font[at..at + 4] == tag.as_bytes() {
            return u32::from_be_bytes([font[at + 8], font[at + 9], font[at + 10], font[at + 11]])
                as usize;
        }
    }
    panic!("{tag}");
}

fn with_fs_type(font: &[u8], fs_type: u16) -> Vec<u8> {
    let mut copy = font.to_vec();
    let os2 = table_offset(&copy, "OS/2");
    copy[os2 + 8..os2 + 10].copy_from_slice(&fs_type.to_be_bytes());
    copy
}

fn subset_dump(data: &[u8]) -> String {
    let mut out = String::new();
    let cases: Vec<(&str, Vec<u8>, Vec<i64>)> = vec![
        ("composite", data.to_vec(), vec![3]),
        ("two", data.to_vec(), vec![2, 5]),
        ("matched", data.to_vec(), vec![6]),
        ("none", data.to_vec(), vec![]),
        ("odd", data.to_vec(), vec![1, 99999, -4, 4]),
        ("forbidden", with_fs_type(data, 2), vec![1]),
        ("whole", with_fs_type(data, 0x100), vec![1]),
        ("garbage", vec![1, 2, 3], vec![1]),
    ];
    for (label, font, ids) in cases {
        let r = subset(&font, &ids, 0);
        out += &format!(
            "subset {label} {:?} map {} length {}\n",
            r.status,
            pairs(&r.glyph_map),
            r.data.len()
        );
        hex_lines(&mut out, &r.data);
        if r.status == SubsetStatus::Ok
            && let Some(again) = TrueTypeFont::parse(&r.data, 0)
        {
            out += &format!(
                "  parsed glyphs {} ps {} family {} upem {}\n",
                again.glyph_count,
                s(again.post_script_name().as_deref()),
                s(again.family_name().as_deref()),
                again.units_per_em
            );
            for to in r.glyph_map.values() {
                out += &format!(
                    "  glyph {to} advance {} box {} {}\n",
                    again.advance(*to),
                    bounds(again.bounds(*to)),
                    outline(again.outline(*to).as_ref())
                );
            }
        }
    }
    out
}

#[test]
fn subset_is_the_golden_file() {
    let font = common::golden_bytes("programs/synthetic.ttf");
    check("programs/subset-output.txt", &subset_dump(&font));
}

fn embedded_dump(data: &[u8]) -> String {
    let mut out = String::new();
    let cases: Vec<(&str, Vec<u8>, Vec<&str>)> = vec![
        ("latin", data.to_vec(), vec!["AB\u{C0} \u{11F}\u{130}"]),
        ("missing", data.to_vec(), vec!["A\u{4E2D}\u{1F600}B", "\t"]),
        ("shared", data.to_vec(), vec!["\u{131}\u{11F}"]),
        ("empty", data.to_vec(), vec![]),
        ("forbidden", with_fs_type(data, 2), vec!["A"]),
        ("whole", with_fs_type(data, 0x100), vec!["AB\u{C0}"]),
        ("garbage", vec![0; 200], vec!["A"]),
    ];
    for (label, font, texts) in cases {
        let (built, status) = lazy_pdf_fonts::build(&font, &texts, 0);
        out += &format!("embed {label} {status:?}\n");
        let Some(e) = built else { continue };
        out += &format!(
            "  base {} cff {} subset {} dw {} program {}\n",
            e.base_font,
            b(e.is_cff),
            b(e.is_subset),
            e.default_width,
            e.program.len()
        );
        hex_lines(&mut out, &e.program);
        let widths: Vec<String> = e
            .widths
            .iter()
            .map(|w| format!("{}[{}]", w.first_cid, join(&w.widths, |x| x.to_string())))
            .collect();
        out += &format!("  widths {}\n", widths.join(" "));
        let d = &e.descriptor;
        out += &format!(
            "  descriptor flags {} bbox {} italic {} ascent {} descent {} cap {} stemV {}\n",
            d.flags,
            join(&d.font_bbox, |x| x.to_string()),
            n(d.italic_angle),
            d.ascent,
            d.descent,
            d.cap_height,
            d.stem_v
        );
        out += &format!("  codes {}\n", pairs(&e.codes));
        out += &format!("  missing {}\n", join(&e.missing, |x| x.to_string()));
        out += &format!("  encode {}\n", common::hex(&e.encode(&texts.concat())));
        out += &format!("  toUnicode {}\n", pairs(&e.to_unicode()));
    }
    out
}

#[test]
fn embedded_font_is_the_golden_file() {
    let font = common::golden_bytes("programs/synthetic.ttf");
    check("programs/embedded-output.txt", &embedded_dump(&font));
}
