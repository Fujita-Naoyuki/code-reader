//! Facts about workspace files: kinds by path, change snapshots for the background
//! scan, names for new files, and display paths.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub(crate) type WorkspaceSnapshot = HashMap<PathBuf, WorkspaceFileFingerprint>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceFileFingerprint {
    pub(crate) modified_unix_nanos: u128,
    pub(crate) len: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceSnapshotDiff {
    pub(crate) added: Vec<PathBuf>,
    pub(crate) modified: Vec<PathBuf>,
    pub(crate) removed: Vec<PathBuf>,
}

impl WorkspaceSnapshotDiff {
    pub(crate) fn len(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    pub(crate) fn requires_full_symbol_reindex(&self) -> bool {
        self.added
            .iter()
            .chain(&self.modified)
            .chain(&self.removed)
            .any(|path| is_dependency_manifest_path(path))
    }
}

pub(crate) fn workspace_search_display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|relative| relative.components().count())
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn workspace_snapshot(files: &[PathBuf]) -> WorkspaceSnapshot {
    files
        .iter()
        .filter_map(|path| Some((path.clone(), workspace_file_fingerprint(path)?)))
        .collect()
}

pub(crate) fn workspace_file_fingerprint(path: &Path) -> Option<WorkspaceFileFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let modified_unix_nanos = modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Some(WorkspaceFileFingerprint {
        modified_unix_nanos,
        len: metadata.len(),
    })
}

#[cfg(test)]
pub(crate) fn workspace_snapshot_change_count(
    previous: &WorkspaceSnapshot,
    next: &WorkspaceSnapshot,
) -> usize {
    workspace_snapshot_diff(previous, next).len()
}

pub(crate) fn workspace_snapshot_diff(
    previous: &WorkspaceSnapshot,
    next: &WorkspaceSnapshot,
) -> WorkspaceSnapshotDiff {
    let mut diff = WorkspaceSnapshotDiff::default();

    for (path, fingerprint) in next {
        match previous.get(path) {
            Some(previous_fingerprint) if previous_fingerprint == fingerprint => {}
            Some(_) => diff.modified.push(path.clone()),
            None => diff.added.push(path.clone()),
        }
    }

    for path in previous.keys() {
        if !next.contains_key(path) {
            diff.removed.push(path.clone());
        }
    }

    diff.added.sort();
    diff.modified.sort();
    diff.removed.sort();
    diff
}

pub(crate) fn is_dependency_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "Cargo.toml" | "Cargo.lock" | "go.mod" | "go.sum"))
}

pub(crate) fn is_csv_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
}

pub(crate) fn is_go_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("go"))
}

pub(crate) fn is_rust_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

pub(crate) fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md")
                || extension.eq_ignore_ascii_case("mdx")
                || extension.eq_ignore_ascii_case("markdown")
        })
}

pub(crate) fn is_external_path(path: &Path) -> bool {
    let display = path.to_string_lossy().replace('\\', "/");
    display.contains("/.cargo/")
        || display.contains("/.rustup/")
        || display.contains("/go/pkg/mod/")
}

pub(crate) fn is_external_rust_path(path: &Path) -> bool {
    if !is_rust_path(path) {
        return false;
    }
    let display = path.to_string_lossy().replace('\\', "/");
    display.contains("/.cargo/") || display.contains("/.rustup/")
}

pub(crate) fn file_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<invalid>")
        .to_owned()
}

pub(crate) fn validate_workspace_file_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("File name is required.".to_owned());
    }
    if name == "." || name == ".." {
        return Err("File name cannot be . or ..".to_owned());
    }
    if name.chars().any(|character| {
        matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
    }) {
        return Err("File name contains an invalid character.".to_owned());
    }
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() != 1 {
        return Err("Enter a file name, not a path.".to_owned());
    }
    Ok(name.to_owned())
}

pub(crate) fn file_name_stem_selection_char_len(file_name: &str) -> usize {
    file_name
        .rfind('.')
        .filter(|index| *index > 0)
        .map(|index| file_name[..index].chars().count())
        .unwrap_or_else(|| file_name.chars().count())
}

