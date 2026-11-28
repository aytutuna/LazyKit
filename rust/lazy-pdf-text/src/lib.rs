//! The text of a PDF page: characters with their places, words, lines and blocks in reading order, hit tests,
//! selection and search.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bidi_text;
mod collector;
mod flow;
mod geometry;
mod hit;
mod layers;
mod model;
mod order;
mod page;
mod quad;
mod search;
mod search_text;
mod selection;
mod tables;
mod text_string;
mod unicode;

pub use bidi_text::{BidiClass, class_of as bidi_class_of};
pub use collector::clean;
pub use geometry::{DisplayRect, PageGeometry};
pub use model::{
    PageLine, TextBlock, TextBreak, TextChar, TextHit, TextPageOptions, TextRange, TextWord,
};
pub use page::TextPage;
pub use quad::{QuadBox, TextQuad};
pub use search::{BadPattern, HitCut, MAX_PATTERN, SearchMatch, TextSearch, TextSearchOptions};
pub use search_text::{collapse, fold, strip_marks};
pub use selection::TextSelection;
pub use text_string::decode as decode_text_string;
pub use unicode::{
    is_letter, is_letter_or_digit, is_lower, is_mark, is_non_spacing_mark, is_white_space,
    lowercase_invariant,
};
