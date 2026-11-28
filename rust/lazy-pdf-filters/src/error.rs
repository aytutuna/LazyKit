use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Damaged,
    Unsupported,
    /// The output or a table would pass the limit the caller gave.
    TooBig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterError {
    pub problem: Problem,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, FilterError>;

impl FilterError {
    pub fn damaged(message: impl Into<String>) -> Self {
        Self {
            problem: Problem::Damaged,
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            problem: Problem::Unsupported,
            message: message.into(),
        }
    }

    pub fn too_big() -> Self {
        Self {
            problem: Problem::TooBig,
            message: "A stream expands past the size limit.".into(),
        }
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FilterError {}
