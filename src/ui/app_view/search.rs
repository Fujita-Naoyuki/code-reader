//! Search Everywhere, the references list and workspace-wide text search.

use super::*;

impl CodeReaderApp {
    pub(super) fn poll_symbol_index(&mut self) {
        let Some(receiver) = self.search.symbol_index_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.search.symbol_index = result.index;
                self.bump_search_index_revision();
                self.set_background_status(format!(
                    "Indexed {} workspace symbols.",
                    self.search.symbol_index.len()
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.search.symbol_index_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.set_background_status("Symbol indexing failed.".to_owned());
            }
        }
    }

    pub(super) fn poll_workspace_search(&mut self, context: &egui::Context) {
        let Some(receiver) = self.search.workspace_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                if result.search_id == self.search.workspace.active_search_id {
                    self.search.workspace.results = result.results;
                    self.search.workspace.searched_query = result.query.clone();
                    self.search.workspace.searching = false;

                    let match_count = self.search.workspace.match_count();
                    let file_count = self.search.workspace.results.len();
                    self.status_message =
                        format!("Workspace search: {match_count} results in {file_count} files.");
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(100));
                self.search.workspace_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.search.workspace.searching = false;
                self.status_message = "Workspace search failed.".to_owned();
            }
        }
    }

    pub(super) fn bump_search_index_revision(&mut self) {
        self.search.index_revision = self.search.index_revision.wrapping_add(1);
        self.search.overlay.cache = None;
    }

    pub(super) fn open_search_overlay(&mut self) {
        self.close_references_overlay();
        self.search.overlay.open = true;
        self.search.overlay.selected_index = 0;
    }

    pub(super) fn close_search_overlay(&mut self) {
        self.search.overlay.open = false;
        self.search.overlay.query.clear();
        self.search.overlay.selected_index = 0;
        self.search.overlay.cache = None;
    }

    pub(super) fn open_references_overlay(&mut self, symbol: &str) {
        self.search.references_overlay.open = true;
        self.search.references_overlay.query.clear();
        self.search.references_overlay.selected_index = 0;
        self.search.references_overlay.symbol = symbol.to_owned();
    }

    pub(super) fn close_references_overlay(&mut self) {
        self.search.references_overlay.open = false;
        self.search.references_overlay.query.clear();
        self.search.references_overlay.selected_index = 0;
        self.search.references_overlay.symbol.clear();
    }

    pub(super) fn search_overlay_results(
        &mut self,
        bookmarks: &[SessionBookmark],
    ) -> Vec<SearchResult> {
        if let Some(cache) = &self.search.overlay.cache
            && cache.query == self.search.overlay.query
            && cache.revision == self.search.index_revision
        {
            return cache.results.clone();
        }

        let results = search_everywhere(
            &self.search.overlay.query,
            &self.file_tree.files,
            self.search.symbol_index.symbols(),
            bookmarks,
            &self.navigation.recent_files,
        );

        self.search.overlay.cache = Some(SearchOverlayCache {
            query: self.search.overlay.query.clone(),
            revision: self.search.index_revision,
            results: results.clone(),
        });
        results
    }

    pub(super) fn open_workspace_search(&mut self) {
        self.layout.show_workspace_sidebar = true;
        self.layout.sidebar_view = SidebarView::Search;
        self.search.workspace.open = true;
        self.search.workspace.focus_requested = true;
        self.shift_tap.last_tap_time = None;
    }

    pub(super) fn close_workspace_search(&mut self) {
        self.search.workspace.open = false;
        self.search.workspace.query.clear();
        self.search.workspace.results.clear();
        self.search.workspace.searched_query.clear();
        self.search.workspace.focus_requested = false;
        self.search.workspace.searching = false;
        self.search.workspace_receiver = None;
    }

    pub(super) fn run_workspace_search(&mut self) {
        let query = self.search.workspace.query.trim().to_owned();
        if query.is_empty() {
            self.search.workspace.results.clear();
            self.search.workspace.searched_query.clear();
            self.search.workspace.searching = false;
            self.search.workspace_receiver = None;
            self.status_message = "Workspace search query is empty.".to_owned();
            return;
        }

        self.search.next_workspace_id = self.search.next_workspace_id.wrapping_add(1);
        let search_id = self.search.next_workspace_id;
        let files = self.file_tree.files.clone();
        let query_for_thread = query.clone();
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let results =
                workspace_search_results(&files, &query_for_thread, WORKSPACE_SEARCH_MAX_MATCHES);
            let _ = sender.send(WorkspaceSearchResult {
                search_id,
                query: query_for_thread,
                results,
            });
        });

        self.search.workspace.active_search_id = search_id;
        self.search.workspace.searching = true;
        self.search.workspace.searched_query = query.clone();
        self.search.workspace_receiver = Some(receiver);
        self.status_message = format!("Workspace search started: {query}");
    }

    /// The results, grouped by file. Returns the match clicked.
    fn workspace_search_results_ui(&mut self, ui: &mut egui::Ui) -> Option<Location> {
        let mut open_location = None;
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let results = self.search.workspace.results.clone();
        ScrollArea::vertical()
            .id_salt("workspace_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.search.workspace.searching {
                    ui.label(RichText::new("Searching...").weak());
                    return;
                }
                if !self.search.workspace.searched_query.is_empty() && results.is_empty() {
                    ui.label(RichText::new("No results").weak());
                    return;
                }

                for file_result in &results {
                    if let Some(location) = workspace_search_file_ui(ui, file_result, &roots) {
                        open_location = Some(location);
                    }
                }
            });
        open_location
    }

    pub(super) fn workspace_search_ui(&mut self, ui: &mut egui::Ui) {
        let mut run_search = false;
        let mut close_search = false;

        let input_id = ui.id().with("workspace_search_query");
        let input_had_focus = ui.memory(|memory| memory.has_focus(input_id));
        if input_had_focus && ui.input(raw_enter_pressed) {
            run_search = true;
        }
        // egui drops focus at the start of the frame that carries a plain Escape, so check
        // who had focus in the previous frame.
        if ui.memory(|memory| memory.had_focus_last_frame(input_id)) && ui.input(raw_escape_pressed)
        {
            close_search = true;
        }

        let response = ui.add(
            TextEdit::singleline(&mut self.search.workspace.query)
                .id(input_id)
                .hint_text("Search workspace")
                .desired_width(f32::INFINITY),
        );
        if self.search.workspace.focus_requested {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.search.workspace.focus_requested = false;
        }

        let input_has_focus =
            response.has_focus() || ui.memory(|memory| memory.has_focus(input_id));
        if !input_had_focus && input_has_focus && ui.input(raw_enter_pressed) {
            run_search = true;
        }
        if !input_had_focus && input_has_focus && ui.input(raw_escape_pressed) {
            close_search = true;
        }

        ui.horizontal(|ui| {
            if ui.button("Search").clicked() {
                run_search = true;
            }
            if ui.button("Clear").clicked() {
                close_search = true;
            }
        });

        let match_count = self.search.workspace.match_count();
        let result_label = if self.search.workspace.searching {
            format!(
                "Searching {} files for `{}`",
                self.file_tree.files.len(),
                self.search.workspace.searched_query
            )
        } else {
            format!(
                "{match_count} results in {} files",
                self.search.workspace.results.len()
            )
        };
        ui.label(RichText::new(result_label).weak());

        let open_location = self.workspace_search_results_ui(ui);

        if run_search {
            self.run_workspace_search();
        }
        if close_search {
            self.close_workspace_search();
        }
        if let Some(location) = open_location {
            let path = location.path.clone();
            let line = location.range.start.line + 1;
            self.open_location_in_focused_pane(location, true);
            self.status_message = format!("Opened search result: {}:{line}", path.display());
        }
    }

    pub(super) fn consume_search_shortcuts(&mut self, ui: &mut egui::Ui) -> bool {
        let shortcut_pressed = ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::P)
                || input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::O)
        });

        if shortcut_pressed {
            self.shift_tap.last_tap_time = None;
            return true;
        }

        // "Shift x2" means two lone Shift taps: a Shift that is part of Shift+Enter,
        // Ctrl+Shift+Z or Shift+Arrow does not count, and neither does a pair with other
        // input in between. Taps count on release, so the second one never shares a frame
        // with a key the opened overlay would take.
        let (shift_down, other_input, time) = ui.input(|input| {
            let other_modifier = input.modifiers.ctrl
                || input.modifiers.alt
                || input.modifiers.mac_cmd
                || input.modifiers.command;
            let other_event = input.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Ime(_)
                )
            });
            (
                input.modifiers.shift,
                other_modifier || other_event,
                input.time,
            )
        });
        let shift_pressed = shift_down && !self.shift_tap.last_down;
        let shift_released = !shift_down && self.shift_tap.last_down;
        self.shift_tap.last_down = shift_down;

        if other_input {
            self.shift_tap.candidate = false;
            self.shift_tap.last_tap_time = None;
            return false;
        }
        if shift_pressed {
            self.shift_tap.candidate = true;
        }
        if !shift_released || !std::mem::take(&mut self.shift_tap.candidate) {
            return false;
        }
        if self.search.overlay.open {
            return false;
        }

        let should_open = self
            .shift_tap
            .last_tap_time
            .map(|last_time| time - last_time <= 0.45)
            .unwrap_or(false);
        self.shift_tap.last_tap_time = Some(time);

        if should_open {
            self.shift_tap.last_tap_time = None;
        }

        should_open
    }

    pub(super) fn show_search_overlay(&mut self, ui: &mut egui::Ui) -> Option<SearchResult> {
        if !self.search.overlay.open {
            return None;
        }

        let bookmarks = self
            .current_session()
            .map(|session| session.bookmarks.clone())
            .unwrap_or_default();
        let results = self.search_overlay_results(&bookmarks);

        if self.search.overlay.selected_index >= results.len() {
            self.search.overlay.selected_index = results.len().saturating_sub(1);
        }

        let mut selected_result = None;
        let mut close = false;

        egui::Window::new("Search Everywhere")
            .id(egui::Id::new("search_everywhere_overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(680.0, 460.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let escape = self.layout.modal_escape == Some(Modal::SearchEverywhere);
                let keys = ListOverlayKeys::consume(ui);

                let query_response = ui.add(
                    TextEdit::singleline(&mut self.search.overlay.query)
                        .hint_text("Search files, symbols, bookmarks")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.search.overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                keys.step_selection(&mut self.search.overlay.selected_index, results.len());
                if keys.enter {
                    selected_result = results
                        .get(self.search.overlay.selected_index)
                        .cloned()
                        .or_else(|| results.first().cloned());
                }

                overlay_summary_ui(ui, format!("{} results", results.len()));

                let results_width = ui.available_width();
                ScrollArea::vertical()
                    .id_salt("search_everywhere_results")
                    .auto_shrink([false, false])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        ui.set_min_width(results_width);

                        if results.is_empty() {
                            ui.label(RichText::new("No results").weak());
                            return;
                        }

                        for (index, result) in results.iter().enumerate() {
                            let selected = index == self.search.overlay.selected_index;
                            let response =
                                search_result_row_ui(ui, index, result, selected, results_width);
                            if response.hovered() {
                                self.search.overlay.selected_index = index;
                            }
                            if response.clicked() {
                                selected_result = Some(result.clone());
                            }
                        }
                    });
            });

        if close {
            self.close_search_overlay();
            return None;
        }

        if selected_result.is_some() {
            self.close_search_overlay();
        }

        selected_result
    }

    /// The references, grouped by file. Returns the one clicked.
    fn reference_groups_ui(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[ReferenceOverlayEntry],
        results_width: f32,
    ) -> Option<Location> {
        let mut clicked = None;
        let mut index = 0;
        while index < entries.len() {
            let group_path = entries[index].path.clone();
            let group_display_path = entries[index].display_path.clone();
            let group_start = index;
            while index < entries.len() && entries[index].path == group_path {
                index += 1;
            }
            let group_end = index;

            ui.horizontal(|ui| {
                ui.label(RichText::new(group_display_path).monospace().strong());
                ui.label(RichText::new(format!("{} references", group_end - group_start)).weak());
            });

            for (entry_index, entry) in entries[group_start..group_end].iter().enumerate() {
                let selected_index = group_start + entry_index;
                let selected = selected_index == self.search.references_overlay.selected_index;
                let text = if entry.preview.is_empty() {
                    format!("{:>5}", entry.line_number)
                } else {
                    format!("{:>5}  {}", entry.line_number, entry.preview)
                };
                let response = ui.add_sized(
                    egui::vec2(results_width, 24.0),
                    egui::Button::new(RichText::new(text).monospace()).selected(selected),
                );
                if response.hovered() {
                    self.search.references_overlay.selected_index = selected_index;
                }
                if response.clicked() {
                    clicked = Some(entry.location.clone());
                }
            }
            ui.add_space(6.0);
        }
        clicked
    }

    /// The overlay's scrolling list. Returns the reference that was clicked.
    fn references_overlay_list_ui(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[ReferenceOverlayEntry],
    ) -> Option<Location> {
        let results_width = ui.available_width();
        ScrollArea::vertical()
            .id_salt("references_overlay_results")
            .auto_shrink([false, false])
            .max_height(410.0)
            .show(ui, |ui| {
                ui.set_min_width(results_width);
                if entries.is_empty() {
                    ui.label(RichText::new("No references").weak());
                    return None;
                }

                self.reference_groups_ui(ui, entries, results_width)
            })
            .inner
    }

    pub(super) fn show_references_overlay(&mut self, ui: &mut egui::Ui) -> Option<Location> {
        if !self.search.references_overlay.open {
            return None;
        }
        if self.lsp.location_list_kind != LocationListKind::References
            || self.lsp.definition_candidates.is_empty()
        {
            self.close_references_overlay();
            return None;
        }

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let entries = reference_overlay_entries(
            &self.lsp.definition_candidates,
            &roots,
            &self.search.references_overlay.query,
        );
        if self.search.references_overlay.selected_index >= entries.len() {
            self.search.references_overlay.selected_index = entries.len().saturating_sub(1);
        }
        let file_count = entries
            .iter()
            .map(|entry| &entry.path)
            .collect::<std::collections::BTreeSet<_>>()
            .len();

        let mut selected_location = None;
        let mut close = false;

        egui::Window::new("References")
            .id(egui::Id::new("references_overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(720.0, 520.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let escape = self.layout.modal_escape == Some(Modal::References);
                let keys = ListOverlayKeys::consume(ui);

                reference_symbol_label_ui(ui, &self.search.references_overlay.symbol);
                let query_response = ui.add(
                    TextEdit::singleline(&mut self.search.references_overlay.query)
                        .hint_text("Filter references")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.search.references_overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                keys.step_selection(
                    &mut self.search.references_overlay.selected_index,
                    entries.len(),
                );
                if keys.enter {
                    selected_location = entries
                        .get(self.search.references_overlay.selected_index)
                        .map(|entry| entry.location.clone())
                        .or_else(|| entries.first().map(|entry| entry.location.clone()));
                }

                overlay_summary_ui(
                    ui,
                    format!("{} references in {} files", entries.len(), file_count),
                );

                if let Some(location) = self.references_overlay_list_ui(ui, &entries) {
                    selected_location = Some(location);
                }
            });

        if close {
            self.close_references_overlay();
            return None;
        }

        if selected_location.is_some() {
            self.close_references_overlay();
        }

        selected_location
    }

    pub(super) fn open_search_result(&mut self, result: SearchResult) {
        let kind = result.kind.label();
        let label = result.label.clone();
        self.open_location_in_focused_pane(result.location, true);
        self.status_message = format!("Opened {kind}: {label}");
    }
}

