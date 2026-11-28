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

pub use cff::CffFont;
pub use embedded::{
    EmbedStatus, EmbeddedFont, FontDescriptorValues, WidthRun, build, build_glyphs,
};
pub use names::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};
pub use outline::{Bounds, Outline, OutlineOp, Segment};
pub use subset::{SubsetResult, SubsetStatus, subset};
pub use tounicode::{OutOfRange, write_to_unicode};
pub use truetype::TrueTypeFont;
