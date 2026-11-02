//! Opening things in the left and right panes, and the unsaved-changes question that
//! guards replacing a pane's content.

use super::*;

impl CodeReaderApp {
    pub(super) fn focused_editor_mut(&mut self) -> &mut EditorPane {
        match self.focused_pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    pub(super) fn focused_editor(&self) -> &EditorPane {
        match self.focused_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    pub(super) fn editor_mut(&mut self, pane: Pane) -> &mut EditorPane {
        match pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    pub(super) fn editor(&self, pane: Pane) -> &EditorPane {
        match pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    pub(super) fn open_file_in_focused_pane(&mut self, path: PathBuf) {
        self.open_file_in_pane(self.focused_pane, path);
    }

    pub(super) fn open_file_in_pane(&mut self, pane: Pane, path: PathBuf) {
        self.navigation_history_mut(pane).clear();
        self.remember_markdown_mode(pane);
        let mut next = EditorPane::from_path(pane, path);
        self.restore_markdown_mode(&mut next);
        if let Some(path) = &next.path {
            if is_external_rust_path(path) {
                // Rust definition navigation is handled by rust-analyzer. Indexing external
                // Rust sources here can be very expensive and is no longer needed for F12.
            } else if is_external_path(path) {
                self.symbol_index
                    .add_external_file(path, &self.language_registry);
            } else {
                self.symbol_index.add_file(path, &self.language_registry);
            }
            self.remember_recent_file(path.clone());
        }
        self.replace_editor(pane, next, true, None);
    }

    /// Puts `next` into `pane`. When the pane holds unsaved edits of another file, the user
    /// is asked first and this returns false; the replacement (with `sync_open_files` and
    /// `status`) then happens once they save or discard. For the same file the unsaved
    /// text is carried over instead.
    pub(super) fn replace_editor(
        &mut self,
        pane: Pane,
        mut next: EditorPane,
        sync_open_files: bool,
        status: Option<String>,
    ) -> bool {
        let current = self.editor_mut(pane);
        if current.has_unsaved_changes() {
            if current.path == next.path && !next.read_only {
                next.adopt_unsaved_text(current);
            } else {
                self.status_message = format!(
                    "{} has unsaved changes: save or discard them first.",
                    current.title
                );
                self.pending_unsaved_action = Some(UnsavedChangesAction::ReplaceEditor {
                    pane,
                    next: Box::new(next),
                    sync_open_files,
                    status,
                });
                return false;
            }
        }
        *self.editor_mut(pane) = next;
        if sync_open_files {
            self.sync_open_files();
            self.save_state();
        }
        if let Some(status) = status {
            self.status_message = status;
        }
        true
    }

    pub(super) fn panes_with_unsaved_changes(&self) -> Vec<Pane> {
        [Pane::Left, Pane::Right]
            .into_iter()
            .filter(|pane| self.editor(*pane).has_unsaved_changes())
            .collect()
    }

    pub(super) fn run_unsaved_changes_action(&mut self, action: UnsavedChangesAction) {
        match action {
            UnsavedChangesAction::ReplaceEditor {
                pane,
                next,
                sync_open_files,
                status,
            } => {
                *self.editor_mut(pane) = *next;
                if sync_open_files {
                    self.sync_open_files();
                    self.save_state();
                }
                if let Some(status) = status {
                    self.status_message = status;
                }
            }
            UnsavedChangesAction::SwitchSession(session_id) => {
                self.switch_session_now(&session_id);
            }
            UnsavedChangesAction::CreateSession => self.create_session_now(),
        }
    }

    pub(super) fn open_markdown_link_in_focused_pane(
        &mut self,
        path: PathBuf,
        fragment: Option<String>,
    ) {
        if !path.exists() {
            self.status_message = format!("Markdown link target not found: {}", path.display());
            return;
        }
        if !is_markdown_path(&path) {
            self.status_message = format!(
                "Only local Markdown links are supported: {}",
                path.display()
            );
            return;
        }

        if let Some(current) = self.focused_editor().navigation_point() {
            self.push_focused_navigation_history(current);
        }

        self.remember_focused_markdown_mode();
        let mut next = EditorPane::from_path(self.focused_pane, path.clone());
        self.restore_markdown_mode(&mut next);

        let mut fragment_status = None;
        if let Some(fragment) = fragment.as_deref().filter(|fragment| !fragment.is_empty()) {
            match MarkdownDocument::parse(path.clone(), next.text.as_str().to_owned()) {
                Ok(document) => {
                    if let Some(target) = markdown_fragment_target(&document, fragment) {
                        next.markdown_active_page_id = Some(target.page_id);
                        next.markdown_active_heading_id = target.heading_id;
                        next.target_position = Some(target.position);
                        next.pending_navigation = Some(PendingEditorNavigation {
                            position: target.position,
                            focus_editor: true,
                        });
                        next.cursor_position = Some(target.position);
                    } else {
                        fragment_status = Some(format!("Fragment not found: #{fragment}"));
                    }
                }
                Err(error) => {
                    fragment_status = Some(format!("Opened Markdown, but parse failed: {error}"));
                }
            }
        }

        self.symbol_index.add_file(&path, &self.language_registry);
        self.remember_recent_file(path.clone());
        self.reveal_workspace_file_in_tree(&path);
        let status =
            fragment_status.unwrap_or_else(|| format!("Opened Markdown link: {}", path.display()));
        self.replace_editor(self.focused_pane, next, true, Some(status));
    }

    pub(super) fn open_external_url(&mut self, url: &str) {
        match platform::current().open_url(url) {
            Ok(()) => {
                self.status_message = format!("Opened external URL: {url}");
            }
            Err(error) => {
                self.status_message = format!("Failed to open external URL: {url} ({error})");
            }
        }
    }

    pub(super) fn remember_focused_markdown_mode(&mut self) {
        self.remember_markdown_mode(self.focused_pane);
    }

    pub(super) fn remember_markdown_mode(&mut self, pane: Pane) {
        let editor = self.editor(pane);
        if editor.is_markdown()
            && let Some(path) = &editor.path
        {
            self.markdown_modes
                .insert(path.clone(), editor.markdown_mode);
        }
    }

    pub(super) fn restore_markdown_mode(&self, editor: &mut EditorPane) {
        if editor.is_markdown()
            && let Some(path) = &editor.path
            && let Some(mode) = self.markdown_modes.get(path)
        {
            editor.markdown_mode = *mode;
        }
    }

    pub(super) fn handle_editor_action(&mut self, action: EditorAction) {
        match action {
            EditorAction::GoToDefinition => self.go_to_definition(),
            EditorAction::Back => self.go_back(),
            EditorAction::OpenNavigationHistory { index } => {
                self.open_navigation_history_entry(index);
            }
            EditorAction::OpenGitCommitDetails {
                repository_name,
                root,
                commit,
            } => {
                self.open_git_commit_details(repository_name, root, commit);
            }
            EditorAction::SaveFile => self.save_focused_file(),
            EditorAction::OpenMarkdownLink { path, fragment } => {
                self.open_markdown_link_in_focused_pane(path, fragment);
            }
            EditorAction::OpenExternalUrl(url) => {
                self.open_external_url(&url);
            }
            EditorAction::ToggleBookmark { path, line } => {
                self.toggle_bookmark_at_line(path, line);
            }
            EditorAction::ShowStatus(message) => {
                self.status_message = message;
            }
        }
    }

    pub(super) fn open_markdown_search(&mut self) -> bool {
        let editor = self.focused_editor_mut();
        if !editor.is_markdown() || editor.markdown_mode == MarkdownMode::Markdown {
            return false;
        }

        // Ctrl+F on an open search box only refocuses it; the current match stays.
        if !editor.markdown_search.open {
            editor.markdown_search.current_match = 0;
        }
        editor.markdown_search.open = true;
        editor.markdown_search.focus_requested = true;
        true
    }

    pub(super) fn open_source_search(&mut self) -> bool {
        let editor = self.focused_editor_mut();
        if !editor.can_source_search() && !editor.can_markdown_source_search() {
            return false;
        }

        // Ctrl+F on an open search box only refocuses it; the current match stays.
        if !editor.source_search.open {
            editor.source_search.current_match = 0;
            editor.source_search.scrolled_match = None;
        }
        editor.source_search.open = true;
        editor.source_search.focus_requested = true;
        true
    }

    pub(super) fn remember_recent_file(&mut self, path: PathBuf) {
        self.recent_files.retain(|recent| recent != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(30);
        self.bump_search_index_revision();
    }

    pub(super) fn save_focused_file(&mut self) {
        self.save_pane(self.focused_pane);
    }

    pub(super) fn save_pane(&mut self, pane: Pane) -> bool {
        match self.editor_mut(pane).save_to_disk() {
            Ok(path) => {
                self.symbol_index.add_file(&path, &self.language_registry);
                self.bump_search_index_revision();
                self.status_message = format!("Saved file: {}", path.display());
                true
            }
            Err(error) => {
                self.status_message = error;
                false
            }
        }
    }

    pub(super) fn show_unsaved_changes_window(&mut self, ui: &mut egui::Ui) {
        if self.pending_unsaved_action.is_none() {
            return;
        }
        let panes = match &self.pending_unsaved_action {
            Some(UnsavedChangesAction::ReplaceEditor { pane, .. }) => vec![*pane],
            _ => self.panes_with_unsaved_changes(),
        };
        let files = panes
            .iter()
            .map(|pane| self.editor(*pane).title.clone())
            .collect::<Vec<_>>();

        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("Unsaved Changes")
            .id(egui::Id::new("unsaved_changes_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0))
            .fixed_size(egui::vec2(440.0, 160.0))
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
                    cancel = true;
                } else if enter {
                    save = true;
                }

                ui.label(
                    RichText::new("These files have unsaved changes:")
                        .small()
                        .weak(),
                );
                for file in &files {
                    ui.label(RichText::new(file).monospace());
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Save").stroke(ui.visuals().selection.stroke))
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.button("Discard").clicked() {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.pending_unsaved_action = None;
            self.status_message = "Kept unsaved changes.".to_owned();
            return;
        }
        if save {
            for pane in panes {
                if !self.save_pane(pane) {
                    // Keep the question open so the edits are not lost.
                    return;
                }
            }
        }
        if (save || discard)
            && let Some(action) = self.pending_unsaved_action.take()
        {
            self.run_unsaved_changes_action(action);
        }
    }
}

/// What to do once the user has saved or discarded unsaved edits.
pub(super) enum UnsavedChangesAction {
    ReplaceEditor {
        pane: Pane,
        next: Box<EditorPane>,
        sync_open_files: bool,
        status: Option<String>,
    },
    SwitchSession(String),
    CreateSession,
}
