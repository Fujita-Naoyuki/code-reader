//! Footnotes and link definitions across the whole document. Pages are rendered one
//! fragment at a time, so definitions are collected from the full AST and handed to each
//! fragment (D12).

use markdown::mdast::Node;

use super::MarkdownDocument;

pub(crate) fn markdown_page_body_start_offset(
    document: &MarkdownDocument,
    page: &crate::core::markdown::MarkdownPage,
) -> usize {
    document
        .headings
        .iter()
        .find(|heading| {
            (heading.level == 1 || heading.level == 2)
                && heading.source_range.start.offset == page.source_range.start.offset
        })
        .map(|heading| heading.source_range.end.offset)
        .unwrap_or(page.source_range.start.offset)
}

pub(crate) fn markdown_footnote_number(
    document: &MarkdownDocument,
    identifier: &str,
) -> Option<usize> {
    markdown_footnote_definitions(document)
        .iter()
        .position(|definition| definition.identifier == identifier)
        .map(|index| index + 1)
}

/// Footnotes listed under a page: those defined on it and those referenced from it, so a
/// definition gathered at the end of the file still shows next to its reference.
pub(crate) fn markdown_footnote_definitions_for_page<'a>(
    document: &'a MarkdownDocument,
    page: &crate::core::markdown::MarkdownPage,
) -> Vec<&'a markdown::mdast::FootnoteDefinition> {
    let in_page = |position: Option<&markdown::unist::Position>| {
        position.is_some_and(|position| {
            page.source_range.start.offset <= position.start.offset
                && position.start.offset < page.source_range.end.offset
        })
    };
    let mut referenced = Vec::new();
    visit_markdown_nodes(&document.ast, &mut |node| {
        if let Node::FootnoteReference(reference) = node
            && in_page(reference.position.as_ref())
        {
            referenced.push(reference.identifier.clone());
        }
    });
    markdown_footnote_definitions(document)
        .into_iter()
        .filter(|definition| {
            in_page(definition.position.as_ref()) || referenced.contains(&definition.identifier)
        })
        .collect()
}

pub(crate) fn visit_markdown_nodes<'a>(node: &'a Node, visit: &mut impl FnMut(&'a Node)) {
    visit(node);
    if let Some(children) = node.children() {
        for child in children {
            visit_markdown_nodes(child, visit);
        }
    }
}

/// Source text of the link and footnote definitions outside `range`, one block each.
pub(crate) fn markdown_definition_sources_outside(
    document: &MarkdownDocument,
    range: std::ops::Range<usize>,
) -> String {
    let mut sources = Vec::new();
    visit_markdown_nodes(&document.ast, &mut |node| {
        let position = match node {
            Node::Definition(definition) => definition.position.as_ref(),
            Node::FootnoteDefinition(definition) => definition.position.as_ref(),
            _ => return,
        };
        if let Some(position) = position
            && !range.contains(&position.start.offset)
            && let Some(source) = document
                .source
                .get(position.start.offset..position.end.offset)
        {
            sources.push(source);
        }
    });
    sources.join("\n\n")
}

pub(crate) fn markdown_definition_url(
    document: &MarkdownDocument,
    identifier: &str,
) -> Option<String> {
    let mut url = None;
    visit_markdown_nodes(&document.ast, &mut |node| {
        if let Node::Definition(definition) = node
            && url.is_none()
            && definition.identifier == identifier
        {
            url = Some(definition.url.clone());
        }
    });
    url
}

pub(crate) fn markdown_footnote_definitions(
    document: &MarkdownDocument,
) -> Vec<&markdown::mdast::FootnoteDefinition> {
    let mut definitions = Vec::new();
    collect_markdown_footnote_definitions(&document.ast, &mut definitions);
    definitions.sort_by_key(|definition| {
        definition
            .position
            .as_ref()
            .map(|position| position.start.offset)
            .unwrap_or(usize::MAX)
    });
    definitions
}

pub(crate) fn collect_markdown_footnote_definitions<'a>(
    node: &'a Node,
    definitions: &mut Vec<&'a markdown::mdast::FootnoteDefinition>,
) {
    if let Node::FootnoteDefinition(definition) = node {
        definitions.push(definition);
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_markdown_footnote_definitions(child, definitions);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_footnotes_are_numbered_and_scoped_to_page() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText[^a].\n\n[^a]: First note.\n\n## Second\n\nText[^b].\n\n[^b]: Second note.\n"
                .to_owned(),
        )
        .unwrap();

        let first = document.page_by_id("first").unwrap();
        let second = document.page_by_id("second").unwrap();
        let first_definitions = markdown_footnote_definitions_for_page(&document, first);
        let second_definitions = markdown_footnote_definitions_for_page(&document, second);

        assert_eq!(markdown_footnote_number(&document, "a"), Some(1));
        assert_eq!(markdown_footnote_number(&document, "b"), Some(2));
        assert_eq!(first_definitions.len(), 1);
        assert_eq!(first_definitions[0].identifier, "a");
        assert_eq!(second_definitions.len(), 1);
        assert_eq!(second_definitions[0].identifier, "b");
    }

    #[test]
    fn markdown_footnotes_defined_elsewhere_are_listed_where_referenced() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText[^a] and [ref][r].\n\n## Notes\n\n[^a]: Far note.\n\n[r]: https://example.com/r\n"
                .to_owned(),
        )
        .unwrap();

        let first = document.page_by_id("first").unwrap();
        let definitions = markdown_footnote_definitions_for_page(&document, first);
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].identifier, "a");
        assert_eq!(
            markdown_definition_url(&document, "r").as_deref(),
            Some("https://example.com/r")
        );
        let outside = markdown_definition_sources_outside(&document, 0..10);
        assert!(outside.contains("[^a]: Far note."));
        assert!(outside.contains("[r]: https://example.com/r"));
    }
}
