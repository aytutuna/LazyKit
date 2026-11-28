// The text golden of the reference engines: one-page files turned into text pages and dumped, searches, and the tables
// of UTF-16 classification and folding that the search is built on, all as text. The fixture lines that open encrypted
// files wait for the crypt layer; they are skipped and counted.

#[path = "../../lazy-pdf-fonts/tests/common/pdf.rs"]
mod pdf;

use std::path::PathBuf;

use lazy_pdf_fonts::StandardFilters;
use lazy_pdf_syntax::{Document, Number, Rect};
use lazy_pdf_text::{
    TextBreak, TextPage, TextPageOptions, TextQuad, TextRange, TextSearch, TextSearchOptions,
    TextSelection, bidi_class_of, clean, collapse, fold, strip_marks,
};
use pdf::TestPdf;

fn golden(path: &str) -> String {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/pdf-fonts")
        .join(path);
    String::from_utf8(std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())))
        .unwrap()
}

fn n(v: f64) -> String {
    Number::from_f64(v).text().to_string()
}

fn r(r: &Rect) -> String {
    format!(
        "[{} {} {} {}]",
        n(r.left),
        n(r.bottom),
        n(r.right),
        n(r.top)
    )
}

fn q(q: &TextQuad) -> String {
    format!(
        "[{} {} {} {} {} {} {} {}]",
        n(q.x0),
        n(q.y0),
        n(q.x1),
        n(q.y1),
        n(q.x2),
        n(q.y2),
        n(q.x3),
        n(q.y3)
    )
}

fn b(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

// UTF-16 units in hex, so that no character is lost to the file's encoding.
fn u(units: &[u16]) -> String {
    units
        .iter()
        .map(|c| format!("{c:04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn esc(text: &str) -> String {
    let mut sb = String::new();
    for c in text.encode_utf16() {
        match c {
            0x5C => sb.push_str("\\\\"),
            0x0A => sb.push_str("\\n"),
            0x09 => sb.push_str("\\t"),
            0x0C => sb.push_str("\\f"),
            c if !(0x20..0x7F).contains(&c) => sb.push_str(&format!("\\u{c:04X}")),
            c => sb.push(c as u8 as char),
        }
    }
    sb
}

fn unescape(text: &str) -> Vec<u8> {
    let s: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] != '\\' {
            out.push(s[i] as u32 as u8);
            i += 1;
            continue;
        }
        i += 1;
        if i >= s.len() {
            break;
        }
        match s[i] {
            'n' => out.push(10),
            'r' => out.push(13),
            't' => out.push(9),
            'x' => {
                let digits: String = s[i + 1..(i + 3).min(s.len())].iter().collect();
                out.push(u8::from_str_radix(&digits, 16).unwrap_or(0));
                i += 2;
            }
            c => out.push(c as u32 as u8),
        }
        i += 1;
    }
    out
}

// \uXXXX, \xHH, \n, \t and \\ in a string of the input.
fn str_of(text: &str) -> String {
    let c = units(text);
    let mut sb: Vec<u16> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] != 0x5C || i + 1 >= c.len() {
            sb.push(c[i]);
            i += 1;
            continue;
        }
        i += 1;
        match c[i] {
            0x6E => sb.push(0x0A),
            0x74 => sb.push(0x09),
            0x75 => {
                let digits = String::from_utf16_lossy(&c[i + 1..(i + 5).min(c.len())]);
                sb.push(u16::from_str_radix(&digits, 16).unwrap_or(0));
                i += 4;
            }
            0x78 => {
                let digits = String::from_utf16_lossy(&c[i + 1..(i + 3).min(c.len())]);
                sb.push(u16::from_str_radix(&digits, 16).unwrap_or(0));
                i += 2;
            }
            other => sb.push(other),
        }
        i += 1;
    }
    String::from_utf16_lossy(&sb)
}

fn line_content(x: &str, y: &str, text: &str, size: &str) -> String {
    format!("BT /F1 {size} Tf {x} {y} Td ({text}) Tj ET\\n")
}

