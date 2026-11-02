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
            if editor.path() == Some(old_path) {
                editor.content = EditorContent::File {
                    path: new_path.to_path_buf(),
                    read_only: is_external_path(new_path),
                };
                editor.title = file_title(new_path);
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
        if self.left.path() == Some(path) {
            self.left = EditorPane::empty(Pane::Left);
        }
        if self.right.path() == Some(path) {
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
        self.set_background_status(format!("Discovered {} files.", self.file_tree.files.len()));
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

pub(super) const WORKSPACE_SECTION_HEADER_HEIGHT: f32 = 24.0;

pub(super) const WORKSPACE_FOOTER_BODY_ALLOWANCE: f32 = 64.0;

pub(super) const WORKSPACE_FILE_TREE_MIN_HEIGHT: f32 = 120.0;

pub(super) const WORKSPACE_CHANGE_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkspaceFileDialog {
    NewFile {
        folder: PathBuf,
        file_name: String,
        error: Option<String>,
    },
    Rename {
        path: PathBuf,
        new_name: String,
        select_stem: bool,
        error: Option<String>,
    },
    Delete {
        path: PathBuf,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // The actions all act on files.
pub(super) enum WorkspaceFileDialogAction {
    CreateFile { folder: PathBuf, file_name: String },
    RenameFile { path: PathBuf, new_name: String },
    DeleteFile { path: PathBuf },
}

#[derive(Debug)]
pub(super) struct WorkspaceChangeScanResult {
    pub(super) roots: Vec<PathBuf>,
    pub(super) files: Vec<PathBuf>,
    pub(super) snapshot: WorkspaceSnapshot,
}

pub(super) fn start_workspace_change_scan(
    roots: Vec<PathBuf>,
) -> Receiver<WorkspaceChangeScanResult> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let files = discover_workspace_files(&roots, FileDiscoveryOptions::default());
        let snapshot = workspace_snapshot(&files);
        let _ = sender.send(WorkspaceChangeScanResult {
            roots,
            files,
            snapshot,
        });
    });
    receiver
}

pub(super) fn workspace_file_tree_height(available_height: f32, footer_header_count: usize) -> f32 {
    if !available_height.is_finite() {
        return WORKSPACE_FILE_TREE_MIN_HEIGHT;
    }

    let reserved_headers = footer_header_count as f32 * WORKSPACE_SECTION_HEADER_HEIGHT + 8.0;
    let reserved_footer = footer_header_count as f32 * WORKSPACE_SECTION_HEADER_HEIGHT
        + WORKSPACE_FOOTER_BODY_ALLOWANCE;
    let max_tree_height = (available_height - reserved_headers).max(60.0);
    (available_height - reserved_footer)
        .max(WORKSPACE_FILE_TREE_MIN_HEIGHT)
        .min(max_tree_height)
}

pub(super) fn draw_workspace_roots(
    ui: &mut egui::Ui,
    roots: &[FileNode],
    root_count: usize,
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
    app: &mut CodeReaderApp,
) {
    for root in roots {
        let mut remove_root = false;
        let has_status = node_contains_git_status(root, git_file_statuses);
        let default_open = app
            .workspace_root_open
            .get(&root.path)
            .copied()
            .unwrap_or(true);
        let state_id = ui.make_persistent_id(("workspace_root", &root.path));
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            state_id,
            default_open,
        );
        if app
            .pending_workspace_tree_reveal_path
            .as_deref()
            .is_some_and(|path| path.starts_with(&root.path))
        {
            state.set_open(true);
        }
        let mut root_label_clicked = false;
        let selected = app.selected_workspace_path.as_deref() == Some(root.path.as_path());
        let mut header = state.show_header(ui, |ui| {
            root_label_clicked = workspace_tree_folder_label(ui, &root.name, selected).clicked();
            if has_status {
                ui.label(git_folder_status_marker());
            }
            if root_count > 1
                && ui
                    .small_button("×")
                    .on_hover_text("Remove workspace folder")
                    .clicked()
            {
                remove_root = true;
            }
        });
        if root_label_clicked {
            app.workspace_sidebar_active = true;
            app.selected_workspace_path = Some(root.path.clone());
            app.pending_workspace_tree_reveal_path = None;
            header.toggle();
        }
        app.workspace_root_open
            .insert(root.path.clone(), header.is_open());
        header.body(|ui| {
            draw_file_nodes(ui, &root.children, git_file_statuses, app);
        });

        if remove_root {
            app.remove_root_from_current_session(&root.path);
            continue;
        }
        ui.add_space(6.0);
    }
}

