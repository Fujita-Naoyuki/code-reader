use std::path::PathBuf;

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::core::{
    location::{Location, Position, Range},
    session::SessionBookmark,
    symbol::{Symbol, SymbolSource},
};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub kind: SearchResultKind,
    pub label: String,
    pub detail: String,
    pub location: Location,
    pub score: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultKind {
    File,
    Symbol,
    Bookmark,
    History,
    External,
}

impl SearchResultKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Symbol => "Symbol",
            Self::Bookmark => "Bookmark",
            Self::History => "History",
            Self::External => "External",
        }
    }
}

pub fn search_everywhere(
    query: &str,
    files: &[PathBuf],
    symbols: &[Symbol],
    bookmarks: &[SessionBookmark],
    recent_files: &[PathBuf],
) -> Vec<SearchResult> {
    let matcher = SkimMatcherV2::default();
    let mut results = Vec::new();

    results.extend(search_files(query, files, recent_files, &matcher));
    results.extend(search_symbols(query, symbols, &matcher));
    results.extend(search_bookmarks(query, bookmarks, &matcher));

    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.kind.label().cmp(right.kind.label()))
            .then_with(|| left.label.cmp(&right.label))
    });
    results.truncate(80);
    results
}

fn search_files(
    query: &str,
    files: &[PathBuf],
    recent_files: &[PathBuf],
    matcher: &SkimMatcherV2,
) -> Vec<SearchResult> {
    let mut results = Vec::new();

    for (index, path) in recent_files.iter().enumerate() {
        if !path.is_file() {
            continue;
        }

        if let Some(score) = score_path(query, path, matcher) {
            results.push(SearchResult {
                kind: SearchResultKind::History,
                label: file_label(path),
                detail: path.display().to_string(),
                location: file_location(path.clone()),
                score: score + 30 - index as i64,
            });
        }
    }

    for path in files {
        if recent_files.iter().any(|recent| recent == path) {
            continue;
        }

        if let Some(score) = score_path(query, path, matcher) {
            results.push(SearchResult {
                kind: SearchResultKind::File,
                label: file_label(path),
                detail: path.display().to_string(),
                location: file_location(path.clone()),
                score,
            });
        }
    }

    results
}

fn search_symbols(query: &str, symbols: &[Symbol], matcher: &SkimMatcherV2) -> Vec<SearchResult> {
    symbols
        .iter()
        .filter(|symbol| symbol.source == SymbolSource::Workspace)
        .filter_map(|symbol| {
            let target = symbol_target(symbol);
            let score = score_text(query, &target, matcher)?;

            Some(SearchResult {
                kind: SearchResultKind::Symbol,
                label: symbol_label(symbol),
                detail: format!(
                    "{}:{}",
                    symbol.location.path.display(),
                    symbol.location.range.start.line + 1
                ),
                location: symbol.location.clone(),
                score: score + 12,
            })
        })
        .collect()
}

fn search_bookmarks(
    query: &str,
    bookmarks: &[SessionBookmark],
    matcher: &SkimMatcherV2,
) -> Vec<SearchResult> {
    bookmarks
        .iter()
        .filter_map(|bookmark| {
            let target = format!("{} {}", bookmark.label, bookmark.path.display());
            let score = score_text(query, &target, matcher)?;

            Some(SearchResult {
                kind: SearchResultKind::Bookmark,
                label: bookmark.label.clone(),
                detail: format!(
                    "{}:{}{}",
                    bookmark.path.display(),
                    bookmark.position.line + 1,
                    if bookmark.temporary { " temporary" } else { "" }
                ),
                location: bookmark_location(bookmark),
                score: score + 20,
            })
        })
        .collect()
}

fn score_path(query: &str, path: &PathBuf, matcher: &SkimMatcherV2) -> Option<i64> {
    let file_name = file_label(path);
    let target = format!("{file_name} {}", path.display());
    score_text(query, &target, matcher)
}

fn score_text(query: &str, target: &str, matcher: &SkimMatcherV2) -> Option<i64> {
    if query.trim().is_empty() {
        Some(0)
    } else {
        matcher.fuzzy_match(target, query)
    }
}

fn file_label(path: &PathBuf) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<invalid>")
        .to_owned()
}

fn file_location(path: PathBuf) -> Location {
    Location::new(path, Range::new(Position::new(0, 0), Position::new(0, 0)))
}

fn bookmark_location(bookmark: &SessionBookmark) -> Location {
    let position = Position::new(bookmark.position.line, bookmark.position.character);
    Location::new(bookmark.path.clone(), Range::new(position, position))
}

fn symbol_label(symbol: &Symbol) -> String {
    match &symbol.container_name {
        Some(container) => format!("{container}::{}", symbol.name),
        None => symbol.name.clone(),
    }
}

fn symbol_target(symbol: &Symbol) -> String {
    format!(
        "{} {} {:?} {}",
        symbol_label(symbol),
        symbol.name,
        symbol.kind,
        symbol.location.path.display()
    )
}
