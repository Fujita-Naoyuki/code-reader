//! Finding the language support for a path.

use std::{path::Path, sync::Arc};

use crate::core::language::LanguageSupport;

#[derive(Clone)]
pub struct LanguageRegistry {
    languages: Vec<Arc<dyn LanguageSupport>>,
}

impl LanguageRegistry {
    pub fn with_languages(languages: Vec<Box<dyn LanguageSupport>>) -> Self {
        Self {
            languages: languages.into_iter().map(Arc::from).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.languages.len()
    }

    pub fn language_for_path(&self, path: &Path) -> Option<Arc<dyn LanguageSupport>> {
        self.languages
            .iter()
            .find(|language| language.matches_path(path))
            .cloned()
    }
}
