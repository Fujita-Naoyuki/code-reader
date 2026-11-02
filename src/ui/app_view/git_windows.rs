//! Git dialogs: create branch, switch branch, discard changes, commit details.

use super::*;

impl CodeReaderApp {
    pub(super) fn show_git_create_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git.create_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git
            .repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git.create_branch_root = None;
            return;
        };

        let mut create_branch = None;
        let mut close = false;
        let input_id = egui::Id::new(("git_create_branch_name", &repository.root));

        egui::Window::new("Create Branch")
            .id(egui::Id::new("git_create_branch_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(420.0, 160.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        self.layout.modal_escape == Some(Modal::GitCreateBranch),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }

                ui.label(RichText::new("Esc: close").weak());
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label(RichText::new(&repository.name).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("from: {}", repository.branch))
                                .monospace()
                                .weak(),
                        )
                        .on_hover_text(&repository.branch);
                    });
                });
                ui.separator();

                ui.label(RichText::new("Branch name"));
                let new_branch_name = self
                    .git
                    .new_branch_names
                    .entry(repository.root.clone())
                    .or_default();
                let response = ui.add(
                    TextEdit::singleline(new_branch_name)
                        .id(input_id)
                        .hint_text("feature/new-branch")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();

                let can_create = !new_branch_name.trim().is_empty();
                if enter && can_create {
                    create_branch = Some(new_branch_name.trim().to_owned());
                    new_branch_name.clear();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(can_create, egui::Button::new("Create"))
                        .clicked()
                    {
                        create_branch = Some(new_branch_name.trim().to_owned());
                        new_branch_name.clear();
                    }
                });
            });

        if let Some(branch_name) = create_branch {
            self.git.create_branch_root = None;
            self.create_git_branch(&repository, &branch_name);
        } else if close {
            self.git.create_branch_root = None;
        }
    }

    pub(super) fn show_git_switch_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git.switch_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git
            .repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git.switch_branch_root = None;
            return;
        };

        let mut switch_to_branch = None;
        let mut close = false;
        let mut cursor = self
            .git
            .switch_branch_cursor
            .clone()
            .filter(|branch| repository.branches.contains(branch))
            .unwrap_or_else(|| repository.branch.clone());

        egui::Window::new("Switch Branch")
            .id(egui::Id::new("git_switch_branch_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(480.0, 320.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, up, down, enter) = ui.input_mut(|input| {
                    (
                        self.layout.modal_escape == Some(Modal::GitSwitchBranch),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }
                if let Some(branch) = moved_branch_cursor(&repository.branches, &cursor, up, down) {
                    cursor = branch.clone();
                }
                if enter {
                    if cursor == repository.branch || !repository.branches.contains(&cursor) {
                        close = true;
                    } else {
                        switch_to_branch = Some(cursor.clone());
                    }
                }

                branch_window_header_ui(ui, &repository);

                if repository.branches.is_empty() {
                    ui.label(RichText::new("No local branches").weak());
                } else {
                    ScrollArea::vertical()
                        .id_salt("git_switch_branch_list")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for branch in &repository.branches {
                                let row = BranchRow {
                                    current: branch == &repository.branch,
                                    highlighted: branch == &cursor,
                                    scroll_to: branch == &cursor && (up || down),
                                };
                                if branch_row_ui(ui, branch, row) {
                                    switch_to_branch = Some(branch.clone());
                                }
                            }
                        });
                }
            });

        if let Some(branch_name) = switch_to_branch {
            self.git.switch_branch_root = None;
            self.git.switch_branch_cursor = None;
            self.switch_git_branch(&repository, &branch_name);
        } else if close {
            self.git.switch_branch_root = None;
            self.git.switch_branch_cursor = None;
        } else {
            self.git.switch_branch_cursor = Some(cursor);
        }
    }

    pub(super) fn show_git_discard_window(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.git.pending_discard.clone() else {
            return;
        };

        let mut discard = false;
        let mut close = false;
        egui::Window::new("Discard Changes")
            .id(egui::Id::new("git_discard_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(440.0, 180.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        self.layout.modal_escape == Some(Modal::GitDiscard),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                } else if enter {
                    discard = true;
                }

                ui.label(RichText::new("Repository").small().weak());
                ui.label(RichText::new(&request.repository_name).monospace());
                ui.add_space(8.0);
                ui.label(RichText::new("File").small().weak());
                ui.label(RichText::new(&request.path).monospace());
                ui.add_space(8.0);
                ui.label(
                    RichText::new("This will discard local changes for this file.")
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Enter confirms, so mark Discard as the default button.
                    if ui
                        .add(egui::Button::new("Discard").stroke(ui.visuals().selection.stroke))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        if discard {
            self.git.pending_discard = None;
            self.discard_git_file(&request);
        } else if close {
            self.git.pending_discard = None;
        }
    }

    pub(super) fn show_git_commit_details_window(&mut self, ui: &mut egui::Ui) {
        let Some(details) = self.git.commit_details.clone() else {
            return;
        };

        let mut close = false;
        let mut open_commit_diff = false;
        egui::Window::new("Commit Details")
            .id(egui::Id::new("git_commit_details_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(760.0, 520.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let escape = self.layout.modal_escape == Some(Modal::GitCommitDetails);
                if escape {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(&details.repository_name).monospace());
                    ui.separator();
                    ui.label(RichText::new(&details.commit.short_hash).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Open Diff").clicked() {
                            open_commit_diff = true;
                        }
                    });
                });
                ui.separator();

                commit_metadata_ui(ui, &details.commit);
                commit_changed_files_ui(ui, &details);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        if open_commit_diff {
            self.open_git_commit_diff(&details);
            self.git.commit_details = None;
            return;
        }
        if close {
            self.git.commit_details = None;
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct GitCommitDetailsWindow {
    pub(super) repository_name: String,
    pub(super) root: PathBuf,
    pub(super) commit: GitCommitNode,
    pub(super) files: Vec<GitCommitFileChange>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct GitDiscardRequest {
    pub(super) repository_root: PathBuf,
    pub(super) repository_name: String,
    pub(super) path: String,
    pub(super) untracked: bool,
}

fn commit_metadata_ui(ui: &mut egui::Ui, commit: &GitCommitNode) {
    ui.label(RichText::new(&commit.subject).strong());
    ui.add_space(4.0);
    egui::Grid::new("git_commit_details_metadata")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Author").weak());
            ui.label(&commit.author);
            ui.end_row();
            ui.label(RichText::new("Date").weak());
            ui.label(&commit.date);
            ui.end_row();
            ui.label(RichText::new("Hash").weak());
            ui.label(RichText::new(&commit.hash).monospace());
            ui.end_row();
            ui.label(RichText::new("Parents").weak());
            ui.label(if commit.parents.is_empty() {
                "none".to_owned()
            } else {
                commit.parents.join(", ")
            });
            ui.end_row();
        });

    if !commit.refs.is_empty() {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Refs").weak());
            for reference in &commit.refs {
                let text = RichText::new(reference)
                    .monospace()
                    .small()
                    .color(Color32::WHITE);
                egui::Frame::new()
                    .fill(git_ref_badge_color(reference))
                    .corner_radius(3.0)
                    .inner_margin(egui::Margin::symmetric(5, 2))
                    .show(ui, |ui| {
                        ui.label(text);
                    });
            }
        });
    }
}

