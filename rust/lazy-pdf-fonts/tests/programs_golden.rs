mod common;

use common::cff_builder::{self, Part};
use common::{b, check, golden, n, outline, s};
use lazy_pdf_fonts::{
    Bounds, CffFont, cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids,
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
