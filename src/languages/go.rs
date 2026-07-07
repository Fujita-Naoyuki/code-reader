use crate::core::{
    document::Document,
    language::{Identifier, LanguageId, LanguageSupport, ResolveContext},
    location::{Location, Position, Range},
    symbol::{Symbol, SymbolKind, SymbolSource},
    syntax::{HighlightKind, HighlightSpan},
};

pub struct GoLanguage;

impl LanguageSupport for GoLanguage {
    fn id(&self) -> LanguageId {
        LanguageId("go")
    }

    fn name(&self) -> &'static str {
        "Go"
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn extract_symbols(&self, document: &Document) -> Vec<Symbol> {
        extract_go_symbols(document)
    }

    fn highlight_spans(&self, document: &Document) -> Vec<HighlightSpan> {
        highlight_go(&document.text)
    }

    fn identifier_at(&self, document: &Document, position: Position) -> Option<Identifier> {
        let name = document.identifier_at(position)?;
        let location = Location::new(document.path.clone(), Range::new(position, position));

        Some(Identifier { name, location })
    }

    fn resolve_definition(
        &self,
        _identifier: &Identifier,
        _context: &ResolveContext<'_>,
    ) -> Vec<Location> {
        Vec::new()
    }
}

fn extract_go_symbols(document: &Document) -> Vec<Symbol> {
    let Some(tree) = parse_go(&document.text) else {
        return Vec::new();
    };

    let mut symbols = Vec::new();
    collect_symbol_nodes(document, tree.root_node(), &mut symbols);
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
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(symbol) = symbol_from_named_node(document, node, SymbolKind::Function, None)
            {
                symbols.push(symbol);
            }
        }
        "method_declaration" => {
            if let Some(symbol) = symbol_from_named_node(
                document,
                node,
                SymbolKind::Method,
                receiver_container_name(document, node),
            ) {
                symbols.push(symbol);
            }
        }
        "type_spec" => {
            if let Some(symbol) =
                symbol_from_named_node(document, node, go_type_symbol_kind(node), None)
            {
                if node.child_by_field_name("type").map(|node| node.kind()) == Some("struct_type") {
                    collect_struct_field_symbols(document, node, &symbol.name, symbols);
                }
                symbols.push(symbol);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbol_nodes(document, child, symbols);
    }
}

fn collect_struct_field_symbols(
    document: &Document,
    type_spec: tree_sitter::Node<'_>,
    container_name: &str,
    symbols: &mut Vec<Symbol>,
) {
    let Some(type_node) = type_spec.child_by_field_name("type") else {
        return;
    };

    collect_field_identifier_nodes(document, type_node, container_name, symbols);
}

