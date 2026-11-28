use std::fmt;

/// What kind of trouble a file is, for the message a person sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// No "%PDF-" header.
    NotPdf,
    /// The cross-reference data or an object is not where or what it should be.
    Damaged,
    /// The file is encrypted; opening those is the crypt layer's job.
    Encrypted,
    /// The file uses something this reader does not handle.
    Unsupported,
    /// A stream or its output passes the size limit it was read under.
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub problem: Problem,
    /// Technical, one char per byte where it quotes the file.
    pub detail: String,
    /// The stream or its output passed the size limit it was read under. The problem is still `Damaged`.
    pub too_large: bool,
}

impl ReadError {
    pub fn new(problem: Problem, detail: impl Into<String>) -> Self {
        Self {
            problem,
            detail: detail.into(),
            too_large: false,
        }
    }

    pub fn too_large(detail: impl Into<String>) -> Self {
        Self {
            too_large: true,
            ..Self::damaged(detail)
        }
    }

    pub fn damaged(detail: impl Into<String>) -> Self {
        Self::new(Problem::Damaged, detail)
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Problem::NotPdf => "NotPdf",
            Problem::Damaged => "Damaged",
            Problem::Encrypted => "Encrypted",
            Problem::Unsupported => "Unsupported",
            Problem::TooLarge => "TooLarge",
        })
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for ReadError {}

/// The bytes as text, one char each, as the reference engines keep names and keywords.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}
