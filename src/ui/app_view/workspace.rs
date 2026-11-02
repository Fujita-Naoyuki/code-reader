//! The workspace file tree: selection, new / rename / delete dialogs and picking up
//! changes on disk.

use super::*;

impl CodeReaderApp {
    pub(super) fn selected_workspace_path(&self) -> Option<&Path> {
        let path = self.selected_workspace_path.as_deref()?;
        (path.exists() && self.path_is_in_current_workspace(path)).then_some(path)
    }

    pub(super) fn selected_workspace_new_file_folder(&self) -> PathBuf {
        if let Some(path) = self.selected_workspace_path() {
            if path.is_dir() {
                return path.to_path_buf();
            }
            if let Some(parent) = path.parent() {
                return parent.to_path_buf();
            }
        }
        self.primary_root()
    }

    pub(super) fn selected_workspace_file(&self) -> Option<PathBuf> {
        self.selected_workspace_path()
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
    }

    pub(super) fn path_is_in_current_workspace(&self, path: &Path) -> bool {
        self.current_session().is_some_and(|session| {
            session
                .root_folders
                .iter()
                .any(|root| path.starts_with(root))
        })
    }

    pub(super) fn workspace_tree_file_path(&self, path: &Path) -> Option<PathBuf> {
        self.file_tree
            .files
            .iter()
            .find(|file| paths_match(file, path))
            .cloned()
    }

    pub(super) fn reveal_workspace_file_in_tree(&mut self, path: &Path) {
        let Some(tree_path) = self.workspace_tree_file_path(path) else {
            return;
        };

        self.selected_workspace_path = Some(tree_path.clone());
        self.pending_workspace_tree_reveal_path = Some(tree_path.clone());
        for root in self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default()
        {
            if tree_path.starts_with(&root) {
                self.workspace_root_open.insert(root, true);
                break;
            }
        }
    }

    pub(super) fn start_workspace_new_file(&mut self) {
        let folder = self.selected_workspace_new_file_folder();
        if !folder.is_dir() {
            self.status_message = format!("Folder not found: {}", folder.display());
            return;
        }
        self.workspace_file_dialog = Some(WorkspaceFileDialog::NewFile {
            folder,
            file_name: String::new(),
            error: None,
        });
    }

    pub(super) fn start_workspace_rename_file(&mut self) {
        let Some(path) = self.selected_workspace_file() else {
            self.status_message = "Select a file to rename.".to_owned();
            return;
        };
        let new_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        self.workspace_file_dialog = Some(WorkspaceFileDialog::Rename {
            path,
            new_name,
            select_stem: true,
            error: None,
        });
    }

    pub(super) fn start_workspace_delete_file(&mut self) {
        let Some(path) = self.selected_workspace_file() else {
            self.status_message = "Select a file to delete.".to_owned();
            return;
        };
        self.workspace_file_dialog = Some(WorkspaceFileDialog::Delete { path, error: None });
    }

