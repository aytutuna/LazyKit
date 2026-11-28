// The paragraph algorithm against the curated lines of the Unicode conformance file and the reference engines' own
// cases.

use std::path::PathBuf;

use lazy_pdf_text::{
    BidiParagraph, BidiType, bidi_mirror, bidi_type_of, is_right_to_left, paired_bracket,
};

fn golden(name: &str) -> String {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/pdf-fonts/bidi")
        .join(name);
    std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

fn check_lines(file: &str) -> usize {
    let mut checked = 0;
    for line in golden(file).lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(';').collect();
        let mut text = String::new();
        let mut first_unit = Vec::new();
        let mut count = 0;
        for cp in f[0].split(' ') {
            first_unit.push(count);
            let c = char::from_u32(u32::from_str_radix(cp, 16).unwrap()).unwrap();
            text.push(c);
            count += c.len_utf16();
        }
        let direction: i32 = f[1].parse().unwrap();
        let para = BidiParagraph::new(&text, (direction != 2).then_some(direction));
        assert_eq!(f[2].parse::<i32>().unwrap(), para.level, "{line}");
        let levels = para.line_levels(0, count);
        let actual: Vec<String> = first_unit
            .iter()
            .map(|&u| {
                if levels[u] < 0 {
                    "x".to_string()
                } else {
                    levels[u].to_string()
                }
            })
            .collect();
        assert_eq!(f[3], actual.join(" "), "{line}");
        let order: Vec<String> = para
            .visual_order(0, count)
            .iter()
            .filter_map(|u| first_unit.iter().position(|f| f == u))
            .map(|i| i.to_string())
            .collect();
        assert_eq!(f[4], order.join(" "), "{line}");
        checked += 1;
    }
    checked
}

fn visual(text: &str, base: Option<i32>) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let para = BidiParagraph::new(text, base);
    let shown: Vec<u16> = para
        .visual_order(0, units.len())
        .iter()
        .map(|&u| units[u])
        .collect();
    String::from_utf16_lossy(&shown)
}

#[test]
fn curated_conformance_lines_pass() {
    assert!(check_lines("bidi-character.txt") >= 250);
}

#[test]
fn overflow_and_line_end_lines_pass() {
    assert_eq!(check_lines("bidi-overflow.txt"), 5);
}

#[test]
fn a_hebrew_word_is_reversed_and_numbers_keep_their_order() {
    assert_eq!(
        visual("\u{05D0}\u{05D1}\u{05D2}", None),
        "\u{05D2}\u{05D1}\u{05D0}"
    );
    assert_eq!(visual("\u{05D0}\u{05D1} 12", None), "12 \u{05D1}\u{05D0}");
}

#[test]
fn the_paragraph_level_comes_from_the_first_strong_character() {
    assert_eq!(BidiParagraph::new("123 \u{0627}", None).level, 1);
    assert_eq!(BidiParagraph::new("abc \u{0627}", None).level, 0);
    assert_eq!(BidiParagraph::new("123", None).level, 0);
    assert_eq!(BidiParagraph::new("abc", Some(1)).level, 1);
    assert_eq!(
        BidiParagraph::new("\u{2067}\u{05D0}\u{2069}a", None).level,
        0
    );
}

#[test]
fn trailing_spaces_take_the_paragraph_level_on_the_line() {
    let para = BidiParagraph::new("\u{202E}ab  ", Some(0));
    assert_eq!(para.levels, [-1, 1, 1, 1, 1]);
    assert_eq!(para.line_levels(0, 5), [-1, 1, 1, 0, 0]);
    assert_eq!(para.visual_order(0, 5), [2, 1, 3, 4]);
    let para = BidiParagraph::new("\u{05D0} \u{05D1}", Some(0));
    assert_eq!(para.line_levels(0, 2), [1, 0]);
}

#[test]
fn a_surrogate_pair_is_one_character_and_stays_in_order() {
    let text = "a \u{10800}\u{10801}";
    let para = BidiParagraph::new(text, None);
    assert_eq!(para.levels, [0, 0, 1, 1, 1, 1]);
    assert_eq!(
        para.visual_order(0, text.encode_utf16().count()),
        [0, 1, 4, 5, 2, 3]
    );
}

#[test]
fn mirroring_and_paired_brackets_come_from_the_tables() {
    assert_eq!(bidi_mirror(0x28), 0x29);
    assert_eq!(bidi_mirror(0x3E), 0x3C);
    assert_eq!(bidi_mirror(0x61), 0x61);
    assert_eq!(paired_bracket(0x28), (0x29, true));
    assert_eq!(paired_bracket(0x5D), (0x5B, false));
    assert_eq!(paired_bracket(0x61).0, 0);
    assert_eq!(bidi_type_of(0x0627), BidiType::Al);
    assert_eq!(bidi_type_of(0x05D0), BidiType::R);
    assert_eq!(bidi_type_of(0x0661), BidiType::An);
    assert_eq!(bidi_type_of(0x08A0), BidiType::Al);
    assert_eq!(bidi_type_of(0x110000), BidiType::L);
}

#[test]
fn brackets_and_odd_formatting_characters() {
    let para = BidiParagraph::new("\u{05D0}(b)\u{05D1}", Some(1));
    assert_eq!(para.levels, [1, 1, 2, 1, 1]);
    let para = BidiParagraph::new("a\u{202C}\u{2069}b\u{202B}", Some(0));
    assert_eq!(para.levels, [0, -1, 0, 0, -1]);
}

#[test]
fn deep_nesting_stops_at_the_limit() {
    let deep = "\u{202B}\u{2066}".repeat(200) + "a";
    let para = BidiParagraph::new(&deep, Some(0));
    let top = *para.levels.iter().max().unwrap();
    assert!((0..=126).contains(&top));
    let brackets = "(".repeat(5000) + "\u{05D0}" + &")".repeat(5000);
    let n = brackets.encode_utf16().count();
    assert_eq!(
        BidiParagraph::new(&brackets, Some(0))
            .visual_order(0, n)
            .len(),
        n
    );
}

#[test]
fn a_long_paragraph_resolves_quickly() {
    let text = "abc \u{05D0}\u{05D1}\u{05D2} 123 (x) ".repeat(20000);
    let start = std::time::Instant::now();
    let para = BidiParagraph::new(&text, Some(0));
    let _ = para.visual_order(0, text.encode_utf16().count());
    assert!(start.elapsed().as_secs() < 10);
}

#[test]
fn the_first_strong_letter_sets_the_direction() {
    let cases = [
        (
            "\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{05E2}\u{05D5}\u{05DC}\u{05DD}",
            true,
        ),
        ("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}", true),
        ("  123 \u{0645}\u{0631}\u{062D}\u{0628}\u{0627}", true),
        ("Hello \u{05E9}\u{05DC}\u{05D5}\u{05DD}", false),
        ("Merhaba d\u{00FC}nya", false),
        ("123", false),
        ("", false),
    ];
    for (text, expected) in cases {
        assert_eq!(is_right_to_left(text), expected, "{text}");
    }
}
