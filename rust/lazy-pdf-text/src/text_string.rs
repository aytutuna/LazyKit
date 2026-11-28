//! Text strings (ISO 32000-1 7.9.2.2): PDFDocEncoding, UTF-16BE or LE with a byte order mark, and UTF-8 with one.

use lazy_pdf_fonts::{pdf_doc, unicode_of_glyph_name};

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    let mut units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            if big_endian {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        })
        .collect();
    // An odd last byte reads as U+FFFD, as Encoding.BigEndianUnicode does.
    if bytes.len() % 2 == 1 {
        units.push(0xFFFD);
    }
    String::from_utf16_lossy(&units)
}

// A language escape (7.9.2.2) and what it holds is dropped.
fn strip_language(text: &str) -> String {
    let mut result = String::new();
    let mut inside = false;
    for c in text.chars() {
        if c == '\u{1B}' {
            inside = !inside;
        } else if !inside {
            result.push(c);
        }
    }
    result
}

fn doc_char(code: u8) -> String {
    // Codes the encoding leaves out read as Latin-1, which is what most writers meant.
    pdf_doc()[usize::from(code)]
        .and_then(unicode_of_glyph_name)
        .filter(|t| t.encode_utf16().count() == 1)
        .unwrap_or_else(|| char::from(code).to_string())
}

pub fn decode(bytes: &[u8]) -> String {
    let text = if bytes.starts_with(&[0xFE, 0xFF]) {
        utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        utf16(&bytes[2..], false)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        return bytes.iter().map(|&b| doc_char(b)).collect();
    };
    strip_language(&text)
}
