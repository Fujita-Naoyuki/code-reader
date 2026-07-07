use std::sync::Arc;

use crate::core::{
    document::Document,
    language::{LanguageSupport, ResolveContext},
    location::{Location, Position},
};

pub fn resolve_definition(
    language: Arc<dyn LanguageSupport>,
    document: &Document,
    position: Position,
    context: &ResolveContext<'_>,
) -> Vec<Location> {
    let Some(identifier) = language.identifier_at(document, position) else {
        return Vec::new();
    };

    language.resolve_definition(&identifier, context)
}
