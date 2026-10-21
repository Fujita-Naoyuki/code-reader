use std::path::Path;

use crate::core::{document::Document, location::Position, symbol::Symbol, syntax::HighlightSpan};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguageId(pub &'static str);

#[derive(Debug, Clone)]
pub struct Identifier {
    pub name: String,
}

pub trait LanguageSupport: Send + Sync {
    fn id(&self) -> LanguageId;
    fn file_extensions(&self) -> &'static [&'static str];

    fn matches_path(&self, path: &Path) -> bool {
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };

        self.file_extensions().contains(&extension)
    }

    fn extract_symbols(&self, document: &Document) -> Vec<Symbol>;

    fn highlight_spans(&self, _document: &Document) -> Vec<HighlightSpan> {
        Vec::new()
    }

    /// The identifier under `position`; definitions themselves come from the language
    /// server (see `ui/app_view` LSP handling).
    fn identifier_at(&self, document: &Document, position: Position) -> Option<Identifier>;
}
