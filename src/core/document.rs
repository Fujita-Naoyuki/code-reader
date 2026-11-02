//! A file's text with the identifier lookup the language support needs.

use std::path::PathBuf;

use crate::core::location::Position;

#[derive(Debug, Clone)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
}

impl Document {
    pub fn new(path: PathBuf, text: String) -> Self {
        Self { path, text }
    }

    pub fn line_text(&self, line: u32) -> Option<&str> {
        self.text.lines().nth(line as usize)
    }

    pub fn identifier_at(&self, position: Position) -> Option<String> {
        let line = self.line_text(position.line)?;
        let bytes = line.as_bytes();
        let mut start = position.character as usize;
        let mut end = start;

        if start > bytes.len() {
            return None;
        }

        while start > 0 && is_identifier_byte(bytes[start - 1]) {
            start -= 1;
        }

        while end < bytes.len() && is_identifier_byte(bytes[end]) {
            end += 1;
        }

        if start == end {
            return None;
        }

        Some(line[start..end].to_owned())
    }
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
