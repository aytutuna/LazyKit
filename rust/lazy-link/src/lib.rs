//! The Lazy phone link: the parts of the protocol that are plain functions of their input.
//! Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)

mod short_code;

pub use short_code::{ALPHABET, SHORT_CODE_LENGTH, normalize_short_code};