// {line X Y text}, {size S X Y text} and {col X TOP prefix N} stand for the content of a line of text and of a column of
// lines 12 points apart.
fn expand(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut sb = String::new();
    let mut i = 0;
    while i < chars.len() {
        let close = if chars[i] == '{' {
            chars[i..].iter().position(|&c| c == '}').map(|p| p + i)
        } else {
            None
        };
        let Some(close) = close else {
            sb.push(chars[i]);
            i += 1;
            continue;
        };
        let body: String = chars[i + 1..close].iter().collect();
        let w: Vec<&str> = body.split(' ').collect();
        let made = if w[0] == "line" && w.len() >= 4 {
            Some(line_content(w[1], w[2], &w[3..].join(" "), "10"))
        } else if w[0] == "size" && w.len() >= 5 {
            Some(line_content(w[2], w[3], &w[4..].join(" "), w[1]))
        } else if w[0] == "col" && w.len() == 5 {
            match (w[2].parse::<i64>(), w[4].parse::<i64>()) {
                (Ok(top), Ok(count)) => Some(
                    (0..count.max(0))
                        .map(|k| {
                            line_content(
                                w[1],
                                &(top - k * 12).to_string(),
                                &format!("{} line {} of the column", w[3], k + 1),
                                "10",
                            )
                        })
                        .collect(),
                ),
                _ => None,
            }
        } else {
            None
        };
        match made {
            Some(made) => {
                sb.push_str(&made);
                i = close + 1;
            }
            None => {
                sb.push(chars[i]);
                i += 1;
            }
        }
    }
    sb
}

// code=hex,code=hex: the text of a code as UTF-16 units in hex, four digits each.
fn font_map(text: &str) -> Vec<(u32, Vec<u16>)> {
    if text == "-" || text.is_empty() {
        return Vec::new();
    }
    text.split(',')
        .map(|entry| {
            let mut parts = entry.split('=');
            let code = parts.next().unwrap().parse().unwrap_or(0);
            let digits: Vec<char> = parts.next().unwrap_or("").chars().collect();
            let units = digits
                .as_chunks::<4>()
                .0
                .iter()
                .map(|d| u16::from_str_radix(&d.iter().collect::<String>(), 16).unwrap_or(0))
                .collect();
            (code, units)
        })
        .collect()
}

// A Type 1 font that is half an em wide everywhere, so glyph positions are easy to work out.
fn font(pdf: &mut TestPdf, number: u32, map: &[(u32, Vec<u16>)]) {
    let widths = vec!["500"; 256].join(" ");
    pdf.obj(
        number,
        &format!(
            "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 /Widths [{widths}] /ToUnicode {} 0 R>>",
            number + 1
        ),
    );
    let mut all: Vec<(u32, Vec<u16>)> = map.to_vec();
    for c in 32u32..127 {
        if !all.iter().any(|(code, _)| *code == c) {
            all.push((c, vec![c as u16]));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <00> <FF> endcodespacerange ",
    );
    cmap += &format!("{} beginbfchar ", all.len());
    for (code, text) in &all {
        let hex: String = text.iter().map(|c| format!("{c:04X}")).collect();
        cmap += &format!("<{code:02X}> <{hex}> ");
    }
    cmap += "endbfchar endcmap";
    pdf.stream(number + 1, "", cmap.as_bytes());
}

// page <pageEntries> ;; <resources> ;; <content> ;; <object>...  where an object is "font N [code=hex,...]",
// "obj N <text>" or "stream N <entries> :: <bytes>". "-" stands for nothing, and for the resources of /F1 as object 5.
fn build(fields: &[&str]) -> Document {
    let page_entries = if fields[0] == "-" { "" } else { fields[0] };
    let resources = if fields[1] == "-" {
        "/Font <</F1 5 0 R>>"
    } else {
        fields[1]
    };
    let content = unescape(&expand(fields[2]));
    let objects = &fields[3..];
    let mut pdf = TestPdf::new();
    let catalog: String = objects
        .iter()
        .filter_map(|o| o.strip_prefix("catalog "))
        .map(|c| format!("{c} "))
        .collect();
    pdf.obj(1, &format!("<</Type /Catalog /Pages 2 0 R {catalog}>>"));
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        &format!(
            "<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<{resources}>> {page_entries}>>"
        ),
    );
    pdf.stream(4, "", &content);
    for field in objects.iter().filter(|o| !o.starts_with("catalog ")) {
        let w: Vec<&str> = field.splitn(3, ' ').collect();
        let number: u32 = w[1].parse().unwrap_or(0);
        match w[0] {
            "font" => font(
                &mut pdf,
                number,
                &font_map(w.get(2).copied().unwrap_or("-")),
            ),
            "obj" => {
                pdf.obj(number, &expand(w[2]));
            }
            "stream" => {
                if let Some((entries, data)) = w[2].split_once(" :: ") {
                    let entries = if entries == "-" { "" } else { entries };
                    pdf.stream(number, entries, &unescape(&expand(data)));
                }
            }
            _ => {}
        }
    }
    Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap()
}

