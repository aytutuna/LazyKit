// The font lines of the content golden: a font dictionary decoding bytes. The other lines of the file belong to
// the content interpreter and are not run here.

mod common;

use common::pdf::TestPdf;
use common::{golden, golden_bytes, n};
use lazy_pdf_fonts::{PdfFont, StandardFilters};
use lazy_pdf_syntax::{Document, Object};

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

fn latin(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
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

fn flag(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

fn uni(text: Option<&str>) -> String {
    match text {
        None => "-".into(),
        Some(t) => t
            .chars()
            .map(|c| format!("{:04X}", c as u32))
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn font_lines(rest: &str) -> Vec<String> {
    let fields: Vec<&str> = rest.split(" ;; ").collect();
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        "<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<>>>>",
    );
    pdf.stream(4, "", b"");
    for (i, raw) in fields[1..].iter().enumerate() {
        let number = 5 + i as u32;
        let field = expand(raw);
        let Some(tail) = field.strip_prefix("stream") else {
            pdf.obj(number, &latin(&unescape(&field)));
            continue;
        };
        let Some((entries, data)) = tail.split_once(" :: ") else {
            continue;
        };
        let entries = latin(&unescape(entries));
        let content = match data.strip_prefix("file:") {
            Some(path) => golden_bytes(path),
            None => unescape(data),
        };
        pdf.stream(number, entries.trim_matches(' '), &content);
    }
    let mut doc = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let Some(object) = doc.get(5) else {
        return vec!["no font dictionary".into()];
    };
    let Object::Dict(dict) = &*object else {
        return vec!["no font dictionary".into()];
    };
    let font = PdfFont::new(&mut doc, dict);
    let m = font.font_matrix;
    let bbox = font.font_bbox.map_or("-".to_string(), |r| {
        format!(
            "[{} {} {} {}]",
            n(r.left),
            n(r.bottom),
            n(r.right),
            n(r.top)
        )
    });
    let mut lines = vec![format!(
        "font subtype={} base={} name={} composite={} type3={} vertical={} embedded={} tounicode={} flags={} bold={} italic={} fixed={} serif={} symbolic={} ascent={} descent={} cap={} matrix=[{} {} {} {} {} {}] bbox={}",
        font.subtype,
        font.base_font,
        font.name,
        flag(font.is_composite()),
        flag(font.is_type3()),
        flag(font.is_vertical),
        flag(font.is_embedded),
        flag(font.has_to_unicode()),
        font.flags,
        flag(font.is_bold),
        flag(font.is_italic),
        flag(font.is_fixed_pitch()),
        flag(font.is_serif()),
        flag(font.is_symbolic()),
        n(font.ascent),
        n(font.descent),
        n(font.cap_height),
        n(m[0]),
        n(m[1]),
        n(m[2]),
        n(m[3]),
        n(m[4]),
        n(m[5]),
        bbox
    )];
    let digits: Vec<u8> = fields[0]
        .chars()
        .filter_map(|c| c.to_digit(16).map(|d| d as u8))
        .collect();
    let input: Vec<u8> = digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| p[0] << 4 | p[1])
        .collect();
    for c in font.decode(&input) {
        lines.push(format!(
            "char code={} len={} cid={} w={} u={} space={} va={} ox={} oy={}",
            c.code,
            c.length,
            c.cid,
            n(c.width),
            uni(c.unicode.as_deref()),
            flag(c.is_space),
            n(c.vertical_advance),
            n(c.origin_x),
            n(c.origin_y)
        ));
    }
    lines.push(format!("text {}", uni(Some(&font.to_text(&input)))));
    lines
}

#[test]
fn fonts_decode_as_the_golden_file_says() {
    let mut actual = String::new();
    for line in golden("content/content-input.txt").split('\n') {
        let Some(rest) = line.strip_prefix("font ") else {
            continue;
        };
        actual += &format!("> {line}\n");
        for result in font_lines(rest) {
            actual += &format!("< {result}\n");
        }
    }

    // The same blocks of the golden output: a "> font" line and what follows up to the next "> " line.
    let mut expected = String::new();
    let mut inside = false;
    for line in golden("content/content-output.txt").split_inclusive('\n') {
        if line.starts_with("> ") {
            inside = line.starts_with("> font ");
        }
        if inside {
            expected += line;
        }
    }
    assert!(
        actual
            .matches(
                "
> font "
            )
            .count()
            + 1
            >= 46
    );
    assert_eq!(
        actual.split('\n').count(),
        expected.split('\n').count(),
        "line count"
    );
    for (i, (a, e)) in actual.split('\n').zip(expected.split('\n')).enumerate() {
        assert_eq!(a, e, "line {}", i + 1);
    }
}
