// Searches the text golden does not reach: whole-word regular expressions at the start of the text, pattern limits.

#[path = "../../lazy-pdf-fonts/tests/common/pdf.rs"]
mod pdf;

use lazy_pdf_fonts::StandardFilters;
use lazy_pdf_syntax::Document;
use lazy_pdf_text::{TextPage, TextPageOptions, TextSearch, TextSearchOptions};
use pdf::TestPdf;

// One page of lines of 10 point text in a font half an em wide whose codes read as ASCII.
fn page(lines: &[&str]) -> TextPage {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        "<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <</Font <</F1 5 0 R>>>>>>",
    );
    let content: String = lines
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let text = text
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            format!(
                "BT /F1 10 Tf 100 {} Td ({text}) Tj ET\n",
                700 - 12 * i as i64
            )
        })
        .collect();
    pdf.stream(4, "", content.as_bytes());
    let widths = vec!["500"; 256].join(" ");
    pdf.obj(
        5,
        &format!(
            "<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding /FirstChar 0 /LastChar 255 /Widths [{widths}]>>"
        ),
    );
    let mut doc = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let pages = doc.pages();
    TextPage::create(&mut doc, &pages[0], &TextPageOptions::default())
}

fn whole(pattern: &str, lines: &[&str]) -> Vec<String> {
    let options = TextSearchOptions {
        regex: true,
        whole_word: true,
        ..TextSearchOptions::default()
    };
    TextSearch::find(&page(lines), pattern, &options)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect()
}

#[test]
fn a_whole_word_hit_may_start_the_text() {
    assert_eq!(whole(r"-?\d+", &["-5 apples"]), ["-5"]);
    assert_eq!(whole(r"\S+", &["(abc) x"]), ["(abc)", "x"]);
    assert_eq!(whole(r"\.", &[".."]), [".", "."]);
}

#[test]
fn whole_word_hits_do_not_touch_letters() {
    assert_eq!(whole(r"\d+", &["a12 34 5b 6"]), ["34", "6"]);
    assert_eq!(whole(r"ab|abc", &["abcd abc"]), ["abc"]);
}

#[test]
fn a_search_stops_at_its_limits() {
    use lazy_pdf_text::SearchLimits;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    let line = "a".repeat(100);
    let lines: Vec<&str> = vec![line.as_str(); 60];
    let page = page(&lines);
    let options = TextSearchOptions {
        regex: true,
        ..TextSearchOptions::default()
    };
    let search = TextSearch::new("a(?:.*b)?", &options).unwrap().unwrap();
    let all = search.matches_limited(&page, false, &SearchLimits::default());
    assert!(!all.timed_out && !all.cancelled);
    assert!(!all.matches.is_empty());
    let quick = SearchLimits {
        time: Duration::ZERO,
        cancel: None,
    };
    let out = search.matches_limited(&page, false, &quick);
    assert!(out.timed_out && out.matches.is_empty());
    let cancel = SearchLimits {
        cancel: Some(Arc::new(AtomicBool::new(true))),
        ..SearchLimits::default()
    };
    assert!(search.matches_limited(&page, false, &cancel).cancelled);
}
