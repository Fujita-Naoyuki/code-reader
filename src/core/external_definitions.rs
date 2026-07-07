use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    core::{
        external_libraries::{discover_cargo_external_libraries, discover_go_external_libraries},
        location::Location,
        symbol::{Symbol, SymbolSource},
        symbol_index::SymbolIndex,
    },
    languages::LanguageRegistry,
};

#[derive(Debug, Clone, Default)]
pub struct ExternalDefinitionResolution {
    pub locations: Vec<Location>,
    pub indexed_file_count: usize,
    pub external_file_count: usize,
    pub external_package_count: usize,
}

pub fn resolve_external_definition_on_demand(
    roots: &[PathBuf],
    identifier: &str,
    qualifier: Option<&str>,
    symbol_index: &mut SymbolIndex,
    language_registry: &LanguageRegistry,
) -> ExternalDefinitionResolution {
    let cargo_libraries = discover_cargo_external_libraries(roots);
    let go_libraries = discover_go_external_libraries(roots);
    let external_package_count = cargo_libraries.lock_packages.len() + go_libraries.modules.len();
    let external_files = cargo_libraries
        .files
        .into_iter()
        .chain(go_libraries.files)
        .collect::<Vec<_>>();

    let mut resolution = resolve_external_definition_in_files(
        identifier,
        qualifier,
        &external_files,
        symbol_index,
        language_registry,
    );
    resolution.external_file_count = external_files.len();
    resolution.external_package_count = external_package_count;
    resolution
}

pub fn resolve_external_definition_in_files(
    identifier: &str,
    qualifier: Option<&str>,
    external_files: &[PathBuf],
    symbol_index: &mut SymbolIndex,
    language_registry: &LanguageRegistry,
) -> ExternalDefinitionResolution {
    let locations = external_symbol_locations_in_files(
        symbol_index.symbols(),
        identifier,
        qualifier,
        external_files,
    );
    if !locations.is_empty() {
        return ExternalDefinitionResolution {
            locations,
            external_file_count: external_files.len(),
            ..Default::default()
        };
    }

    const EXTERNAL_DEFINITION_BATCH_SIZE: usize = 64;
    let mut pending_external_files = Vec::new();
    let mut indexed_file_count = 0;

    for path in external_files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if !text.contains(identifier) {
            continue;
        }

        pending_external_files.push(path.clone());

        if pending_external_files.len() >= EXTERNAL_DEFINITION_BATCH_SIZE {
            indexed_file_count += pending_external_files.len();
            symbol_index.add_external_files(&pending_external_files, language_registry);
            pending_external_files.clear();

            let locations = external_symbol_locations_in_files(
                symbol_index.symbols(),
                identifier,
                qualifier,
                external_files,
            );
            if !locations.is_empty() {
                return ExternalDefinitionResolution {
                    locations,
                    indexed_file_count,
                    external_file_count: external_files.len(),
                    ..Default::default()
                };
            }
        }
    }

    if !pending_external_files.is_empty() {
        indexed_file_count += pending_external_files.len();
        symbol_index.add_external_files(&pending_external_files, language_registry);
    }

    ExternalDefinitionResolution {
        locations: external_symbol_locations_in_files(
            symbol_index.symbols(),
            identifier,
            qualifier,
            external_files,
        ),
        indexed_file_count,
        external_file_count: external_files.len(),
        ..Default::default()
    }
}

pub fn external_symbol_locations(
    symbols: &[Symbol],
    identifier: &str,
    qualifier: Option<&str>,
) -> Vec<Location> {
    external_symbol_locations_filtered(symbols, identifier, qualifier, |_| true)
}

fn external_symbol_locations_in_files(
    symbols: &[Symbol],
    identifier: &str,
    qualifier: Option<&str>,
    external_files: &[PathBuf],
) -> Vec<Location> {
    if external_files.is_empty() {
        return Vec::new();
    }

    let allowed_paths = external_files
        .iter()
        .map(PathBuf::as_path)
        .collect::<HashSet<_>>();

    external_symbol_locations_filtered(symbols, identifier, qualifier, |path| {
        allowed_paths.contains(path)
    })
}

fn external_symbol_locations_filtered(
    symbols: &[Symbol],
    identifier: &str,
    qualifier: Option<&str>,
    path_matches: impl Fn(&Path) -> bool,
) -> Vec<Location> {
    let mut locations = symbols
        .iter()
        .filter(|symbol| symbol.source == SymbolSource::ExternalLibrary)
        .filter(|symbol| symbol.name == identifier)
        .filter(|symbol| symbol_matches_qualifier(symbol.container_name.as_deref(), qualifier))
        .filter(|symbol| path_matches(&symbol.location.path))
        .map(|symbol| symbol.location.clone())
        .collect::<Vec<_>>();

    locations.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
    });
    locations.dedup();
    locations
}

fn symbol_matches_qualifier(container_name: Option<&str>, qualifier: Option<&str>) -> bool {
    let Some(qualifier) = qualifier else {
        return true;
    };

    container_name
        .map(|container| {
            container == qualifier
                || container.starts_with(&format!("{qualifier}<"))
                || container.ends_with(&format!("::{qualifier}"))
        })
        .unwrap_or(false)
}
