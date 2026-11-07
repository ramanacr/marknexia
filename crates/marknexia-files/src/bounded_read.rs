//! Bounded reading of untrusted document bytes.

use std::io::Read;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundedReadError {
    TooLarge,
    Io,
}

#[derive(Clone, Copy, Debug)]
pub struct BoundedReader {
    max_bytes: usize,
}

impl BoundedReader {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }

    pub fn read_from<R: Read>(&self, mut source: R) -> Result<Vec<u8>, BoundedReadError> {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let count = source.read(&mut chunk).map_err(|_| BoundedReadError::Io)?;
            if count == 0 {
                return Ok(bytes);
            }
            let remaining = self.max_bytes.saturating_sub(bytes.len());
            if count > remaining {
                return Err(BoundedReadError::TooLarge);
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
    }
}
