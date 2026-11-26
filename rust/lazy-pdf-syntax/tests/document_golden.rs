mod common;

use common::fixtures::{edit, make_save_fixture};
use common::open;
use common::pdf::{TestPdf, object_stream_file};
use common::{escape, golden, latin};
use lazy_pdf_syntax::{Dict, Document, Number, Object, ReadError, Rect, SaveOptions};

// The reference engines' document golden file, minus the files they encrypt: those need the crypt layer, and their
// blocks are left out of the comparison. One line of input is "<op> <name> [<how>]": a file built by both sides
// (the names in make), read, repaired, updated or rewritten, and what came of it, in text.
const EXACT: [&str; 3] = ["table", "stream", "hybrid"];

fn run(input: &str) -> String {
    let mut output = String::new();
    for line in input.split('\n') {
        if line.is_empty() || line.starts_with("# ") || line.contains("aes") {
            continue;
        }
        let words: Vec<&str> = line.split(' ').collect();
        output.push_str(&format!("> {line}\n"));
        for result in op(words[0], words[1], words.get(2).copied()) {
            output.push_str(&format!("< {result}\n"));
        }
    }
    output
}

fn op(op: &str, name: &str, how: Option<&str>) -> Vec<String> {
    match op {
        "open" => describe(make(name), !packed(name)),
        "incremental" => {
            let original = make(name);
            let mut document = open(original.clone());
            edit(&mut document);
            let saved = document.save_incremental().unwrap();
            if EXACT.contains(&name) {
                lines(&saved[original.len()..])
            } else {
                describe(saved, false)
            }
        }
        "rewrite" => {
            let mut document = open(make(name));
            let saved = document
                .save_rewrite(&SaveOptions {
                    use_object_streams: how == Some("packed"),
                    document_id: None,
                })
                .bytes;
            if EXACT.contains(&name) && how == Some("plain") {
                lines(&saved)
            } else {
                describe(saved, false)
            }
        }
        other => vec![format!("unknown op {other}")],
    }
}

fn packed(name: &str) -> bool {
    name.starts_with("aes") || name.starts_with("object")
}

/// The bytes as escaped lines, one to each line of the file, with the second /ID string zeroed.
fn lines(file: &[u8]) -> Vec<String> {
    let masked = mask_id(file);
    let mut lines: Vec<String> = masked.split(|b| *b == 10).map(escape).collect();
    if masked.last() == Some(&10) {
        lines.pop();
    }
    lines
}

fn mask_id(file: &[u8]) -> Vec<u8> {
    let mut copy = file.to_vec();
    let marker = b"/ID [<";
    let mut i = 0;
    while i + marker.len() < copy.len() {
        if &copy[i..i + marker.len()] != marker {
            i += 1;
            continue;
        }
        let mut p = i + marker.len();
        while p < copy.len() && copy[p] != b'>' {
            p += 1;
        }
        if p + 2 >= copy.len() || copy[p + 1] != b' ' || copy[p + 2] != b'<' {
            i += 1;
            continue;
        }
        p += 3;
        while p < copy.len() && copy[p] != b'>' {
            copy[p] = b'0';
            p += 1;
        }
        i += 1;
    }
    copy
}

