//! Rust: tree-sitter symbols and highlighting. Definitions come from rust-analyzer.

use std::path::Path;

use crate::core::{
    document::Document,
    language::{Identifier, LanguageId, LanguageSupport},
    location::{Location, Position, Range},
    symbol::{Symbol, SymbolKind, SymbolSource},
    syntax::{HighlightKind, HighlightSpan},
};

pub struct RustLanguage;

impl LanguageSupport for RustLanguage {
    fn id(&self) -> LanguageId {
        LanguageId("rust")
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn extract_symbols(&self, document: &Document) -> Vec<Symbol> {
        extract_rust_symbols(document)
    }

    fn highlight_spans(&self, document: &Document) -> Vec<HighlightSpan> {
        highlight_rust(&document.text)
    }

    fn identifier_at(&self, document: &Document, position: Position) -> Option<Identifier> {
        Some(Identifier {
            name: document.identifier_at(position)?,
        })
    }
}

fn extract_rust_symbols(document: &Document) -> Vec<Symbol> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_rust::LANGUAGE.into();

    if parser.set_language(&language).is_err() {
        return Vec::new();
    }

    let Some(tree) = parser.parse(&document.text, None) else {
        return Vec::new();
    };

    let mut symbols = Vec::new();
    collect_symbol_nodes(document, tree.root_node(), None, &mut symbols);
    symbols.sort_by(|left, right| {
        left.location
            .path
            .cmp(&right.location.path)
            .then_with(|| {
                left.location
                    .range
                    .start
                    .line
                    .cmp(&right.location.range.start.line)
            })
            .then_with(|| {
                left.location
                    .range
                    .start
                    .character
                    .cmp(&right.location.range.start.character)
            })
    });
    symbols
}

fn collect_symbol_nodes(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: Option<String>,
    symbols: &mut Vec<Symbol>,
) {
    let mut next_container = container_name.clone();

    match node.kind() {
        "function_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Function, container_name.clone())
            {
                symbols.push(symbol);
            }
        }
        "function_signature_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Method, container_name.clone())
            {
                symbols.push(symbol);
            }
        }
        "struct_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Struct, container_name.clone())
            {
                next_container = Some(symbol.name.clone());
                collect_struct_field_symbols(document, node, &symbol.name, symbols);
                symbols.push(symbol);
            }
        }
        "enum_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Enum, container_name.clone())
            {
                next_container = Some(symbol.name.clone());
                symbols.push(symbol);
            }
        }
        "trait_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Trait, container_name.clone())
            {
                next_container = Some(symbol.name.clone());
                symbols.push(symbol);
            }
        }
        "type_item" => {
            if let Some(symbol) = symbol_from_named_node(
                document,
                node,
                SymbolKind::TypeAlias,
                container_name.clone(),
            ) {
                symbols.push(symbol);
            }
        }
        "mod_item" => {
            if let Some(symbol) = symbol_from_mod_item(document, node, container_name.clone()) {
                next_container = Some(symbol.name.clone());
                symbols.push(symbol);
            }
        }
        "union_item" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, SymbolKind::Struct, container_name.clone())
            {
                next_container = Some(symbol.name.clone());
                collect_struct_field_symbols(document, node, &symbol.name, symbols);
                symbols.push(symbol);
            }
        }
        "impl_item" => {
            next_container = impl_container_name(document, node).or(container_name.clone());
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbol_nodes(document, child, next_container.clone(), symbols);
    }
}

fn collect_struct_field_symbols(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: &str,
    symbols: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_field_declaration_nodes(document, child, container_name, symbols);
    }
}

fn collect_field_declaration_nodes(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: &str,
    symbols: &mut Vec<Symbol>,
) {
    if node.kind() == "field_declaration" {
        if let Some(symbol) = symbol_from_field_declaration(document, node, container_name) {
            symbols.push(symbol);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_field_declaration_nodes(document, child, container_name, symbols);
    }
}

fn symbol_from_mod_item(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: Option<String>,
) -> Option<Symbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = name.utf8_text(document.text.as_bytes()).ok()?.to_owned();
    let location = if module_item_has_body(node) {
        Location::new(document.path.clone(), node_range(name))
    } else {
        module_file_location(&document.path, &name_text)
            .unwrap_or_else(|| Location::new(document.path.clone(), node_range(name)))
    };

    Some(Symbol {
        name: name_text,
        kind: SymbolKind::Module,
        location,
        container_name,
        language: LanguageId("rust"),
        source: SymbolSource::Workspace,
    })
}

fn module_item_has_body(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "declaration_list")
}

fn module_file_location(document_path: &Path, module_name: &str) -> Option<Location> {
    let parent = document_path.parent()?;
    let direct = parent.join(format!("{module_name}.rs"));
    let nested = parent.join(module_name).join("mod.rs");
    let target = if direct.exists() {
        direct
    } else if nested.exists() {
        nested
    } else {
        return None;
    };
    Some(Location::new(
        target,
        Range::new(Position::new(0, 0), Position::new(0, 0)),
    ))
}

fn symbol_from_field_declaration(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: &str,
) -> Option<Symbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = name.utf8_text(document.text.as_bytes()).ok()?.to_owned();
    let range = node_range(name);

    Some(Symbol {
        name: name_text,
        kind: SymbolKind::Field,
        location: Location::new(document.path.clone(), range),
        container_name: Some(container_name.to_owned()),
        language: LanguageId("rust"),
        source: SymbolSource::Workspace,
    })
}

