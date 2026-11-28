// The write lines of the content golden: parsed operations written back.

mod common;

use common::golden;
use lazy_pdf_fonts::{ContentWriter, parse_content};

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

#[test]
fn written_operations_are_the_golden_file() {
    let mut actual = String::new();
    for line in golden("content/content-input.txt").split('\n') {
        let Some(rest) = line.strip_prefix("write ") else {
            continue;
        };
        actual += &format!("> {line}\n");
        let written = ContentWriter::write(&parse_content(&unescape(rest)));
        actual += &format!("< {}\n", escape(&written));
    }
    let mut expected = String::new();
    let mut inside = false;
    for line in golden("content/content-output.txt").split_inclusive('\n') {
        if line.starts_with("> ") {
            inside = line.starts_with("> write ");
        }
        if inside {
            expected += line;
        }
    }
    assert_eq!(actual.matches("> write ").count(), 6);
    assert_eq!(actual.split('\n').count(), expected.split('\n').count());
    for (i, (a, e)) in actual.split('\n').zip(expected.split('\n')).enumerate() {
        assert_eq!(a, e, "line {}", i + 1);
    }
}
