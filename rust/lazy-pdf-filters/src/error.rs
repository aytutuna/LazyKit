use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Damaged,
    Unsupported,
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
        Self::unsupported("A stream expands past the size limit.")
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FilterError {}
