//! Editor text storage and position math, independent of egui.
//!
//! `EditorTextBuffer` keeps the text in chunks with a line index so large files can be
//! edited and laid out line by line. Positions are LSP-style (line, byte column).

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
};

use crate::core::location::Position;

pub(crate) const EDITOR_TEXT_CHUNK_TARGET_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LineInfo {
    pub(crate) start: usize,
    pub(crate) leading_end: usize,
    pub(crate) content_end: usize,
    pub(crate) end: usize,
    pub(crate) is_blank: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LineIndex {
    pub(crate) source_len: usize,
    pub(crate) source_hash: u64,
    pub(crate) lines: Vec<LineInfo>,
    pub(crate) longest_line_chars: usize,
}

impl LineIndex {
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn line(&self, index: usize) -> Option<LineInfo> {
        self.lines.get(index).copied()
    }
}

pub(crate) struct EditorTextBuffer {
    pub(crate) text: String,
    pub(crate) chunks: Vec<TextChunk>,
    pub(crate) text_hash: u64,
    pub(crate) line_index: Rc<LineIndex>,
}

pub(crate) struct TextChunk {
    pub(crate) start: usize,
    pub(crate) char_start: usize,
    pub(crate) text: String,
}

impl TextChunk {
    pub(crate) fn end(&self) -> usize {
        self.start + self.text.len()
    }

    pub(crate) fn char_end(&self) -> usize {
        self.char_start + self.text.chars().count()
    }
}

impl EditorTextBuffer {
    pub(crate) fn new(text: String) -> Self {
        let text_hash = stable_hash(&text);
        let chunks = build_text_chunks(&text);
        let line_index = Rc::new(build_line_index_from_chunks(&chunks, text.len(), text_hash));
        Self {
            text,
            chunks,
            text_hash,
            line_index,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }

    pub(crate) fn fingerprint(&self) -> (usize, u64) {
        (self.text.len(), self.text_hash)
    }

    pub(crate) fn line_index(&self) -> Rc<LineIndex> {
        self.line_index.clone()
    }

    pub(crate) fn line_text(&self, line: LineInfo) -> Option<&str> {
        let chunk = self.chunk_for_range(line.start, line.content_end)?;
        let start = line.start - chunk.start;
        let end = line.content_end - chunk.start;
        chunk.text.get(start..end)
    }

    pub(crate) fn line_text_or_empty(&self, line: LineInfo) -> &str {
        self.line_text(line).unwrap_or_else(|| {
            self.text
                .get(line.start..line.content_end)
                .unwrap_or_default()
        })
    }

    pub(crate) fn char_index_to_byte_index(&self, char_index: usize) -> Option<usize> {
        if char_index == self.char_len() {
            return Some(self.text.len());
        }

        let chunk = self.chunk_for_char_index(char_index)?;
        let local_char_index = char_index - chunk.char_start;
        chunk
            .text
            .char_indices()
            .nth(local_char_index)
            .map(|(index, _)| chunk.start + index)
    }

    pub(crate) fn char_index_to_position(&self, char_index: usize) -> Option<Position> {
        let byte_index = self.char_index_to_byte_index(char_index)?;
        self.byte_index_to_position(byte_index)
    }

    pub(crate) fn byte_index_to_position(&self, byte_index: usize) -> Option<Position> {
        let byte_index = clamp_to_char_boundary(self.as_str(), byte_index);
        let line_index = self
            .line_index
            .lines
            .partition_point(|line| line.end <= byte_index)
            .min(self.line_index.lines.len().saturating_sub(1));
        let line = self.line_index.line(line_index)?;
        let character = byte_index.saturating_sub(line.start) as u32;
        Some(Position::new(line_index as u32, character))
    }

    pub(crate) fn position_to_char_index(&self, position: Position) -> Option<usize> {
        let line = self.line_index.line(position.line as usize)?;
        let target_byte = (line.start + position.character as usize).min(line.content_end);
        let target_byte = clamp_to_char_boundary(self.as_str(), target_byte);
        let chunk = self.chunk_for_byte_index(target_byte)?;
        let local_byte = target_byte - chunk.start;
        Some(chunk.char_start + chunk.text[..local_byte].chars().count())
    }

    pub(crate) fn next_char_at(&self, char_index: usize) -> Option<char> {
        let byte_index = self.char_index_to_byte_index(char_index)?;
        self.as_str()[byte_index..].chars().next()
    }

    pub(crate) fn replace(&mut self, text: String) {
        self.text = text;
        self.refresh_metadata();
    }

    pub(crate) fn insert_str(&mut self, byte_index: usize, text: &str) {
        self.text.insert_str(byte_index, text);
        self.refresh_metadata();
    }

    pub(crate) fn replace_byte_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        self.text.replace_range(range, replacement);
        self.refresh_metadata();
    }

    pub(crate) fn refresh_metadata(&mut self) {
        self.text_hash = stable_hash(&self.text);
        self.chunks = build_text_chunks(&self.text);
        self.line_index = Rc::new(build_line_index_from_chunks(
            &self.chunks,
            self.text.len(),
            self.text_hash,
        ));
    }

    pub(crate) fn chunk_for_range(&self, start: usize, end: usize) -> Option<&TextChunk> {
        let index = self
            .chunks
            .partition_point(|chunk| chunk.end() <= start)
            .min(self.chunks.len());
        let chunk = self.chunks.get(index)?;
        (start >= chunk.start && end <= chunk.end()).then_some(chunk)
    }

    pub(crate) fn chunk_for_byte_index(&self, byte_index: usize) -> Option<&TextChunk> {
        if byte_index == self.text.len() {
            return self.chunks.last();
        }

        let index = self
            .chunks
            .partition_point(|chunk| chunk.end() <= byte_index)
            .min(self.chunks.len());
        self.chunks.get(index)
    }

    pub(crate) fn chunk_for_char_index(&self, char_index: usize) -> Option<&TextChunk> {
        let index = self
            .chunks
            .partition_point(|chunk| chunk.char_end() <= char_index)
            .min(self.chunks.len());
        self.chunks.get(index)
    }

    pub(crate) fn char_len(&self) -> usize {
        self.chunks.last().map_or(0, TextChunk::char_end)
    }
}

impl std::ops::Deref for EditorTextBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

pub(crate) fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn build_text_chunks(source: &str) -> Vec<TextChunk> {
    if source.is_empty() {
        return vec![TextChunk {
            start: 0,
            char_start: 0,
            text: String::new(),
        }];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_start = 0;
    let mut current_char_start = 0;
    let mut cursor = 0;
    let mut char_cursor = 0;

    for line in source.split_inclusive('\n') {
        if current.is_empty() {
            current_start = cursor;
            current_char_start = char_cursor;
        }
        if !current.is_empty() && current.len() + line.len() > EDITOR_TEXT_CHUNK_TARGET_BYTES {
            chunks.push(TextChunk {
                start: current_start,
                char_start: current_char_start,
                text: std::mem::take(&mut current),
            });
            current_start = cursor;
            current_char_start = char_cursor;
        }

        current.push_str(line);
        cursor += line.len();
        char_cursor += line.chars().count();

        if current.len() >= EDITOR_TEXT_CHUNK_TARGET_BYTES {
            chunks.push(TextChunk {
                start: current_start,
                char_start: current_char_start,
                text: std::mem::take(&mut current),
            });
        }
    }

    if !current.is_empty() {
        chunks.push(TextChunk {
            start: current_start,
            char_start: current_char_start,
            text: current,
        });
    }

    chunks
}

#[cfg(test)]
pub(crate) fn build_line_index(source: &str, source_len: usize, source_hash: u64) -> LineIndex {
    let chunks = build_text_chunks(source);
    build_line_index_from_chunks(&chunks, source_len, source_hash)
}

pub(crate) fn build_line_index_from_chunks(
    chunks: &[TextChunk],
    source_len: usize,
    source_hash: u64,
) -> LineIndex {
    let mut lines = Vec::new();
    let mut longest_line_chars = 0;
    let mut last_end = 0;

    for chunk in chunks {
        let mut line_start = chunk.start;
        for line in chunk.text.split_inclusive('\n') {
            let end = line_start + line.len();
            let content_end = end
                - line
                    .as_bytes()
                    .iter()
                    .rev()
                    .take_while(|byte| matches!(byte, b'\n' | b'\r'))
                    .count();
            let char_count = line.trim_end_matches(['\r', '\n']).chars().count();
            longest_line_chars = longest_line_chars.max(char_count);
            let leading_len = line
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            let leading_end = (line_start + leading_len).min(content_end);
            let is_blank = line[..content_end.saturating_sub(line_start)]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
            lines.push(LineInfo {
                start: line_start,
                leading_end,
                content_end,
                end,
                is_blank,
            });
            line_start = end;
            last_end = end;
        }
    }

    if lines.is_empty() || last_end < source_len {
        let start = last_end;
        let trailing = chunks
            .last()
            .and_then(|chunk| chunk.text.get(start.saturating_sub(chunk.start)..))
            .unwrap_or_default();
        let content_end = source_len
            - trailing
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| matches!(byte, b'\n' | b'\r'))
                .count();
        let char_count = trailing.trim_end_matches(['\r', '\n']).chars().count();
        longest_line_chars = longest_line_chars.max(char_count);
        let leading_len = trailing
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let leading_end = (start + leading_len).min(content_end);
        let is_blank = trailing[..content_end.saturating_sub(start)]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
        lines.push(LineInfo {
            start,
            leading_end,
            content_end,
            end: source_len,
            is_blank,
        });
    }

    let ends_with_newline = chunks
        .last()
        .and_then(|chunk| chunk.text.as_bytes().last())
        .is_some_and(|byte| *byte == b'\n');
    if ends_with_newline {
        lines.push(LineInfo {
            start: source_len,
            leading_end: source_len,
            content_end: source_len,
            end: source_len,
            is_blank: true,
        });
    }

    LineIndex {
        source_len,
        source_hash,
        lines,
        longest_line_chars,
    }
}

pub(crate) fn next_char_boundary(source: &str, index: usize) -> usize {
    source[index..]
        .chars()
        .next()
        .map(|character| index + character.len_utf8())
        .unwrap_or(index)
        .min(source.len())
}

pub(crate) fn previous_char_boundary(source: &str, byte_index: usize) -> usize {
    let mut index = byte_index.min(source.len());
    if index == 0 {
        return 0;
    }
    index -= 1;
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn leading_indent_columns(line: &str, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let mut columns = 0;

    for character in line.chars() {
        match character {
            ' ' => columns += 1,
            '\t' => columns += tab_width - columns % tab_width,
            _ => break,
        }
    }

    columns
}

pub(crate) fn clamp_to_char_boundary(source: &str, mut index: usize) -> usize {
    index = index.min(source.len());
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn char_index_to_byte_index(source: &str, char_index: usize) -> Option<usize> {
    if char_index == source.chars().count() {
        return Some(source.len());
    }

    source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
}

#[cfg(test)]
pub(crate) fn char_index_to_position(source: &str, char_index: usize) -> Option<Position> {
    let byte_index = source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(source.len());
    let before_cursor = &source[..byte_index];
    let line = before_cursor.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let character = before_cursor[line_start..].len() as u32;

    Some(Position::new(line, character))
}

#[cfg(test)]
pub(crate) fn byte_index_to_position(source: &str, byte_index: usize) -> Option<Position> {
    let byte_index = clamp_to_char_boundary(source, byte_index);
    let before_cursor = source.get(..byte_index)?;
    let line = before_cursor.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let character = before_cursor[line_start..].len() as u32;

    Some(Position::new(line, character))
}

pub(crate) fn position_to_char_index(source: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut line_start_byte = 0;

    for (index, byte) in source.bytes().enumerate() {
        if line == position.line {
            break;
        }

        if byte == b'\n' {
            line += 1;
            line_start_byte = index + 1;
        }
    }

    if line != position.line {
        return None;
    }

    let target_byte = (line_start_byte + position.character as usize).min(source.len());
    let target_byte = clamp_to_char_boundary(source, target_byte);

    Some(source[..target_byte].chars().count())
}

pub(crate) fn closing_pair(character: char) -> Option<char> {
    match character {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        _ => None,
    }
}

pub(crate) fn is_closing_bracket(character: char) -> bool {
    matches!(character, ')' | ']' | '}' | '"')
}

pub(crate) fn smart_newline(source: &str, cursor_index: usize, tab_width: usize) -> String {
    let byte_index = char_index_to_byte_index(source, cursor_index).unwrap_or(source.len());
    let before_cursor = &source[..byte_index];
    let line_start = before_cursor
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let current_line = &before_cursor[line_start..];
    let mut indent: String = current_line
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();

    if before_cursor
        .trim_end_matches(|character: char| character.is_whitespace())
        .ends_with(['{', '(', '['])
    {
        indent.push_str(&" ".repeat(tab_width));
    }

    format!("\n{indent}")
}

pub(crate) fn strip_trailing_whitespace(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());

    for segment in source.split_inclusive('\n') {
        if let Some(line) = segment.strip_suffix("\r\n") {
            stripped.push_str(line.trim_end_matches([' ', '\t']));
            stripped.push_str("\r\n");
        } else if let Some(line) = segment.strip_suffix('\n') {
            stripped.push_str(line.trim_end_matches([' ', '\t']));
            stripped.push('\n');
        } else {
            stripped.push_str(segment.trim_end_matches([' ', '\t']));
        }
    }

    stripped
}

pub(crate) fn find_matching_bracket_pair(
    source: &str,
    position: Position,
) -> Option<(usize, usize)> {
    let cursor_index = position_to_char_index(source, position)?;
    let characters: Vec<char> = source.chars().collect();

    let candidates = [
        cursor_index.checked_sub(1),
        (cursor_index < characters.len()).then_some(cursor_index),
    ];

    for candidate in candidates.into_iter().flatten() {
        let character = characters[candidate];
        if let Some(close) = closing_pair(character).filter(|close| *close != character)
            && let Some(match_index) =
                find_matching_forward(&characters, candidate, character, close)
        {
            return Some((
                char_index_to_byte_index(source, candidate)?,
                char_index_to_byte_index(source, match_index)?,
            ));
        }

        if let Some(open) = opening_pair(character)
            && let Some(match_index) =
                find_matching_backward(&characters, candidate, open, character)
        {
            return Some((
                char_index_to_byte_index(source, match_index)?,
                char_index_to_byte_index(source, candidate)?,
            ));
        }
    }

    None
}

pub(crate) fn opening_pair(character: char) -> Option<char> {
    match character {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

pub(crate) fn find_matching_forward(
    characters: &[char],
    start: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0;

    for (index, character) in characters.iter().enumerate().skip(start) {
        if *character == open {
            depth += 1;
        } else if *character == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

pub(crate) fn find_matching_backward(
    characters: &[char],
    start: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0;

    for (index, character) in characters.iter().enumerate().take(start + 1).rev() {
        if *character == close {
            depth += 1;
        } else if *character == open {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_newline_keeps_indent_and_adds_one_level_after_open_brace() {
        let source = "fn main() {\n    if ready {";
        let cursor = source.chars().count();

        assert_eq!(smart_newline(source, cursor, 4), "\n        ");
    }

    #[test]
    fn strip_trailing_whitespace_preserves_newlines() {
        let source = "let a = 1;   \r\nlet b = 2;\t\nlet c = 3;  ";

        assert_eq!(
            strip_trailing_whitespace(source),
            "let a = 1;\r\nlet b = 2;\nlet c = 3;"
        );
    }

    #[test]
    fn leading_indent_columns_expands_tabs_to_tab_stops() {
        assert_eq!(leading_indent_columns("    value", 4), 4);
        assert_eq!(leading_indent_columns("\tvalue", 4), 4);
        assert_eq!(leading_indent_columns("  \tvalue", 4), 4);
        assert_eq!(leading_indent_columns("    \tvalue", 4), 8);
        assert_eq!(leading_indent_columns("value", 4), 0);
    }

    #[test]
    fn byte_index_to_position_uses_line_and_byte_column() {
        let source = "abc\nあいう\nxyz";
        let byte_index = source.find("う").unwrap();

        assert_eq!(
            byte_index_to_position(source, byte_index),
            Some(Position::new(1, 6))
        );
    }

    #[test]
    fn chunked_text_buffer_preserves_line_index() {
        let source = format!(
            "{}\n{}\n{}",
            "a".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2),
            "b".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2),
            "終"
        );
        let buffer = EditorTextBuffer::new(source.clone());
        let line_index = buffer.line_index();

        assert!(buffer.chunks.len() >= 2);
        assert_eq!(line_index.line_count(), 3);
        let second_line = line_index.line(1).unwrap();
        assert_eq!(second_line.leading_end, second_line.start);
        assert!(!second_line.is_blank);
        assert_eq!(
            buffer.line_text(line_index.line(0).unwrap()),
            Some(source.lines().next().unwrap())
        );
        assert_eq!(buffer.line_text(line_index.line(2).unwrap()), Some("終"));
    }

    #[test]
    fn line_index_tracks_leading_indent_and_blank_lines() {
        let source = "fn main() {\n    value\n\t  \n}\n";
        let line_index = build_line_index(source, source.len(), stable_hash(source));

        let indented_line = line_index.line(1).unwrap();
        assert_eq!(indented_line.leading_end - indented_line.start, 4);
        assert!(!indented_line.is_blank);

        let blank_line = line_index.line(2).unwrap();
        assert_eq!(blank_line.leading_end, blank_line.content_end);
        assert!(blank_line.is_blank);
    }

    #[test]
    fn chunked_text_buffer_converts_positions_across_chunks() {
        let first = "a".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2);
        let second = "β".repeat(EDITOR_TEXT_CHUNK_TARGET_BYTES / 2);
        let source = format!("{first}\n{second}\nlast");
        let buffer = EditorTextBuffer::new(source);

        let position = Position::new(1, 4);
        let char_index = buffer.position_to_char_index(position).unwrap();
        let byte_index = buffer.char_index_to_byte_index(char_index).unwrap();

        assert_eq!(buffer.char_index_to_position(char_index), Some(position));
        assert_eq!(buffer.byte_index_to_position(byte_index), Some(position));
        assert_eq!(buffer.next_char_at(char_index), Some('β'));
    }

    #[test]
    fn find_matching_bracket_pair_finds_pair_next_to_cursor() {
        let source = "call(value[0])";
        let position = char_index_to_position(source, 5).unwrap();
        let pair = find_matching_bracket_pair(source, position).unwrap();

        assert_eq!(&source[pair.0..=pair.0], "(");
        assert_eq!(&source[pair.1..=pair.1], ")");
    }
}
