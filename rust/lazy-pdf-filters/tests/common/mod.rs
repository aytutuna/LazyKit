#![allow(dead_code)]

use std::path::PathBuf;

use lazy_pdf_filters::{FilterError, Problem};

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pdf-filters")
}

pub fn read(name: &str) -> Vec<u8> {
    std::fs::read(data_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

pub fn read_text(name: &str) -> String {
    String::from_utf8(read(name)).unwrap().replace("\r\n", "\n")
}

/// The golden files write bytes as text: \n \r \t \\ and \xHH stand for the ones that do not belong in a text file.
pub fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes[i] {
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'x' => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Short results in hex, long ones as their length and an FNV-1a sum.
pub fn show(data: &[u8]) -> String {
    if data.len() <= 64 {
        return data.iter().map(|b| format!("{b:02X}")).collect();
    }
    let mut sum = 2166136261u32;
    for &b in data {
        sum = (sum ^ b as u32).wrapping_mul(16777619);
    }
    format!("length={} fnv={sum:08x}", data.len())
}

pub fn show_error(e: &FilterError) -> String {
    let problem = match e.problem {
        Problem::Damaged => "damaged",
        Problem::Unsupported => "unsupported",
    };
    format!("error {problem}: {}", e.message)
}

pub fn flips(spec: &str, data: &mut [u8]) {
    if spec == "-" {
        return;
    }
    for flip in spec.split(',') {
        let (at, xor) = flip.split_once(':').unwrap();
        data[at.parse::<usize>().unwrap()] ^= u8::from_str_radix(xor, 16).unwrap();
    }
}

/// Compares with the golden output case by case, so a failure names the case. `known` lists the
/// cases where the result is meant to differ: the start of the input line and what is expected instead.
pub fn check(
    input: &str,
    expected: &str,
    run: impl Fn(&str, &[&str]) -> String,
    known: &[(&str, &str)],
) {
    let mut want = expected.lines();
    let mut failures = Vec::new();
    for line in input.split('\n') {
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let words: Vec<&str> = line.split(' ').collect();
        let got = run(words[0], &words[1..]);
        let (case, result) = (want.next().unwrap_or(""), want.next().unwrap_or(""));
        assert_eq!(case, format!("> {line}"), "golden files out of step");
        let result = match known.iter().find(|(l, _)| line.starts_with(l)) {
            Some((_, ours)) => format!("< {ours}"),
            None => result.to_owned(),
        };
        if result != format!("< {got}") {
            failures.push(format!("{case}\n  want {result}\n  got  < {got}"));
        }
    }
    assert!(
        want.next().is_none(),
        "golden file has cases the input lacks"
    );
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
