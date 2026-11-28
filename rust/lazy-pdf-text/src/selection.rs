//! Characters of a page picked by a drag, a rectangle or a click, as runs of indices in reading order. Points and
//! rectangles are in user space; the viewer converts from display points with `PageGeometry`.

use lazy_pdf_syntax::Rect;

use crate::model::TextRange;
use crate::page::TextPage;
use crate::quad::TextQuad;

pub struct TextSelection<'a> {
    pub page: &'a TextPage,
    pub ranges: Vec<TextRange>,
}

impl<'a> TextSelection<'a> {
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The characters between two insertion points, as dragging a caret from the first to the second selects them.
    /// Either order.
    pub fn between(page: &'a TextPage, x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        let a = page.caret_at(x1, y1);
        let b = page.caret_at(x2, y2);
        Self::of(page, TextRange::new(a.min(b), a.max(b)))
    }

    /// The characters whose boxes lie at least half inside a rectangle (a marquee), by lines in reading order.
    pub fn in_rectangle(page: &'a TextPage, rect: &Rect) -> Self {
        let mut ranges = Vec::new();
        let mut start: Option<usize> = None;
        for i in 0..=page.chars.len() {
            let inside = i < page.chars.len() && covered(&page.chars[i].quad, rect);
            if inside && start.is_none() {
                start = Some(i);
            } else if !inside && let Some(from) = start.take() {
                ranges.push(TextRange::new(from, i));
            }
        }
        TextSelection { page, ranges }
    }

    /// The word under a point; a point on a space takes the word after it, else the one before it.
    pub fn word_at(page: &'a TextPage, x: f64, y: f64) -> Self {
        let none = || TextSelection {
            page,
            ranges: Vec::new(),
        };
        let Some(hit) = page.hit_test(x, y, f64::INFINITY) else {
            return none();
        };
        let c = &page.chars[hit.char];
        if let Some(word) = c.word {
            return Self::of(page, page.word_range(word));
        }
        let line = page.lines[c.line].range;
        for i in hit.char..line.end {
            if let Some(word) = page.chars[i].word {
                return Self::of(page, page.word_range(word));
            }
        }
        for i in (line.start..=hit.char).rev() {
            if let Some(word) = page.chars[i].word {
                return Self::of(page, page.word_range(word));
            }
        }
        none()
    }

    /// The line under a point (a triple click).
    pub fn line_at(page: &'a TextPage, x: f64, y: f64) -> Self {
        match page.hit_test(x, y, f64::INFINITY) {
            Some(hit) => Self::of(page, page.line_range(hit.line)),
            None => TextSelection {
                page,
                ranges: Vec::new(),
            },
        }
    }

    pub fn of(page: &'a TextPage, range: TextRange) -> Self {
        let start = range.start.min(page.chars.len());
        let end = range.end.max(start).min(page.chars.len());
        TextSelection {
            page,
            ranges: if start == end {
                Vec::new()
            } else {
                vec![TextRange::new(start, end)]
            },
        }
    }

    /// The selected text; runs that are not next to each other are separated by a line feed.
    pub fn get_text(&self, join_hyphens: bool) -> String {
        self.ranges
            .iter()
            .map(|r| self.page.get_text(*r, join_hyphens))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One box per stretch of a line, for painting.
    pub fn quads(&self) -> Vec<TextQuad> {
        self.ranges
            .iter()
            .flat_map(|r| self.page.quads(*r))
            .collect()
    }
}

fn covered(quad: &TextQuad, rect: &Rect) -> bool {
    let b = quad.bounds();
    if b.width() <= 0.0 || b.height() <= 0.0 {
        let c = quad.center();
        return c.0 >= rect.left && c.0 <= rect.right && c.1 >= rect.bottom && c.1 <= rect.top;
    }
    let w = b.right.min(rect.right) - b.left.max(rect.left);
    let h = b.top.min(rect.top) - b.bottom.max(rect.bottom);
    w > 0.0 && h > 0.0 && w * h >= 0.5 * b.width() * b.height()
}
