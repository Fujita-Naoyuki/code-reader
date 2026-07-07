use crate::core::{language::LanguageId, location::Location};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    pub container_name: Option<String>,
    pub language: LanguageId,
    pub source: SymbolSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Interface,
    Module,
    Constant,
    Variable,
    Field,
    TypeAlias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolSource {
    Workspace,
    ExternalLibrary,
}
