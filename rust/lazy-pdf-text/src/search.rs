//! Finding a query on a page.
//!
//! Regular expressions run on the `regex` crate, which takes time linear in the text and has no lookaround or
//! backreferences; the reference engines run ICU's and .NET's, which can take exponential time and are stopped by a
//! two second limit. A pattern those engines accept that this one does not (a lookaround, a backreference) is a bad
//! pattern here, and a pattern longer than `MAX_PATTERN` characters or compiling to more than `REGEX_SIZE` bytes is
//! refused. The whole word option, which the reference wraps in lookarounds, is run as a match that takes one
//! neighbour on each side and is searched again from the last character of each hit.

use std::fmt;

use regex::{Regex, RegexBuilder};

use crate::model::TextRange;
use crate::page::TextPage;
use crate::quad::TextQuad;
use crate::search_text::{SearchText, collapse, fold, strip_marks};
use crate::unicode::is_letter_or_digit;

/// The longest regular expression taken, in characters.
pub const MAX_PATTERN: usize = 4096;
const REGEX_SIZE: usize = 4 << 20;

/// How a search reads its query. `match_case`: capitals must match; by default they do not matter (dotless i stays a
/// different letter from i). `whole_word`: the match may not touch a letter or digit on either side. `regex`: the query
/// is a regular expression over the text of the page, where a line break is one space. `fold_diacritics`: accents and
/// marks do not matter, and I, dotted and dotless I and i are all the same letter.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextSearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub fold_diacritics: bool,
}

/// A hit in the searched text: which text (0, or 1 with line-end hyphens dropped), where it starts, and the offsets it
/// cuts in the text of its first and last character. When the hit ends on the space the search adds at a line break,
/// `end` is 0; `only_break` is set when the hit is nothing but that space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitCut {
    pub order: usize,
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub only_break: bool,
}

/// A hit: the page, its characters and the boxes to paint, one for each line it crosses.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub page: usize,
    pub range: TextRange,
    pub text: String,
    pub quads: Vec<TextQuad>,
    /// Where the hit is in the text that was searched, for a replacement that has to cut a character's own text.
    pub source: HitCut,
}

#[derive(Debug)]
pub struct BadPattern(pub String);

impl fmt::Display for BadPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad pattern: {}", self.0)
    }
}

impl std::error::Error for BadPattern {}

struct Hit {
    range: TextRange,
    order: usize,
    index: usize,
    length: usize,
}

pub struct TextSearch {
    options: TextSearchOptions,
    query: Vec<u16>,
    regex: Option<Regex>,
}

impl TextSearch {
    /// None for an empty query. Fails for a bad pattern.
    pub fn new(query: &str, options: &TextSearchOptions) -> Result<Option<TextSearch>, BadPattern> {
        if query.is_empty() {
            return Ok(None);
        }
        if options.regex {
            let units: Vec<u16> = query.encode_utf16().collect();
            let folded = if options.fold_diacritics {
                String::from_utf16_lossy(&strip_marks(&units))
            } else {
                query.to_string()
            };
            if folded.chars().count() > MAX_PATTERN {
                return Err(BadPattern("too long".into()));
            }
            let build = |pattern: &str| {
                RegexBuilder::new(pattern)
                    .case_insensitive(!options.match_case)
                    .size_limit(REGEX_SIZE)
                    .dfa_size_limit(REGEX_SIZE)
                    .nest_limit(64)
                    .build()
                    .map_err(|e| BadPattern(e.to_string()))
            };
            let plain = build(&folded)?;
            let regex = if options.whole_word {
                build(&format!(
                    "(?:[^\\p{{L}}\\p{{N}}]|^)(?P<hit>(?:{folded}))(?:[^\\p{{L}}\\p{{N}}]|$)"
                ))?
            } else {
                plain
            };
            return Ok(Some(TextSearch {
                options: *options,
                query: Vec::new(),
                regex: Some(regex),
            }));
        }
        let units: Vec<u16> = query.encode_utf16().collect();
        let plain = collapse(&fold(&units, options));
        if plain.is_empty() {
            return Ok(None);
        }
        Ok(Some(TextSearch {
            options: *options,
            query: plain,
            regex: None,
        }))
    }

    /// Finds a query on a page. White space in the query matches any run of white space and line breaks; a hyphen at a
    /// line end can be part of the match or not (`in-` and `formation` match "information", `well-` and `known` match
    /// "well-known").
    pub fn find(
        page: &TextPage,
        query: &str,
        options: &TextSearchOptions,
    ) -> Result<Vec<SearchMatch>, BadPattern> {
        Ok(match Self::new(query, options)? {
            Some(search) => search.matches(page, false),
            None => Vec::new(),
        })
    }

    /// Every hit on a page, several in one character included (hits in a span's replacement text), for a replacement
    /// that cuts them out of that text.
    pub fn find_all(
        page: &TextPage,
        query: &str,
        options: &TextSearchOptions,
    ) -> Result<Vec<SearchMatch>, BadPattern> {
        Ok(match Self::new(query, options)? {
            Some(search) => search.matches(page, true),
            None => Vec::new(),
        })
    }

