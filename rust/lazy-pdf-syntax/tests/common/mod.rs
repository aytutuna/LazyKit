#![allow(dead_code)]

use std::path::PathBuf;

use lazy_pdf_syntax::{DecodeError, Filters, Parm};

/// The golden files of the reference engines, in testdata/pdf.
pub fn golden(path: &str) -> String {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/pdf")
        .join(path);
    String::from_utf8(std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())))
        .unwrap()
}

/// The bytes a golden input line stands for: \n \r \t \\ and \xHH.
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
            other => out.push(other),
        }
        i += 1;
    }
    out
}

pub fn escape(bytes: &[u8]) -> String {
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

/// A text with every char standing for one byte, as the reference tests spell their files.
pub fn latin(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u8).collect()
}

/// Flate through miniz_oxide, and no other filter.
pub struct TestFilters;

impl Filters for TestFilters {
    fn decode(
        &self,
        filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        _strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        match filter {
            b"FlateDecode" | b"Fl" => {
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, max_size)
                    .map_err(|e| DecodeError::Damaged(format!("{e:?}")))
            }
            other => Err(DecodeError::Unsupported(format!(
                "The /{} filter is not supported.",
                String::from_utf8_lossy(other)
            ))),
        }
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }
}

pub mod fixtures;
pub mod pdf;

pub fn open(file: Vec<u8>) -> lazy_pdf_syntax::Document {
    lazy_pdf_syntax::Document::open(file, Box::new(TestFilters)).unwrap()
}
