//! Workspace sessions: create, switch, rename, delete, root folders and saving state.

use super::*;

impl CodeReaderApp {
    pub(super) fn current_session(&self) -> Option<&WorkspaceSession> {
        self.app_state.current_session()
    }

    pub(super) fn current_session_mut(&mut self) -> Option<&mut WorkspaceSession> {
        self.app_state.current_session_mut()
    }

    pub(super) fn primary_root(&self) -> PathBuf {
        self.current_session()
            .and_then(|session| session.root_folders.first())
            .cloned()
            .unwrap_or_else(|| platform::current().current_dir())
    }

    pub(super) fn create_session(&mut self) {
        if !self.panes_with_unsaved_changes().is_empty() {
            self.layout.pending_unsaved_action = Some(UnsavedChangesAction::CreateSession);
            return;
        }
        self.create_session_now();
    }

    pub(super) fn create_session_now(&mut self) {
        let Some(root) = pick_workspace_folder("New workspace session") else {
            self.status_message = "New session canceled.".to_owned();
            return;
        };

        self.session_edit.editing_name = false;
        self.session_edit.name_edit.clear();
        self.session_edit.pending_delete_id = None;
        self.app_state.create_session(root);
        self.left = EditorPane::empty(Pane::Left);
        self.right = EditorPane::empty(Pane::Right);
        self.focused_pane = Pane::Left;
        self.layout.show_right_pane = false;
        self.workspace.dragging_file = None;
        self.git.commit_details = None;
        self.workspace.selected_path = None;
        self.workspace.pending_reveal_path = None;
        self.workspace.sidebar_active = false;
        self.close_references_overlay();
        self.navigation.recent_files.clear();
        self.rebuild_file_tree();
        self.status_message = "Created a new session.".to_owned();
        self.save_state();
    }

    pub(super) fn add_root_to_current_session(&mut self) {
        let Some(path) = pick_workspace_folder("Add workspace folder") else {
            self.status_message = "Add folder canceled.".to_owned();
            return;
        };

        if let Some(session) = self.current_session_mut() {
            if !session.root_folders.iter().any(|root| root == &path) {
                session.root_folders.push(path.clone());
                session.touch();
            } else {
                self.status_message = format!("Folder is already in workspace: {}", path.display());
                return;
            }
        }

        self.rebuild_file_tree();
        self.status_message = format!("Added folder: {}", path.display());
        self.save_state();
    }

    pub(super) fn remove_root_from_current_session(&mut self, path: &Path) {
        let removed = self.current_session_mut().and_then(|session| {
            if session.root_folders.len() <= 1 {
                return None;
            }

            let index = session.root_folders.iter().position(|root| root == path)?;
            let removed = session.root_folders.remove(index);
            session.touch();
            Some(removed)
        });

        match removed {
            Some(path) => {
                self.rebuild_file_tree();
                self.status_message = format!("Removed folder: {}", path.display());
                self.save_state();
            }
            None => {
                self.status_message = "Keep at least one root folder in a session.".to_owned();
            }
        }
    }

    pub(super) fn switch_session(&mut self, session_id: &str) {
        if !self.panes_with_unsaved_changes().is_empty() {
            self.layout.pending_unsaved_action =
                Some(UnsavedChangesAction::SwitchSession(session_id.to_owned()));
            return;
        }
        self.switch_session_now(session_id);
    }

    pub(super) fn switch_session_now(&mut self, session_id: &str) {
        self.sync_open_files();
        self.session_edit.pending_delete_id = None;
        self.session_edit.editing_name = false;
        self.session_edit.name_edit.clear();
        self.app_state.switch_to(session_id);

        if let Some(session) = self.current_session().cloned() {
            self.left = session
                .open_files
                .left
                .clone()
                .map(|path| {
                    editor_pane_from_session_path(Pane::Left, path, session.pane_positions.left)
                })
                .unwrap_or_else(|| EditorPane::empty(Pane::Left));
            self.right = session
                .open_files
                .right
                .clone()
                .map(|path| {
                    editor_pane_from_session_path(Pane::Right, path, session.pane_positions.right)
                })
                .unwrap_or_else(|| EditorPane::empty(Pane::Right));
            self.navigation.recent_files = recent_files_from_open_files(&session.open_files);
        }

        self.focused_pane = Pane::Left;
        self.navigation.left_history.clear();
        self.navigation.right_history.clear();
        self.lsp.definition_candidates.clear();
        self.close_references_overlay();
        self.layout.show_right_pane = false;
        self.workspace.dragging_file = None;
        self.git.commit_details = None;
        self.workspace.selected_path = None;
        self.workspace.pending_reveal_path = None;
        self.workspace.sidebar_active = false;
        self.rebuild_file_tree();
        self.status_message = "Switched session.".to_owned();
        self.save_state();
    }

    pub(super) fn delete_session(&mut self, session_id: &str) {
        match self.app_state.delete_session(session_id) {
            Ok(session) => {
                self.session_edit.pending_delete_id = None;
                self.status_message = format!("Deleted session: {}", session.display_name);
                self.save_state();
            }
            Err(error) => {
                self.session_edit.pending_delete_id = None;
                self.status_message = error;
            }
        }
    }

