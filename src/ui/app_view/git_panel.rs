//! The Git sidebar: repositories, commit box and changed files.

use super::*;

impl CodeReaderApp {
    pub(super) fn refresh_git_view(&mut self) {
        self.reload_git_repositories();
        self.status_message = format!("Git: {} repositories.", self.git.repositories.len());
    }

    pub(super) fn ensure_git_repositories_loaded(&mut self) {
        if !self.git.repositories_loaded {
            self.reload_git_repositories();
        }
    }

    pub(super) fn reload_git_repositories(&mut self) {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.git.repositories = discover_git_repositories(&roots);
        self.git.repositories_loaded = true;
        self.git
            .commit_messages
            .retain(|root, _| self.git.repositories.iter().any(|repo| &repo.root == root));
        self.git
            .repo_open
            .retain(|root, _| self.git.repositories.iter().any(|repo| &repo.root == root));
        self.git
            .new_branch_names
            .retain(|root, _| self.git.repositories.iter().any(|repo| &repo.root == root));
        if self
            .git
            .create_branch_root
            .as_ref()
            .is_some_and(|root| !self.git.repositories.iter().any(|repo| &repo.root == root))
        {
            self.git.create_branch_root = None;
        }
        if self
            .git
            .switch_branch_root
            .as_ref()
            .is_some_and(|root| !self.git.repositories.iter().any(|repo| &repo.root == root))
        {
            self.git.switch_branch_root = None;
            self.git.switch_branch_cursor = None;
        }
        for repository in &self.git.repositories {
            self.git
                .commit_messages
                .entry(repository.root.clone())
                .or_default();
            self.git
                .repo_open
                .entry(repository.root.clone())
                .or_insert(true);
            self.git
                .new_branch_names
                .entry(repository.root.clone())
                .or_default();
        }
    }

    pub(super) fn git_ui(&mut self, ui: &mut egui::Ui) {
        if !self.git.repositories_loaded {
            self.refresh_git_view();
        }

        if self.git.repositories.is_empty() {
            ui.label(RichText::new("No Git repositories found in this workspace.").weak());
            return;
        }

        egui::CollapsingHeader::new("Repositories")
            .default_open(true)
            .show(ui, |ui| {
                for repository in self.git.repositories.clone() {
                    self.git_repository_row_ui(ui, &repository);
                }
            });

        egui::CollapsingHeader::new("Changes")
            .default_open(true)
            .show(ui, |ui| {
                for repository in self.git.repositories.clone() {
                    self.git_repository_changes_ui(ui, &repository);
                    ui.add_space(4.0);
                }
            });
    }

