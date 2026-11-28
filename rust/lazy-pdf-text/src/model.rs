//! What a `TextPage` holds: characters, words, lines and blocks, and the options it is built with.

use std::sync::Arc;

use lazy_pdf_fonts::{PdfFont, RgbColor};
use lazy_pdf_syntax::{Object, Rect};

use crate::quad::TextQuad;

/// A run of characters of a `TextPage` by index: `start` up to, not including, `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub fn new(start: usize, end: usize) -> Self {
        TextRange { start, end }
    }

    pub fn count(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// What comes between a line and the one before it in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextBreak {
    #[default]
    None,
    /// Same row, another cell: a table row, or columns the reader meets one after the other.
    Tab,
    Line,
    /// A new block: a paragraph, a column, a table.
    Block,
}

/// One shown glyph, or a space the gap between two glyphs implies. A ligature is one glyph with several characters in
/// `text`.
#[derive(Clone)]
pub struct TextChar {
    /// Position in `TextPage::chars`, the reading order (logical order within a right-to-left run).
    pub index: usize,
    /// Unicode text; empty for a glyph the font gives no text for. Arabic presentation forms and Latin ligature code
    /// points are folded to the plain letters.
    pub text: String,
    pub quad: TextQuad,
    /// None for an implied space.
    pub font: Option<Arc<PdfFont>>,
    /// The font size as drawn, in user space units (text matrix and CTM included).
    pub size: f64,
    /// Fill colour; stroke colour for render mode 1.
    pub color: RgbColor,
    pub render_mode: i64,
    /// Index of the operation in the parse of the content stream it belongs to; -1 for an implied space.
    pub operator_index: i64,
    /// The innermost form XObject the glyph came from, none on the page itself.
    pub form: Option<Arc<Object>>,
    /// Operand index and byte position of the code, as in `GlyphInfo`.
    pub element: usize,
    pub offset: usize,
    pub mcid: Option<i64>,
    pub is_artifact: bool,
    /// A space glyph, a space between words implied by a gap, or another white space character.
    pub is_space: bool,
    pub is_implied: bool,
    /// The glyph sits in a right-to-left run.
    pub is_rtl: bool,
    /// Index in `TextPage::words`; none for a space.
    pub word: Option<usize>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct TextWord {
    pub index: usize,
    pub range: TextRange,
    pub text: String,
    pub bounds: Rect,
    pub line: usize,
}

/// A row of characters on one baseline with no wide gap in it; a table cell is one line. `quad` is the box of the whole
/// line in the frame of its first glyph, `before` what separates it from the previous line in text.
#[derive(Debug, Clone)]
pub struct PageLine {
    pub index: usize,
    pub range: TextRange,
    pub text: String,
    pub bounds: Rect,
    pub quad: TextQuad,
    pub block: usize,
    pub before: TextBreak,
    /// The line runs top to bottom (a vertical writing font).
    pub is_vertical: bool,
    pub is_rtl: bool,
}

/// A paragraph, column piece or table.
#[derive(Debug, Clone)]
pub struct TextBlock {
    pub index: usize,
    pub lines: TextRange,
    pub bounds: Rect,
    pub is_table: bool,
}

/// What the text of a page is built from. `include_artifacts` keeps text marked /Artifact (headers, footers, page
/// numbers); `remove_duplicates` drops a glyph drawn again at nearly the same place, how bold is faked;
/// `include_annotations` adds the text of the appearances of annotations and form fields that are shown, placed on the
/// page. Those characters have `TextChar::form` set to the appearance stream, which the page's own content does not
/// reach.
#[derive(Debug, Clone, Copy)]
pub struct TextPageOptions {
    pub include_artifacts: bool,
    pub remove_duplicates: bool,
    pub include_annotations: bool,
}

impl Default for TextPageOptions {
    fn default() -> Self {
        TextPageOptions {
            include_artifacts: true,
            remove_duplicates: true,
            include_annotations: false,
        }
    }
}

/// The nearest character to a point, with the word and line it is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextHit {
    pub char: usize,
    pub word: Option<usize>,
    pub line: usize,
    pub distance: f64,
}
