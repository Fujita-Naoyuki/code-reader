use std::path::{Path, PathBuf};

use crate::core::{
    document::Document,
    location::{Location, Position},
    symbol::Symbol,
    syntax::HighlightSpan,
    workspace::Workspace,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguageId(pub &'static str);

#[derive(Debug, Clone)]
pub struct Identifier {
    pub name: String,
    pub location: Location,
}

#[derive(Debug, Clone)]
pub struct DependencyRoot {
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub language: LanguageId,
}

#[derive(Debug, Clone)]
pub struct ResolveContext<'a> {
    pub workspace: &'a Workspace,
    pub symbols: &'a [Symbol],
}

pub trait LanguageSupport: Send + Sync {
    fn id(&self) -> LanguageId;
    fn name(&self) -> &'static str;
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

    fn identifier_at(&self, document: &Document, position: Position) -> Option<Identifier>;

    fn resolve_definition(
        &self,
        identifier: &Identifier,
        context: &ResolveContext<'_>,
    ) -> Vec<Location>;

    fn dependency_roots(&self, _workspace: &Workspace) -> Vec<DependencyRoot> {
        Vec::new()
    }
}
