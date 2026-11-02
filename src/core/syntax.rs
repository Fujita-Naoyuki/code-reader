//! Highlight spans shared by the language support and the editors.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

impl HighlightSpan {
    pub fn new(range: Range<usize>, kind: HighlightKind) -> Self {
        Self { range, kind }
    }
}
