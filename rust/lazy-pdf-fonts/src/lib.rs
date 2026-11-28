//! PDF fonts: font programs (TrueType, CFF, Type 1), their tables and CMaps, and the subsetter.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bytes;
mod cff;
mod cff_interp;
mod cmap;
mod color;
mod content_parser;
mod content_writer;
mod data;
mod embedded;
mod encodings;
mod filters;
mod font;
mod function;
mod interpreter;
mod matrix;
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
pub use color::{ColorSpaceInfo, PdfColor, RgbColor};
pub use content_parser::{
    ContentOperation, InlineImage, TooManyOperations, parse as parse_content,
    parse_limited as parse_content_limited,
};
pub use content_writer::ContentWriter;
pub use embedded::{
    EmbedStatus, EmbeddedFont, FontDescriptorValues, WidthRun, build, build_glyphs,
};
pub use encodings::{
    Encoding, code_of, encoding_by_name, glyph_name_of, mac_expert, mac_roman, pdf_doc, standard,
    symbol, unicode_of_glyph_name, win_ansi, zapf_dingbats, zapf_name_of,
};
pub use filters::StandardFilters;
pub use font::{FontChar, PdfFont};
pub use function::{Function, FunctionError};
pub use interpreter::{
    ContentContext, ContentInterpreter, ContentVisitor, FormInfo, GlyphInfo, GraphicsState,
    ImageInfo, MarkedContent, OptionalContent, PathInfo, PathSegment, PathSegmentKind, ShadingInfo,
};
pub use matrix::Matrix;
pub use names::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};
pub use outline::{Bounds, Outline, OutlineOp, Segment};
pub use standard14::{FontBox, Standard14};
pub use subset::{SubsetResult, SubsetStatus, subset};
pub use tounicode::{OutOfRange, write_to_unicode};
pub use truetype::TrueTypeFont;
pub use type1::Type1Font;
