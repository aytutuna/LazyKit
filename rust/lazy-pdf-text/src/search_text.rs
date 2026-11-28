//! A page's text as a search sees it: white space runs and line breaks become one space, case and diacritics are folded
//! as asked, and every character of it points back at the page character it came from. Text is held as UTF-16 units,
//! so the positions mean the same as in the reference engines.

use unicode_normalization::UnicodeNormalization;

use crate::model::{TextBreak, TextChar};
use crate::page::TextPage;
use crate::search::TextSearchOptions;
use crate::unicode::{
    is_letter, is_mark, is_non_spacing_mark, is_white_space, lowercase_invariant,
};

#[derive(Debug, Default)]
pub struct SearchText {
    pub text: Vec<u16>,
    /// The page character each unit of `text` comes from.
    pub map: Vec<usize>,
    /// For each unit of `text`, the offsets in its page character's own text that a hit on it takes: from the start of
    /// the letter it was made from (with the mark it is on, if it is a mark) to the end of that letter and the marks
    /// after it. A space the search made up at a line break takes none (0 to 0).
    pub from: Vec<usize>,
    pub to: Vec<usize>,
    /// The page has a hyphen at a line end that a join would take out.
    pub has_hyphen_break: bool,
}

impl SearchText {
    pub fn start_of(&self, index: usize) -> usize {
        self.from[index]
    }

    pub fn end_of(&self, index: usize) -> usize {
        self.to[index]
    }

    pub fn build(page: &TextPage, options: &TextSearchOptions, drop_hyphens: bool) -> SearchText {
        let mut r = SearchText::default();
        let mut hyphen_break = false;
        let mut line: Option<usize> = None;
        let mut soft = false;

        fn space(r: &mut SearchText, index: usize, end: usize) {
            if r.text.last().is_none_or(|&u| u == 0x20) {
                return;
            }
            r.text.push(0x20);
            r.map.push(index);
            r.from.push(0);
            r.to.push(end);
        }

        for c in &page.chars {
            if line != Some(c.line) {
                if line.is_some() && r.text.last().is_some_and(|&u| u != 0x20) {
                    let before = page.lines[c.line].before;
                    if soft && before == TextBreak::Line {
                        hyphen_break = true;
                    } else if before == TextBreak::Line
                        && !c.is_space
                        && joins_after_hyphen(&r.text, c)
                    {
                        hyphen_break = true;
                        if drop_hyphens {
                            r.text.pop();
                            r.map.pop();
                            r.from.pop();
                            r.to.pop();
                        }
                    } else {
                        space(&mut r, c.index, 0);
                    }
                }
                line = Some(c.line);
                soft = false;
            }
            if c.is_space {
                space(&mut r, c.index, 1);
                continue;
            }
            if c.text == "\u{AD}" {
                soft = true;
                continue;
            }
            soft = false;
            let units: Vec<u16> = c.text.encode_utf16().collect();
            for (u, &original) in units.iter().enumerate() {
                let single = [original];
                let source: &[u16] = if units.len() == 1 { &units } else { &single };
                for f in fold(source, options) {
                    r.text.push(f);
                    r.map.push(c.index);
                    let mut start = u;
                    let mut end = u + 1;
                    while start > 0 && is_mark(units[start]) {
                        start -= 1;
                    }
                    while end < units.len() && is_mark(units[end]) {
                        end += 1;
                    }
                    r.from.push(start);
                    r.to.push(end);
                }
            }
        }
        r.has_hyphen_break = hyphen_break;
        r
    }
}

fn joins_after_hyphen(text: &[u16], next: &TextChar) -> bool {
    let Some(letter) = next.text.encode_utf16().next() else {
        return false;
    };
    if text.len() < 2 {
        return false;
    }
    let hyphen = text[text.len() - 1];
    matches!(hyphen, 0x2D | 0x2010 | 0x2011) && is_letter(text[text.len() - 2]) && is_letter(letter)
}

// Dotless i is a different letter until diacritics are folded; dotted capital I is always i.
pub fn fold(units: &[u16], options: &TextSearchOptions) -> Vec<u16> {
    let stripped = if options.fold_diacritics {
        strip_marks(units)
    } else {
        units.to_vec()
    };
    if options.match_case {
        return stripped;
    }
    stripped
        .into_iter()
        .map(|u| {
            if u == 0x130 {
                0x69
            } else {
                lowercase_invariant(u)
            }
        })
        .collect()
}

/// Leading and trailing white space is dropped and every run of it inside becomes one space.
pub fn collapse(units: &[u16]) -> Vec<u16> {
    let mut end = units.len();
    while end > 0 && is_white_space(units[end - 1]) {
        end -= 1;
    }
    let lead = units[..end]
        .iter()
        .take_while(|&&u| is_white_space(u))
        .count();
    let mut sb: Vec<u16> = Vec::with_capacity(end - lead);
    for &c in &units[lead..end] {
        if is_white_space(c) {
            if sb.last().is_some_and(|&l| l != 0x20) {
                sb.push(0x20);
            }
        } else {
            sb.push(c);
        }
    }
    sb
}

pub fn strip_marks(text: &[u16]) -> Vec<u16> {
    if !text.iter().any(|&c| c >= 0xC0 || c == 0x131) {
        return text.to_vec();
    }
    let mut sb: Vec<u16> = Vec::with_capacity(text.len() + 2);
    for &c in text {
        match c {
            0x131 => sb.push(0x69),
            0x130 => sb.push(0x49),
            0xDF => sb.extend([0x73, 0x73]),
            0xE6 => sb.extend([0x61, 0x65]),
            0xC6 => sb.extend([0x41, 0x45]),
            0x153 => sb.extend([0x6F, 0x65]),
            0x152 => sb.extend([0x4F, 0x45]),
            0xF8 => sb.push(0x6F),
            0xD8 => sb.push(0x4F),
            0x142 => sb.push(0x6C),
            0x141 => sb.push(0x4C),
            0x111 => sb.push(0x64),
            0x110 => sb.push(0x44),
            _ => {
                // Hangul syllables and the like stay whole: decomposing them would change what a CJK search finds.
                let scalar = if (0xC0..0x2E80).contains(&c) {
                    char::from_u32(u32::from(c))
                } else {
                    None
                };
                let Some(scalar) = scalar else {
                    sb.push(c);
                    continue;
                };
                let mut buffer = [0u16; 2];
                for d in std::iter::once(scalar).nfd() {
                    for &unit in d.encode_utf16(&mut buffer).iter() {
                        if !is_non_spacing_mark(unit) {
                            sb.push(unit);
                        }
                    }
                }
            }
        }
    }
    sb
}
