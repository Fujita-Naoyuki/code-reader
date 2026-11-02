//! Git dialogs: create branch, switch branch, discard changes, commit details.

use super::*;

impl CodeReaderApp {
    pub(super) fn show_git_create_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git_create_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git_repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git_create_branch_root = None;
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
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
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
                    .git_new_branch_names
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
            self.git_create_branch_root = None;
            self.create_git_branch(&repository, &branch_name);
        } else if close {
            self.git_create_branch_root = None;
        }
    }

    pub(super) fn show_git_switch_branch_window(&mut self, ui: &mut egui::Ui) {
        let Some(repository_root) = self.git_switch_branch_root.clone() else {
            return;
        };
        let Some(repository) = self
            .git_repositories
            .iter()
            .find(|repository| repository.root == repository_root)
            .cloned()
        else {
            self.git_switch_branch_root = None;
            return;
        };

        let mut switch_to_branch = None;
        let mut close = false;
        let mut cursor = self
            .git_switch_branch_cursor
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
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }
                let cursor_index = repository
                    .branches
                    .iter()
                    .position(|branch| branch == &cursor);
                let moved_index = match cursor_index {
                    Some(index) if up => Some(index.saturating_sub(1)),
                    Some(index) if down => {
                        Some((index + 1).min(repository.branches.len().saturating_sub(1)))
                    }
                    None if up || down => Some(0),
                    _ => None,
                };
                if let Some(branch) = moved_index.and_then(|index| repository.branches.get(index)) {
                    cursor = branch.clone();
                }
                if enter {
                    if cursor == repository.branch || !repository.branches.contains(&cursor) {
                        close = true;
                    } else {
                        switch_to_branch = Some(cursor.clone());
                    }
                }

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} branches", repository.branches.len())).weak(),
                    );
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

                if repository.branches.is_empty() {
                    ui.label(RichText::new("No local branches").weak());
                } else {
                    let row_height = 24.0;
                    let current_width = 72.0;
                    let body_font = TextStyle::Body.resolve(ui.style());
                    let mono_font = TextStyle::Monospace.resolve(ui.style());
                    let draw_left_text =
                        |ui: &egui::Ui,
                         rect: egui::Rect,
                         text: &str,
                         font: egui::FontId,
                         color: egui::Color32| {
                            ui.painter().with_clip_rect(rect).text(
                                rect.left_center(),
                                egui::Align2::LEFT_CENTER,
                                text,
                                font,
                                color,
                            );
                        };
                    ScrollArea::vertical()
                        .id_salt("git_switch_branch_list")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for branch in &repository.branches {
                                let selected = branch == &repository.branch;
                                let row_width = ui.available_width();
                                let (row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let current_rect = egui::Rect::from_min_size(
                                    egui::pos2(row_rect.right() - current_width, row_rect.top()),
                                    egui::vec2(current_width, row_height),
                                );
                                let branch_rect = egui::Rect::from_min_max(
                                    row_rect.min,
                                    egui::pos2(current_rect.left() - 8.0, row_rect.bottom()),
                                );
                                let response = ui.interact(
                                    row_rect,
                                    ui.make_persistent_id(("git_switch_branch", branch)),
                                    if selected {
                                        egui::Sense::hover()
                                    } else {
                                        egui::Sense::click()
                                    },
                                );
                                let highlighted = branch == &cursor;
                                if response.hovered() || highlighted {
                                    ui.painter().rect_filled(
                                        row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                        2.0,
                                        ui.visuals().widgets.hovered.bg_fill,
                                    );
                                }
                                if highlighted && (up || down) {
                                    ui.scroll_to_rect(row_rect, None);
                                }
                                if selected {
                                    ui.painter().rect_filled(
                                        row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                        2.0,
                                        ui.visuals().selection.bg_fill,
                                    );
                                    if highlighted {
                                        ui.painter().rect_stroke(
                                            row_rect.shrink2(egui::vec2(0.0, 2.0)),
                                            2.0,
                                            ui.visuals().widgets.hovered.fg_stroke,
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                }

                                let text_color = if selected {
                                    ui.visuals().selection.stroke.color
                                } else {
                                    ui.visuals().text_color()
                                };
                                draw_left_text(
                                    ui,
                                    branch_rect.shrink2(egui::vec2(4.0, 0.0)),
                                    branch,
                                    mono_font.clone(),
                                    text_color,
                                );
                                if selected {
                                    draw_left_text(
                                        ui,
                                        current_rect,
                                        "current",
                                        body_font.clone(),
                                        ui.visuals().selection.stroke.color,
                                    );
                                }
                                if !selected
                                    && response.on_hover_text("Switch to this branch").clicked()
                                {
                                    switch_to_branch = Some(branch.clone());
                                }
                            }
                        });
                }
            });

        if let Some(branch_name) = switch_to_branch {
            self.git_switch_branch_root = None;
            self.git_switch_branch_cursor = None;
            self.switch_git_branch(&repository, &branch_name);
        } else if close {
            self.git_switch_branch_root = None;
            self.git_switch_branch_cursor = None;
        } else {
            self.git_switch_branch_cursor = Some(cursor);
        }
    }

    pub(super) fn show_git_discard_window(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.pending_git_discard.clone() else {
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
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
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
            self.pending_git_discard = None;
            self.discard_git_file(&request);
        } else if close {
            self.pending_git_discard = None;
        }
    }

    pub(super) fn show_git_commit_details_window(&mut self, ui: &mut egui::Ui) {
        let Some(details) = self.git_commit_details.clone() else {
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
                let escape = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
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

                ui.label(RichText::new(&details.commit.subject).strong());
                ui.add_space(4.0);
                egui::Grid::new("git_commit_details_metadata")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Author").weak());
                        ui.label(&details.commit.author);
                        ui.end_row();
                        ui.label(RichText::new("Date").weak());
                        ui.label(&details.commit.date);
                        ui.end_row();
                        ui.label(RichText::new("Hash").weak());
                        ui.label(RichText::new(&details.commit.hash).monospace());
                        ui.end_row();
                        ui.label(RichText::new("Parents").weak());
                        ui.label(if details.commit.parents.is_empty() {
                            "none".to_owned()
                        } else {
                            details.commit.parents.join(", ")
                        });
                        ui.end_row();
                    });

                if !details.commit.refs.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Refs").weak());
                        for reference in &details.commit.refs {
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
                                let label =
                                    format!("{:<3} {}", file.display_status(), display_path);
                                ui.label(RichText::new(label).monospace());
                            }
                        });
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        if open_commit_diff {
            self.open_git_commit_diff(&details);
            self.git_commit_details = None;
            return;
        }
        if close {
            self.git_commit_details = None;
        }
    }
}
