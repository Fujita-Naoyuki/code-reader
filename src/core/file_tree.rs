use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;

const MAX_FILE_SIZE_BYTES: u64 = 2 * 1024 * 1024;
const EXCLUDED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".idea",
    ".vscode",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "vendor",
];
const DISCOVERABLE_EXTENSIONS: &[&str] = &[
    "csv", "go", "json", "jsonc", "lock", "md", "mdx", "rs", "toml", "tsv", "txt", "yaml", "yml",
];
const DISCOVERABLE_FILE_NAMES: &[&str] = &[
    ".env",
    ".gitattributes",
    ".gitignore",
    "Cargo.lock",
    "Cargo.toml",
    "Dockerfile",
    "LICENSE",
    "LICENSE.md",
    "Makefile",
    "README",
    "README.md",
    "TODO",
    "TODO.md",
    "go.mod",
    "go.sum",
];

#[derive(Debug, Clone)]
pub struct FileTree {
    pub roots: Vec<FileNode>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FileNode {
    pub name: String,
    pub path: PathBuf,
    pub kind: FileNodeKind,
    pub children: Vec<FileNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNodeKind {
    Directory,
    File,
}

impl FileNode {
    pub fn is_dir(&self) -> bool {
        self.kind == FileNodeKind::Directory
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use super::{FileDiscoveryOptions, discover_workspace_files};

    fn temp_root(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "code_reader_file_tree_{test_name}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn write_file(path: &std::path::Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn relative_files(root: &std::path::Path, files: Vec<PathBuf>) -> Vec<String> {
        let mut relative = files
            .into_iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        relative.sort();
        relative
    }

    #[test]
    fn discover_workspace_uses_supported_names_and_skips_heavy_directories() {
        let root = temp_root("supported_names");
        write_file(&root.join("src/main.go"), "package main\n");
        write_file(&root.join("src/lib.rs"), "pub fn run() {}\n");
        write_file(&root.join("Cargo.toml"), "[package]\n");
        write_file(&root.join("README"), "read me\n");
        write_file(&root.join(".env.local"), "RUST_LOG=debug\n");
        write_file(&root.join("index.html"), "<html></html>\n");
        write_file(&root.join("image.png"), "not really an image\n");
        write_file(&root.join(".git/config"), "[core]\n");
        write_file(&root.join("build/generated.go"), "package generated\n");
        write_file(&root.join("target/debug/output.txt"), "output\n");
        write_file(&root.join("vendor/example.com/lib/lib.go"), "package lib\n");

        let files = relative_files(
            &root,
            discover_workspace_files(std::slice::from_ref(&root), FileDiscoveryOptions::default()),
        );

        assert_eq!(
            files,
            vec![
                ".env.local",
                "Cargo.toml",
                "README",
                "src/lib.rs",
                "src/main.go",
            ]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discover_workspace_applies_size_limit_after_name_filter() {
        let root = temp_root("size_limit");
        write_file(&root.join("small.go"), "go");
        write_file(&root.join("large.txt"), "12345");

        let files = relative_files(
            &root,
            discover_workspace_files(
                std::slice::from_ref(&root),
                FileDiscoveryOptions {
                    max_file_size_bytes: 4,
                },
            ),
        );

        assert_eq!(files, vec!["small.go"]);

        fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FileDiscoveryOptions {
    pub max_file_size_bytes: u64,
}

impl Default for FileDiscoveryOptions {
    fn default() -> Self {
        Self {
            max_file_size_bytes: MAX_FILE_SIZE_BYTES,
        }
    }
}

pub fn discover_workspace(roots: &[PathBuf], options: FileDiscoveryOptions) -> FileTree {
    let files = discover_workspace_files(roots, options);
    build_file_tree(roots, files)
}

pub fn discover_workspace_files(roots: &[PathBuf], options: FileDiscoveryOptions) -> Vec<PathBuf> {
    let mut all_files = Vec::new();

    for root in roots.iter().filter(|root| root.is_dir()) {
        all_files.extend(discover_files(root, options));
    }

    all_files.sort();
    all_files
}

pub fn build_file_tree(roots: &[PathBuf], files: Vec<PathBuf>) -> FileTree {
    let mut root_nodes = Vec::new();

    for root in roots.iter().filter(|root| root.is_dir()) {
        root_nodes.push(build_root_node(root, &files));
    }

    FileTree {
        roots: root_nodes,
        files,
    }
}

fn discover_files(root: &Path, options: FileDiscoveryOptions) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    let root_for_filter = root.to_path_buf();
    builder
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .filter_entry(move |entry| {
            entry.path() == root_for_filter || !is_excluded_directory(entry.path())
        });

    let mut files = builder
        .build()
        .filter_map(Result::ok)
        .map(|entry| entry.into_path())
        .filter(|path| path.is_file())
        .filter(|path| is_discoverable_file(path, options))
        .collect::<Vec<_>>();

    files.sort();
    files
}

fn is_excluded_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            EXCLUDED_DIRECTORY_NAMES
                .iter()
                .any(|excluded| name.eq_ignore_ascii_case(excluded))
        })
}

fn is_discoverable_file(path: &Path, options: FileDiscoveryOptions) -> bool {
    if !has_discoverable_name(path) {
        return false;
    }

    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    metadata.len() <= options.max_file_size_bytes
}

fn has_discoverable_name(path: &Path) -> bool {
    if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
        if file_name == ".env" || file_name.starts_with(".env.") {
            return true;
        }
        if DISCOVERABLE_FILE_NAMES
            .iter()
            .any(|discoverable| file_name.eq_ignore_ascii_case(discoverable))
        {
            return true;
        }
    }

    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };

    DISCOVERABLE_EXTENSIONS
        .iter()
        .any(|discoverable| extension.eq_ignore_ascii_case(discoverable))
}

fn build_root_node(root: &Path, files: &[PathBuf]) -> FileNode {
    let mut builder = DirectoryBuilder::default();

    for file in files {
        let Ok(relative) = file.strip_prefix(root) else {
            continue;
        };

        builder.insert_file(root, relative, file);
    }

    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| root.display().to_string());

