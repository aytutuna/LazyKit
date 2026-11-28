// The parse lines of the content golden: a content stream as operations.

mod common;

use common::golden;
use lazy_pdf_fonts::parse_content;

fn unescape(text: &str) -> Vec<u8> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        match b[i] {
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'x' => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

fn escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if !(0x20..0x7F).contains(&b) => out.push_str(&format!("\\x{b:02x}")),
            b => out.push(b as char),
        }
    }
    out
}

// {N*text} stands for text N times.
fn expand(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let close = chars[j.min(chars.len())..]
                .iter()
                .position(|&c| c == '}')
                .map(|p| p + j);
            if j > i + 1
                && j < chars.len()
                && chars[j] == '*'
                && let Some(close) = close
            {
                let count: usize = chars[i + 1..j]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                let unit: String = chars[j + 1..close].iter().collect();
                result += &unit.repeat(count);
                i = close + 1;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn parse_lines(rest: &str) -> Vec<String> {
    let bytes = unescape(&expand(rest));
    let mut lines = Vec::new();
    for op in parse_content(&bytes) {
        let operands: Vec<String> = op.operands.iter().map(ToString::to_string).collect();
        let mut line = format!("op {} [{},{}]", op.operator, op.start, op.end);
        if !operands.is_empty() {
            line += &format!(" {}", operands.join(" "));
        }
        lines.push(line);
        if let Some(image) = &op.image {
            lines.push(format!(
                "image {} {}",
                image.data.len(),
                escape(&image.data)
            ));
            lines.push(format!("dictionary {}", image.dictionary));
            lines.push(format!("expanded {}", image.expanded(None)));
        }
    }
    lines
}

#[test]
fn parse_lines_are_the_golden_file() {
    let mut actual = String::new();
    for line in golden("content/content-input.txt").split('\n') {
        let Some(rest) = line.strip_prefix("parse ") else {
            continue;
        };
        actual += &format!("> {line}\n");
        for result in parse_lines(rest) {
            actual += &format!("< {result}\n");
        }
    }
    let mut expected = String::new();
    let mut inside = false;
    for line in golden("content/content-output.txt").split_inclusive('\n') {
        if line.starts_with("> ") {
            inside = line.starts_with("> parse ");
        }
        if inside {
            expected += line;
        }
    }
    assert!(!expected.is_empty());
    assert_eq!(
        actual.split('\n').count(),
        expected.split('\n').count(),
        "line count"
    );
    for (i, (a, e)) in actual.split('\n').zip(expected.split('\n')).enumerate() {
        let cut = |s: &str| s.chars().take(500).collect::<String>();
        assert_eq!(cut(a), cut(e), "line {}", i + 1);
    }
}
