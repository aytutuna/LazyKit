use crate::error::{FilterError, Result};

/// Output of a decoder: grows on demand and fails once it passes the size cap.
pub(crate) struct Output {
    bytes: Vec<u8>,
    max: u64,
}

impl Output {
    pub fn new(max: u64, capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity.clamp(256, 1 << 22)),
            max,
        }
    }

    pub fn push(&mut self, value: u8) -> Result<()> {
        self.room(1)?;
        self.bytes.push(value);
        Ok(())
    }

    pub fn extend(&mut self, values: &[u8]) -> Result<()> {
        self.room(values.len())?;
        self.bytes.extend_from_slice(values);
        Ok(())
    }

    pub fn fill(&mut self, count: usize, value: u8) -> Result<()> {
        self.room(count)?;
        self.bytes.resize(self.bytes.len() + count, value);
        Ok(())
    }

    /// Counts `count` more bytes, zeroed, and returns them for the caller to fill.
    pub fn append(&mut self, count: usize) -> Result<&mut [u8]> {
        self.room(count)?;
        let at = self.bytes.len();
        self.bytes.resize(at + count, 0);
        Ok(&mut self.bytes[at..])
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }

    fn room(&self, count: usize) -> Result<()> {
        if self.bytes.len() as u64 + count as u64 > self.max {
            return Err(FilterError::too_big());
        }
        Ok(())
    }
}