fn symbol_from_named_node(
    document: &Document,
    node: tree_sitter::Node<'_>,
    kind: SymbolKind,
    container_name: Option<String>,
) -> Option<Symbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = name.utf8_text(document.text.as_bytes()).ok()?.to_owned();
    let range = node_range(name);

    Some(Symbol {
        name: name_text,
        kind,
        location: Location::new(document.path.clone(), range),
        container_name,
        language: LanguageId("rust"),
        source: SymbolSource::Workspace,
    })
}

fn impl_container_name(document: &Document, node: tree_sitter::Node<'_>) -> Option<String> {
    let type_node = node.child_by_field_name("type")?;
    type_node
        .utf8_text(document.text.as_bytes())
        .ok()
        .map(str::to_owned)
}

fn node_range(node: tree_sitter::Node<'_>) -> Range {
    let start = node.start_position();
    let end = node.end_position();

    Range::new(
        Position::new(start.row as u32, start.column as u32),
        Position::new(end.row as u32, end.column as u32),
    )
}

fn highlight_rust(source: &str) -> Vec<HighlightSpan> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_rust::LANGUAGE.into();

    if parser.set_language(&language).is_err() {
        return Vec::new();
    }

    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    collect_tree_sitter_spans(tree.root_node(), &mut spans);
    collect_keyword_spans(source, &mut spans);

    spans.sort_by_key(|span| (span.range.start, span.range.end));
    spans
}

fn collect_tree_sitter_spans(node: tree_sitter::Node<'_>, spans: &mut Vec<HighlightSpan>) {
    match node.kind() {
        "line_comment" | "block_comment" => {
            push_node_span(spans, node, HighlightKind::Comment);
        }
        "string_literal" | "raw_string_literal" | "char_literal" => {
            push_node_span(spans, node, HighlightKind::String);
        }
        "integer_literal" | "float_literal" => {
            push_node_span(spans, node, HighlightKind::Number);
        }
        "function_item" => {
            push_named_child_span(spans, node, HighlightKind::Function);
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" => {
            push_named_child_span(spans, node, HighlightKind::Type);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tree_sitter_spans(child, spans);
    }
}

fn push_node_span(
    spans: &mut Vec<HighlightSpan>,
    node: tree_sitter::Node<'_>,
    kind: HighlightKind,
) {
    spans.push(HighlightSpan::new(node.start_byte()..node.end_byte(), kind));
}

fn push_named_child_span(
    spans: &mut Vec<HighlightSpan>,
    node: tree_sitter::Node<'_>,
    kind: HighlightKind,
) {
    if let Some(name) = node.child_by_field_name("name") {
        push_node_span(spans, name, kind);
    }
}

fn collect_keyword_spans(source: &str, spans: &mut Vec<HighlightSpan>) {
    let bytes = source.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if is_identifier_start(bytes[index]) {
            let start = index;
            index += 1;

            while index < bytes.len() && is_identifier_continue(bytes[index]) {
                index += 1;
            }

            let word = &source[start..index];
            if is_rust_keyword(word) && !overlaps_existing_span(start..index, spans) {
                spans.push(HighlightSpan::new(start..index, HighlightKind::Keyword));
            }
        } else {
            index += 1;
        }
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
    )
}

fn overlaps_existing_span(range: std::ops::Range<usize>, spans: &[HighlightSpan]) -> bool {
    spans
        .iter()
        .any(|span| range.start < span.range.end && span.range.start < range.end)
}

#[cfg(test)]
mod tests {
    use crate::core::{document::Document, language::LanguageSupport, symbol::SymbolKind};

    use super::*;

    #[test]
    fn extracts_struct_name_with_lifetime_parameter() {
        let language = RustLanguage;
        let document = Document::new(
            "builder.rs".into(),
            "pub struct TextEdit<'t> {\n    text: &'t mut String,\n}\n\nimpl<'t> TextEdit<'t> {\n    pub fn multiline(text: &'t mut dyn TextBuffer) -> Self {\n        todo!()\n    }\n}\n"
                .to_owned(),
        );

        let symbols = language.extract_symbols(&document);

        assert!(symbols.iter().any(|symbol| symbol.name == "TextEdit"));
        assert!(symbols.iter().any(|symbol| {
            symbol.name == "text"
                && symbol.kind == SymbolKind::Field
                && symbol.container_name.as_deref() == Some("TextEdit")
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.name == "multiline" && symbol.container_name.as_deref() == Some("TextEdit<'t>")
        }));
    }

    #[test]
    fn extracts_rust_struct_fields() {
        let language = RustLanguage;
        let document = Document::new(
            "editor.rs".into(),
            "struct EditorPane {\n    title: String,\n    cursor_position: Option<Position>,\n}\n"
                .to_owned(),
        );

        let symbols = language.extract_symbols(&document);

        assert!(symbols.iter().any(|symbol| {
            symbol.name == "title"
                && symbol.kind == SymbolKind::Field
                && symbol.container_name.as_deref() == Some("EditorPane")
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.name == "cursor_position"
                && symbol.kind == SymbolKind::Field
                && symbol.container_name.as_deref() == Some("EditorPane")
        }));
    }

    #[test]
    fn extracts_rust_module_symbols() {
        let language = RustLanguage;
        let document = Document::new(
            "lib.rs".into(),
            "pub mod text;\nmod inline {\n    pub struct Cursor;\n}\n".to_owned(),
        );

        let symbols = language.extract_symbols(&document);

        assert!(
            symbols
                .iter()
                .any(|symbol| { symbol.name == "text" && symbol.kind == SymbolKind::Module })
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| { symbol.name == "inline" && symbol.kind == SymbolKind::Module })
        );
    }
}
