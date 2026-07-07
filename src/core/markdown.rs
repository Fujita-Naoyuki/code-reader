use std::{collections::HashMap, path::PathBuf};

use markdown::{
    ParseOptions,
    mdast::{Node, ReferenceKind},
    unist::{Point, Position},
};

#[derive(Debug, Clone)]
pub struct MarkdownDocument {
    pub path: PathBuf,
    pub source: String,
    pub ast: Node,
    pub pages: Vec<MarkdownPage>,
    pub headings: Vec<MarkdownHeading>,
    pub links: Vec<MarkdownLink>,
    pub rendered_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPage {
    pub id: String,
    pub title: String,
    pub depth: u8,
    pub source_range: MarkdownSourceRange,
    pub rendered_text_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownHeading {
    pub level: u8,
    pub title: String,
    pub slug: String,
    pub source_range: MarkdownSourceRange,
    pub page_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLink {
    pub text: String,
    pub url: String,
    pub title: Option<String>,
    pub source_range: MarkdownSourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownSourceRange {
    pub start: MarkdownSourcePoint,
    pub end: MarkdownSourcePoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownSourcePoint {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl MarkdownDocument {
    pub fn parse(path: PathBuf, source: String) -> Result<Self, String> {
        let ast = markdown::to_mdast(&source, &ParseOptions::gfm())
            .map_err(|message| message.to_string())?;
        let mut headings = Vec::new();
        let mut links = Vec::new();
        collect_markdown_metadata(&ast, &mut headings, &mut links);
        let mut pages = build_pages(&source, &ast, &headings);
        let rendered_text = assign_rendered_text_ranges(&source, &mut pages);
        let headings = headings_with_page_ids(&headings, &pages);

        Ok(Self {
            path,
            source,
            ast,
            pages,
            headings,
            links,
            rendered_text,
        })
    }

    pub fn page_by_id(&self, page_id: &str) -> Option<&MarkdownPage> {
        self.pages.iter().find(|page| page.id == page_id)
    }

    pub fn page_index_by_id(&self, page_id: &str) -> Option<usize> {
        self.pages.iter().position(|page| page.id == page_id)
    }

    pub fn active_or_first_page_index(&self, page_id: Option<&str>) -> Option<usize> {
        page_id
            .and_then(|page_id| self.page_index_by_id(page_id))
            .or_else(|| (!self.pages.is_empty()).then_some(0))
    }

    pub fn active_or_first_page(&self, page_id: Option<&str>) -> Option<&MarkdownPage> {
        self.active_or_first_page_index(page_id)
            .and_then(|index| self.pages.get(index))
    }

    pub fn page_rendered_text(&self, page: &MarkdownPage) -> &str {
        self.rendered_text
            .get(page.rendered_text_range.clone())
            .unwrap_or_default()
    }

    pub fn page_source(&self, page: &MarkdownPage) -> &str {
        source_slice(&self.source, &page.source_range)
    }

    pub fn source_text_between(&self, start_offset: usize, end_offset: usize) -> &str {
        source_slice_between(&self.source, start_offset, end_offset)
    }

    pub fn rendered_text_for_source(source: &str) -> String {
        markdown::to_mdast(source, &ParseOptions::gfm())
            .ok()
            .map(|ast| rendered_text_for_node(&ast))
            .unwrap_or_default()
    }

    pub fn page_outline<'a>(&'a self, page_id: &str) -> Vec<&'a MarkdownHeading> {
        self.headings
            .iter()
            .filter(|heading| {
                heading.page_id.as_deref() == Some(page_id) && (3..=6).contains(&heading.level)
            })
            .collect()
    }
}

fn collect_markdown_metadata(
    node: &Node,
    headings: &mut Vec<MarkdownHeading>,
    links: &mut Vec<MarkdownLink>,
) {
    match node {
        Node::Heading(heading) => {
            if let Some(source_range) = position_to_range(node.position()) {
                headings.push(MarkdownHeading {
                    level: heading.depth,
                    title: rendered_text_for_children(&heading.children),
                    slug: String::new(),
                    source_range,
                    page_id: None,
                });
            }
        }
        Node::Link(link) => {
            if let Some(source_range) = position_to_range(node.position()) {
                links.push(MarkdownLink {
                    text: rendered_text_for_children(&link.children),
                    url: link.url.clone(),
                    title: link.title.clone(),
                    source_range,
                });
            }
        }
        Node::LinkReference(link) => {
            if let Some(source_range) = position_to_range(node.position()) {
                let url = match link.reference_kind {
                    ReferenceKind::Collapsed | ReferenceKind::Shortcut => link.identifier.clone(),
                    ReferenceKind::Full => link.identifier.clone(),
                };
                links.push(MarkdownLink {
                    text: rendered_text_for_children(&link.children),
                    url,
                    title: None,
                    source_range,
                });
            }
        }
        _ => {}
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_markdown_metadata(child, headings, links);
        }
    }
}

fn build_pages(source: &str, ast: &Node, headings: &[MarkdownHeading]) -> Vec<MarkdownPage> {
    let boundary_headings = headings
        .iter()
        .filter(|heading| heading.level == 1 || heading.level == 2)
        .cloned()
        .collect::<Vec<_>>();
    let document_end = ast
        .position()
        .map(|position| source_point_from_point(&position.end))
        .unwrap_or_else(|| source_point_at_offset(source, source.len()));
    let mut pages = Vec::new();
    let mut slug_counts = HashMap::new();

    if let Some(first_heading) = boundary_headings.first() {
        let pre_heading = &source[..first_heading.source_range.start.offset.min(source.len())];
        if !pre_heading.trim().is_empty() {
            let id = unique_slug("Introduction", &mut slug_counts);
            pages.push(MarkdownPage {
                id,
                title: "Introduction".to_owned(),
                depth: 1,
                source_range: MarkdownSourceRange {
                    start: source_point_at_offset(source, 0),
                    end: first_heading.source_range.start,
                },
                rendered_text_range: 0..0,
            });
        }
    } else if !source.trim().is_empty() {
        let id = unique_slug("Introduction", &mut slug_counts);
        pages.push(MarkdownPage {
            id,
            title: "Introduction".to_owned(),
            depth: 1,
            source_range: MarkdownSourceRange {
                start: source_point_at_offset(source, 0),
                end: document_end,
            },
            rendered_text_range: 0..0,
        });
    }

    for (index, heading) in boundary_headings.iter().enumerate() {
        let next_start = boundary_headings
            .get(index + 1)
            .map(|next| next.source_range.start)
            .unwrap_or(document_end);
        let id = unique_slug(&heading.title, &mut slug_counts);
        pages.push(MarkdownPage {
            id,
            title: heading.title.clone(),
            depth: heading.level,
            source_range: MarkdownSourceRange {
                start: heading.source_range.start,
                end: next_start,
            },
            rendered_text_range: 0..0,
        });
    }

    pages
}

fn assign_rendered_text_ranges(source: &str, pages: &mut [MarkdownPage]) -> String {
    let mut rendered_text = String::new();

    for page in pages {
        let page_source = source_slice(source, &page.source_range);
        let page_text = markdown::to_mdast(page_source, &ParseOptions::gfm())
            .ok()
            .map(|ast| rendered_text_for_node(&ast))
            .unwrap_or_default();

        if !rendered_text.is_empty() && !page_text.is_empty() {
            rendered_text.push('\n');
        }
        let start = rendered_text.len();
        rendered_text.push_str(&page_text);
        let end = rendered_text.len();
        page.rendered_text_range = start..end;
    }

    rendered_text
}

pub fn headings_with_page_ids(
    headings: &[MarkdownHeading],
    pages: &[MarkdownPage],
) -> Vec<MarkdownHeading> {
    headings
        .iter()
        .map(|heading| {
            let mut heading = heading.clone();
            heading.slug = unique_heading_slug(&heading.title, heading.level);
            heading.page_id = pages
                .iter()
                .find(|page| {
                    page.source_range.start.offset <= heading.source_range.start.offset
                        && heading.source_range.start.offset < page.source_range.end.offset
                })
                .map(|page| page.id.clone());
            heading
        })
        .collect()
}

fn rendered_text_for_node(node: &Node) -> String {
    match node {
        Node::Text(text) => text.value.clone(),
        Node::InlineCode(code) => code.value.clone(),
        Node::Code(code) => code.value.clone(),
        Node::InlineMath(math) => math.value.clone(),
        Node::Math(math) => math.value.clone(),
        Node::Html(html) => html.value.clone(),
        Node::Break(_) => "\n".to_owned(),
        Node::Image(image) => image.alt.clone(),
        Node::ImageReference(image) => image.alt.clone(),
        _ => node
            .children()
            .map(|children| rendered_text_for_children(children))
            .unwrap_or_default(),
    }
}

fn rendered_text_for_children(children: &[Node]) -> String {
    let mut text = String::new();

    for child in children {
        let child_text = rendered_text_for_node(child);
        if !text.is_empty() && needs_text_separator(&text, &child_text) {
            text.push(' ');
        }
        text.push_str(&child_text);
    }

    text
}

fn needs_text_separator(left: &str, right: &str) -> bool {
    left.chars()
        .last()
        .is_some_and(|character| !character.is_whitespace())
        && right
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
}

fn position_to_range(position: Option<&Position>) -> Option<MarkdownSourceRange> {
    let position = position?;
    Some(MarkdownSourceRange {
        start: source_point_from_point(&position.start),
        end: source_point_from_point(&position.end),
    })
}

fn source_point_from_point(point: &Point) -> MarkdownSourcePoint {
    MarkdownSourcePoint {
        line: point.line.saturating_sub(1),
        column: point.column.saturating_sub(1),
        offset: point.offset,
    }
}

fn source_point_at_offset(source: &str, offset: usize) -> MarkdownSourcePoint {
    let offset = offset.min(source.len());
    let mut line = 0;
    let mut column = 0;

    for (index, character) in source.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }

    MarkdownSourcePoint {
        line,
        column,
        offset,
    }
}

fn source_slice<'a>(source: &'a str, range: &MarkdownSourceRange) -> &'a str {
    source_slice_between(source, range.start.offset, range.end.offset)
}