fn commit_changed_files_ui(ui: &mut egui::Ui, details: &GitCommitDetailsWindow) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Changed Files").strong());
        ui.label(RichText::new(format!("{}", details.files.len())).weak());
    });
    ui.separator();

    if let Some(error) = &details.error {
        ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
    } else if details.files.is_empty() {
        ui.label(RichText::new("No changed files").weak());
    } else {
        ScrollArea::vertical()
            .id_salt("git_commit_details_files")
            .max_height(220.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for file in &details.files {
                    let display_path = file.display_path();
                    let label = format!("{:<3} {}", file.display_status(), display_path);
                    ui.label(RichText::new(label).monospace());
                }
            });
    }
}

/// Where Up / Down moves the Switch Branch highlight.
fn moved_branch_cursor<'a>(
    branches: &'a [String],
    cursor: &str,
    up: bool,
    down: bool,
) -> Option<&'a String> {
    let cursor_index = branches.iter().position(|branch| branch == cursor);
    let moved_index = match cursor_index {
        Some(index) if up => Some(index.saturating_sub(1)),
        Some(index) if down => Some((index + 1).min(branches.len().saturating_sub(1))),
        None if up || down => Some(0),
        _ => None,
    };
    moved_index.and_then(|index| branches.get(index))
}

#[derive(Clone, Copy)]
struct BranchRow {
    current: bool,
    highlighted: bool,
    scroll_to: bool,
}

/// One branch in the Switch Branch list. Returns true when it was clicked.
fn branch_row_ui(ui: &mut egui::Ui, branch: &str, row: BranchRow) -> bool {
    const ROW_HEIGHT: f32 = 24.0;
    const CURRENT_WIDTH: f32 = 72.0;
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let current_rect = egui::Rect::from_min_size(
        egui::pos2(row_rect.right() - CURRENT_WIDTH, row_rect.top()),
        egui::vec2(CURRENT_WIDTH, ROW_HEIGHT),
    );
    let branch_rect = egui::Rect::from_min_max(
        row_rect.min,
        egui::pos2(current_rect.left() - 8.0, row_rect.bottom()),
    );
    let response = ui.interact(
        row_rect,
        ui.make_persistent_id(("git_switch_branch", branch)),
        if row.current {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        },
    );
    let fill_rect = row_rect.shrink2(egui::vec2(0.0, 2.0));
    if response.hovered() || row.highlighted {
        ui.painter()
            .rect_filled(fill_rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
    }
    if row.scroll_to {
        ui.scroll_to_rect(row_rect, None);
    }
    if row.current {
        ui.painter()
            .rect_filled(fill_rect, 2.0, ui.visuals().selection.bg_fill);
        if row.highlighted {
            ui.painter().rect_stroke(
                fill_rect,
                2.0,
                ui.visuals().widgets.hovered.fg_stroke,
                egui::StrokeKind::Inside,
            );
        }
    }

    let text_color = if row.current {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color()
    };
    paint_row_text(
        ui,
        branch_rect.shrink2(egui::vec2(4.0, 0.0)),
        branch,
        &TextStyle::Monospace.resolve(ui.style()),
        text_color,
    );
    if row.current {
        paint_row_text(
            ui,
            current_rect,
            "current",
            &TextStyle::Body.resolve(ui.style()),
            ui.visuals().selection.stroke.color,
        );
    }
    !row.current && response.on_hover_text("Switch to this branch").clicked()
}

fn branch_window_header_ui(ui: &mut egui::Ui, repository: &GitRepositoryState) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} branches", repository.branches.len())).weak());
        ui.separator();
        ui.label(RichText::new("Up/Down: select  Enter: switch  Esc: close").weak());
    });
    ui.separator();

    ui.horizontal(|ui| {
        ui.label(RichText::new(&repository.name).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("current: {}", repository.branch))
                    .monospace()
                    .weak(),
            );
        });
    });
    ui.separator();
}