fn page_op(fields: &[&str], options: &TextPageOptions) -> Vec<String> {
    let mut doc = build(fields);
    let pages = doc.pages();
    dump_page(&TextPage::create(&mut doc, &pages[0], options))
}

fn page_options(rest: &str) -> Vec<String> {
    let Some((flags, spec)) = rest.split_once(" ;; ") else {
        return Vec::new();
    };
    let options = TextPageOptions {
        include_artifacts: !flags.contains('A'),
        remove_duplicates: !flags.contains('D'),
        include_annotations: flags.contains('N'),
    };
    let fields: Vec<&str> = spec.split(" ;; ").collect();
    page_op(&fields, &options)
}

fn break_name(before: TextBreak) -> &'static str {
    match before {
        TextBreak::None => "None",
        TextBreak::Tab => "Tab",
        TextBreak::Line => "Line",
        TextBreak::Block => "Block",
    }
}

fn dump_page(page: &TextPage) -> Vec<String> {
    let mut lines = vec![format!(
        "page index={} box={} truncated={} annotations={} chars={} words={} lines={} blocks={}",
        page.page_index,
        r(&page.bounds),
        b(page.is_truncated),
        b(page.includes_annotations),
        page.chars.len(),
        page.words.len(),
        page.lines.len(),
        page.blocks.len()
    )];
    for c in &page.chars {
        lines.push(format!(
            "char {} u={} q={} font={} size={} rgb={},{},{} mode={} op={} form={} el={} off={} mcid={} art={} space={} implied={} rtl={} word={} line={}",
            c.index,
            u(&units(&c.text)),
            q(&c.quad),
            c.font.as_ref().map_or("-", |f| f.name.as_str()),
            n(c.size),
            c.color.r,
            c.color.g,
            c.color.b,
            c.render_mode,
            c.operator_index,
            b(c.form.is_some()),
            c.element,
            c.offset,
            c.mcid.map_or("-".to_string(), |m| m.to_string()),
            b(c.is_artifact),
            b(c.is_space),
            b(c.is_implied),
            b(c.is_rtl),
            c.word.map_or(-1, |w| w as i64),
            c.line
        ));
    }
    for w in &page.words {
        lines.push(format!(
            "word {} [{},{}) u={} b={} line={}",
            w.index,
            w.range.start,
            w.range.end,
            u(&units(&w.text)),
            r(&w.bounds),
            w.line
        ));
    }
    for l in &page.lines {
        lines.push(format!(
            "line {} [{},{}) u={} b={} q={} block={} before={} vertical={} rtl={}",
            l.index,
            l.range.start,
            l.range.end,
            u(&units(&l.text)),
            r(&l.bounds),
            q(&l.quad),
            l.block,
            break_name(l.before),
            b(l.is_vertical),
            b(l.is_rtl)
        ));
    }
    for k in &page.blocks {
        lines.push(format!(
            "block {} lines=[{},{}) b={} table={}",
            k.index,
            k.lines.start,
            k.lines.end,
            r(&k.bounds),
            b(k.is_table)
        ));
    }
    let count = page.chars.len();
    lines.push(format!("text {}", esc(page.text())));
    lines.push(format!(
        "joined {}",
        esc(&page.get_text(TextRange::new(0, count), true))
    ));
    lines.push(format!(
        "quads {}",
        page.quads(TextRange::new(0, count))
            .iter()
            .map(q)
            .collect::<Vec<_>>()
            .join(";")
    ));
    if count == 0 {
        return lines;
    }
    let mut seen = Vec::new();
    for i in [0, count / 3, count / 2, 2 * count / 3, count - 1] {
        if seen.contains(&i) {
            continue;
        }
        seen.push(i);
        let center = page.chars[i].quad.center();
        for (x, y) in [(center.0, center.1), (center.0 + 3.0, center.1 - 2.0)] {
            let hit = page.hit_test(x, y, f64::INFINITY);
            let shown = hit.map_or("-".to_string(), |h| {
                format!(
                    "{}/{}/{}/{}",
                    h.char,
                    h.word.map_or(-1, |w| w as i64),
                    h.line,
                    n(h.distance)
                )
            });
            lines.push(format!(
                "probe {i} p={},{} hit={shown} caret={} word={} line={}",
                n(x),
                n(y),
                page.caret_at(x, y),
                esc(&TextSelection::word_at(page, x, y).get_text(false)),
                esc(&TextSelection::line_at(page, x, y).get_text(false))
            ));
        }
    }
    let first = page.chars[0].quad.center();
    let last = page.chars[count - 1].quad.center();
    lines.push(format!(
        "between {}",
        esc(&TextSelection::between(page, first.0, first.1, last.0, last.1).get_text(false))
    ));
    let bx = page.bounds;
    let half = Rect {
        left: bx.left,
        bottom: bx.bottom + bx.height() / 2.0,
        right: bx.right,
        top: bx.top,
    };
    lines.push(format!(
        "rectangle {}",
        esc(&TextSelection::in_rectangle(page, &half).get_text(false))
    ));
    lines
}

