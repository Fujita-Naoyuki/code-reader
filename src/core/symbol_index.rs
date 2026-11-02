//! All symbols of the workspace, for Search Everywhere.

use std::fs;
use std::path::{Path, PathBuf};

use crate::core::{
    document::Document,
    symbol::{Symbol, SymbolSource},
};
use crate::languages::LanguageRegistry;

#[derive(Debug, Clone, Default)]
pub struct SymbolIndex {
    symbols: Vec<Symbol>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rebuild_for_files(files: &[PathBuf], language_registry: &LanguageRegistry) -> Self {
        let mut index = Self::new();

        index.extend_files_with_source(files, language_registry, SymbolSource::Workspace);
        index.sort();

        index
    }

    pub fn add_file(&mut self, path: &Path, language_registry: &LanguageRegistry) {
        self.add_file_with_source(path, language_registry, SymbolSource::Workspace);
    }

    pub fn add_external_file(&mut self, path: &Path, language_registry: &LanguageRegistry) {
        self.add_file_with_source(path, language_registry, SymbolSource::ExternalLibrary);
    }

    pub fn replace_file(&mut self, path: &Path, language_registry: &LanguageRegistry) {
        self.remove_file(path);
        self.add_file(path, language_registry);
    }

    fn add_file_with_source(
        &mut self,
        path: &Path,
        language_registry: &LanguageRegistry,
        source: SymbolSource,
    ) {
        let Some(symbols) = file_symbols_with_source(path, language_registry, source) else {
            return;
        };

        self.remove_file(path);
        self.symbols.extend(symbols);
        self.sort();
    }

    fn extend_files_with_source(
        &mut self,
        paths: &[PathBuf],
        language_registry: &LanguageRegistry,
        source: SymbolSource,
    ) {
        self.symbols.extend(
            paths
                .iter()
                .filter_map(|path| file_symbols_with_source(path, language_registry, source))
                .flatten(),
        );
    }

    pub fn remove_file(&mut self, path: &Path) {
        self.symbols.retain(|symbol| symbol.location.path != path);
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    fn sort(&mut self) {
        self.symbols.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.location.path.cmp(&right.location.path))
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
    }
}

fn file_symbols_with_source(
    path: &Path,
    language_registry: &LanguageRegistry,
    source: SymbolSource,
) -> Option<Vec<Symbol>> {
    let language = language_registry.language_for_path(path)?;
    let Ok(text) = fs::read_to_string(path) else {
        return None;
    };

    let document = Document::new(path.to_path_buf(), text);
    let mut symbols = language.extract_symbols(&document);

    for symbol in &mut symbols {
        symbol.source = source;
    }

    Some(symbols)
}
