//! What the syntax layer needs from the filter layer, which lives in its own crate. The caller hands a table in
//! when it opens a document.

/// A decode parameter as a plain value. A stream among the parameters (the JBIG2 globals) arrives decoded, as
/// bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Parm {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    String(Vec<u8>),
    Array(Vec<Parm>),
    Dict(Vec<(Vec<u8>, Parm)>),
    Bytes(std::sync::Arc<Vec<u8>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The data does not decode.
    Damaged(String),
    /// The filter is not supported, or is an image codec.
    Unsupported(String),
    /// The output would pass `max_size`.
    TooLarge(String),
}

/// What the document layer needs from the filter layer, as an adapter over lazy-pdf-filters makes it:
///
/// - `lazy_pdf_filters::Problem::Damaged` is `DecodeError::Damaged`.
/// - `Problem::TooBig` (the output or a table would pass the limit) is `DecodeError::TooLarge`. The budgets in
///   this crate rely on it: a decode that hits its limit is charged in full.
/// - `Problem::Unsupported` (an unknown filter, an image codec in the chain) is `DecodeError::Unsupported`.
///
/// `max_size` is passed through as the filter layer's limit. The document layer charges a failed decode for the
/// bytes the earlier stages of the chain produced, so a failure costs the same whatever its kind.
pub trait Filters: Send + Sync {
    /// One stage of a filter chain: `filter` as the file names it (abbreviations included), the stage's
    /// `/DecodeParms` entries, the data in. The result may not pass `max_size` bytes. With `strict` a Flate
    /// stream that breaks off or does not decode cleanly is damage, not a short result.
    fn decode(
        &self,
        filter: &[u8],
        parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        strict: bool,
    ) -> Result<Vec<u8>, DecodeError>;

    /// zlib data, for the object streams and cross-reference streams a rewrite packs.
    fn flate(&self, data: &[u8]) -> Vec<u8>;
}