fn fixture(rest: &str) -> Vec<String> {
    let words: Vec<&str> = rest.split(' ').collect();
    let name = words[0].strip_prefix("golden/text/").unwrap();
    let bytes = golden(&format!("text/{name}")).into_bytes();
    let mut doc = Document::open(bytes, Box::new(StandardFilters)).unwrap();
    let pages = doc.pages();
    let mut lines = Vec::new();
    for page in pages.iter() {
        lines.push(format!("page {}", page.index));
        lines.extend(dump_page(&TextPage::create(
            &mut doc,
            page,
            &TextPageOptions::default(),
        )));
    }
    lines
}

// search <flags> ;; <query> ;; <page spec as in "page">. Flags are c (match case), w (whole word), r (regex) and f
// (fold diacritics), or "-".
fn search(rest: &str) -> Vec<String> {
    let fields: Vec<&str> = rest.split(" ;; ").collect();
    let options = TextSearchOptions {
        match_case: fields[0].contains('c'),
        whole_word: fields[0].contains('w'),
        regex: fields[0].contains('r'),
        fold_diacritics: fields[0].contains('f'),
    };
    let mut doc = build(&fields[2..]);
    let pages = doc.pages();
    let page = TextPage::create(&mut doc, &pages[0], &TextPageOptions::default());
    let mut lines = Vec::new();
    match TextSearch::find(&page, &str_of(fields[1]), &options) {
        Ok(found) => {
            for m in found {
                lines.push(format!(
                    "match [{},{}) page={} text={} quads={}",
                    m.range.start,
                    m.range.end,
                    m.page,
                    esc(&m.text),
                    m.quads.iter().map(q).collect::<Vec<_>>().join(";")
                ));
            }
        }
        Err(_) => lines.push("bad pattern".to_string()),
    }
    lines.push(format!("count {}", lines.len()));
    lines
}

fn class_name(text: &str) -> &'static str {
    use lazy_pdf_text::BidiClass::*;
    match bidi_class_of(text) {
        L => "L",
        R => "R",
        En => "EN",
        An => "AN",
        N => "N",
    }
}

// text <flags> <string>: what the search does to one string.
fn strings(rest: &str) -> Vec<String> {
    let Some((flags, text)) = rest.split_once(' ') else {
        return Vec::new();
    };
    let text = str_of(text);
    let options = TextSearchOptions {
        match_case: flags.contains('c'),
        fold_diacritics: flags.contains('f'),
        ..TextSearchOptions::default()
    };
    let all = units(&text);
    vec![
        format!("fold {}", u(&fold(&all, &options))),
        format!("strip {}", u(&strip_marks(&all))),
        format!("clean {}", u(&units(&clean(&text)))),
        format!("collapse {}", u(&collapse(&all))),
        format!("class {}", class_name(&text)),
    ]
}

