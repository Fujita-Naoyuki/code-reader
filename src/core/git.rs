//! Git through the `git` binary: repository discovery and state, status / branch /
//! history parsing, diff text for files, commits and the whole worktree, and the
//! push-or-publish rule. Tests run it against temporary repositories with the
//! developer's global and system config ignored (`isolate_git_config_for_tests`).

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::core::process::hide_windows_console;

#[derive(Debug, Clone)]
pub(crate) struct GitRepositoryState {
    pub(crate) root: PathBuf,
    pub(crate) name: String,
    pub(crate) branch: String,
    pub(crate) upstream: Option<String>,
    pub(crate) tracking: Option<GitTrackingStatus>,
    pub(crate) branches: Vec<String>,
    pub(crate) changes: Vec<GitChange>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitTrackingStatus {
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommitNode {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) parents: Vec<String>,
    pub(crate) subject: String,
    pub(crate) author: String,
    pub(crate) date: String,
    pub(crate) refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitGraphRow {
    pub(crate) graph: String,
    pub(crate) commit: Option<GitCommitNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommitFileChange {
    pub(crate) status: String,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
}

impl GitCommitFileChange {
    pub(crate) fn display_status(&self) -> &str {
        if self.status.starts_with('R') {
            "R"
        } else if self.status.starts_with('C') {
            "C"
        } else {
            self.status.as_str()
        }
    }

    pub(crate) fn display_path(&self) -> String {
        match &self.old_path {
            Some(old_path) => format!("{old_path} -> {}", self.path),
            None => self.path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GitChange {
    pub(crate) status: String,
    pub(crate) index_status: char,
    pub(crate) worktree_status: char,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceGitStatus {
    Modified,
    Added,
    Untracked,
}

impl WorkspaceGitStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Untracked => "?",
        }
    }
}

impl GitChange {
    pub(crate) fn is_staged(&self) -> bool {
        self.index_status != ' ' && self.index_status != '?'
    }

    pub(crate) fn has_worktree_change(&self) -> bool {
        self.worktree_status != ' ' || self.is_untracked()
    }

    pub(crate) fn is_untracked(&self) -> bool {
        self.index_status == '?' && self.worktree_status == '?'
    }

    pub(crate) fn display_status(&self) -> &str {
        if self.status.is_empty() {
            "?"
        } else {
            &self.status
        }
    }

    pub(crate) fn workspace_status(&self) -> Option<WorkspaceGitStatus> {
        if self.is_untracked() {
            Some(WorkspaceGitStatus::Untracked)
        } else if self.index_status == 'A' || self.worktree_status == 'A' {
            Some(WorkspaceGitStatus::Added)
        } else if self.index_status == 'M' || self.worktree_status == 'M' {
            Some(WorkspaceGitStatus::Modified)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitDiffScope {
    All,
    Worktree,
    Staged,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitDiffContextMode {
    Changes,
    FullFile,
}

pub(crate) fn discover_git_repositories(roots: &[PathBuf]) -> Vec<GitRepositoryState> {
    let mut repositories = Vec::new();
    let mut seen_roots = Vec::new();

    for root in roots {
        let Some(repo_root) = git_top_level(root) else {
            continue;
        };
        if seen_roots.iter().any(|seen| seen == &repo_root) {
            continue;
        }
        seen_roots.push(repo_root.clone());
        repositories.push(git_repository_state(repo_root));
    }

    repositories.sort_by(|left, right| left.name.cmp(&right.name));
    repositories
}

pub(crate) fn git_repository_state(root: PathBuf) -> GitRepositoryState {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<repository>")
        .to_owned();
    let branch = git_output(&root, &["branch", "--show-current"])
        .ok()
        .filter(|branch| !branch.is_empty())
        .or_else(|| git_output(&root, &["rev-parse", "--short", "HEAD"]).ok())
        .unwrap_or_else(|| "unknown".to_owned());
    let upstream = git_output(
        &root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()
    .filter(|upstream| !upstream.is_empty());
    let tracking = upstream.as_ref().and_then(|_| {
        git_output(
            &root,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        )
        .ok()
        .and_then(|output| parse_git_tracking_status(&output))
    });
    let branches = git_output(&root, &["branch", "--format=%(refname:short)"])
        .map(|output| parse_git_branches(&output))
        .unwrap_or_default();

    let (changes, error) = match git_output(&root, &["status", "--porcelain=v1", "-z", "-uall"]) {
        Ok(output) => (parse_git_status(&output), None),
        Err(error) => (Vec::new(), Some(error)),
    };

    GitRepositoryState {
        root,
        name,
        branch,
        upstream,
        tracking,
        branches,
        changes,
        error,
    }
}

pub(crate) fn git_top_level(path: &Path) -> Option<PathBuf> {
    let output = git_output(path, &["rev-parse", "--show-toplevel"]).ok()?;
    let path = PathBuf::from(output);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

pub(crate) fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git_command_output(cwd, args)
}

pub(crate) fn git_command_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    hide_windows_console(&mut command);
    isolate_git_config_for_tests(&mut command);
    let output = command
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;

    let stdout = trim_git_command_output(&String::from_utf8_lossy(&output.stdout));
    let stderr = trim_git_command_output(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        if stderr.is_empty() {
            Ok(stdout)
        } else if stdout.is_empty() {
            Ok(stderr)
        } else {
            Ok(format!("{stdout}\n{stderr}"))
        }
    } else {
        Err(if stdout.is_empty() && stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else if stdout.is_empty() {
            stderr
        } else if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        })
    }
}

/// Tests run real `git` against temporary repositories. Ignore the developer's global and
/// system configuration (signing, hooks, aliases, credential helpers) so results are stable.
pub(crate) fn isolate_git_config_for_tests(command: &mut Command) {
    #[cfg(test)]
    {
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                std::env::temp_dir().join("code-reader-test-gitconfig"),
            )
            .env("GIT_TERMINAL_PROMPT", "0");
    }
    #[cfg(not(test))]
    let _ = command;
}

pub(crate) fn git_history_nodes(
    repository: &GitRepositoryState,
    show_remote_branches: bool,
) -> Result<Vec<GitGraphRow>, String> {
    const FORMAT: &str = "%x1f%H%x1f%P%x1f%an%x1f%ad%x1f%D%x1f%s%x1e";
    let revision_scope = if show_remote_branches {
        "--all"
    } else {
        "--branches"
    };
    let output = git_command_output(
        &repository.root,
        &[
            "log",
            revision_scope,
            "--graph",
            "--date-order",
            "-n",
            "200",
            "--date=iso-strict",
            &format!("--pretty=format:{FORMAT}"),
        ],
    )?;
    Ok(parse_git_history_graph_log(&output))
}

pub(crate) fn git_commit_changed_files(
    root: &Path,
    commit_hash: &str,
) -> Result<Vec<GitCommitFileChange>, String> {
    let output = git_command_output(
        root,
        &[
            "show",
            "--name-status",
            "--format=",
            "--find-renames",
            commit_hash,
        ],
    )?;
    Ok(parse_git_commit_changed_files(&output))
}

pub(crate) fn git_commit_diff_text(root: &Path, commit_hash: &str) -> Result<String, String> {
    git_command_output(root, &["show", "--format=", "--find-renames", commit_hash])
}

pub(crate) fn parse_git_history_graph_log(output: &str) -> Vec<GitGraphRow> {
    output
        .lines()
        .filter_map(|record| {
            let record = record.trim_end_matches('\r');
            if record.is_empty() {
                return None;
            }
            let Some((graph, commit_fields)) = record.split_once('\x1f') else {
                let graph = record.trim_end().to_owned();
                return git_graph_prefix_has_marks(&graph).then_some(GitGraphRow {
                    graph,
                    commit: None,
                });
            };
            let graph = graph.trim_end().to_owned();
            let commit_fields = commit_fields.trim_end_matches('\x1e');
            let fields = commit_fields.splitn(6, '\x1f').collect::<Vec<_>>();
            if fields.len() != 6 {
                return None;
            }
            let hash = fields[0].to_owned();
            let parents = fields[1]
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let refs = parse_git_history_refs(fields[4]);
            Some(GitGraphRow {
                graph,
                commit: Some(GitCommitNode {
                    short_hash: hash.chars().take(8).collect(),
                    hash,
                    parents,
                    author: fields[2].to_owned(),
                    date: display_git_history_date(fields[3]),
                    refs,
                    subject: fields[5].to_owned(),
                }),
            })
        })
        .collect()
}

pub(crate) fn parse_git_commit_changed_files(output: &str) -> Vec<GitCommitFileChange> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                return None;
            }
            let parts = line.split('\t').collect::<Vec<_>>();
            let status = parts.first()?.trim();
            if status.is_empty() {
                return None;
            }
            if (status.starts_with('R') || status.starts_with('C')) && parts.len() >= 3 {
                return Some(GitCommitFileChange {
                    status: status.to_owned(),
                    old_path: Some(parts[1].to_owned()),
                    path: parts[2].to_owned(),
                });
            }
            Some(GitCommitFileChange {
                status: status.to_owned(),
                path: parts.get(1)?.to_string(),
                old_path: None,
            })
        })
        .collect()
}

pub(crate) fn git_graph_prefix_has_marks(graph: &str) -> bool {
    graph
        .chars()
        .any(|character| matches!(character, '*' | '|' | '/' | '\\' | '_' | '-'))
}

pub(crate) fn parse_git_history_refs(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .filter_map(|reference| {
            if reference == "HEAD" {
                None
            } else if let Some((_, branch)) = reference.split_once(" -> ") {
                Some(branch.to_owned())
            } else {
                Some(reference.to_owned())
            }
        })
        .collect()
}

pub(crate) fn display_git_history_date(text: &str) -> String {
    let mut date = text.chars().take(16).collect::<String>();
    date = date.replace('T', " ");
    date
}

pub(crate) fn git_all_diff_text(repository: &GitRepositoryState) -> Result<String, String> {
    let mut sections = Vec::new();

    let staged = git_command_output(&repository.root, &["diff", "--staged"])?;
    if !staged.trim().is_empty() {
        sections.push(format_git_diff_section("Staged Changes", &staged));
    }

    let worktree = git_command_output(&repository.root, &["diff"])?;
    if !worktree.trim().is_empty() {
        sections.push(format_git_diff_section("Worktree Changes", &worktree));
    }

    for change in repository
        .changes
        .iter()
        .filter(|change| change.is_untracked())
    {
        sections.push(format_git_diff_section(
            "Untracked",
            &untracked_file_diff_text(repository, change)?,
        ));
    }

    if sections.is_empty() {
        Ok("No diffs.".to_owned())
    } else {
        Ok(sections.join("\n"))
    }
}

pub(crate) fn git_file_diff_text(
    repository: &GitRepositoryState,
    change: &GitChange,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
) -> Result<String, String> {
    git_file_diff_text_for_path(
        &repository.root,
        change.path.as_str(),
        scope,
        context_mode,
        change.is_untracked(),
    )
}

pub(crate) fn git_file_diff_text_for_path(
    root: &Path,
    path: &str,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
    untracked: bool,
) -> Result<String, String> {
    if untracked && scope == GitDiffScope::Worktree {
        return untracked_file_diff_text_from_path(root, path);
    }

    let args = git_file_diff_args(path, scope, context_mode);
    let diff = git_command_output(root, &args)?;
    if diff.trim().is_empty() {
        Ok(format!("No diff for {path}."))
    } else {
        Ok(diff)
    }
}

pub(crate) fn git_file_diff_args(
    path: &str,
    scope: GitDiffScope,
    context_mode: GitDiffContextMode,
) -> Vec<&str> {
    let full_file = context_mode == GitDiffContextMode::FullFile;
    match (scope, full_file) {
        (GitDiffScope::All | GitDiffScope::Worktree, false) => vec!["diff", "--", path],
        (GitDiffScope::All | GitDiffScope::Worktree, true) => {
            vec!["diff", "-U999999", "--", path]
        }
        (GitDiffScope::Staged, false) => vec!["diff", "--staged", "--", path],
        (GitDiffScope::Staged, true) => vec!["diff", "--staged", "-U999999", "--", path],
        (GitDiffScope::Commit, false) => vec!["diff", "--", path],
        (GitDiffScope::Commit, true) => vec!["diff", "-U999999", "--", path],
    }
}

pub(crate) fn format_git_diff_section(title: &str, text: &str) -> String {
    format!("diff --code-reader {title}\n{text}\n")
}

pub(crate) fn untracked_file_diff_text(
    repository: &GitRepositoryState,
    change: &GitChange,
) -> Result<String, String> {
    untracked_file_diff_text_from_path(&repository.root, change.path.as_str())
}

pub(crate) fn untracked_file_diff_text_from_path(
    root: &Path,
    relative_path: &str,
) -> Result<String, String> {
    let path = root.join(relative_path);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read untracked file: {error}"))?;
    let mut diff = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n",
        relative_path,
        text.lines().count().max(1)
    );
    for line in text.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    if text.is_empty() {
        diff.push('+');
        diff.push('\n');
    }
    Ok(diff)
}

pub(crate) fn trim_git_command_output(output: &str) -> String {
    output.trim_end_matches(&['\r', '\n'][..]).to_owned()
}

/// Pushes the current branch, publishing it (`push -u <remote> <branch>`) when it has no
/// upstream yet.
pub(crate) fn push_or_publish_git_branch(
    repository: &GitRepositoryState,
) -> Result<String, String> {
    if repository.upstream.is_some() {
        return git_command_output(&repository.root, &["push"]);
    }
    if !repository
        .branches
        .iter()
        .any(|branch| branch == &repository.branch)
    {
        return Err("current HEAD is not on a named branch".to_owned());
    }
    let remote = git_default_remote(&repository.root)?;
    git_command_output(
        &repository.root,
        &["push", "-u", &remote, &repository.branch],
    )
}

/// Parses `git status --porcelain=v1 -z` output. NUL-separated entries keep paths with
/// spaces or non-ASCII characters verbatim (no quoting or octal escapes); a rename or copy
/// entry is followed by an extra entry holding the original path.
pub(crate) fn parse_git_status(output: &str) -> Vec<GitChange> {
    let mut entries = output.split('\0');
    let mut changes = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let index_status = chars.next().unwrap_or(' ');
        let worktree_status = chars.next().unwrap_or(' ');
        if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
            entries.next();
        }
        let Some(path) = entry.get(3..) else {
            continue;
        };
        let status = entry.get(0..2).unwrap_or_default().trim().to_owned();
        changes.push(GitChange {
            status: if status.is_empty() {
                "?".to_owned()
            } else {
                status
            },
            index_status,
            worktree_status,
            path: path.to_owned(),
        });
    }
    changes
}

