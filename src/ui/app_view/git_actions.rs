//! Git commands started from the UI and opening their results (diffs, history).

use super::*;

impl CodeReaderApp {
    pub(super) fn open_git_all_diff(&mut self, repository: &GitRepositoryState) {
        match git_all_diff_text(repository) {
            Ok(text) => {
                let title = format!("{}: All Diffs", repository.name);
                let diff_view = GitDiffView {
                    title,
                    repository_name: repository.name.clone(),
                    repository_root: Some(repository.root.clone()),
                    file_path: None,
                    scope: GitDiffScope::All,
                    context_mode: GitDiffContextMode::Changes,
                    untracked: false,
                    return_history: None,
                };
                let pane = self.focused_pane;
                let next = EditorPane::from_diff(pane, diff_view, text);
                self.replace_editor(
                    pane,
                    next,
                    false,
                    Some(format!("Opened all diffs: {}", repository.name)),
                );
            }
            Err(error) => {
                self.status_message =
                    format!("Open all diffs failed: {} | {error}", repository.name);
            }
        }
    }

    pub(super) fn open_git_file_diff(
        &mut self,
        repository: &GitRepositoryState,
        change: &GitChange,
        scope: GitDiffScope,
    ) {
        let context_mode = GitDiffContextMode::Changes;
        match git_file_diff_text(repository, change, scope, context_mode) {
            Ok(text) => {
                let suffix = match scope {
                    GitDiffScope::All => "Diff",
                    GitDiffScope::Worktree => "Diff",
                    GitDiffScope::Staged => "Staged Diff",
                    GitDiffScope::Commit => "Commit Diff",
                };
                let title = format!("{}: {suffix}", change.path);
                let diff_view = GitDiffView {
                    title,
                    repository_name: repository.name.clone(),
                    repository_root: Some(repository.root.clone()),
                    file_path: Some(change.path.clone()),
                    scope,
                    context_mode,
                    untracked: change.is_untracked(),
                    return_history: None,
                };
                let pane = self.focused_pane;
                let next = EditorPane::from_diff(pane, diff_view, text);
                self.replace_editor(
                    pane,
                    next,
                    false,
                    Some(format!(
                        "Opened diff: {} | {}",
                        repository.name, change.path
                    )),
                );
            }
            Err(error) => {
                self.status_message = format!(
                    "Open diff failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
    }

    pub(super) fn open_git_history(&mut self, repository: &GitRepositoryState) {
        match git_history_nodes(repository, true) {
            Ok(rows) => {
                let history_view = GitHistoryView {
                    repository_name: repository.name.clone(),
                    root: repository.root.clone(),
                    rows,
                    selected_commit_hash: None,
                };
                let pane = self.focused_pane;
                let next = EditorPane::from_git_history(pane, history_view);
                let status = format!("Opened Git graph: {}", repository.name);
                self.replace_editor(pane, next, false, Some(status));
            }
            Err(error) => {
                self.status_message =
                    format!("Open Git graph failed: {} | {error}", repository.name);
            }
        }
    }

    pub(super) fn open_git_commit_details(
        &mut self,
        repository_name: String,
        root: PathBuf,
        commit: GitCommitNode,
    ) {
        let (files, error) = match git_commit_changed_files(&root, &commit.hash) {
            Ok(files) => (files, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.git_commit_details = Some(GitCommitDetailsWindow {
            repository_name,
            root,
            commit: commit.clone(),
            files,
            error,
        });
        self.status_message = format!(
            "Opened commit details: {} {}",
            commit.short_hash, commit.subject
        );
    }

    pub(super) fn open_git_commit_diff(&mut self, details: &GitCommitDetailsWindow) {
        let return_history = self.focused_editor().git_history_view.clone().map(|view| {
            let editor = self.focused_editor();
            GitHistoryReturnState {
                view,
                scroll_y: editor.scroll.y,
                scroll_x: editor.scroll.x,
            }
        });
        match git_commit_diff_text(&details.root, &details.commit.hash) {
            Ok(text) => {
                let short_hash = details.commit.short_hash.clone();
                let title = format!("{}: {short_hash} Diff", details.repository_name);
                let diff_view = GitDiffView {
                    title,
                    repository_name: details.repository_name.clone(),
                    repository_root: Some(details.root.clone()),
                    file_path: None,
                    scope: GitDiffScope::Commit,
                    context_mode: GitDiffContextMode::Changes,
                    untracked: false,
                    return_history,
                };
                let pane = self.focused_pane;
                let next = EditorPane::from_diff(pane, diff_view, text);
                self.replace_editor(
                    pane,
                    next,
                    false,
                    Some(format!("Opened commit diff: {short_hash}")),
                );
            }
            Err(error) => {
                self.status_message = format!(
                    "Open commit diff failed: {} | {error}",
                    details.commit.short_hash
                );
            }
        }
    }

    pub(super) fn fetch_git_repository(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["fetch"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Fetch completed.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git fetch completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!("Git fetch failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn pull_git_repository(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["pull", "--ff-only"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Already up to date.".to_owned()
                } else {
                    output
                };
                self.status_message = format!("Git pull completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!("Git pull failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn push_git_repository(&mut self, repository: &GitRepositoryState) {
        match push_or_publish_git_branch(repository) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    if repository.upstream.is_some() {
                        "Push completed.".to_owned()
                    } else {
                        "Branch published.".to_owned()
                    }
                } else {
                    output
                };
                if repository.upstream.is_some() {
                    self.status_message =
                        format!("Git push completed: {} | {detail}", repository.name);
                } else {
                    self.status_message =
                        format!("Git publish completed: {} | {detail}", repository.name);
                }
            }
            Err(error) => {
                if repository.upstream.is_some() {
                    self.status_message = format!("Git push failed: {} | {error}", repository.name);
                } else {
                    self.status_message =
                        format!("Git publish failed: {} | {error}", repository.name);
                }
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn commit_git_repository(
        &mut self,
        repository: &GitRepositoryState,
        push_after_commit: bool,
    ) {
        let message = self
            .git_commit_messages
            .get(&repository.root)
            .map(|message| message.trim().to_owned())
            .unwrap_or_default();
        if message.is_empty() {
            self.status_message =
                format!("Git commit failed: {} | message is empty", repository.name);
            return;
        }

        match git_command_output(&repository.root, &["commit", "-m", &message]) {
            Ok(output) => {
                self.git_commit_messages
                    .entry(repository.root.clone())
                    .or_default()
                    .clear();
                let commit_detail = if output.is_empty() {
                    "Commit completed.".to_owned()
                } else {
                    output
                };
                if push_after_commit {
                    match push_or_publish_git_branch(repository) {
                        Ok(push_output) => {
                            let push_detail = if push_output.is_empty() {
                                "Push completed.".to_owned()
                            } else {
                                push_output
                            };
                            self.status_message = format!(
                                "Git commit & push completed: {} | {commit_detail} | {push_detail}",
                                repository.name
                            );
                        }
                        Err(error) => {
                            self.status_message = format!(
                                "Git commit completed but push failed: {} | {commit_detail} | {error}",
                                repository.name
                            );
                        }
                    }
                } else {
                    self.status_message = format!(
                        "Git commit completed: {} | {commit_detail}",
                        repository.name
                    );
                }
            }
            Err(error) => {
                self.status_message = format!("Git commit failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn stage_all_git_changes(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["add", "-A"]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Staged all changes.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git stage all completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git stage all failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn stage_git_file(&mut self, repository: &GitRepositoryState, change: &GitChange) {
        match git_command_output(&repository.root, &["add", "--", &change.path]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Staged {}.", change.path)
                } else {
                    output
                };
                self.status_message =
                    format!("Git stage completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!(
                    "Git stage failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn unstage_all_git_changes(&mut self, repository: &GitRepositoryState) {
        match git_command_output(&repository.root, &["restore", "--staged", "--", "."]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    "Unstaged all changes.".to_owned()
                } else {
                    output
                };
                self.status_message =
                    format!("Git unstage all completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git unstage all failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn unstage_git_file(&mut self, repository: &GitRepositoryState, change: &GitChange) {
        match git_command_output(
            &repository.root,
            &["restore", "--staged", "--", &change.path],
        ) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Unstaged {}.", change.path)
                } else {
                    output
                };
                self.status_message =
                    format!("Git unstage completed: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message = format!(
                    "Git unstage failed: {} | {} | {error}",
                    repository.name, change.path
                );
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn discard_git_file(&mut self, request: &GitDiscardRequest) {
        let args = if request.untracked {
            ["clean", "-f", "--", request.path.as_str()]
        } else {
            ["restore", "--worktree", "--", request.path.as_str()]
        };
        match git_command_output(&request.repository_root, &args) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Discarded {}.", request.path)
                } else {
                    output
                };
                self.status_message = format!(
                    "Git discard completed: {} | {detail}",
                    request.repository_name
                );
            }
            Err(error) => {
                self.status_message = format!(
                    "Git discard failed: {} | {} | {error}",
                    request.repository_name, request.path
                );
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn switch_git_branch(&mut self, repository: &GitRepositoryState, branch_name: &str) {
        match git_command_output(&repository.root, &["switch", branch_name]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Switched to {branch_name}.")
                } else {
                    output
                };
                self.status_message =
                    format!("Git branch switched: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git branch switch failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }

    pub(super) fn create_git_branch(&mut self, repository: &GitRepositoryState, branch_name: &str) {
        match git_command_output(&repository.root, &["switch", "-c", branch_name]) {
            Ok(output) => {
                let detail = if output.is_empty() {
                    format!("Created and switched to {branch_name}.")
                } else {
                    output
                };
                self.status_message = format!("Git branch created: {} | {detail}", repository.name);
            }
            Err(error) => {
                self.status_message =
                    format!("Git branch create failed: {} | {error}", repository.name);
            }
        }
        self.reload_git_repositories();
    }
}
