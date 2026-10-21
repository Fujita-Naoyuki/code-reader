//! Resolving Markdown links: external URLs, local files, `#fragment` targets matched
//! against page and heading slugs, and local image paths.

use std::path::{Path, PathBuf};

use crate::core::location::Position;

use super::MarkdownDocument;

pub(crate) fn markdown_code_path_for_language(language: &str) -> Option<PathBuf> {
    let extension = match language.to_ascii_lowercase().as_str() {
        "rs" | "rust" => "rs",
        "go" | "golang" => "go",
        _ => return None,
    };
    Some(PathBuf::from(format!("snippet.{extension}")))
}

pub(crate) fn markdown_resolve_local_image_path(
    document: &MarkdownDocument,
    url: &str,
) -> Option<PathBuf> {
    if url.trim().is_empty()
        || url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
    {
        return None;
    }

    let path = PathBuf::from(url);
    if path.is_absolute() {
        Some(path)
    } else {
        document.path.parent().map(|parent| parent.join(path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownFragmentTarget {
    pub(crate) page_id: String,
    pub(crate) heading_id: Option<String>,
    pub(crate) position: Position,
}

pub(crate) fn is_external_markdown_link(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("data:")
        || target.starts_with("file://")
}

pub(crate) fn split_markdown_link_target(target: &str) -> (String, Option<String>) {
    let Some((path, fragment)) = target.split_once('#') else {
        return (percent_decode_lossy(target.trim()), None);
    };

    let fragment = percent_decode_lossy(fragment.trim());
    (
        percent_decode_lossy(path.trim()),
        (!fragment.is_empty()).then_some(fragment),
    )
}

pub(crate) fn resolve_markdown_link_path(
    current_document_path: &Path,
    target_path: &str,
) -> PathBuf {
    let path = PathBuf::from(target_path);
    if path.is_absolute() {
        path
    } else {
        current_document_path
            .parent()
            .map(|parent| parent.join(&path))
            .unwrap_or(path)
    }
}

pub(crate) fn markdown_fragment_target(
    document: &MarkdownDocument,
    fragment: &str,
) -> Option<MarkdownFragmentTarget> {
    let fragment = markdown_normalize_fragment(fragment);
    if fragment.is_empty() {
        return None;
    }

    if let Some(page) = document.pages.iter().find(|page| page.id == fragment) {
        return Some(MarkdownFragmentTarget {
            page_id: page.id.clone(),
            heading_id: None,
            position: Position::new(
                page.source_range.start.line as u32,
                page.source_range.start.column as u32,
            ),
        });
    }

    document.headings.iter().find_map(|heading| {
        if !markdown_heading_matches_fragment(heading, &fragment) {
            return None;
        }
        let page_id = heading.page_id.clone().or_else(|| {
            document
                .pages
                .iter()
                .find(|page| {
                    page.source_range.start.offset <= heading.source_range.start.offset
                        && heading.source_range.start.offset <= page.source_range.end.offset
                })
                .map(|page| page.id.clone())
        })?;

        Some(MarkdownFragmentTarget {
            page_id,
            heading_id: Some(heading.slug.clone()),
            position: Position::new(
                heading.source_range.start.line as u32,
                heading.source_range.start.column as u32,
            ),
        })
    })
}

pub(crate) fn markdown_heading_matches_fragment(
    heading: &crate::core::markdown::MarkdownHeading,
    fragment: &str,
) -> bool {
    if heading.slug == fragment {
        return true;
    }

    let heading_prefix = format!("h{}-", heading.level);
    heading
        .slug
        .strip_prefix(&heading_prefix)
        .is_some_and(|slug| slug == fragment)
}

pub(crate) fn markdown_normalize_fragment(fragment: &str) -> String {
    let fragment = percent_decode_lossy(fragment.trim().trim_start_matches('#'));
    let mut slug = String::new();
    let mut previous_dash = false;

    for character in fragment.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.extend(character.to_lowercase());
            previous_dash = false;
        } else if !character.is_ascii() && !character.is_whitespace() {
            slug.push_str(&format!("u{:x}", character as u32));
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    slug
}

pub(crate) fn percent_decode_lossy(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_link_target_splits_path_and_decodes_fragment() {
        let (path, fragment) = split_markdown_link_target("docs/My%20Page.md#Preview%20Mode");

        assert_eq!(path, "docs/My Page.md");
        assert_eq!(fragment.as_deref(), Some("Preview Mode"));
    }

    #[test]
    fn markdown_fragment_target_matches_page_and_heading_slugs() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# Introduction\n\nText\n\n## Mode Toggle\n\n### Preview Mode\n\nBody\n".to_owned(),
        )
        .unwrap();

        let page = markdown_fragment_target(&document, "mode-toggle").unwrap();
        assert_eq!(page.page_id, "mode-toggle");
        assert_eq!(page.heading_id, None);
        assert_eq!(page.position, Position::new(4, 0));

        let heading = markdown_fragment_target(&document, "preview-mode").unwrap();
        assert_eq!(heading.page_id, "mode-toggle");
        assert_eq!(heading.heading_id.as_deref(), Some("h3-preview-mode"));
        assert_eq!(heading.position, Position::new(6, 0));
    }

    #[test]
    fn markdown_fragment_target_matches_non_ascii_heading() {
        let document =
            MarkdownDocument::parse("guide.md".into(), "# はじめに\n\n### 詳細\n".to_owned())
                .unwrap();

        let target = markdown_fragment_target(&document, "詳細").unwrap();

        assert_eq!(target.page_id, "u306fu3058u3081u306b");
        assert_eq!(target.heading_id.as_deref(), Some("h3-u8a73u7d30"));
    }
}