pub(crate) fn initial_workspace_file_content(path: &Path) -> String {
    if !is_markdown_path(path) {
        return String::new();
    }

    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("New Document");
    format!("# {title}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_workspace_file_name_accepts_simple_names() {
        assert_eq!(
            validate_workspace_file_name(" idea.md "),
            Ok("idea.md".to_owned())
        );
        assert_eq!(
            validate_workspace_file_name("Cargo.lock"),
            Ok("Cargo.lock".to_owned())
        );
    }

    #[test]
    fn validate_workspace_file_name_rejects_paths() {
        assert!(validate_workspace_file_name("../idea.md").is_err());
        assert!(validate_workspace_file_name("docs/idea.md").is_err());
        assert!(validate_workspace_file_name("C:\\temp\\idea.md").is_err());
        assert!(validate_workspace_file_name("").is_err());
    }

    #[test]
    fn initial_workspace_file_content_uses_markdown_for_markdown_files() {
        assert_eq!(
            initial_workspace_file_content(Path::new("idea.md")),
            "# idea\n"
        );
        assert_eq!(
            initial_workspace_file_content(Path::new("note.mdx")),
            "# note\n"
        );
    }

    #[test]
    fn initial_workspace_file_content_keeps_non_markdown_files_empty() {
        assert_eq!(initial_workspace_file_content(Path::new("new.csv")), "");
        assert_eq!(initial_workspace_file_content(Path::new("main.rs")), "");
        assert_eq!(initial_workspace_file_content(Path::new("config.toml")), "");
    }

    #[test]
    fn file_name_stem_selection_char_len_selects_before_extension() {
        assert_eq!(file_name_stem_selection_char_len("README.md"), 6);
        assert_eq!(file_name_stem_selection_char_len("archive.tar.gz"), 11);
    }

    #[test]
    fn file_name_stem_selection_char_len_selects_all_without_extension() {
        assert_eq!(file_name_stem_selection_char_len(".gitignore"), 10);
        assert_eq!(file_name_stem_selection_char_len("README"), 6);
    }

    #[test]
    fn workspace_snapshot_change_count_counts_added_modified_and_removed_files() {
        let unchanged = PathBuf::from("unchanged.rs");
        let modified = PathBuf::from("modified.rs");
        let added = PathBuf::from("added.rs");
        let removed = PathBuf::from("removed.rs");

        let previous = HashMap::from([
            (
                unchanged.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                removed,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);
        let next = HashMap::from([
            (
                unchanged,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 11,
                    len: 20,
                },
            ),
            (
                added,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);

        assert_eq!(workspace_snapshot_change_count(&previous, &next), 3);
    }

    #[test]
    fn workspace_snapshot_diff_classifies_changed_files() {
        let unchanged = PathBuf::from("unchanged.rs");
        let modified = PathBuf::from("modified.rs");
        let added = PathBuf::from("added.rs");
        let removed = PathBuf::from("removed.rs");

        let previous = HashMap::from([
            (
                unchanged.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                removed.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);
        let next = HashMap::from([
            (
                unchanged,
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
            (
                modified.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 11,
                    len: 20,
                },
            ),
            (
                added.clone(),
                WorkspaceFileFingerprint {
                    modified_unix_nanos: 10,
                    len: 20,
                },
            ),
        ]);

        let diff = workspace_snapshot_diff(&previous, &next);

        assert_eq!(diff.added, vec![added]);
        assert_eq!(diff.modified, vec![modified]);
        assert_eq!(diff.removed, vec![removed]);
    }

    #[test]
    fn workspace_snapshot_diff_requires_full_reindex_for_dependency_files() {
        let diff = WorkspaceSnapshotDiff {
            added: Vec::new(),
            modified: vec![PathBuf::from("Cargo.lock")],
            removed: Vec::new(),
        };

        assert!(diff.requires_full_symbol_reindex());
    }

    #[test]
    fn workspace_search_display_path_prefers_shortest_root_relative_path() {
        let path = PathBuf::from(r"C:\git\code-reader\src\ui\app_view.rs");
        let roots = vec![
            PathBuf::from(r"C:\git"),
            PathBuf::from(r"C:\git\code-reader"),
        ];

        assert_eq!(
            workspace_search_display_path(&path, &roots),
            PathBuf::from(r"src\ui\app_view.rs").display().to_string()
        );
    }
}
