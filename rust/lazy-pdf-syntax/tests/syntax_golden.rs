mod common;

use common::{escape, golden, unescape};
use lazy_pdf_syntax::{Lexer, Name, Number, Object, PdfString, ReadError};
use sha2::{Digest, Sha256};

#[test]
fn golden_files_are_the_ones_that_were_copied() {
    let sums = golden("SHA256SUMS");
    assert!(!sums.is_empty());
    for line in sums.lines() {
        let (sum, name) = line.split_once(" *").unwrap();
        let file = golden(name);
        let hex: String = Sha256::digest(file.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, sum, "{name}");
    }
}

fn describe(value: &Object, lexer: &Lexer) -> String {
    let kind = match value {
        Object::Null => "PdfNull",
        Object::Bool(_) => "PdfBoolean",
        Object::Number(_) => "PdfNumber",
        Object::Name(_) => "PdfName",
        Object::String(_) => "PdfString",
        Object::Array(_) => "PdfArray",
        Object::Dict(_) => "PdfDictionary",
        Object::Ref(_) => "PdfReference",
        Object::Stream(_) => "PdfStream",
    };
    let written = match value {
        Object::Number(n) => format!(
            "{} integer={}",
            n.text(),
            n.integer().map_or("none".to_string(), |i| i.to_string())
        ),
        Object::Stream(s) => format!("{} data@{}", s.dict, s.data_offset.unwrap()),
        other => other.to_string(),
    };
    format!("{kind} {} pos={}", escape(written.as_bytes()), lexer.pos)
}

fn error_line(e: &ReadError) -> String {
    let problem = e.problem.to_string();
    let mut chars = problem.chars();
    let lower: String = chars.next().unwrap().to_lowercase().chain(chars).collect();
    let detail: Vec<u8> = e.detail.chars().map(|c| c as u8).collect();
    format!("error {lower}: {}", escape(&detail))
}

fn run(op: &str, data: &[u8]) -> String {
    let mut lexer = Lexer::new(data, 0);
    let result: Result<String, ReadError> = (|| match op {
        "object" => {
            let value = lexer.read_object(true)?;
            Ok(describe(&value, &lexer))
        }
        "value" => {
            let value = lexer.read_object(false)?;
            Ok(describe(&value, &lexer))
        }
        "header" => {
            let (number, generation) = lexer.read_object_header()?;
            Ok(format!("({number}, {generation}) pos={}", lexer.pos))
        }
        "keyword" => {
            let keyword = lexer.read_keyword();
            Ok(format!("\"{}\" pos={}", escape(keyword), lexer.pos))
        }
        "unsigned" => {
            let value = lexer.try_read_unsigned();
            Ok(format!(
                "{} pos={}",
                value.map_or("none".to_string(), |v| v.to_string()),
                lexer.pos
            ))
        }
        "string" => {
            let mut text = PdfString {
                bytes: data.to_vec(),
                hex: false,
            };
            let literal = Object::String(text.clone()).to_string();
            text.hex = true;
            Ok(format!("{literal} {}", Object::String(text)))
        }
        "name" => Ok(Object::Name(Name(data.to_vec())).to_string()),
        other => panic!("No op \"{other}\"."),
    })();
    match result {
        Ok(text) => text,
        Err(e) => error_line(&e),
    }
}

#[test]
fn lexer_output_is_the_golden_file() {
    let mut output = String::new();
    for line in golden("syntax/lexer-input.txt").split('\n') {
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let (op, rest) = line.split_once(' ').unwrap_or((line, ""));
        output.push_str(&format!("> {line}\n< {}\n", run(op, &unescape(rest))));
    }
    assert_eq!(output, golden("syntax/lexer-output.txt"));
}

#[test]
fn number_format_is_the_golden_file() {
    let mut output = String::new();
    for line in golden("syntax/number-format-input.txt").split('\n') {
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let value: f64 = line.parse().unwrap();
        output.push_str(&format!("{line} -> {}\n", Number::from_f64(value).text()));
    }
    assert_eq!(output, golden("syntax/number-format-output.txt"));
}
