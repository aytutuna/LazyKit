//! PDF syntax: the lexer, the object model, the reader with its cross-reference tables, streams, object streams and
//! repair of broken files, and the writer with the incremental update. Stream filters come in through [`Filters`].
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod bytes;
mod document;
mod error;
mod facts;
mod filters;
mod lexer;
mod object;
mod objects;
mod pages;
mod repair;
mod save;
mod streams;
mod writer;
mod xmp;
mod xref;

pub use document::{Document, Entry, EntryKind, MAX_OBJECT_NUMBER, Value};
pub use error::{Problem, ReadError, latin1};
pub use filters::{DecodeError, Filters, Parm};
pub use lexer::{Lexer, is_delimiter, is_regular, is_white_space};
pub use object::{Dict, Name, Number, Object, PdfString, Ref, Stream};
pub use pages::{Page, Rect};
pub use save::{Rewritten, SaveOptions};
pub use streams::{DEFAULT_MAX_SIZE, Decoded, Stage};
