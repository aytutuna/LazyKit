#![allow(dead_code)]

use std::path::PathBuf;

use lazy_pdf_fonts::Outline;
use lazy_pdf_syntax::Number;

/// The golden files of the reference engines, in testdata/pdf-fonts.
pub fn golden_bytes(path: &str) -> Vec<u8> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/pdf-fonts")
        .join(path);
    std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

pub fn golden(path: &str) -> String {
    String::from_utf8(golden_bytes(path)).unwrap()
}

/// Compares line by line first, so a failure names the line.
pub fn check(path: &str, actual: &str) {
    let expected = golden(path);
    let want: Vec<&str> = expected.split('\n').collect();
    let got: Vec<&str> = actual.split('\n').collect();
    for (i, (w, g)) in want.iter().zip(&got).enumerate() {
        if w != g {
            let cut = |s: &str| s.chars().take(600).collect::<String>();
            panic!(
                "{path} line {}\n  expected {}\n  actual   {}",
                i + 1,
                cut(w),
                cut(g)
            );
        }
    }
    assert_eq!(want.len(), got.len(), "{path}: line count");
    assert!(expected == actual, "{path}");
}

/// A number the way both reference engines write it.
pub fn n(v: f64) -> String {
    Number::from_f64(v).text().to_string()
}

pub fn b(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

pub fn s(v: Option<&str>) -> String {
    v.unwrap_or("null").to_string()
}

pub fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn outline(o: Option<&Outline>) -> String {
    use lazy_pdf_fonts::OutlineOp::*;
    let Some(o) = o else { return "null".into() };
    if o.is_empty() {
        return "empty".into();
    }
    o.segments
        .iter()
        .map(|s| match s.op {
            Move => format!("M {} {}", n(s.x), n(s.y)),
            Line => format!("L {} {}", n(s.x), n(s.y)),
            Quad => format!("Q {} {} {} {}", n(s.x1), n(s.y1), n(s.x), n(s.y)),
            Cubic => format!(
                "C {} {} {} {} {} {}",
                n(s.x1),
                n(s.y1),
                n(s.x2),
                n(s.y2),
                n(s.x),
                n(s.y)
            ),
            Close => "Z".into(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
