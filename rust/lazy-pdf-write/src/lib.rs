//! Writing PDFs: the object writer with its cross-reference table, the glyphless font of an invisible text layer, PDF/A-2b
//! metadata, and scanned pages as a document. The bytes are the same as Lazy Scan's Swift and C# writers make.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

pub mod document;
pub mod glyphless;
mod number;
pub mod pdfa;
mod writer;

pub use document::{
    Conformance, G4Image, Image, OcrLine, OcrWord, Options, PageInput, PageSize, Placement, Rect,
    make, read_jpeg_header,
};
pub use number::num;
pub use pdfa::Date;
pub use writer::{ObjectWriter, ascii_string};