pub(super) const WORKSPACE_SEARCH_MAX_MATCHES: usize = 2000;

pub(super) const WORKSPACE_SEARCH_PREVIEW_CHARS: usize = 140;

#[derive(Debug, Clone, Default)]
pub(super) struct SearchOverlay {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) selected_index: usize,
    pub(super) cache: Option<SearchOverlayCache>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReferencesOverlay {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) selected_index: usize,
    pub(super) symbol: String,
}

#[derive(Debug, Clone)]
pub(super) struct ReferenceOverlayEntry {
    pub(super) path: PathBuf,
    pub(super) display_path: String,
    pub(super) line_number: u32,
    pub(super) preview: String,
    pub(super) location: Location,
}

#[derive(Debug, Clone)]
pub(super) struct SearchOverlayCache {
    pub(super) query: String,
    pub(super) revision: u64,
    pub(super) results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct WorkspaceSearchState {
    pub(super) open: bool,
    pub(super) focus_requested: bool,
    pub(super) searching: bool,
    pub(super) active_search_id: u64,
    pub(super) query: String,
    pub(super) searched_query: String,
    pub(super) results: Vec<WorkspaceSearchFileResult>,
}

impl WorkspaceSearchState {
    pub(super) fn match_count(&self) -> usize {
        self.results.iter().map(|result| result.matches.len()).sum()
    }
}

#[derive(Debug)]
pub(super) struct WorkspaceSearchResult {
    pub(super) search_id: u64,
    pub(super) query: String,
    pub(super) results: Vec<WorkspaceSearchFileResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceSearchFileResult {
    pub(super) path: PathBuf,
    pub(super) matches: Vec<WorkspaceSearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceSearchMatch {
    pub(super) position: Position,
    pub(super) preview: String,
}

pub(super) fn workspace_search_results(
    files: &[PathBuf],
    query: &str,
    max_matches: usize,
) -> Vec<WorkspaceSearchFileResult> {
    let query = query.trim();
    if query.is_empty() || max_matches == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut remaining_matches = max_matches;

    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Some(mut result) = workspace_search_file(path.clone(), &text, query) else {
            continue;
        };

        if result.matches.len() > remaining_matches {
            result.matches.truncate(remaining_matches);
        }
        remaining_matches = remaining_matches.saturating_sub(result.matches.len());
        results.push(result);

        if remaining_matches == 0 {
            break;
        }
    }

    results
}

pub(super) fn workspace_search_file(
    path: PathBuf,
    text: &str,
    query: &str,
) -> Option<WorkspaceSearchFileResult> {
    if query.is_empty() {
        return None;
    }

    let mut matches = Vec::new();

    for (line_index, line) in text.lines().enumerate() {
        let mut cursor = 0;
        while let Some(relative_start) = line[cursor..].find(query) {
            let character = cursor + relative_start;
            matches.push(WorkspaceSearchMatch {
                position: Position::new(line_index as u32, character as u32),
                preview: workspace_search_preview(line),
            });
            cursor = character + query.len();
        }
    }

    if matches.is_empty() {
        None
    } else {
        Some(WorkspaceSearchFileResult { path, matches })
    }
}

pub(super) fn workspace_search_preview(line: &str) -> String {
    let trimmed = line.trim();
    let mut preview = trimmed
        .chars()
        .take(WORKSPACE_SEARCH_PREVIEW_CHARS)
        .collect::<String>();
    if trimmed.chars().count() > WORKSPACE_SEARCH_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview
}

pub(super) fn location_result_label_with_preview(
    location: &Location,
    kind: LocationListKind,
    preview: Option<&str>,
) -> String {
    let base = format!(
        "{}:{}",
        location.path.display(),
        location.range.start.line + 1
    );
    if kind != LocationListKind::References {
        return base;
    }

    match preview {
        Some(preview) if !preview.is_empty() => format!("{base}  {preview}"),
        _ => base,
    }
}

pub(super) fn location_line_preview(location: &Location) -> Option<String> {
    let text = fs::read_to_string(&location.path).ok()?;
    let line = text.lines().nth(location.range.start.line as usize)?;
    Some(truncate_location_preview(line.trim()))
}

pub(super) fn reference_overlay_entries(
    candidates: &[DefinitionCandidate],
    roots: &[PathBuf],
    query: &str,
) -> Vec<ReferenceOverlayEntry> {
    let query = query.trim().to_lowercase();
    let mut entries = candidates
        .iter()
        .filter_map(|candidate| {
            let display_path = workspace_search_display_path(&candidate.location.path, roots);
            let line_number = candidate.location.range.start.line + 1;
            let preview = candidate.preview.clone().unwrap_or_default();
            let file_name = candidate
                .location
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let search_text =
                format!("{display_path} {file_name} {line_number} {preview}").to_lowercase();
            if !query.is_empty() && !search_text.contains(&query) {
                return None;
            }
            Some(ReferenceOverlayEntry {
                path: candidate.location.path.clone(),
                display_path,
                line_number,
                preview,
                location: candidate.location.clone(),
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| {
        left.display_path
            .cmp(&right.display_path)
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| {
                left.location
                    .range
                    .start
                    .character
                    .cmp(&right.location.range.start.character)
            })
    });
    entries
}

pub(super) fn truncate_location_preview(value: &str) -> String {
    const LIMIT: usize = 120;
    if value.chars().count() <= LIMIT {
        return value.to_owned();
    }
    let mut preview = value
        .chars()
        .take(LIMIT.saturating_sub(3))
        .collect::<String>();
    preview.push_str("...");
    preview
}

/// One result in Search Everywhere: kind, name and detail columns.
fn search_result_row_ui(
    ui: &mut egui::Ui,
    index: usize,
    result: &SearchResult,
    selected: bool,
    width: f32,
) -> egui::Response {
    const KIND_WIDTH: f32 = 88.0;
    const NAME_WIDTH: f32 = 260.0;
    const ROW_HEIGHT: f32 = 24.0;
    const COLUMN_GAP: f32 = 16.0;
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
    let kind_rect = egui::Rect::from_min_size(row_rect.min, egui::vec2(KIND_WIDTH, ROW_HEIGHT));
    let name_rect = egui::Rect::from_min_size(
        egui::pos2(kind_rect.right() + COLUMN_GAP, row_rect.top()),
        egui::vec2(NAME_WIDTH, ROW_HEIGHT),
    );
    let detail_rect = egui::Rect::from_min_max(
        egui::pos2(name_rect.right() + COLUMN_GAP, row_rect.top()),
        row_rect.right_bottom(),
    );
    let response = ui.interact(
        row_rect,
        ui.make_persistent_id(("search_everywhere_result", index)),
        egui::Sense::click(),
    );

    if response.hovered() {
        ui.painter().rect_filled(
            row_rect.shrink2(egui::vec2(0.0, 2.0)),
            2.0,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    if selected {
        ui.painter().rect_filled(
            row_rect.shrink2(egui::vec2(0.0, 2.0)),
            2.0,
            ui.visuals().selection.bg_fill,
        );
    }

    let (text_color, detail_color) = if selected {
        let color = ui.visuals().selection.stroke.color;
        (color, color)
    } else {
        (ui.visuals().text_color(), ui.visuals().weak_text_color())
    };
    let body_font = TextStyle::Body.resolve(ui.style());
    paint_row_text(
        ui,
        kind_rect.shrink2(egui::vec2(4.0, 0.0)),
        result.kind.label(),
        &body_font,
        text_color,
    );
    paint_row_text(
        ui,
        name_rect,
        &result.label,
        &TextStyle::Monospace.resolve(ui.style()),
        text_color,
    );
    paint_row_text(ui, detail_rect, &result.detail, &body_font, detail_color);
    response
}

/// One file's matches in the workspace search results. Returns the match clicked.
fn workspace_search_file_ui(
    ui: &mut egui::Ui,
    file_result: &WorkspaceSearchFileResult,
    roots: &[PathBuf],
) -> Option<Location> {
    let mut open_location = None;
    let label = format!(
        "{} ({})",
        workspace_search_display_path(&file_result.path, roots),
        file_result.matches.len()
    );
    ui.push_id(&file_result.path, |ui| {
        egui::CollapsingHeader::new(RichText::new(label).monospace())
            .default_open(true)
            .show(ui, |ui| {
                for (match_index, search_match) in file_result.matches.iter().enumerate() {
                    let line_label = format!(
                        "{:>5}: {}",
                        search_match.position.line + 1,
                        search_match.preview
                    );
                    let clicked = ui
                        .push_id(match_index, |ui| {
                            ui.button(RichText::new(line_label).monospace()).clicked()
                        })
                        .inner;
                    if clicked {
                        let range = Range::new(search_match.position, search_match.position);
                        open_location = Some(Location::new(file_result.path.clone(), range));
                    }
                }
            });
    });
    open_location
}

/// Enter, Up and Down as the list overlays consume them.
struct ListOverlayKeys {
    enter: bool,
    arrow_up: bool,
    arrow_down: bool,
}

impl ListOverlayKeys {
    fn consume(ui: &egui::Ui) -> Self {
        ui.input_mut(|input| Self {
            enter: input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            arrow_up: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            arrow_down: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
        })
    }

    fn step_selection(&self, selected_index: &mut usize, count: usize) {
        if self.arrow_down && count > 0 {
            *selected_index = (*selected_index + 1).min(count - 1);
        }
        if self.arrow_up && count > 0 {
            *selected_index = selected_index.saturating_sub(1);
        }
    }
}

fn reference_symbol_label_ui(ui: &mut egui::Ui, symbol: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Symbol").weak());
        ui.label(RichText::new(symbol).monospace());
    });
}

/// The line between an overlay's query and its list.
fn overlay_summary_ui(ui: &mut egui::Ui, summary: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(summary).weak());
        ui.separator();
        ui.label(RichText::new("Enter: open | Esc: close").weak());
    });
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_search_file_returns_line_positions_and_previews() {
        let path = PathBuf::from("src/app.rs");
        let result = workspace_search_file(
            path.clone(),
            "alpha\nlet beta = alpha;\nalpha beta",
            "alpha",
        )
        .unwrap();

        assert_eq!(result.path, path);
        assert_eq!(
            result.matches,
            vec![
                WorkspaceSearchMatch {
                    position: Position::new(0, 0),
                    preview: "alpha".to_owned(),
                },
                WorkspaceSearchMatch {
                    position: Position::new(1, 11),
                    preview: "let beta = alpha;".to_owned(),
                },
                WorkspaceSearchMatch {
                    position: Position::new(2, 0),
                    preview: "alpha beta".to_owned(),
                },
            ]
        );
    }
}
