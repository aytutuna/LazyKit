mod common;

use common::{check, golden};
use lazy_pdf_fonts::write_to_unicode;

fn text(hex: &str) -> Vec<u16> {
    hex.as_bytes()
        .chunks(4)
        .map(|c| u16::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0))
        .collect()
}

fn unit(v: i64) -> u16 {
    v as u16
}

fn pairs(parts: &[&str]) -> Vec<(i64, Vec<u16>)> {
    parts[2..]
        .iter()
        .map(|part| {
            let (code, value) = part.split_once('=').unwrap();
            (
                i64::from(u32::from_str_radix(code, 16).unwrap_or(0) as i32),
                text(value),
            )
        })
        .collect()
}

fn many(count: i64) -> Vec<(i64, Vec<u16>)> {
    (0..count)
        .map(|i| (i * 2, vec![u16::from(b'x'), unit(0x4E00 + i * 3)]))
        .collect()
}

fn spans(count: i64) -> Vec<(i64, Vec<u16>)> {
    let mut map = Vec::new();
    for i in 0..count {
        map.push((i * 0x10, vec![unit(0x4E00 + i * 3)]));
        map.push((i * 0x10 + 1, vec![unit(0x4E00 + i * 3 + 1)]));
    }
    map
}

fn to_unicode(input: &str) -> String {
    let mut out = String::new();
    for line in input.split('\n').filter(|l| !l.is_empty()) {
        let parts: Vec<&str> = line.split(' ').collect();
        let code_bytes: u32 = parts[0].parse().unwrap_or(0);
        let map = match parts[1] {
            "pairs" => pairs(&parts),
            "many" => many(parts[2].parse().unwrap_or(0)),
            _ => spans(parts[2].parse().unwrap_or(0)),
        };
        out += &format!("== {line}\n");
        out += &String::from_utf8(write_to_unicode(&map, code_bytes).unwrap()).unwrap();
    }
    out
}

#[test]
fn to_unicode_cmap_is_the_golden_file() {
    check(
        "tables/tounicode-output.txt",
        &to_unicode(&golden("tables/tounicode-input.txt")),
    );
}