    FileNode {
        name: root_name,
        path: root.to_path_buf(),
        kind: FileNodeKind::Directory,
        children: builder.into_children(),
    }
}

#[derive(Debug, Default)]
struct DirectoryBuilder {
    directories: BTreeMap<String, DirectoryBuilder>,
    files: BTreeMap<String, PathBuf>,
    path: PathBuf,
}

impl DirectoryBuilder {
    fn insert_file(&mut self, root: &Path, relative: &Path, file: &Path) {
        let mut components = relative.components().peekable();
        let mut directory = self;
        let mut current_path = root.to_path_buf();

        while let Some(component) = components.next() {
            let name = component
                .as_os_str()
                .to_str()
                .unwrap_or("<invalid>")
                .to_owned();

            if components.peek().is_none() {
                directory.files.insert(name, file.to_path_buf());
                return;
            }

            current_path.push(component.as_os_str());
            directory = directory
                .directories
                .entry(name)
                .or_insert_with(|| DirectoryBuilder {
                    path: current_path.clone(),
                    ..Default::default()
                });
        }
    }

    fn into_children(self) -> Vec<FileNode> {
        let mut children = Vec::with_capacity(self.directories.len() + self.files.len());

        children.extend(
            self.directories
                .into_iter()
                .map(|(name, directory)| FileNode {
                    name,
                    path: directory.path.clone(),
                    kind: FileNodeKind::Directory,
                    children: directory.into_children(),
                }),
        );

        children.extend(self.files.into_iter().map(|(name, path)| FileNode {
            name,
            path,
            kind: FileNodeKind::File,
            children: Vec::new(),
        }));

        children
    }
}