fn collect_field_identifier_nodes(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: &str,
    symbols: &mut Vec<Symbol>,
) {
    if node.kind() == "field_identifier" {
        if let Some(symbol) = symbol_from_field_identifier(document, node, container_name) {
            symbols.push(symbol);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_field_identifier_nodes(document, child, container_name, symbols);
    }
}

fn symbol_from_field_identifier(
    document: &Document,
    node: tree_sitter::Node<'_>,
    container_name: &str,
) -> Option<Symbol> {
    let name_text = node.utf8_text(document.text.as_bytes()).ok()?.to_owned();

    Some(Symbol {
        name: name_text,
        kind: SymbolKind::Field,
        location: Location::new(document.path.clone(), node_range(node)),
        container_name: Some(container_name.to_owned()),
        language: LanguageId("go"),
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

    Some(Symbol {
        name: name_text,
        kind,
        location: Location::new(document.path.clone(), node_range(name)),
        container_name,
        language: LanguageId("go"),
        source: SymbolSource::Workspace,
    })
}

fn go_type_symbol_kind(node: tree_sitter::Node<'_>) -> SymbolKind {
    match node.child_by_field_name("type").map(|node| node.kind()) {
        Some("struct_type") => SymbolKind::Struct,
        Some("interface_type") => SymbolKind::Interface,
        _ => SymbolKind::TypeAlias,
    }
}

fn receiver_container_name(document: &Document, node: tree_sitter::Node<'_>) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    let text = receiver.utf8_text(document.text.as_bytes()).ok()?;
    receiver_type_name(text)
}

fn receiver_type_name(receiver: &str) -> Option<String> {
    let trimmed = receiver
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let last = trimmed.split_whitespace().last().unwrap_or(trimmed);
    let name = last
        .trim_start_matches('*')
        .trim_start_matches("[]")
        .rsplit('.')
        .next()?
        .trim();

    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

fn highlight_go(source: &str) -> Vec<HighlightSpan> {
    let Some(tree) = parse_go(source) else {
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
        "comment" => {
            push_node_span(spans, node, HighlightKind::Comment);
        }
        "interpreted_string_literal" | "raw_string_literal" | "rune_literal" => {
            push_node_span(spans, node, HighlightKind::String);
        }
        "int_literal" | "float_literal" | "imaginary_literal" => {
            push_node_span(spans, node, HighlightKind::Number);
        }
        "function_declaration" | "method_declaration" => {
            push_named_child_span(spans, node, HighlightKind::Function);
        }
        "type_spec" => {
            push_named_child_span(spans, node, HighlightKind::Type);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tree_sitter_spans(child, spans);
    }
}

fn parse_go(source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_go::LANGUAGE.into();
    parser.set_language(&language).ok()?;
    parser.parse(source, None)
}

fn node_range(node: tree_sitter::Node<'_>) -> Range {
    let start = node.start_position();
    let end = node.end_position();

    Range::new(
        Position::new(start.row as u32, start.column as u32),
        Position::new(end.row as u32, end.column as u32),
    )
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
            if is_go_keyword(word) && !overlaps_existing_span(start..index, spans) {
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

fn is_go_keyword(word: &str) -> bool {
    matches!(
        word,
        "break"
            | "case"
            | "chan"
            | "const"
            | "continue"
            | "default"
            | "defer"
            | "else"
            | "fallthrough"
            | "for"
            | "func"
            | "go"
            | "goto"
            | "if"
            | "import"
            | "interface"
            | "map"
            | "package"
            | "range"
            | "return"
            | "select"
            | "struct"
            | "switch"
            | "type"
            | "var"
    )
}

fn overlaps_existing_span(range: std::ops::Range<usize>, spans: &[HighlightSpan]) -> bool {
    spans
        .iter()
        .any(|span| range.start < span.range.end && span.range.start < range.end)
}

#[cfg(test)]
mod tests {
    use crate::core::{
        document::Document, language::LanguageSupport, symbol::SymbolKind, syntax::HighlightKind,
    };

    use super::{GoLanguage, receiver_type_name};

    #[test]
    fn resolves_go_files() {
        let language = GoLanguage;

        assert!(language.matches_path(std::path::Path::new("main.go")));
        assert!(!language.matches_path(std::path::Path::new("main.rs")));
    }

    #[test]
    fn extracts_go_function_method_struct_and_interface_symbols() {
        let language = GoLanguage;
        let document = Document::new(
            "main.go".into(),
            "package main\n\ntype Server struct { MutableStyle string }\ntype Runner interface { Run() error }\ntype Name string\n\nfunc NewServer() *Server { return &Server{} }\nfunc (s *Server) Serve() error { return nil }\n"
                .to_owned(),
        );

        let symbols = language.extract_symbols(&document);

        assert!(
            symbols
                .iter()
                .any(|symbol| { symbol.name == "Server" && symbol.kind == SymbolKind::Struct })
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| { symbol.name == "Runner" && symbol.kind == SymbolKind::Interface })
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| { symbol.name == "Name" && symbol.kind == SymbolKind::TypeAlias })
        );
        assert!(
            symbols.iter().any(|symbol| {
                symbol.name == "NewServer" && symbol.kind == SymbolKind::Function
            })
        );
        assert!(symbols.iter().any(|symbol| {
            symbol.name == "Serve"
                && symbol.kind == SymbolKind::Method
                && symbol.container_name.as_deref() == Some("Server")
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.name == "MutableStyle"
                && symbol.kind == SymbolKind::Field
                && symbol.container_name.as_deref() == Some("Server")
        }));
    }

    #[test]
    fn highlights_go_basics() {
        let language = GoLanguage;
        let document = Document::new(
            "main.go".into(),
            "package main\n\n// hello\nfunc main() { println(\"hi\", 123) }\n".to_owned(),
        );

        let spans = language.highlight_spans(&document);

        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
        assert!(
            spans
                .iter()
                .any(|span| span.kind == HighlightKind::Function)
        );
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Number));
    }

    #[test]
    fn extracts_receiver_type_name() {
        assert_eq!(receiver_type_name("(s *Server)").as_deref(), Some("Server"));
        assert_eq!(
            receiver_type_name("(repo pkg.Repository)").as_deref(),
            Some("Repository")
        );
    }
}
