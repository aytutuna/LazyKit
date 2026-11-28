//! The character nearest to a point, and the caret.

use crate::model::{PageLine, TextHit};
use crate::page::TextPage;
use crate::quad::{smax, smin};

impl TextPage {
    /// The character nearest to a point in user space, with its word and line; none when the page has no text or the
    /// nearest one is farther than `max_distance` (user space units). A point inside a character's box has distance 0.
    pub fn hit_test(&self, x: f64, y: f64, max_distance: f64) -> Option<TextHit> {
        let (line, line_distance) = self.nearest_line(x, y);
        let line = line?;
        if line_distance > max_distance {
            return None;
        }
        let (index, distance) = self.nearest_char(line, x, y);
        let index = index?;
        if distance > max_distance {
            return None;
        }
        let c = &self.chars[index];
        Some(TextHit {
            char: index,
            word: c.word,
            line: c.line,
            distance,
        })
    }

    /// The insertion point nearest to a point, as a character index from 0 to `chars.len()`: the gap before that
    /// character. Used with a second one it gives a selection like a text caret does.
    pub fn caret_at(&self, x: f64, y: f64) -> usize {
        let (line, _) = self.nearest_line(x, y);
        let Some(line) = line else { return 0 };
        let (top, bottom) = self.extent();
        let shown = self.geometry.to_display(x, y).1;
        if shown < top {
            return 0;
        }
        if shown > bottom {
            return self.chars.len();
        }
        let (index, _) = self.nearest_char(line, x, y);
        let Some(index) = index else {
            return self.lines[line].range.start;
        };
        let c = &self.chars[index];
        let center = c.quad.center();
        let axis = reading_axis(&self.lines[line]);
        let along = (x - center.0) * axis.0 + (y - center.1) * axis.1;
        if if c.is_rtl { along < 0.0 } else { along > 0.0 } {
            index + 1
        } else {
            index
        }
    }

    // The vertical extent of the text as shown, so that a point above it or below it can be told from one beside it.
    fn extent(&self) -> (f64, f64) {
        *self.extent.get_or_init(|| {
            let mut top = f64::MAX;
            let mut bottom = -f64::MAX;
            for line in &self.lines {
                let r = self.geometry.rect_to_display(&line.bounds);
                top = smin(top, r.y);
                bottom = smax(bottom, r.y + r.height);
            }
            (top, bottom)
        })
    }

    fn nearest_line(&self, x: f64, y: f64) -> (Option<usize>, f64) {
        let mut best = None;
        let mut distance = f64::MAX;
        for (i, line) in self.lines.iter().enumerate() {
            let b = &line.bounds;
            let dx = smax(smax(b.left - x, x - b.right), 0.0);
            let dy = smax(smax(b.bottom - y, y - b.top), 0.0);
            if (dx * dx + dy * dy).sqrt() - 1e-9 > distance {
                continue;
            }
            let d = line.quad.distance_to(x, y);
            if d < distance {
                distance = d;
                best = Some(i);
            }
        }
        (best, distance)
    }

    fn nearest_char(&self, line: usize, x: f64, y: f64) -> (Option<usize>, f64) {
        let mut best: Option<usize> = None;
        let mut distance = f64::MAX;
        let range = self.lines[line].range;
        for i in range.start..range.end {
            let d = self.chars[i].quad.distance_to(x, y);
            if d < distance
                || (d == distance
                    && best.is_some_and(|b| self.chars[b].is_space)
                    && !self.chars[i].is_space)
            {
                distance = d;
                best = Some(i);
            }
        }
        (best, distance)
    }
}

// The direction a line is read in, in user space.
fn reading_axis(line: &PageLine) -> (f64, f64) {
    let q = &line.quad;
    let (x, y) = if line.is_vertical {
        (q.x0 - q.x3, q.y0 - q.y3)
    } else {
        (q.x1 - q.x0, q.y1 - q.y0)
    };
    let length = (x * x + y * y).sqrt();
    if length < 1e-12 {
        (1.0, 0.0)
    } else {
        (x / length, y / length)
    }
}