fn yes(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn describe(file: Vec<u8>, layout: bool) -> Vec<String> {
    let mut document = match Document::open(file, Box::new(common::TestFilters)) {
        Ok(document) => document,
        Err(e) => return vec![error(&e)],
    };
    let mut lines = Vec::new();
    let id = document.id().first().map_or("-".to_string(), |id| {
        id.iter().map(|b| format!("{b:02X}")).collect()
    });
    let version = document.version();
    lines.push(if layout {
        format!("version {version} encrypted no id {id}")
    } else {
        format!("version {version} encrypted no")
    });
    // No fixture carries metadata, so no file claims a PDF/A part; that identification is not ported yet.
    lines.push(format!(
        "facts signed {} certified {} xfa {} tagged {} linearized {} pdfa ",
        yes(document.is_signed()),
        yes(document.is_certified()),
        yes(document.has_xfa()),
        yes(document.is_tagged()),
        yes(document.is_linearized()),
    ));
    if layout {
        lines.push(format!(
            "layout rebuilt {} stream {} start {} next {} length {}",
            yes(document.xref_rebuilt()),
            yes(document.xref_is_stream()),
            document.start_xref(),
            document.next_object_number(),
            document.length()
        ));
    } else {
        lines.push(format!(
            "layout rebuilt {} stream {} next {}",
            yes(document.xref_rebuilt()),
            yes(document.xref_is_stream()),
            document.next_object_number()
        ));
    }
    lines.push(format!("repairs {}", document.repairs().len()));
    for repair in document.repairs().to_vec() {
        lines.push(format!("repair {repair}"));
    }

    let pages = document.pages();
    lines.push(format!("pages {}", pages.len()));
    for page in pages.iter() {
        let media = page.media_box(&mut document);
        let crop = page.crop_box(&mut document);
        let rotate = page.rotate(&mut document);
        let unit = Number::from_f64(page.user_unit(&mut document));
        let contents = page.contents(&mut document);
        lines.push(format!(
            "page {} ref {} {} media {} crop {} rotate {rotate} unit {unit} contents {}",
            page.index,
            page.reference.number,
            page.reference.generation,
            boxed(&media),
            boxed(&crop),
            escape(&contents)
        ));
    }

    for number in document.object_numbers() {
        let entry = if layout {
            let e = document.entry(number).unwrap();
            format!(" {:?} {}", e.kind, e.offset)
        } else {
            String::new()
        };
        let generation = document.generation_of(number);
        let value = document.get(number).unwrap();
        match &*value {
            Object::Stream(stream) => {
                let xref = document.name_of(&stream.dict, "Type").as_deref() == Some(b"XRef");
                let mut copy = Dict::new();
                for (key, item) in stream.dict.entries() {
                    // The second string of /ID is random, and /Prev is where a compressed section starts.
                    if !key.is("Length") && !(xref && (key.is("ID") || key.is("Prev"))) {
                        copy.add(key.clone(), item.clone());
                    }
                }
                if xref {
                    lines.push(format!("obj {number} gen {generation}{entry} {copy}"));
                    continue;
                }
                let data = match document.stream_data(stream) {
                    Ok(data) => escape(&data),
                    Err(e) => format!("error {}", e.problem),
                };
                lines.push(format!(
                    "obj {number} gen {generation}{entry} {copy} data {data}"
                ));
            }
            other => lines.push(format!("obj {number} gen {generation}{entry} {other}")),
        }
    }
    lines
}

fn error(e: &ReadError) -> String {
    format!("error {} {}", e.problem, e.detail)
}

fn boxed(rect: &Rect) -> String {
    format!(
        "[{} {} {} {}]",
        Number::from_f64(rect.left),
        Number::from_f64(rect.bottom),
        Number::from_f64(rect.right),
        Number::from_f64(rect.top)
    )
}

/// The file a name stands for.
fn make(name: &str) -> Vec<u8> {
    match name {
        "table" | "stream" | "hybrid" | "objects" => make_save_fixture(name),
        "simple2" => TestPdf::simple(2),
        "junk-before" => [latin("junk before\r\n"), TestPdf::simple(2)].concat(),
        "header-late" => [vec![0; 2000], TestPdf::simple(2)].concat(),
        "not-pdf" => latin("hello"),
        "no-startxref" => TestPdf::replace(&TestPdf::simple(2), "startxref", "startxxxxx"),
        "startxref-past-end" => {
            let mut pdf = TestPdf::new();
            pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
                .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 1>>")
                .obj(3, "<</Type /Page /Parent 2 0 R>>");
            pdf.finish_with("/Root 1 0 R", 0, "99999999")
        }
        "shifted" => TestPdf::with_pages(2).finish_with("/Root 1 0 R", 7, ""),
        "wrong-length" => {
            let mut pdf = TestPdf::with_pages(1);
            pdf.stream_with(9, "", &latin("twelve bytes"), Some(3), true)
                .stream_with(10, "", &latin("ten bytes!"), Some(500), true)
                .stream_with(11, "", &latin("no length"), Some(-1), true);
            pdf.finish()
        }
        "no-endstream" => {
            let mut pdf = TestPdf::with_pages(1);
            pdf.stream_with(9, "", &latin("cut short"), Some(400), false);
            pdf.finish()
        }
        "no-trailer" => TestPdf::with_pages(2).finish_without_xref(),
        "no-root" => TestPdf::with_pages(2).finish_with("", 0, ""),
        "truncated" => {
            let file = TestPdf::simple(3);
            file[..file.len() - 120].to_vec()
        }
        "no-catalog" => {
            let mut pdf = TestPdf::new();
            pdf.obj(2, "<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>")
                .obj(3, "<</Type /Page /Parent 2 0 R>>")
                .obj(4, "<</Type /Page /Parent 2 0 R>>");
            pdf.finish_without_xref()
        }
        "no-tree" => {
            let mut pdf = TestPdf::new();
            pdf.obj(3, "<</Type /Page /MediaBox [0 0 10 10]>>")
                .obj(4, "<</Type /Page /MediaBox [0 0 20 20]>>");
            pdf.finish_without_xref()
        }
        "later-definition" => {
            let mut pdf = TestPdf::with_pages(1);
            pdf.obj(
                3,
                "<</Type /Page /Parent 2 0 R /Rotate 180 /Contents 4 0 R>>",
            );
            pdf.finish_without_xref()
        }
        "pdf-in-stream" => {
            let mut pdf = TestPdf::with_pages(1);
            pdf.stream(
                8,
                "",
                "%PDF-1.4\n9 0 obj\n<</Type /Page /Rotate 90>>\nendobj\ntrailer\n<</Root 9 0 R>>\n",
            );
            pdf.finish_without_xref()
        }
        "prev-broken" => {
            let first = TestPdf::with_pages(1).finish();
            let object_text =
                "3 0 obj\n<</Type /Page /Parent 2 0 R /Rotate 90 /Contents 4 0 R>>\nendobj\n";
            let text = format!(
                "{object_text}xref\n3 1\n{:010} 00000 n \ntrailer\n<</Size 5 /Root 1 0 R /Prev 5000000>>\nstartxref\n{}\n%%EOF\n",
                first.len(),
                first.len() + object_text.len()
            );
            [first, latin(&text)].concat()
        }
        "tree-ancestors" => {
            let mut pdf = TestPdf::new();
            pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
                .obj(2, "<</Type /Pages /Kids [3 0 R] /Count 2 /MediaBox [0 0 100 200] /Rotate 90 /Resources <</Font <<>>>>>>")
                .obj(3, "<</Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 /CropBox [10 10 60 60]>>")
                .obj(4, "<</Type /Page /Parent 3 0 R>>")
                .obj(5, "<</Type /Page /Parent 3 0 R /Rotate 270 /MediaBox [0 0 50 50] /Resources <</XObject <<>>>>>>");
            pdf.finish()
        }
        "tree-cycle" => {
            let mut pdf = TestPdf::new();
            pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
                .obj(2, "<</Type /Pages /Kids [3 0 R 2 0 R 4 0 R] /Count 2>>")
                .obj(3, "<</Type /Page /Parent 2 0 R>>")
                .obj(4, "<</Type /Pages /Parent 2 0 R /Kids [2 0 R 5 0 R]>>")
                .obj(5, "<</Type /Page /Parent 4 0 R>>");
            pdf.finish()
        }
        "tree-junk" => {
            let mut pdf = TestPdf::new();
            pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
                .obj(2, "<</Type /Pages /Kids [3 0 R 42 (text) null 77 0 R 6 0 R <</Type /Page>> 5 0 R 3 0 R] /Count 2>>")
                .obj(3, "<</Type /Page>>")
                .obj(5, "<</Type /Font /BaseFont /Helvetica>>")
                .obj(6, "[1 2 3]");
            pdf.finish()
        }
        "tree-boxes" => {
            let mut pdf = TestPdf::new();
            pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>")
                .obj(2, "<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>")
                .obj(3, "<</Type /Page /MediaBox [300 400 0 0] /TrimBox [5 5 15 25] /Rotate 450 /UserUnit 2>>")
                .obj(4, "<</Type /Page /MediaBox [0 0 0 0] /CropBox [1 2] /Rotate -90 /UserUnit -3 /BleedBox (x)>>");
            pdf.finish()
        }
        "object-stream" => {
            let (pdf, xref) = object_stream_file();
            [
                pdf.finish_without_xref(),
                latin(&format!("startxref\n{xref}\n%%EOF\n")),
            ]
            .concat()
        }
        "object-stream-lost" => {
            let (pdf, _) = object_stream_file();
            [pdf.finish_without_xref(), latin("startxref\n12\n%%EOF\n")].concat()
        }
        other => panic!("{other}"),
    }
}

#[test]
fn document_output_is_the_golden_file() {
    let actual = run(&golden("document/document-input.txt"));
    let expected: String = golden("document/document-output.txt")
        .split_inclusive('\n')
        .fold((String::new(), true), |(mut kept, keep), line| {
            let keep = match line.strip_prefix("> ") {
                Some(command) => !command.contains("aes"),
                None => keep,
            };
            if keep {
                kept.push_str(line);
            }
            (kept, keep)
        })
        .0;
    for (n, (a, e)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(a, e, "line {}", n + 1);
    }
    assert_eq!(actual.lines().count(), expected.lines().count());
}