pub(super) fn draw_file_nodes(
    ui: &mut egui::Ui,
    nodes: &[FileNode],
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
    app: &mut CodeReaderApp,
) {
    for node in nodes {
        if node.is_dir() {
            let has_status = node_contains_git_status(node, git_file_statuses);
            let state_id = ui.make_persistent_id(("workspace_dir", &node.path));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                state_id,
                false,
            );
            if app
                .pending_workspace_tree_reveal_path
                .as_deref()
                .is_some_and(|path| path.starts_with(&node.path))
            {
                state.set_open(true);
            }
            let mut folder_label_clicked = false;
            let selected = app.selected_workspace_path.as_deref() == Some(node.path.as_path());
            let mut header = state.show_header(ui, |ui| {
                folder_label_clicked =
                    workspace_tree_folder_label(ui, &node.name, selected).clicked();
                if has_status {
                    ui.label(git_folder_status_marker());
                }
            });
            if folder_label_clicked {
                app.workspace_sidebar_active = true;
                app.selected_workspace_path = Some(node.path.clone());
                app.pending_workspace_tree_reveal_path = None;
                header.toggle();
            }
            header.body(|ui| {
                draw_file_nodes(ui, &node.children, git_file_statuses, app);
            });
        } else {
            let status = git_file_statuses.get(&node.path).copied();
            let mut label = RichText::new(&node.name).monospace();
            if let Some(status) = status {
                label = label.color(status.color());
            }
            ui.horizontal(|ui| {
                let selected = app.selected_workspace_path.as_deref() == Some(node.path.as_path());
                let response = ui.add(
                    egui::Button::new(label)
                        .selected(selected)
                        .sense(egui::Sense::click_and_drag()),
                );
                if response.clicked() {
                    app.workspace_sidebar_active = true;
                    app.selected_workspace_path = Some(node.path.clone());
                    app.pending_workspace_tree_reveal_path = None;
                    app.open_file_in_focused_pane(node.path.clone());
                }
                if app
                    .pending_workspace_tree_reveal_path
                    .as_deref()
                    .is_some_and(|path| path == node.path.as_path())
                {
                    response.scroll_to_me(Some(egui::Align::Center));
                    app.pending_workspace_tree_reveal_path = None;
                }
                if response.drag_started() || response.dragged() {
                    app.dragging_workspace_file = Some(node.path.clone());
                }
                if let Some(status) = status {
                    ui.label(
                        RichText::new(status.label())
                            .monospace()
                            .color(status.color()),
                    );
                }
            });
        }
    }
}

pub(super) fn workspace_tree_folder_label(
    ui: &mut egui::Ui,
    name: &str,
    selected: bool,
) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(name).monospace())
            .selected(selected)
            .frame(false)
            .sense(egui::Sense::click()),
    )
}

pub(super) fn git_folder_status_marker() -> RichText {
    RichText::new("•")
        .monospace()
        .color(WorkspaceGitStatus::Modified.color())
}

pub(super) fn node_contains_git_status(
    node: &FileNode,
    git_file_statuses: &HashMap<PathBuf, WorkspaceGitStatus>,
) -> bool {
    if git_file_statuses.contains_key(&node.path) {
        return true;
    }
    node.children
        .iter()
        .any(|child| node_contains_git_status(child, git_file_statuses))
}

pub(super) fn workspace_right_pane_drop_target(ui: &mut egui::Ui, path: &Path) -> bool {
    let available = ui.available_size();
    let size = egui::vec2(available.x.max(120.0), available.y.max(160.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let hovered = response.hovered();
    let visuals = ui.visuals();
    let fill = if hovered {
        visuals.selection.bg_fill
    } else {
        visuals.faint_bg_color
    };
    let stroke = if hovered {
        visuals.selection.stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter()
        .rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<file>");
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("Drop {file_name} here"),
        FontId::proportional(14.0),
        visuals.text_color(),
    );

    hovered && ui.input(|input| input.pointer.any_released())
}

pub(super) fn pick_workspace_folder(title: &str) -> Option<PathBuf> {
    platform::current().pick_folder(title)
}
