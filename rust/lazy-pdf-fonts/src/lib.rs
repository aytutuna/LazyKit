//! PDF fonts: font programs (TrueType, CFF, Type 1), their tables and CMaps, and the subsetter.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bytes;
mod cff;
mod cff_interp;
mod embedded;
mod names;
mod outline;
mod subset;
mod tounicode;
mod truetype;
mod type1;
mod type1_interp;

pub use cff::CffFont;
pub use embedded::{
    EmbedStatus, EmbeddedFont, FontDescriptorValues, WidthRun, build, build_glyphs,
};
pub use names::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};
pub use outline::{Bounds, Outline, OutlineOp, Segment};
pub use subset::{SubsetResult, SubsetStatus, subset};
pub use tounicode::{OutOfRange, write_to_unicode};
pub use truetype::TrueTypeFont;
pub use type1::Type1Font;