// Runtimes carry different versions of the Unicode data, and the blocks that got letters or case pairs in versions 16
// and 17 differ between them; these have not changed for years.
const STABLE_BLOCKS: [u32; 17] = [
    0x00, 0x01, 0x03, 0x04, 0x05, 0x06, 0x07, 0x1E, 0x1F, 0x20, 0x21, 0x24, 0xFB, 0xFC, 0xFD, 0xFE,
    0xFF,
];

fn ranges(block: u32, test: impl Fn(u16) -> bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut start: Option<u32> = None;
    for i in 0..=256 {
        let unit = block * 256 + i;
        let on = i < 256 && test(unit as u16);
        match (on, start) {
            (true, None) => start = Some(unit),
            (false, Some(from)) => {
                parts.push(if from == unit - 1 {
                    format!("{from:04X}")
                } else {
                    format!("{from:04X}-{:04X}", unit - 1)
                });
                start = None;
            }
            _ => {}
        }
    }
    parts.join(" ")
}

fn changes(block: u32, map: impl Fn(u16) -> Vec<u16>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for i in 0..256 {
        let unit = block * 256 + i;
        if (0xD800..=0xDFFF).contains(&unit) {
            continue;
        }
        let result = map(unit as u16);
        if result != [unit as u16] {
            parts.push(format!("{unit:04X}>{}", u(&result).replace(' ', "+")));
        }
    }
    parts.join(" ")
}

fn blocks(kind: &str) -> Vec<String> {
    use lazy_pdf_text::{is_letter, is_lower, is_non_spacing_mark, is_white_space};
    let plain = TextSearchOptions::default();
    let folding = TextSearchOptions {
        fold_diacritics: true,
        ..TextSearchOptions::default()
    };
    let mut lines = Vec::new();
    for block in STABLE_BLOCKS {
        let line = match kind {
            "letter" => ranges(block, is_letter),
            "lower" => ranges(block, is_lower),
            "white" => ranges(block, is_white_space),
            "mark" => ranges(block, is_non_spacing_mark),
            "tolower" => changes(block, |c| fold(&[c], &plain)),
            "toloweri" => changes(block, |c| fold(&[c], &folding)),
            "strip" => changes(block, |c| strip_marks(&[c])),
            "clean" => changes(block, |c| units(&clean(&String::from_utf16_lossy(&[c])))),
            _ => String::new(),
        };
        if !line.is_empty() {
            lines.push(format!("{block:02X} {line}"));
        }
    }
    lines
}

fn run(op: &str, rest: &str) -> Vec<String> {
    let fields = || rest.split(" ;; ").collect::<Vec<_>>();
    match op {
        "page" => page_op(&fields(), &TextPageOptions::default()),
        "pageopts" => page_options(rest),
        "search" => search(rest),
        "fixture" => fixture(rest),
        "text" => strings(rest),
        "blocks" => blocks(rest),
        _ => vec![format!("unknown op {op}")],
    }
}

#[test]
fn text_is_the_golden_file() {
    let mut actual = String::new();
    let mut skipped = 0;
    for line in golden("text/text-input.txt").split('\n') {
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let (op, rest) = line.split_once(' ').unwrap_or((line, ""));
        if op == "fixture" && !rest.starts_with("golden/") {
            skipped += 1;
            continue;
        }
        let units = units(line);
        let shown = if units.len() > 160 {
            format!("{}...", String::from_utf16_lossy(&units[..160]))
        } else {
            line.to_string()
        };
        actual += &format!("> {shown}\n");
        for result in run(op, rest) {
            actual += &format!("< {result}\n");
        }
    }
    let mut expected = String::new();
    let mut inside = false;
    for line in golden("text/text-output.txt").split_inclusive('\n') {
        if line.starts_with("> ") {
            inside = !line.starts_with("> fixture ") || line.starts_with("> fixture golden/");
        }
        if inside {
            expected += line;
        }
    }
    assert!(expected.len() > 500_000);
    // Eight fixture lines open encrypted files.
    assert_eq!(skipped, 8);
    for (i, (a, e)) in actual.split('\n').zip(expected.split('\n')).enumerate() {
        assert_eq!(a, e, "line {}", i + 1);
    }
    assert_eq!(actual.split('\n').count(), expected.split('\n').count());
}
