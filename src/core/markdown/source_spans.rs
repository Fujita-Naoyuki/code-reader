//! Spans for coloring Markdown in source mode (headings, emphasis, code, links, quotes,
//! list markers), found line by line without a full parse so they stay cheap to redo
//! on every edit. Spans must not overlap: the layout drops overlapping ones (R03).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownSourceSpanKind {
    Heading,
    Strong,
    Emphasis,
    InlineCode,
    CodeBlock,
    Link,
    Blockquote,
    ListMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownSourceSpan {
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) kind: MarkdownSourceSpanKind,
}

pub(crate) fn markdown_source_spans(source: &str) -> Vec<MarkdownSourceSpan> {
    let mut spans = Vec::new();
    let mut line_start = 0;
    let mut fenced_code_marker: Option<String> = None;

    for line in source.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        let content_end = line_start + line_without_newline.len();
        let leading_spaces = line_without_newline
            .bytes()
            .take_while(|byte| *byte == b' ' || *byte == b'\t')
            .count();
        let content_start = line_start + leading_spaces;
        let trimmed = &line_without_newline[leading_spaces..];

        if let Some(marker) = &fenced_code_marker {
            spans.push(MarkdownSourceSpan {
                range: line_start..content_end,
                kind: MarkdownSourceSpanKind::CodeBlock,
            });
            if trimmed.starts_with(marker) {
                fenced_code_marker = None;
            }
            line_start = line_end;
            continue;
        }

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().take(3).collect::<String>();
            fenced_code_marker = Some(marker);
            spans.push(MarkdownSourceSpan {
                range: content_start..content_end,
                kind: MarkdownSourceSpanKind::CodeBlock,
            });
            line_start = line_end;
            continue;
        }

        if let Some(hash_count) = markdown_heading_marker_len(trimmed) {
            // Marker and text are separate spans: the layout drops overlapping ones.
            spans.push(MarkdownSourceSpan {
                range: content_start..content_start + hash_count,
                kind: MarkdownSourceSpanKind::ListMarker,
            });
            spans.push(MarkdownSourceSpan {
                range: content_start + hash_count..content_end,
                kind: MarkdownSourceSpanKind::Heading,
            });
        } else {
            if trimmed.starts_with('>') {
                spans.push(MarkdownSourceSpan {
                    range: content_start..(content_start + 1),
                    kind: MarkdownSourceSpanKind::Blockquote,
                });
            }

            if let Some(marker_len) = markdown_list_marker_len(trimmed) {
                spans.push(MarkdownSourceSpan {
                    range: content_start..content_start + marker_len,
                    kind: MarkdownSourceSpanKind::ListMarker,
                });
            }

            markdown_inline_source_spans(line_without_newline, line_start, &mut spans);
        }

        line_start = line_end;
    }

    spans
}

pub(crate) fn markdown_heading_marker_len(line: &str) -> Option<usize> {
    let count = line.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&count).then_some(count).filter(|count| {
        line.as_bytes()
            .get(*count)
            .is_some_and(u8::is_ascii_whitespace)
    })
}

pub(crate) fn markdown_list_marker_len(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1].is_ascii_whitespace()
    {
        return Some(1);
    }

    let digit_count = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count > 0
        && bytes.get(digit_count) == Some(&b'.')
        && bytes
            .get(digit_count + 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return Some(digit_count + 1);
    }

    None
}

pub(crate) fn markdown_inline_source_spans(
    line: &str,
    line_start: usize,
    spans: &mut Vec<MarkdownSourceSpan>,
) {
    markdown_pair_spans(
        line,
        line_start,
        "`",
        MarkdownSourceSpanKind::InlineCode,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "**",
        MarkdownSourceSpanKind::Strong,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "__",
        MarkdownSourceSpanKind::Strong,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "*",
        MarkdownSourceSpanKind::Emphasis,
        spans,
    );
    markdown_pair_spans(
        line,
        line_start,
        "_",
        MarkdownSourceSpanKind::Emphasis,
        spans,
    );
    markdown_link_spans(line, line_start, spans);
}

pub(crate) fn markdown_pair_spans(
    line: &str,
    line_start: usize,
    marker: &str,
    kind: MarkdownSourceSpanKind,
    spans: &mut Vec<MarkdownSourceSpan>,
) {
    let mut cursor = 0;
    while let Some(open) = line[cursor..].find(marker).map(|index| cursor + index) {
        if marker.len() == 1 && line[open..].starts_with(&marker.repeat(2)) {
            cursor = open + 2;
            continue;
        }
        let content_start = open + marker.len();
        let Some(close) = line[content_start..]
            .find(marker)
            .map(|index| content_start + index)
        else {
            break;
        };
        if close > content_start {
            spans.push(MarkdownSourceSpan {
                range: line_start + open..line_start + close + marker.len(),
                kind,
            });
        }
        cursor = close + marker.len();
    }
}

pub(crate) fn markdown_link_spans(
    line: &str,
    line_start: usize,
    spans: &mut Vec<MarkdownSourceSpan>,
) {
    let mut cursor = 0;
    while let Some(open) = line[cursor..].find('[').map(|index| cursor + index) {
        let Some(close_label) = line[open + 1..].find(']').map(|index| open + 1 + index) else {
            break;
        };
        if line.as_bytes().get(close_label + 1) != Some(&b'(') {
            cursor = close_label + 1;
            continue;
        }
        let Some(close_url) = line[close_label + 2..]
            .find(')')
            .map(|index| close_label + 2 + index)
        else {
            break;
        };
        spans.push(MarkdownSourceSpan {
            range: line_start + open..line_start + close_url + 1,
            kind: MarkdownSourceSpanKind::Link,
        });
        cursor = close_url + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_source_heading_spans_do_not_overlap_the_marker() {
        let spans = markdown_source_spans("## Title\n");
        assert_eq!(
            spans,
            vec![
                MarkdownSourceSpan {
                    range: 0..2,
                    kind: MarkdownSourceSpanKind::ListMarker,
                },
                MarkdownSourceSpan {
                    range: 2..8,
                    kind: MarkdownSourceSpanKind::Heading,
                },
            ]
        );
    }

    #[test]
    fn markdown_source_spans_detect_common_constructs() {
        let source = "# Title\n\n> quote\n\n- item with **bold** and *em*\n\n[link](next.md) and `code`\n\n```rust\nfn main() {}\n```\n";
        let spans = markdown_source_spans(source);

        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Heading)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Blockquote)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::ListMarker)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Strong)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Emphasis)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::Link)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::InlineCode)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == MarkdownSourceSpanKind::CodeBlock)
        );
    }
}