    pub fn matches(&self, page: &TextPage, all: bool) -> Vec<SearchMatch> {
        let mut found: Vec<Hit> = Vec::new();
        let mut texts = vec![SearchText::build(page, &self.options, false)];
        self.collect(&texts[0], 0, &mut found);
        if texts[0].has_hyphen_break {
            texts.push(SearchText::build(page, &self.options, true));
            self.collect(&texts[1], 1, &mut found);
        }
        found.sort_by(|a, b| {
            a.range
                .start
                .cmp(&b.range.start)
                .then_with(|| {
                    if all {
                        let x = texts[a.order].start_of(a.index);
                        let y = texts[b.order].start_of(b.index);
                        x.cmp(&y)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then(b.range.end.cmp(&a.range.end))
                .then(a.order.cmp(&b.order))
                .then(a.index.cmp(&b.index))
        });
        let key = |h: &Hit| {
            let t = &texts[h.order];
            (
                h.range.start,
                h.range.end,
                t.start_of(h.index),
                t.end_of(h.index + h.length - 1),
            )
        };
        let seen: std::collections::HashSet<_> = if all {
            found.iter().filter(|h| h.order == 0).map(key).collect()
        } else {
            Default::default()
        };
        let mut result = Vec::new();
        let mut end: i64 = -1;
        for hit in &found {
            let range = hit.range;
            if all && hit.order == 1 && seen.contains(&key(hit)) {
                continue;
            }
            if !all && (range.start as i64) < end {
                continue;
            }
            end = range.end as i64;
            let text = &texts[hit.order];
            let tip = hit.index + hit.length - 1;
            let tip_end = text.end_of(tip);
            result.push(SearchMatch {
                page: page.page_index,
                range,
                text: page.get_text(range, true),
                quads: page.quads(range),
                source: HitCut {
                    order: hit.order,
                    index: hit.index,
                    start: text.start_of(hit.index),
                    end: tip_end,
                    only_break: tip_end == 0 && hit.length == 1,
                },
            });
        }
        result
    }

    fn collect(&self, text: &SearchText, order: usize, found: &mut Vec<Hit>) {
        if text.text.is_empty() {
            return;
        }
        let hit = |from: usize, to: usize| Hit {
            range: TextRange::new(text.map[from], text.map[to - 1] + 1),
            order,
            index: from,
            length: to - from,
        };
        let Some(regex) = &self.regex else {
            let query = &self.query;
            let mut from = 0;
            while text.text.len() >= query.len() && from <= text.text.len() - query.len() {
                let Some(i) = index_of(&text.text, query, from) else {
                    break;
                };
                from = i + query.len();
                if self.options.whole_word && !self.is_whole(&text.text, i, from) {
                    from = i + 1;
                    continue;
                }
                found.push(hit(i, from));
            }
            return;
        };
        let (haystack, units) = to_haystack(&text.text);
        let spans = if self.options.whole_word {
            whole_word_hits(regex, &haystack)
        } else {
            regex
                .find_iter(&haystack)
                .map(|m| (m.start(), m.end()))
                .collect()
        };
        for (start, end) in spans {
            let (from, to) = (units[start] as usize, units[end] as usize);
            if to > from {
                found.push(hit(from, to));
            }
        }
    }

    // The edge of the query decides: a query that starts with a punctuation mark has no word edge there.
    fn is_whole(&self, text: &[u16], start: usize, end: usize) -> bool {
        if let Some(&first) = self.query.first()
            && is_letter_or_digit(first)
            && start > 0
            && is_letter_or_digit(text[start - 1])
        {
            return false;
        }
        let Some(&last) = self.query.last() else {
            return true;
        };
        !(is_letter_or_digit(last) && end < text.len() && is_letter_or_digit(text[end]))
    }
}

fn index_of(text: &[u16], query: &[u16], from: usize) -> Option<usize> {
    let last = text.len().checked_sub(query.len())?;
    (from..=last).find(|&i| text[i..i + query.len()] == *query)
}

// The units as a string, with the unit offset of each byte offset (one past the end included). A lone surrogate reads as
// U+FFFD, one unit like the surrogate itself.
fn to_haystack(units: &[u16]) -> (String, Vec<u32>) {
    let mut text = String::with_capacity(units.len());
    let mut offsets: Vec<u32> = Vec::with_capacity(units.len() + 1);
    let mut at = 0u32;
    for decoded in char::decode_utf16(units.iter().copied()) {
        let (c, width) = match decoded {
            Ok(c) => (c, c.len_utf16() as u32),
            Err(_) => ('\u{FFFD}', 1),
        };
        for _ in 0..c.len_utf8() {
            offsets.push(at);
        }
        at += width;
        text.push(c);
    }
    offsets.push(at);
    (text, offsets)
}

fn char_len_at(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(0, char::len_utf8)
}

fn char_len_before(text: &str, at: usize) -> usize {
    text[..at].chars().next_back().map_or(0, char::len_utf8)
}

// The hits of a whole word pattern: each match takes a neighbour on both sides, so the next search starts at the last
// character of the previous hit, which can be the neighbour of the next one. A match that starts before the end of the
// previous hit is that hit found again by the start-of-text alternative and is skipped.
fn whole_word_hits(regex: &Regex, haystack: &str) -> Vec<(usize, usize)> {
    let mut hits = Vec::new();
    let mut at = 0;
    let mut min_start = 0;
    while at <= haystack.len() {
        let Some(caps) = regex.captures_at(haystack, at) else {
            break;
        };
        let Some(group) = caps.name("hit") else { break };
        let (start, end) = (group.start(), group.end());
        if start < min_start {
            let step = char_len_at(haystack, at);
            if step == 0 {
                break;
            }
            at += step;
            continue;
        }
        if start == end {
            min_start = end + char_len_at(haystack, end).max(1);
            at = end;
            continue;
        }
        hits.push((start, end));
        min_start = end;
        at = end - char_len_before(haystack, end);
    }
    hits
}