pub(crate) fn parse_git_branches(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn git_default_remote(root: &Path) -> Result<String, String> {
    let output = git_command_output(root, &["remote"])?;
    preferred_git_remote(&output).ok_or_else(|| "no Git remote is configured".to_owned())
}

pub(crate) fn preferred_git_remote(output: &str) -> Option<String> {
    let remotes = output
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .collect::<Vec<_>>();
    remotes
        .iter()
        .find(|remote| **remote == "origin")
        .or_else(|| remotes.first())
        .map(|remote| (*remote).to_owned())
}

pub(crate) fn parse_git_tracking_status(output: &str) -> Option<GitTrackingStatus> {
    let mut counts = output.split_whitespace();
    let ahead = counts.next()?.parse().ok()?;
    let behind = counts.next()?.parse().ok()?;
    Some(GitTrackingStatus { ahead, behind })
}

pub(crate) fn workspace_git_file_statuses(
    repositories: &[GitRepositoryState],
) -> HashMap<PathBuf, WorkspaceGitStatus> {
    let mut statuses = HashMap::new();
    for repository in repositories {
        for change in &repository.changes {
            let Some(status) = change.workspace_status() else {
                continue;
            };
            statuses.insert(
                git_change_workspace_path(&repository.root, &change.path),
                status,
            );
        }
    }
    statuses
}

pub(crate) fn git_change_workspace_path(repository_root: &Path, git_path: &str) -> PathBuf {
    let mut path = repository_root.to_path_buf();
    for component in git_path.split('/') {
        if !component.is_empty() {
            path.push(component);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_git_command_output_preserves_porcelain_status_prefix() {
        assert_eq!(
            trim_git_command_output(" M idea.md\r\n M src/ui/app_view.rs\r\n"),
            " M idea.md\r\n M src/ui/app_view.rs"
        );
    }

    #[test]
    fn parse_git_status_keeps_unstaged_first_line_path_and_state() {
        let changes = parse_git_status(" M idea.md\0 M src/ui/app_view.rs\0");

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "idea.md");
        assert!(!changes[0].is_staged());
        assert!(changes[0].has_worktree_change());
        assert_eq!(changes[1].path, "src/ui/app_view.rs");
        assert!(!changes[1].is_staged());
    }

    #[test]
    fn parse_git_status_reads_nul_separated_paths_and_skips_rename_sources() {
        let changes = parse_git_status("R  new name.txt\0old name.txt\0?? 日本語.txt\0");

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].status, "R");
        assert_eq!(changes[0].path, "new name.txt");
        assert_eq!(changes[1].status, "??");
        assert_eq!(changes[1].path, "日本語.txt");
    }

    #[test]
    fn parse_git_tracking_status_reads_ahead_and_behind_counts() {
        assert_eq!(
            parse_git_tracking_status("0\t3\n"),
            Some(GitTrackingStatus {
                ahead: 0,
                behind: 3,
            })
        );
    }

    #[test]
    fn preferred_git_remote_prefers_origin() {
        assert_eq!(
            preferred_git_remote("upstream\norigin\nfork\n"),
            Some("origin".to_owned())
        );
    }

    #[test]
    fn preferred_git_remote_uses_first_remote_without_origin() {
        assert_eq!(
            preferred_git_remote("upstream\nfork\n"),
            Some("upstream".to_owned())
        );
    }

    #[test]
    fn preferred_git_remote_returns_none_for_empty_output() {
        assert_eq!(preferred_git_remote("\n  \n"), None);
    }

    #[test]
    fn workspace_git_file_statuses_maps_git_paths_to_workspace_paths() {
        let repository_root = PathBuf::from(r"C:\git\code-reader");
        let repository = GitRepositoryState {
            root: repository_root.clone(),
            name: "code-reader".to_owned(),
            branch: "main".to_owned(),
            upstream: Some("origin/main".to_owned()),
            tracking: None,
            branches: vec!["main".to_owned()],
            changes: vec![
                GitChange {
                    status: "M".to_owned(),
                    index_status: ' ',
                    worktree_status: 'M',
                    path: "src/ui/app_view.rs".to_owned(),
                },
                GitChange {
                    status: "?".to_owned(),
                    index_status: '?',
                    worktree_status: '?',
                    path: "notes/todo.md".to_owned(),
                },
            ],
            error: None,
        };

        let statuses = workspace_git_file_statuses(&[repository]);

        assert_eq!(
            statuses.get(&repository_root.join("src").join("ui").join("app_view.rs")),
            Some(&WorkspaceGitStatus::Modified)
        );
        assert_eq!(
            statuses.get(&repository_root.join("notes").join("todo.md")),
            Some(&WorkspaceGitStatus::Untracked)
        );
    }

    #[test]
    fn parse_git_history_graph_log_reads_commit_nodes_refs_and_graph_rows() {
        let output = concat!(
            "* \x1faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f",
            "Fujita Naoyuki\x1f2026-05-28T20:30:12+09:00\x1fHEAD -> main, origin/main\x1f",
            "Diff graph MVP\x1e\n",
            "|/  \n",
            "* \x1fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x1f\x1f",
            "Fujita Naoyuki\x1f2026-05-27T23:59:00+09:00\x1f\x1fInitial graph\x1e\n",
        );

        let rows = parse_git_history_graph_log(output);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].graph, "*");
        let first_commit = rows[0]
            .commit
            .as_ref()
            .expect("first row should contain a commit");
        assert_eq!(first_commit.short_hash, "aaaaaaaa");
        assert_eq!(
            first_commit.parents,
            vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
        );
        assert_eq!(first_commit.refs, vec!["main", "origin/main"]);
        assert_eq!(first_commit.date, "2026-05-28 20:30");
        assert_eq!(rows[1].graph, "|/");
        assert!(rows[1].commit.is_none());
        assert_eq!(rows[2].graph, "*");
        assert_eq!(
            rows[2]
                .commit
                .as_ref()
                .map(|commit| commit.short_hash.as_str()),
            Some("bbbbbbbb")
        );
    }

    #[test]
    fn parse_git_commit_changed_files_reads_status_and_renames() {
        let files = parse_git_commit_changed_files(concat!(
            "M\tREADME.md\n",
            "A\tsrc/app.rs\n",
            "R100\told.rs\tnew.rs\n",
            "D\tremoved.txt\n",
        ));

        assert_eq!(
            files,
            vec![
                GitCommitFileChange {
                    status: "M".to_owned(),
                    path: "README.md".to_owned(),
                    old_path: None,
                },
                GitCommitFileChange {
                    status: "A".to_owned(),
                    path: "src/app.rs".to_owned(),
                    old_path: None,
                },
                GitCommitFileChange {
                    status: "R100".to_owned(),
                    path: "new.rs".to_owned(),
                    old_path: Some("old.rs".to_owned()),
                },
                GitCommitFileChange {
                    status: "D".to_owned(),
                    path: "removed.txt".to_owned(),
                    old_path: None,
                },
            ]
        );
    }

    #[test]
    fn git_file_diff_args_request_changes_context() {
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Worktree,
                GitDiffContextMode::Changes
            ),
            vec!["diff", "--", "test.md"]
        );
        assert_eq!(
            git_file_diff_args("test.md", GitDiffScope::Staged, GitDiffContextMode::Changes),
            vec!["diff", "--staged", "--", "test.md"]
        );
    }

    #[test]
    fn git_file_diff_args_request_full_file_context() {
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Worktree,
                GitDiffContextMode::FullFile
            ),
            vec!["diff", "-U999999", "--", "test.md"]
        );
        assert_eq!(
            git_file_diff_args(
                "test.md",
                GitDiffScope::Staged,
                GitDiffContextMode::FullFile
            ),
            vec!["diff", "--staged", "-U999999", "--", "test.md"]
        );
    }
}