    pub(super) fn start_session_rename(&mut self) {
        let Some(session) = self.current_session() else {
            self.status_message = "No current session to rename.".to_owned();
            return;
        };

        self.session_edit.name_edit = session.display_name.clone();
        self.session_edit.editing_name = true;
        self.session_edit.name_select_all = true;
        self.session_edit.pending_delete_id = None;
    }

    pub(super) fn cancel_session_rename(&mut self) {
        self.session_edit.editing_name = false;
        self.session_edit.name_edit.clear();
        self.session_edit.name_select_all = false;
    }

    pub(super) fn commit_session_rename(&mut self) {
        let name = self.session_edit.name_edit.trim().to_owned();
        match self.app_state.rename_current_session(&name) {
            Ok(()) => {
                self.session_edit.editing_name = false;
                self.session_edit.name_edit.clear();
                self.session_edit.name_select_all = false;
                self.sync_open_files();
                match save_state(&self.state_path, &self.app_state) {
                    Ok(()) => {
                        self.status_message = format!("Renamed session: {name}");
                    }
                    Err(error) => {
                        self.status_message = format!("Renamed session, but save failed: {error}");
                    }
                }
            }
            Err(error) => {
                self.status_message = error;
            }
        }
    }

    pub(super) fn sync_open_files(&mut self) {
        let left = self.left.path().map(Path::to_path_buf);
        let right = self.right.path().map(Path::to_path_buf);
        let pane_positions = PanePositions {
            left: text_position_from_position(self.left.cursor_position),
            right: text_position_from_position(self.right.cursor_position),
        };
        if let Some(session) = self.current_session_mut() {
            session.open_files = OpenFiles { left, right };
            session.pane_positions = pane_positions;
            session.touch();
        }
    }

    pub(super) fn save_state(&mut self) {
        self.sync_open_files();
        match save_state(&self.state_path, &self.app_state) {
            Ok(()) => {}
            Err(error) => {
                self.status_message = format!("Failed to save state: {error}");
            }
        }
    }

    pub(super) fn save_state_silent(&mut self) {
        self.sync_open_files();
        if let Err(error) = save_state(&self.state_path, &self.app_state) {
            self.status_message = format!("Failed to save state: {error}");
        }
    }

