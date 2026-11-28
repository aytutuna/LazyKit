//! PDF fonts: font programs (TrueType, CFF, Type 1), their tables and CMaps, and the subsetter.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bytes;
mod cff;
mod cff_interp;
mod names;
mod outline;
mod tounicode;

pub use cff::CffFont;
pub use names::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};
pub use outline::{Bounds, Outline, OutlineOp, Segment};
pub use tounicode::{OutOfRange, write_to_unicode};
