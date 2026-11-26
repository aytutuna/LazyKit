//! PDF syntax: the lexer, the object model, the reader with its cross-reference tables, streams, object streams and
//! repair of broken files, and the writer with the incremental update. Stream filters come in through [`Filters`].
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod error;
mod filters;
mod lexer;
mod object;

pub use error::{Problem, ReadError, latin1};
pub use filters::{DecodeError, Filters, Parm};
pub use lexer::{Lexer, is_delimiter, is_regular, is_white_space};
pub use object::{Dict, Name, Number, Object, PdfString, Ref, Stream};
