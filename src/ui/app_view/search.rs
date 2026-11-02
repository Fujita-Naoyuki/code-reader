//! Search Everywhere, the references list and workspace-wide text search.

use super::*;

impl CodeReaderApp {
    pub(super) fn poll_symbol_index(&mut self) {
        let Some(receiver) = self.symbol_index_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.symbol_index = result.index;
                self.bump_search_index_revision();
                self.status_message =
                    format!("Indexed {} workspace symbols.", self.symbol_index.len());
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.symbol_index_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status_message = "Symbol indexing failed.".to_owned();
            }
        }
    }

    pub(super) fn poll_workspace_search(&mut self, context: &egui::Context) {
        let Some(receiver) = self.workspace_search_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                if result.search_id == self.workspace_search.active_search_id {
                    self.workspace_search.results = result.results;
                    self.workspace_search.searched_query = result.query.clone();
                    self.workspace_search.searching = false;

                    let match_count = self.workspace_search.match_count();
                    let file_count = self.workspace_search.results.len();
                    self.status_message =
                        format!("Workspace search: {match_count} results in {file_count} files.");
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(100));
                self.workspace_search_receiver = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.workspace_search.searching = false;
                self.status_message = "Workspace search failed.".to_owned();
            }
        }
    }

    pub(super) fn bump_search_index_revision(&mut self) {
        self.search_index_revision = self.search_index_revision.wrapping_add(1);
        self.search_overlay.cache = None;
    }

    pub(super) fn open_search_overlay(&mut self) {
        self.close_references_overlay();
        self.search_overlay.open = true;
        self.search_overlay.selected_index = 0;
    }

    pub(super) fn close_search_overlay(&mut self) {
        self.search_overlay.open = false;
        self.search_overlay.query.clear();
        self.search_overlay.selected_index = 0;
        self.search_overlay.cache = None;
    }

    pub(super) fn open_references_overlay(&mut self, symbol: &str) {
        self.references_overlay.open = true;
        self.references_overlay.query.clear();
        self.references_overlay.selected_index = 0;
        self.references_overlay.symbol = symbol.to_owned();
    }

    pub(super) fn close_references_overlay(&mut self) {
        self.references_overlay.open = false;
        self.references_overlay.query.clear();
        self.references_overlay.selected_index = 0;
        self.references_overlay.symbol.clear();
    }

    pub(super) fn search_overlay_results(
        &mut self,
        bookmarks: &[SessionBookmark],
    ) -> Vec<SearchResult> {
        if let Some(cache) = &self.search_overlay.cache
            && cache.query == self.search_overlay.query
            && cache.revision == self.search_index_revision
        {
            return cache.results.clone();
        }

        let results = search_everywhere(
            &self.search_overlay.query,
            &self.file_tree.files,
            self.symbol_index.symbols(),
            bookmarks,
            &self.recent_files,
        );

        self.search_overlay.cache = Some(SearchOverlayCache {
            query: self.search_overlay.query.clone(),
            revision: self.search_index_revision,
            results: results.clone(),
        });
        results
    }

    pub(super) fn open_workspace_search(&mut self) {
        self.show_workspace_sidebar = true;
        self.sidebar_view = SidebarView::Search;
        self.workspace_search.open = true;
        self.workspace_search.focus_requested = true;
        self.last_shift_tap_time = None;
    }

    pub(super) fn close_workspace_search(&mut self) {
        self.workspace_search.open = false;
        self.workspace_search.query.clear();
        self.workspace_search.results.clear();
        self.workspace_search.searched_query.clear();
        self.workspace_search.focus_requested = false;
        self.workspace_search.searching = false;
        self.workspace_search_receiver = None;
    }

    pub(super) fn run_workspace_search(&mut self) {
        let query = self.workspace_search.query.trim().to_owned();
        if query.is_empty() {
            self.workspace_search.results.clear();
            self.workspace_search.searched_query.clear();
            self.workspace_search.searching = false;
            self.workspace_search_receiver = None;
            self.status_message = "Workspace search query is empty.".to_owned();
            return;
        }

        self.next_workspace_search_id = self.next_workspace_search_id.wrapping_add(1);
        let search_id = self.next_workspace_search_id;
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

        self.workspace_search.active_search_id = search_id;
        self.workspace_search.searching = true;
        self.workspace_search.searched_query = query.clone();
        self.workspace_search_receiver = Some(receiver);
        self.status_message = format!("Workspace search started: {query}");
    }

    pub(super) fn workspace_search_ui(&mut self, ui: &mut egui::Ui) {
        let mut run_search = false;
        let mut close_search = false;
        let mut open_location = None;

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
            TextEdit::singleline(&mut self.workspace_search.query)
                .id(input_id)
                .hint_text("Search workspace")
                .desired_width(f32::INFINITY),
        );
        if self.workspace_search.focus_requested {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.workspace_search.focus_requested = false;
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

        let match_count = self.workspace_search.match_count();
        let result_label = if self.workspace_search.searching {
            format!(
                "Searching {} files for `{}`",
                self.file_tree.files.len(),
                self.workspace_search.searched_query
            )
        } else {
            format!(
                "{match_count} results in {} files",
                self.workspace_search.results.len()
            )
        };
        ui.label(RichText::new(result_label).weak());

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let results = self.workspace_search.results.clone();
        ScrollArea::vertical()
            .id_salt("workspace_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.workspace_search.searching {
                    ui.label(RichText::new("Searching...").weak());
                    return;
                }
                if !self.workspace_search.searched_query.is_empty() && results.is_empty() {
                    ui.label(RichText::new("No results").weak());
                    return;
                }

                for file_result in results {
                    let label = format!(
                        "{} ({})",
                        workspace_search_display_path(&file_result.path, &roots),
                        file_result.matches.len()
                    );
                    ui.push_id(&file_result.path, |ui| {
                        egui::CollapsingHeader::new(RichText::new(label).monospace())
                            .default_open(true)
                            .show(ui, |ui| {
                                for (match_index, search_match) in
                                    file_result.matches.iter().enumerate()
                                {
                                    let line_label = format!(
                                        "{:>5}: {}",
                                        search_match.position.line + 1,
                                        search_match.preview
                                    );
                                    let clicked = ui
                                        .push_id(match_index, |ui| {
                                            ui.button(RichText::new(line_label).monospace())
                                                .clicked()
                                        })
                                        .inner;
                                    if clicked {
                                        let range = Range::new(
                                            search_match.position,
                                            search_match.position,
                                        );
                                        open_location =
                                            Some(Location::new(file_result.path.clone(), range));
                                    }
                                }
                            });
                    });
                }
            });

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
            self.last_shift_tap_time = None;
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
        let shift_pressed = shift_down && !self.last_shift_down;
        let shift_released = !shift_down && self.last_shift_down;
        self.last_shift_down = shift_down;

        if other_input {
            self.shift_tap_candidate = false;
            self.last_shift_tap_time = None;
            return false;
        }
        if shift_pressed {
            self.shift_tap_candidate = true;
        }
        if !shift_released || !std::mem::take(&mut self.shift_tap_candidate) {
            return false;
        }
        if self.search_overlay.open {
            return false;
        }

        let should_open = self
            .last_shift_tap_time
            .map(|last_time| time - last_time <= 0.45)
            .unwrap_or(false);
        self.last_shift_tap_time = Some(time);

        if should_open {
            self.last_shift_tap_time = None;
        }

        should_open
    }

    pub(super) fn show_search_overlay(&mut self, ui: &mut egui::Ui) -> Option<SearchResult> {
        if !self.search_overlay.open {
            return None;
        }

        let bookmarks = self
            .current_session()
            .map(|session| session.bookmarks.clone())
            .unwrap_or_default();
        let results = self.search_overlay_results(&bookmarks);

        if self.search_overlay.selected_index >= results.len() {
            self.search_overlay.selected_index = results.len().saturating_sub(1);
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
                let (escape, enter, arrow_up, arrow_down) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    )
                });

                let query_response = ui.add(
                    TextEdit::singleline(&mut self.search_overlay.query)
                        .hint_text("Search files, symbols, bookmarks")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.search_overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                if arrow_down && !results.is_empty() {
                    self.search_overlay.selected_index =
                        (self.search_overlay.selected_index + 1).min(results.len() - 1);
                }
                if arrow_up && !results.is_empty() {
                    self.search_overlay.selected_index =
                        self.search_overlay.selected_index.saturating_sub(1);
                }
                if enter {
                    selected_result = results
                        .get(self.search_overlay.selected_index)
                        .cloned()
                        .or_else(|| results.first().cloned());
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} results", results.len())).weak());
                    ui.separator();
                    ui.label(RichText::new("Enter: open | Esc: close").weak());
                });
                ui.separator();

                let results_width = ui.available_width();
                let kind_width = 88.0;
                let name_width = 260.0;
                let row_height = 24.0;
                let column_gap = 16.0;
                let body_font = TextStyle::Body.resolve(ui.style());
                let name_font = TextStyle::Monospace.resolve(ui.style());
                let weak_text_color = ui.visuals().weak_text_color();
                let draw_left_text = |ui: &egui::Ui,
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
                            let selected = index == self.search_overlay.selected_index;
                            let (row_rect, _) = ui.allocate_exact_size(
                                egui::vec2(results_width, row_height),
                                egui::Sense::hover(),
                            );
                            let kind_rect = egui::Rect::from_min_size(
                                row_rect.min,
                                egui::vec2(kind_width, row_height),
                            );
                            let name_rect = egui::Rect::from_min_size(
                                egui::pos2(kind_rect.right() + column_gap, row_rect.top()),
                                egui::vec2(name_width, row_height),
                            );
                            let detail_rect = egui::Rect::from_min_max(
                                egui::pos2(name_rect.right() + column_gap, row_rect.top()),
                                row_rect.right_bottom(),
                            );
                            let response = ui.interact(
                                row_rect,
                                ui.make_persistent_id(("search_everywhere_result", index)),
                                egui::Sense::click(),
                            );

                            if response.hovered() {
                                self.search_overlay.selected_index = index;
                            }
                            if response.clicked() {
                                selected_result = Some(result.clone());
                            }

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

                            let text_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                ui.visuals().text_color()
                            };
                            let detail_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                weak_text_color
                            };
                            draw_left_text(
                                ui,
                                kind_rect.shrink2(egui::vec2(4.0, 0.0)),
                                result.kind.label(),
                                body_font.clone(),
                                text_color,
                            );
                            draw_left_text(
                                ui,
                                name_rect,
                                &result.label,
                                name_font.clone(),
                                text_color,
                            );
                            draw_left_text(
                                ui,
                                detail_rect,
                                &result.detail,
                                body_font.clone(),
                                detail_color,
                            );
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

    pub(super) fn show_references_overlay(&mut self, ui: &mut egui::Ui) -> Option<Location> {
        if !self.references_overlay.open {
            return None;
        }
        if self.location_list_kind != LocationListKind::References
            || self.definition_candidates.is_empty()
        {
            self.close_references_overlay();
            return None;
        }

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let entries = reference_overlay_entries(
            &self.definition_candidates,
            &roots,
            &self.references_overlay.query,
        );
        if self.references_overlay.selected_index >= entries.len() {
            self.references_overlay.selected_index = entries.len().saturating_sub(1);
        }
        let mut files = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        files.sort();
        files.dedup();
        let file_count = files.len();

        let mut selected_location = None;
        let mut close = false;

        egui::Window::new("References")
            .id(egui::Id::new("references_overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2(720.0, 520.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let (escape, enter, arrow_up, arrow_down) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    )
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Symbol").weak());
                    ui.label(RichText::new(&self.references_overlay.symbol).monospace());
                });
                let query_response = ui.add(
                    TextEdit::singleline(&mut self.references_overlay.query)
                        .hint_text("Filter references")
                        .desired_width(f32::INFINITY),
                );
                query_response.request_focus();
                if query_response.changed() {
                    self.references_overlay.selected_index = 0;
                }

                if escape {
                    close = true;
                }
                if arrow_down && !entries.is_empty() {
                    self.references_overlay.selected_index =
                        (self.references_overlay.selected_index + 1).min(entries.len() - 1);
                }
                if arrow_up && !entries.is_empty() {
                    self.references_overlay.selected_index =
                        self.references_overlay.selected_index.saturating_sub(1);
                }
                if enter {
                    selected_location = entries
                        .get(self.references_overlay.selected_index)
                        .map(|entry| entry.location.clone())
                        .or_else(|| entries.first().map(|entry| entry.location.clone()));
                }

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} references in {} files",
                            entries.len(),
                            file_count
                        ))
                        .weak(),
                    );
                    ui.separator();
                    ui.label(RichText::new("Enter: open | Esc: close").weak());
                });
                ui.separator();

                let results_width = ui.available_width();
                ScrollArea::vertical()
                    .id_salt("references_overlay_results")
                    .auto_shrink([false, false])
                    .max_height(410.0)
                    .show(ui, |ui| {
                        ui.set_min_width(results_width);
                        if entries.is_empty() {
                            ui.label(RichText::new("No references").weak());
                            return;
                        }

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
                                ui.label(
                                    RichText::new(format!(
                                        "{} references",
                                        group_end - group_start
                                    ))
                                    .weak(),
                                );
                            });

                            for (entry_index, entry) in
                                entries[group_start..group_end].iter().enumerate()
                            {
                                let selected_index = group_start + entry_index;
                                let selected =
                                    selected_index == self.references_overlay.selected_index;
                                let text = if entry.preview.is_empty() {
                                    format!("{:>5}", entry.line_number)
                                } else {
                                    format!("{:>5}  {}", entry.line_number, entry.preview)
                                };
                                let response = ui.add_sized(
                                    egui::vec2(results_width, 24.0),
                                    egui::Button::new(RichText::new(text).monospace())
                                        .selected(selected),
                                );
                                if response.hovered() {
                                    self.references_overlay.selected_index = selected_index;
                                }
                                if response.clicked() {
                                    selected_location = Some(entry.location.clone());
                                }
                            }
                            ui.add_space(6.0);
                        }
                    });
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