    pub(super) fn show_workspace_rename_window(&mut self, ui: &mut egui::Ui) {
        if !self.session_edit.editing_name {
            return;
        }

        let current_name = self
            .current_session()
            .map(|session| session.display_name.clone())
            .unwrap_or_else(|| "No session".to_owned());
        let input_id = egui::Id::new("workspace_rename_name");
        if self.session_edit.name_select_all {
            let mut state = TextEditState::load(ui.ctx(), input_id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0),
                CCursor::new(self.session_edit.name_edit.chars().count()),
            )));
            state.store(ui.ctx(), input_id);
            self.session_edit.name_select_all = false;
        }

        let mut rename = false;
        let mut close = false;
        egui::Window::new("Rename Workspace")
            .id(egui::Id::new("workspace_rename_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(420.0, 180.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter) = ui.input_mut(|input| {
                    (
                        self.layout.modal_escape == Some(Modal::RenameWorkspace),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    )
                });
                if escape {
                    close = true;
                }

                ui.label(RichText::new("Esc: close").weak());
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Current").weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(current_name.clone()).monospace());
                    });
                });
                ui.add_space(8.0);
                ui.label("New name");
                ui.add_sized(
                    [ui.available_width(), 22.0],
                    TextEdit::singleline(&mut self.session_edit.name_edit)
                        .id(input_id)
                        .font(TextStyle::Monospace),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Rename").clicked() || enter {
                        rename = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                ui.memory_mut(|memory| memory.request_focus(input_id));
            });

        if rename {
            self.commit_session_rename();
        } else if close {
            self.cancel_session_rename();
        }
    }

    pub(super) fn show_recent_workspaces_window(&mut self, ui: &mut egui::Ui) {
        if !self.layout.recent_workspaces_open {
            return;
        }

        let mut sessions = self.app_state.sessions.clone();
        sessions.sort_by(|left, right| {
            right
                .last_updated_unix
                .cmp(&left.last_updated_unix)
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        let current_session_id = self.app_state.current_session_id.clone();
        let session_count = self.app_state.sessions.len();
        let mut close = false;

        egui::Window::new("Recent Workspaces")
            .id(egui::Id::new("recent_workspaces_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(640.0, 420.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                if self.layout.modal_escape == Some(Modal::RecentWorkspaces) {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} workspaces", sessions.len())).weak());
                    ui.separator();
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                ScrollArea::vertical()
                    .id_salt("recent_workspaces_results")
                    .auto_shrink([false, false])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if sessions.is_empty() {
                            ui.label(RichText::new("No recent workspaces").weak());
                            return;
                        }

                        for session in sessions {
                            let is_current = session.id == current_session_id;
                            let is_pending_delete =
                                self.session_edit.pending_delete_id.as_deref() == Some(&session.id);
                            let can_delete = !is_current && session_count > 1;
                            let title = if is_current {
                                format!("▶ {}", session.display_name)
                            } else {
                                session.display_name.clone()
                            };
                            let row_width = ui.available_width();
                            let name_width = 180.0;
                            let age_width = 76.0;
                            let row_height = 22.0;
                            let column_gap = 16.0;
                            let age_label = recent_workspace_age_label(session.last_updated_unix);
                            let row_text_color = ui.visuals().weak_text_color();
                            let body_font = TextStyle::Body.resolve(ui.style());
                            let name_font = TextStyle::Body.resolve(ui.style());
                            let draw_left_text =
                                |ui: &egui::Ui,
                                 rect: egui::Rect,
                                 text: String,
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

                            let (row_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            let name_rect = egui::Rect::from_min_size(
                                row_rect.min,
                                egui::vec2(name_width, row_height),
                            );
                            let age_rect = egui::Rect::from_min_size(
                                egui::pos2(name_rect.right() + column_gap, row_rect.top()),
                                egui::vec2(age_width, row_height),
                            );
                            let delete_rect = egui::Rect::from_min_size(
                                egui::pos2(row_rect.right() - 22.0, row_rect.top() + 1.0),
                                egui::vec2(22.0, row_height - 2.0),
                            );
                            let path_rect = egui::Rect::from_min_max(
                                egui::pos2(age_rect.right() + column_gap, row_rect.top()),
                                egui::pos2(delete_rect.left() - 8.0, row_rect.bottom()),
                            );

                            let name_response = ui.interact(
                                name_rect,
                                ui.make_persistent_id(("recent_workspace", &session.id)),
                                egui::Sense::click(),
                            );
                            if name_response.hovered() {
                                ui.painter().rect_filled(
                                    name_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().widgets.hovered.bg_fill,
                                );
                            }
                            if is_current {
                                ui.painter().rect_filled(
                                    name_rect.shrink2(egui::vec2(0.0, 2.0)),
                                    2.0,
                                    ui.visuals().selection.bg_fill,
                                );
                            }
                            if name_response.clicked() && !is_current && !is_pending_delete {
                                self.switch_session(&session.id);
                                close = true;
                            }

                            draw_left_text(
                                ui,
                                name_rect.shrink2(egui::vec2(4.0, 0.0)),
                                title,
                                name_font.clone(),
                                if is_current {
                                    ui.visuals().selection.stroke.color
                                } else {
                                    ui.visuals().text_color()
                                },
                            );
                            draw_left_text(
                                ui,
                                age_rect,
                                age_label,
                                body_font.clone(),
                                row_text_color,
                            );
                            let primary_root = session
                                .root_folders
                                .first()
                                .map(|root| root.display().to_string())
                                .unwrap_or_else(|| "<no root>".to_owned());
                            draw_left_text(
                                ui,
                                path_rect,
                                primary_root,
                                body_font.clone(),
                                row_text_color,
                            );

                            if can_delete
                                && ui
                                    .put(delete_rect, egui::Button::new("×"))
                                    .on_hover_text("Delete session")
                                    .clicked()
                            {
                                self.session_edit.pending_delete_id = Some(session.id.clone());
                            }
                            if is_pending_delete {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Delete this session?")
                                            .small()
                                            .color(ui.visuals().warn_fg_color),
                                    );
                                    if ui.button("Delete").clicked() {
                                        self.delete_session(&session.id);
                                    }
                                    if ui.button("Cancel").clicked() {
                                        self.session_edit.pending_delete_id = None;
                                    }
                                });
                            }
                            for root in session.root_folders.iter().skip(1).take(3) {
                                let (root_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let root_path_rect = egui::Rect::from_min_max(
                                    egui::pos2(path_rect.left(), root_rect.top()),
                                    egui::pos2(path_rect.right(), root_rect.bottom()),
                                );
                                draw_left_text(
                                    ui,
                                    root_path_rect,
                                    format!("+ {}", root.display()),
                                    body_font.clone(),
                                    row_text_color,
                                );
                            }
                            if session.root_folders.len() > 4 {
                                let (more_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let more_path_rect = egui::Rect::from_min_max(
                                    egui::pos2(path_rect.left(), more_rect.top()),
                                    egui::pos2(path_rect.right(), more_rect.bottom()),
                                );
                                draw_left_text(
                                    ui,
                                    more_path_rect,
                                    format!("+ {} more roots", session.root_folders.len() - 4),
                                    body_font,
                                    row_text_color,
                                );
                            }
                            ui.add_space(6.0);
                        }
                    });
            });

        if close {
            self.layout.recent_workspaces_open = false;
            self.session_edit.pending_delete_id = None;
        }
    }
}

pub(super) fn text_position_from_position(position: Option<Position>) -> TextPosition {
    position
        .map(|position| TextPosition::new(position.line, position.character))
        .unwrap_or_default()
}
