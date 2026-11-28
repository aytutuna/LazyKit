//! PDF fonts: font programs (TrueType, CFF, Type 1), their tables and CMaps, and the subsetter.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bytes;
mod cff;
mod cff_interp;
mod cmap;
mod data;
mod embedded;
mod encodings;
mod filters;
mod font;
mod names;
mod outline;
mod program;
mod standard14;
mod subset;
mod tounicode;
mod truetype;
mod type1;
mod type1_interp;

pub use cff::CffFont;
pub use cmap::CMap;
pub use embedded::{
    EmbedStatus, EmbeddedFont, FontDescriptorValues, WidthRun, build, build_glyphs,
};
pub use encodings::{
    Encoding, code_of, encoding_by_name, glyph_name_of, mac_expert, mac_roman, pdf_doc, standard,
    symbol, unicode_of_glyph_name, win_ansi, zapf_dingbats, zapf_name_of,
};
pub use filters::StandardFilters;
pub use font::{FontChar, PdfFont};
pub use names::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};
pub use outline::{Bounds, Outline, OutlineOp, Segment};
pub use standard14::{FontBox, Standard14};
pub use subset::{SubsetResult, SubsetStatus, subset};
pub use tounicode::{OutOfRange, write_to_unicode};
pub use truetype::TrueTypeFont;
pub use type1::Type1Font;