    /// One repository: name, branch, ahead / behind, Pull and the "..." menu.
    fn git_repository_row_ui(&mut self, ui: &mut egui::Ui, repository: &GitRepositoryState) {
        let tracking = git_tracking_label(repository);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&repository.name));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.git_repository_menu(ui, repository);
                if ui.button("Pull").clicked() {
                    self.pull_git_repository(repository);
                }
                if let Some((label, tooltip)) = tracking {
                    ui.label(RichText::new(label).weak()).on_hover_text(tooltip);
                }

                let branch_label = format!("{} ▾", repository.branch);
                if ui
                    .add_sized(
                        [72.0, ui.spacing().interact_size.y],
                        egui::Label::new(RichText::new(branch_label).monospace())
                            .sense(egui::Sense::click())
                            .truncate(),
                    )
                    .on_hover_text(format!("{}\nSwitch Branch", repository.branch))
                    .clicked()
                {
                    self.git.switch_branch_root = Some(repository.root.clone());
                }
            });
        });
        if let Some(error) = &repository.error {
            ui.label(
                RichText::new(error)
                    .small()
                    .color(ui.visuals().warn_fg_color),
            );
        }
    }

    /// One repository's collapsible section: its commit box and changed files.
    fn git_repository_changes_ui(&mut self, ui: &mut egui::Ui, repository: &GitRepositoryState) {
        let change_count = repository.changes.len();
        let default_open = self
            .git
            .repo_open
            .get(&repository.root)
            .copied()
            .unwrap_or(true);
        let state_id = ui.make_persistent_id(("git_repo_changes", &repository.root));
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            state_id,
            default_open,
        );
        let mut open_all_diffs = false;
        let mut open_history = false;
        let header = state.show_header(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} ({change_count})", repository.name));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("History")
                        .on_hover_text("Open Git history")
                        .clicked()
                    {
                        open_history = true;
                    }
                    if ui
                        .add_enabled(
                            !repository.changes.is_empty(),
                            egui::Button::new("All Diffs"),
                        )
                        .on_hover_text("Open all repository diffs")
                        .clicked()
                    {
                        open_all_diffs = true;
                    }
                });
            });
        });
        self.git
            .repo_open
            .insert(repository.root.clone(), header.is_open());
        if open_all_diffs {
            self.open_git_all_diff(repository);
        }
        if open_history {
            self.open_git_history(repository);
        }
        header.body(|ui| {
            self.git_commit_box_ui(ui, repository);
            self.git_changes_files_ui(ui, repository);
        });
    }

    fn git_commit_box_ui(&mut self, ui: &mut egui::Ui, repository: &GitRepositoryState) {
        let mut commit = false;
        let mut commit_and_push = false;
        let message = self
            .git
            .commit_messages
            .entry(repository.root.clone())
            .or_default();
        ui.add(
            TextEdit::multiline(message)
                .id(ui.make_persistent_id(("git_commit_message", &repository.root)))
                .hint_text("Commit message")
                .desired_width(f32::INFINITY)
                .desired_rows(4),
        );
        ui.horizontal(|ui| {
            if ui.button("Commit").clicked() {
                commit = true;
            }
            if ui.button("Commit & Push").clicked() {
                commit_and_push = true;
            }
        });

        if commit {
            self.commit_git_repository(repository, false);
        }
        if commit_and_push {
            self.commit_git_repository(repository, true);
        }
    }

    pub(super) fn git_repository_menu(
        &mut self,
        ui: &mut egui::Ui,
        repository: &GitRepositoryState,
    ) {
        ui.menu_button("...", |ui| {
            if ui.button("Fetch").clicked() {
                self.fetch_git_repository(repository);
                ui.close();
            }

            let push_label = if repository.upstream.is_some() {
                "Push"
            } else {
                "Publish Branch"
            };
            let can_push = repository.upstream.is_some()
                || repository
                    .branches
                    .iter()
                    .any(|branch| branch == &repository.branch);
            if ui
                .add_enabled(can_push, egui::Button::new(push_label))
                .on_hover_text(if repository.upstream.is_some() {
                    "Push commits to the upstream branch"
                } else {
                    "Push this branch and set upstream"
                })
                .clicked()
            {
                self.push_git_repository(repository);
                ui.close();
            }

            if ui.button("Switch Branch...").clicked() {
                self.git.switch_branch_root = Some(repository.root.clone());
                ui.close();
            }

            if ui.button("Create Branch...").clicked() {
                self.git.create_branch_root = Some(repository.root.clone());
                self.git
                    .new_branch_names
                    .entry(repository.root.clone())
                    .or_default()
                    .clear();
                ui.close();
            }
        })
        .response
        .on_hover_text("More Git actions");
    }

    pub(super) fn git_changes_files_ui(
        &mut self,
        ui: &mut egui::Ui,
        repository: &GitRepositoryState,
    ) {
        let staged_changes = repository
            .changes
            .iter()
            .filter(|change| change.is_staged())
            .collect::<Vec<_>>();
        let changed_files = repository
            .changes
            .iter()
            .filter(|change| change.has_worktree_change())
            .collect::<Vec<_>>();

        if repository.changes.is_empty() {
            ui.label(RichText::new("No changes").small().weak());
            return;
        }

        if !staged_changes.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Staged Changes").small().strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("- Unstage All").clicked() {
                        self.unstage_all_git_changes(repository);
                    }
                });
            });
            for change in staged_changes {
                self.git_change_row(ui, repository, change, false);
            }
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Changed Files").small().strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!changed_files.is_empty(), egui::Button::new("+ Stage All"))
                    .clicked()
                {
                    self.stage_all_git_changes(repository);
                }
            });
        });
        if changed_files.is_empty() {
            ui.label(RichText::new("No unstaged changes").small().weak());
        }
        for change in changed_files {
            self.git_change_row(ui, repository, change, true);
        }
    }

    pub(super) fn git_change_row(
        &mut self,
        ui: &mut egui::Ui,
        repository: &GitRepositoryState,
        change: &GitChange,
        show_worktree_actions: bool,
    ) {
        let row_width = ui.available_width();
        let row_height = 24.0;
        let status_width = 28.0;
        let action_button_width = 24.0;
        let action_gap = 4.0;
        let action_count = if show_worktree_actions { 2.0 } else { 1.0 };
        let action_width = action_count * action_button_width + (action_count - 1.0) * action_gap;
        let column_gap = 8.0;
        let (row_rect, _) =
            ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::hover());
        let status_rect =
            egui::Rect::from_min_size(row_rect.min, egui::vec2(status_width, row_height));
        let action_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.right() - action_width, row_rect.top() + 1.0),
            egui::vec2(action_width, row_height - 2.0),
        );
        let path_rect = egui::Rect::from_min_max(
            egui::pos2(status_rect.right() + column_gap, row_rect.top()),
            egui::pos2(action_rect.left() - column_gap, row_rect.bottom()),
        );
        let monospace_font = TextStyle::Monospace.resolve(ui.style());
        let draw_left_text = |ui: &egui::Ui, rect: egui::Rect, text: &str| {
            ui.painter().with_clip_rect(rect).text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                text,
                monospace_font.clone(),
                ui.visuals().text_color(),
            );
        };

        draw_left_text(ui, status_rect, change.display_status());
        let path_response = ui.interact(
            path_rect,
            ui.make_persistent_id(("git_change_path", &repository.root, &change.path)),
            egui::Sense::click(),
        );
        if path_response.hovered() {
            ui.painter().rect_filled(
                path_rect.shrink2(egui::vec2(0.0, 2.0)),
                2.0,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        draw_left_text(ui, path_rect, &change.path);
        if path_response
            .on_hover_text(format!("{}\nOpen diff", change.path))
            .clicked()
        {
            let scope = if show_worktree_actions {
                GitDiffScope::Worktree
            } else {
                GitDiffScope::Staged
            };
            self.open_git_file_diff(repository, change, scope);
        }

        if show_worktree_actions {
            let discard_rect = egui::Rect::from_min_size(
                action_rect.min,
                egui::vec2(action_button_width, action_rect.height()),
            );
            let stage_rect = egui::Rect::from_min_size(
                egui::pos2(discard_rect.right() + action_gap, action_rect.top()),
                egui::vec2(action_button_width, action_rect.height()),
            );
            if ui
                .put(discard_rect, egui::Button::new("↶"))
                .on_hover_text("Discard changes")
                .clicked()
            {
                self.git.pending_discard = Some(GitDiscardRequest {
                    repository_root: repository.root.clone(),
                    repository_name: repository.name.clone(),
                    path: change.path.clone(),
                    untracked: change.is_untracked(),
                });
            }
            if ui
                .put(stage_rect, egui::Button::new("+"))
                .on_hover_text("Stage file")
                .clicked()
            {
                self.stage_git_file(repository, change);
            }
        } else if ui
            .put(action_rect, egui::Button::new("-"))
            .on_hover_text("Unstage file")
            .clicked()
        {
            self.unstage_git_file(repository, change);
        }
    }
}

impl WorkspaceGitStatus {
    pub(super) fn color(self) -> Color32 {
        match self {
            Self::Modified => Color32::from_rgb(196, 160, 72),
            Self::Added => Color32::from_rgb(72, 160, 92),
            Self::Untracked => Color32::from_rgb(86, 156, 214),
        }
    }
}

/// The ahead / behind label and its tooltip, when there is anything to pull or push.
fn git_tracking_label(repository: &GitRepositoryState) -> Option<(String, String)> {
    let tracking = repository.tracking.as_ref()?;
    if tracking.ahead == 0 && tracking.behind == 0 {
        return None;
    }
    let upstream = repository.upstream.as_deref().unwrap_or("upstream");
    Some((
        format!("{}↓ {}↑", tracking.behind, tracking.ahead),
        format!(
            "{} commits to pull from {upstream}\n{} commits to push to {upstream}",
            tracking.behind, tracking.ahead
        ),
    ))
}
