//! The text of a PDF page: characters with their places, words, lines and blocks in reading order, hit tests,
//! selection and search.
//!
//! Differences from the reference engines that are meant to stay:
//! - Regular expressions use the `regex` crate: no lookaround or backreferences (a bad pattern), a pattern limit, a
//!   two second limit per page checked between matches (none in WebAssembly), and whole word is run without lookaround.
//!   In regex mode `ß` is not folded to `ss` when diacritics are folded; the text is folded the way plain search folds
//!   it, which does not cover it, and the pattern's dotted capital I is turned into `i`.
//! - Unicode data comes from three places: the bidi tables are UCD 17.0.0, general categories are Unicode 16.0.0
//!   (`unicode-general-category`), and white space, lower case, NFD and NFKC come from Rust's standard library and
//!   `unicode-normalization`. A character added in Unicode 16 or 17 can be classified differently from the Swift or C#
//!   engine, whose runtimes carry their own versions.
//! - `is_right_to_left` looks at the first 256 characters, where the reference takes 256 grapheme clusters.
//! - A bidi line cut inside a surrogate pair takes the whole pair.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bidi;
mod bidi_tables;
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

pub use bidi::{
    BidiParagraph, BidiType, is_right_to_left, mirror as bidi_mirror, paired_bracket,
    type_of as bidi_type_of,
};
pub use bidi_text::{BidiClass, class_of as bidi_class_of};
pub use collector::clean;
pub use geometry::{DisplayRect, PageGeometry};
pub use model::{
    PageLine, TextBlock, TextBreak, TextChar, TextHit, TextPageOptions, TextRange, TextWord,
};
pub use page::TextPage;
pub use quad::{QuadBox, TextQuad};
pub use search::{
    BadPattern, HitCut, MAX_PATTERN, SearchLimits, SearchMatch, SearchOutcome, TextSearch,
    TextSearchOptions,
};
pub use search_text::{collapse, fold, strip_marks};
pub use selection::TextSelection;
pub use text_string::decode as decode_text_string;
pub use unicode::{
    is_letter, is_letter_or_digit, is_lower, is_mark, is_non_spacing_mark, is_white_space,
    lowercase_invariant,
};
