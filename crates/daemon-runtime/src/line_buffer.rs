//! Incremental UTF-8 line splitting for JSONL provider protocols.

/// Buffers raw process output and yields complete lines.
#[derive(Default)]
pub(crate) struct LineBuffer {
    buffer: Vec<u8>,
}

impl LineBuffer {
    /// Create an empty buffer.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a chunk and return every complete line it contains.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=position).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() {
                lines.push(String::from_utf8_lossy(&line).to_string());
            }
        }
        lines
    }

    /// Flush any remaining bytes as a final line.
    pub(crate) fn finish(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let mut line = std::mem::take(&mut self.buffer);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            Vec::new()
        } else {
            vec![String::from_utf8_lossy(&line).to_string()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_partial_lines() {
        let mut buffer = LineBuffer::new();
        assert!(buffer.push(b"{\"a\":1").is_empty());
        assert_eq!(buffer.push(b"}\n{\"b\""), vec!["{\"a\":1}"]);
        assert_eq!(buffer.push(b":2}\r\n"), vec!["{\"b\":2}"]);
        assert!(buffer.finish().is_empty());
    }
}