    pub(super) fn handle_workspace_file_shortcuts(&mut self, ui: &egui::Ui) {
        if !self.workspace_file_shortcuts_enabled() {
            return;
        }

        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F2)) {
            self.start_workspace_rename_file();
        } else if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
            self.start_workspace_delete_file();
        }
    }

    pub(super) fn workspace_file_shortcuts_enabled(&self) -> bool {
        self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
            && self.workspace_file_dialog.is_none()
            && !self.editing_session_name
            && !self.search_overlay.open
    }

    pub(super) fn create_workspace_file(
        &mut self,
        folder: &Path,
        file_name: &str,
    ) -> Result<(), String> {
        if !folder.is_dir() {
            return Err(format!("Folder not found: {}", folder.display()));
        }
        if !self.path_is_in_current_workspace(folder) {
            return Err("Folder is outside the current workspace.".to_owned());
        }

        let file_name = validate_workspace_file_name(file_name)?;
        let path = folder.join(&file_name);
        if path.exists() {
            return Err(format!("File already exists: {}", path.display()));
        }

        let content = initial_workspace_file_content(&path);
        fs::write(&path, content)
            .map_err(|error| format!("Failed to create file: {} ({error})", path.display()))?;

        self.selected_workspace_path = Some(path.clone());
        self.pending_workspace_tree_reveal_path = Some(path.clone());
        self.rebuild_file_tree();
        self.open_file_in_focused_pane(path.clone());
        self.status_message = format!("Created file: {}", path.display());
        Ok(())
    }

    pub(super) fn rename_workspace_file(
        &mut self,
        path: &Path,
        new_name: &str,
    ) -> Result<(), String> {
        if !path.is_file() {
            return Err("Select a file to rename.".to_owned());
        }
        if !self.path_is_in_current_workspace(path) {
            return Err("File is outside the current workspace.".to_owned());
        }

        let new_name = validate_workspace_file_name(new_name)?;
        let Some(parent) = path.parent() else {
            return Err("File has no parent folder.".to_owned());
        };
        let next_path = parent.join(new_name);
        if next_path == path {
            return Err("The new name is the same as the current name.".to_owned());
        }
        if next_path.exists() {
            return Err(format!("File already exists: {}", next_path.display()));
        }

        fs::rename(path, &next_path).map_err(|error| {
            format!(
                "Failed to rename file: {} -> {} ({error})",
                path.display(),
                next_path.display()
            )
        })?;

        self.remap_workspace_file_references(path, &next_path);
        self.selected_workspace_path = Some(next_path.clone());
        self.pending_workspace_tree_reveal_path = Some(next_path.clone());
        self.rebuild_file_tree();
        self.status_message = format!("Renamed file: {}", next_path.display());
        self.save_state();
        Ok(())
    }

    pub(super) fn delete_workspace_file(&mut self, path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err("Select a file to delete.".to_owned());
        }
        if !self.path_is_in_current_workspace(path) {
            return Err("File is outside the current workspace.".to_owned());
        }

        fs::remove_file(path)
            .map_err(|error| format!("Failed to delete file: {} ({error})", path.display()))?;

        self.remove_workspace_file_references(path);
        self.selected_workspace_path = None;
        self.pending_workspace_tree_reveal_path = None;
        self.rebuild_file_tree();
        self.status_message = format!("Deleted file: {}", path.display());
        self.save_state();
        Ok(())
    }

    pub(super) fn remap_workspace_file_references(&mut self, old_path: &Path, new_path: &Path) {
        for editor in [&mut self.left, &mut self.right] {
            if editor.path.as_deref() == Some(old_path) {
                editor.path = Some(new_path.to_path_buf());
                editor.title = file_title(new_path);
                editor.read_only = is_external_path(new_path);
                editor.mark_text_changed();
            }
        }
        self.left_navigation_history.remap_path(old_path, new_path);
        self.right_navigation_history.remap_path(old_path, new_path);
        for recent in &mut self.recent_files {
            if recent == old_path {
                *recent = new_path.to_path_buf();
            }
        }
        if let Some(session) = self.current_session_mut() {
            for bookmark in &mut session.bookmarks {
                if bookmark.path == old_path {
                    bookmark.path = new_path.to_path_buf();
                    bookmark.label = format!(
                        "{}:{}",
                        new_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("<invalid>"),
                        bookmark.position.line + 1
                    );
                }
            }
            session.touch();
        }
    }

    pub(super) fn remove_workspace_file_references(&mut self, path: &Path) {
        if self.left.path.as_deref() == Some(path) {
            self.left = EditorPane::empty(Pane::Left);
        }
        if self.right.path.as_deref() == Some(path) {
            self.right = EditorPane::empty(Pane::Right);
        }
        self.left_navigation_history.retain_path_not(path);
        self.right_navigation_history.retain_path_not(path);
        self.recent_files.retain(|recent| recent != path);
        if let Some(session) = self.current_session_mut() {
            session.bookmarks.retain(|bookmark| bookmark.path != path);
            session.touch();
        }
    }

    pub(super) fn rebuild_file_tree(&mut self) {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.file_tree = discover_workspace(&roots, FileDiscoveryOptions::default());
        self.workspace_snapshot = workspace_snapshot(&self.file_tree.files);
        self.bump_search_index_revision();
        self.next_workspace_change_poll = Instant::now() + WORKSPACE_CHANGE_POLL_INTERVAL;
        self.workspace_change_receiver = None;
        self.go_lsp_client = None;
        self.pending_go_lsp_request = None;
        self.go_lsp_synced_hashes.clear();
        self.go_lsp_request_started_at.clear();
        self.go_lsp_debug = LspDebugState::default();
        self.rust_lsp_client = None;
        self.pending_rust_lsp_request = None;
        self.rust_lsp_synced_hashes.clear();
        self.rust_lsp_request_started_at.clear();
        self.rust_lsp_debug = LspDebugState::default();
        self.symbol_index_receiver = Some(start_symbol_indexing(
            self.file_tree.files.clone(),
            self.language_registry.clone(),
        ));
        self.warm_up_rust_lsp_for_workspace();
        self.git_repositories_loaded = false;
        self.git_repositories.clear();
        self.status_message = format!("Discovered {} files.", self.file_tree.files.len());
    }

    pub(super) fn poll_workspace_changes(&mut self, context: &egui::Context) {
        context.request_repaint_after(WORKSPACE_CHANGE_POLL_INTERVAL);

        if let Some(receiver) = self.workspace_change_receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    self.apply_workspace_change_scan(result);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.workspace_change_receiver = Some(receiver);
                    context.request_repaint_after(Duration::from_millis(100));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }

        let now = Instant::now();
        if now < self.next_workspace_change_poll {
            return;
        }
        self.next_workspace_change_poll = now + WORKSPACE_CHANGE_POLL_INTERVAL;

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.workspace_change_receiver = Some(start_workspace_change_scan(roots));
        context.request_repaint_after(Duration::from_millis(100));
    }

    pub(super) fn apply_workspace_change_scan(&mut self, result: WorkspaceChangeScanResult) {
        let roots = result.roots;
        let next_files = result.files;
        let next_snapshot = result.snapshot;
        if next_snapshot == self.workspace_snapshot {
            return;
        }

        let diff = workspace_snapshot_diff(&self.workspace_snapshot, &next_snapshot);
        let change_count = diff.len();
        let full_symbol_reindex =
            diff.requires_full_symbol_reindex() || self.symbol_index_receiver.is_some();
        let next_file_tree = build_file_tree(&roots, next_files);
        self.file_tree = next_file_tree;
        self.workspace_snapshot = next_snapshot;
        if full_symbol_reindex {
            self.symbol_index_receiver = Some(start_symbol_indexing(
                self.file_tree.files.clone(),
                self.language_registry.clone(),
            ));
        } else {
            for path in &diff.removed {
                self.symbol_index.remove_file(path);
            }
            for path in diff.added.iter().chain(&diff.modified) {
                self.symbol_index
                    .replace_file(path, &self.language_registry);
            }
        }
        self.bump_search_index_revision();
        self.reload_git_repositories();
        let symbol_update = if full_symbol_reindex {
            "reindexing all symbols"
        } else {
            "updated changed symbols"
        };
        self.status_message = format!(
            "Detected {change_count} external file changes; refreshed workspace, Git, and {symbol_update}."
        );
    }

    pub(super) fn show_workspace_file_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut dialog) = self.workspace_file_dialog.take() else {
            return;
        };

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let mut keep_open = true;
        let mut action = None;

        match &mut dialog {
            WorkspaceFileDialog::NewFile {
                folder,
                file_name,
                error,
            } => {
                let folder_label = workspace_search_display_path(folder, &roots);
                egui::Window::new("New File")
                    .id(egui::Id::new("workspace_new_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 180.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Folder").weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(folder_label.clone()).monospace());
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.label("File name");
                        let edit_id = ui.id().with("workspace_new_file_name");
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            TextEdit::singleline(file_name)
                                .id(edit_id)
                                .font(TextStyle::Monospace),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Create").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::CreateFile {
                                    folder: folder.clone(),
                                    file_name: file_name.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                        ui.memory_mut(|memory| memory.request_focus(edit_id));
                    });
            }
            WorkspaceFileDialog::Rename {
                path,
                new_name,
                select_stem,
                error,
            } => {
                let path_label = workspace_search_display_path(path, &roots);
                egui::Window::new("Rename File")
                    .id(egui::Id::new("workspace_rename_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 180.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Current").weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new(path_label.clone()).monospace());
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.label("New name");
                        let edit_id = ui.id().with("workspace_rename_file_name");
                        if *select_stem {
                            let selection_end = file_name_stem_selection_char_len(new_name);
                            let mut state =
                                TextEditState::load(ui.ctx(), edit_id).unwrap_or_default();
                            state.cursor.set_char_range(Some(CCursorRange::two(
                                CCursor::new(0),
                                CCursor::new(selection_end),
                            )));
                            state.store(ui.ctx(), edit_id);
                            *select_stem = false;
                        }
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            TextEdit::singleline(new_name)
                                .id(edit_id)
                                .font(TextStyle::Monospace),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Rename").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::RenameFile {
                                    path: path.clone(),
                                    new_name: new_name.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                        ui.memory_mut(|memory| memory.request_focus(edit_id));
                    });
            }
            WorkspaceFileDialog::Delete { path, error } => {
                let path_label = workspace_search_display_path(path, &roots);
                egui::Window::new("Delete File")
                    .id(egui::Id::new("workspace_delete_file_window"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
                    .fixed_size(egui::vec2(420.0, 160.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                        }) {
                            keep_open = false;
                        }

                        ui.label(RichText::new("Esc: close").weak());
                        ui.separator();
                        ui.label(RichText::new(path_label.clone()).monospace());
                        ui.label(
                            RichText::new("This cannot be undone.")
                                .color(ui.visuals().warn_fg_color),
                        );
                        if let Some(error) = error {
                            ui.label(
                                RichText::new(error.as_str()).color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Delete").clicked()
                                || ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(WorkspaceFileDialogAction::DeleteFile {
                                    path: path.clone(),
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
            }
        }

        if let Some(action) = action {
            let result = match action {
                WorkspaceFileDialogAction::CreateFile { folder, file_name } => {
                    self.create_workspace_file(&folder, &file_name)
                }
                WorkspaceFileDialogAction::RenameFile { path, new_name } => {
                    self.rename_workspace_file(&path, &new_name)
                }
                WorkspaceFileDialogAction::DeleteFile { path } => self.delete_workspace_file(&path),
            };

            match result {
                Ok(()) => keep_open = false,
                Err(message) => match &mut dialog {
                    WorkspaceFileDialog::NewFile { error, .. }
                    | WorkspaceFileDialog::Rename { error, .. }
                    | WorkspaceFileDialog::Delete { error, .. } => {
                        *error = Some(message);
                    }
                },
            }
        }

        if keep_open {
            self.workspace_file_dialog = Some(dialog);
        }
    }
}
