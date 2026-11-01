use std::io::{self, Read, Write};

/// A reader that fails once more than `limit` bytes are consumed.
pub struct BoundedReader<R> {
    inner: R,
    remaining: u64,
    limit: u64,
}

impl<R> BoundedReader<R> {
    pub fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
            limit,
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut byte = [0u8; 1];
            return match self.inner.read(&mut byte)? {
                0 => Ok(0),
                _ => Err(limit_error(self.limit)),
            };
        }
        let allowed = buffer.len().min(self.remaining as usize);
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

pub fn limit_error(limit: u64) -> io::Error {
    io::Error::other(format!("input exceeds the {limit}-byte limit"))
}

pub fn read_to_end<R: Read>(reader: R, limit: u64) -> io::Result<Vec<u8>> {
    let mut reader = BoundedReader::new(reader, limit);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn copy_to<R: Read, W: Write>(reader: R, writer: &mut W, limit: u64) -> io::Result<u64> {
    let mut reader = BoundedReader::new(reader, limit);
    io::copy(&mut reader, writer)
}

#[cfg(test)]
mod tests {
    use super::{read_to_end, BoundedReader};
    use std::io::Read;

    #[test]
    fn exact_limit_is_allowed() {
        let reader = BoundedReader::new(std::io::Cursor::new(vec![0u8; 4]), 4);
        let bytes = read_to_end(reader, 4).expect("exact limit");
        assert_eq!(bytes.len(), 4);
    }

    #[test]
    fn over_limit_is_rejected() {
        let mut reader = BoundedReader::new(std::io::Cursor::new(vec![0u8; 5]), 4);
        let mut bytes = Vec::new();
        let error = reader.read_to_end(&mut bytes).expect_err("over limit");
        assert!(error.to_string().contains("4-byte limit"));
    }
}