fn source_slice_between(source: &str, start_offset: usize, end_offset: usize) -> &str {
    let start = clamp_to_char_boundary(source, start_offset.min(source.len()));
    let end = clamp_to_char_boundary(source, end_offset.min(source.len()));
    &source[start.min(end)..end.max(start)]
}

fn clamp_to_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn unique_slug(title: &str, counts: &mut HashMap<String, usize>) -> String {
    let base = slugify(title);
    let count = counts.entry(base.clone()).or_insert(0);
    *count += 1;

    if *count == 1 {
        base
    } else {
        format!("{base}-{}", *count)
    }
}

fn unique_heading_slug(title: &str, level: u8) -> String {
    format!("h{level}-{}", slugify(title))
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for character in title.chars() {
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

    if slug.is_empty() {
        "section".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_document_pages_headings_and_links() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "Intro text with [link](other.md).\n\n# Setup\n\n### Install\n\nRun `cargo`.\n\n## Usage\n\n![Alt](image.png)\n"
                .to_owned(),
        )
        .unwrap();
        let headings = headings_with_page_ids(&document.headings, &document.pages);

        assert_eq!(document.path, PathBuf::from("guide.md"));
        assert!(matches!(document.ast, Node::Root(_)));
        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Introduction", "Setup", "Usage"]
        );
        assert_eq!(document.pages[1].id, "setup");
        assert_eq!(document.pages[2].depth, 2);
        assert!(headings.iter().any(|heading| {
            heading.title == "Install" && heading.page_id.as_deref() == Some("setup")
        }));
        assert_eq!(document.links[0].text, "link");
        assert_eq!(document.links[0].url, "other.md");
        assert!(document.rendered_text.contains("Intro text"));
        assert!(document.rendered_text.contains("Alt"));
    }

    #[test]
    fn duplicate_page_slugs_are_disambiguated() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# Same\n\n## Same\n\n# Same\n".to_owned(),
        )
        .unwrap();

        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.id.as_str())
                .collect::<Vec<_>>(),
            vec!["same", "same-2", "same-3"]
        );
    }

    #[test]
    fn empty_pre_heading_content_does_not_create_introduction() {
        let document =
            MarkdownDocument::parse("guide.md".into(), "\n\n# Start\n".to_owned()).unwrap();

        assert_eq!(document.pages.len(), 1);
        assert_eq!(document.pages[0].title, "Start");
    }

    #[test]
    fn setext_h1_and_h2_create_pages() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "Setext One\n==========\n\nBody\n\nSetext Two\n----------\n\nMore\n".to_owned(),
        )
        .unwrap();

        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| (page.title.as_str(), page.depth))
                .collect::<Vec<_>>(),
            vec![("Setext One", 1), ("Setext Two", 2)]
        );
    }

    #[test]
    fn headings_inside_fenced_code_blocks_do_not_create_pages() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# Real\n\n```md\n# Not a page\n## Also not a page\n```\n\n## Next\n".to_owned(),
        )
        .unwrap();

        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Real", "Next"]
        );
    }

    #[test]
    fn japanese_page_titles_get_url_safe_slugs() {
        let document =
            MarkdownDocument::parse("guide.md".into(), "# はじめに\n\n## はじめに\n".to_owned())
                .unwrap();

        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.id.as_str())
                .collect::<Vec<_>>(),
            vec!["u306fu3058u3081u306b", "u306fu3058u3081u306b-2"]
        );
    }

    #[test]
    fn page_source_ranges_cover_until_next_page_boundary() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText\n\n## Second\n\nMore\n".to_owned(),
        )
        .unwrap();

        assert_eq!(document.pages[0].source_range.start.line, 0);
        assert_eq!(document.pages[0].source_range.end.line, 4);
        assert_eq!(document.pages[1].source_range.start.line, 4);
    }

    #[test]
    fn exposes_active_page_rendered_text_source_and_outline() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText\n\n### Detail\n\nMore\n\n## Second\n\nNext\n".to_owned(),
        )
        .unwrap();
        let page = document.active_or_first_page(Some("first")).unwrap();

        assert_eq!(page.title, "First");
        assert!(document.page_rendered_text(page).contains("Detail"));
        assert!(document.page_source(page).contains("### Detail"));
        assert_eq!(
            MarkdownDocument::rendered_text_for_source("Before `code`").trim(),
            "Before code"
        );
        assert!(
            document
                .source_text_between(
                    page.source_range.start.offset,
                    document.page_outline("first")[0].source_range.start.offset,
                )
                .contains("Text")
        );
        assert_eq!(
            document
                .page_outline("first")
                .iter()
                .map(|heading| heading.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Detail"]
        );
    }

    #[test]
    fn page_source_with_non_ascii_does_not_leak_next_page() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "## Mode Toggle\n\n- H3–H6 headings\n\n## 12. Document Links\n\nNext\n".to_owned(),
        )
        .unwrap();
        let page = document.page_by_id("mode-toggle").unwrap();

        assert!(document.page_source(page).contains("H3–H6"));
        assert!(!document.page_source(page).contains("12. Document Links"));
    }
}
