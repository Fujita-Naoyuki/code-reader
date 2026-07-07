use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
};

use crate::{core::symbol_index::SymbolIndex, languages::LanguageRegistry};

#[derive(Debug, Clone)]
pub struct IndexingResult {
    pub index: SymbolIndex,
    pub external_file_count: usize,
    pub external_package_count: usize,
}

pub fn start_symbol_indexing(
    files: Vec<PathBuf>,
    language_registry: LanguageRegistry,
) -> Receiver<IndexingResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let index = SymbolIndex::rebuild_for_files(&files, &language_registry);

        let _ = sender.send(IndexingResult {
            index,
            external_file_count: 0,
            external_package_count: 0,
        });
    });

    receiver
}
