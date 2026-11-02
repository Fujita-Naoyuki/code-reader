//! The regular editor built on egui's TextEdit, its input assistance and the find bar
//! shared with Markdown source mode.

use super::*;

impl EditorPane {
    pub(super) fn apply_immediate_wheel_scroll(&mut self, ui: &egui::Ui, max_scroll_y: f32) {
        let scroll_rect = ui.available_rect_before_wrap();
        let is_pointer_over_editor = ui.rect_contains_pointer(scroll_rect);
        let scroll_delta_y = ui.input(|input| input.smooth_scroll_delta().y);

        if is_pointer_over_editor && scroll_delta_y.abs() > f32::EPSILON {
            self.scroll.y = (self.scroll.y - scroll_delta_y).clamp(0.0, max_scroll_y.max(0.0));
            ui.input_mut(|input| {
                input.smooth_scroll_delta.y = 0.0;
            });
        }
    }

    pub(super) fn can_source_search(&self) -> bool {
        self.path().is_some()
            && !self.is_markdown()
            && (!self.is_csv() || self.csv.mode == CsvMode::Source)
    }

    pub(super) fn can_markdown_source_search(&self) -> bool {
        self.is_markdown() && self.markdown.mode == MarkdownMode::Markdown
    }

    pub(super) fn handle_input_assistance(
        &mut self,
        ui: &mut egui::Ui,
        text_edit_id: egui::Id,
        tab_width: usize,
    ) {
        if !ui.memory(|memory| memory.has_focus(text_edit_id)) {
            return;
        }

        let mut state = TextEditState::load(ui.ctx(), text_edit_id).unwrap_or_default();
        let Some(cursor_range) = state.cursor.char_range() else {
            return;
        };
        if cursor_range.primary.index != cursor_range.secondary.index {
            return;
        }

        let cursor_index = cursor_range.primary.index;
        let mut command = None;

        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                command = Some(InputAssistCommand::Insert {
                    text: " ".repeat(tab_width),
                    cursor_delta: tab_width,
                });
                return;
            }

            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                let text = smart_newline(&self.text, cursor_index, tab_width);
                command = Some(InputAssistCommand::Insert {
                    cursor_delta: text.chars().count(),
                    text,
                });
                return;
            }

            input.events.retain(|event| {
                let egui::Event::Text(text) = event else {
                    return true;
                };
                let mut chars = text.chars();
                let Some(character) = chars.next() else {
                    return true;
                };
                if chars.next().is_some() {
                    return true;
                }

                if let Some(close) = closing_pair(character) {
                    command = Some(InputAssistCommand::Insert {
                        text: format!("{character}{close}"),
                        cursor_delta: 1,
                    });
                    return false;
                }

                if is_closing_bracket(character)
                    && self.text.next_char_at(cursor_index) == Some(character)
                {
                    command = Some(InputAssistCommand::Move { cursor_delta: 1 });
                    return false;
                }

                true
            });
        });

        let Some(command) = command else {
            return;
        };

        let (new_cursor_index, text_changed) = match command {
            InputAssistCommand::Insert { text, cursor_delta } => {
                let Some(byte_index) = self.text.char_index_to_byte_index(cursor_index) else {
                    return;
                };
                self.text.insert_str(byte_index, &text);
                self.mark_text_changed();
                (cursor_index + cursor_delta, true)
            }
            InputAssistCommand::Move { cursor_delta } => (cursor_index + cursor_delta, false),
        };

        let cursor = CCursor::new(new_cursor_index);
        state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
        state.store(ui.ctx(), text_edit_id);
        self.cursor_position = self.text.char_index_to_position(new_cursor_index);
        if text_changed {
            self.target_position = None;
        }
        ui.ctx().request_repaint();
    }

    pub(super) fn cached_bracket_pair(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Option<(usize, usize)> {
        let cursor_position = self.cursor_position;
        if let Some(cache) = self.caches.bracket_pair
            && cache.source_len == source_len
            && cache.source_hash == source_hash
            && cache.cursor_position == cursor_position
        {
            return cache.pair;
        }

        let pair =
            cursor_position.and_then(|position| find_matching_bracket_pair(&self.text, position));
        self.caches.bracket_pair = Some(BracketPairCache {
            source_len,
            source_hash,
            cursor_position,
            pair,
        });
        pair
    }

    pub(super) fn cached_source_search_matches(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<SourceSearchMatch>> {
        let query = self.source_search.query.clone();
        if let Some(cache) = &self.caches.source_search
            && cache.source_len == source_len
            && cache.source_hash == source_hash
            && cache.query == query
        {
            return cache.matches.clone();
        }

        let matches = Rc::new(source_search_matches(&self.text, &query));
        self.caches.source_search = Some(SourceSearchCache {
            source_len,
            source_hash,
            query,
            matches: matches.clone(),
        });
        matches
    }

    pub(super) fn source_search_ui(&mut self, ui: &mut egui::Ui, matches: &[SourceSearchMatch]) {
        let search_id = ui.id().with(format!("source_search_{:?}", self.pane));
        let bar = find_bar_ui(
            ui,
            search_id,
            "Search current file",
            FindBarFields {
                query: &mut self.source_search.query,
                current_match: &mut self.source_search.current_match,
                focus_requested: &mut self.source_search.focus_requested,
            },
            matches.len(),
        );
        if bar.query_changed {
            self.source_search.scrolled_match = None;
        }

        if bar.close {
            self.close_source_search();
            return;
        }

        if bar.query_changed {
            return;
        }

        if bar.go_next {
            self.move_source_search_match(matches, 1);
        } else if bar.go_previous {
            self.move_source_search_match(matches, -1);
        } else {
            self.navigate_to_current_source_match(matches);
        }
    }

    pub(super) fn handle_source_search_global_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        matches: &[SourceSearchMatch],
    ) {
        if matches.is_empty() {
            return;
        }

        let mut go_next = false;
        let mut go_previous = false;
        ui.input_mut(|input| {
            if input.key_pressed(egui::Key::F3) {
                if input.modifiers.shift {
                    go_previous = true;
                } else {
                    go_next = true;
                }
                let _ = input.consume_key(input.modifiers, egui::Key::F3);
            }
        });

        if go_next {
            self.move_source_search_match(matches, 1);
        } else if go_previous {
            self.move_source_search_match(matches, -1);
        }
    }

    pub(super) fn move_source_search_match(&mut self, matches: &[SourceSearchMatch], delta: isize) {
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let current = self.source_search.current_match as isize;
        self.source_search.current_match = (current + delta).rem_euclid(len) as usize;
        self.source_search.scrolled_match = None;
        self.navigate_to_current_source_match(matches);
    }

    pub(super) fn navigate_to_current_source_match(&mut self, matches: &[SourceSearchMatch]) {
        if self.source_search.scrolled_match == Some(self.source_search.current_match) {
            return;
        }
        let Some(search_match) = matches.get(self.source_search.current_match) else {
            return;
        };
        if let Some(position) = self.text.byte_index_to_position(search_match.start) {
            self.pending_navigation = Some(PendingEditorNavigation {
                position,
                focus_editor: false,
            });
            self.cursor_position = Some(position);
            self.target_position = None;
            self.source_search.scrolled_match = Some(self.source_search.current_match);
        }
    }

    pub(super) fn close_source_search(&mut self) {
        self.source_search.open = false;
        self.source_search.focus_requested = false;
        self.source_search.query.clear();
        self.source_search.current_match = 0;
        self.source_search.match_count = 0;
        self.source_search.scrolled_match = None;
    }

    pub(super) fn source_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        bookmark_lines: &[u32],
    ) {
        let total_start = Instant::now();
        let scroll_before_y = self.scroll.y;
        let (source_len, source_hash) = self.text_fingerprint();
        let source_search_matches = self.source_search_frame(ui, focused, source_len, source_hash);
        let active_source_search_match = self
            .source_search
            .open
            .then_some(self.source_search.current_match);
        let source_search_match_count = source_search_matches.len();

        let pending = self.take_pending_source_navigation(ui);
        let text_edit_id = ui.id().with(format!("editor_text_{:?}", self.pane));
        let viewport_height = ui.available_height();
        let line_index = self.cached_line_index(source_len, source_hash);
        if should_use_virtual_source_view(source_len, line_index.line_count()) {
            self.virtual_source_editor_ui(
                ui,
                focused,
                text_edit_id,
                VirtualSourceFrame {
                    language_registry,
                    tab_width,
                    source_len,
                    source_hash,
                    search_matches: source_search_matches,
                    active_search_match: active_source_search_match,
                    pending_position: pending.position,
                    bookmark_lines,
                },
            );
            return;
        }
        let metrics = self.cached_editor_metrics(source_len, source_hash);
        let line_count = metrics.line_count;
        let editor_width = metrics.editor_width(ui, ui.available_width());
        self.prepare_source_scroll(ui, focused, text_edit_id, line_count, viewport_height);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("editor_pane_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll.y);

        let body_start = Instant::now();
        let scroll_output = scroll_area.show(ui, |ui| {
            self.apply_pending_source_cursor(ui, text_edit_id, pending);
            if focused && !self.read_only() {
                self.handle_input_assistance(ui, text_edit_id, tab_width);
            }

            let bracket_pair = self.cached_bracket_pair(source_len, source_hash);
            let highlight_spans =
                self.cached_syntax_highlight_spans(ui, language_registry, source_len, source_hash);

            ui.horizontal_top(|ui| {
                let gutter_rect = allocate_line_number_gutter(ui, line_count);

                let mut layouter = move |ui: &egui::Ui, text: &dyn TextBuffer, _wrap_width: f32| {
                    let mut job = highlighted_layout_job(
                        ui,
                        text.as_str(),
                        highlight_spans.as_slice(),
                        bracket_pair,
                        source_search_matches.as_slice(),
                        active_source_search_match,
                    );
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                let output = self.source_text_edit(ui, text_edit_id, editor_width, &mut layouter);

                if let Some(cursor_range) = output.cursor_range {
                    let next_position =
                        self.text.char_index_to_position(cursor_range.primary.index);
                    if self.cursor_position != next_position {
                        self.cursor_position = next_position;
                        ui.ctx().request_repaint();
                    }
                }

                let current_line = self.record_source_render_debug(ui, &output, line_count);
                let viewport_rect = ui.clip_rect();
                paint_current_line_background(ui, &output, viewport_rect, current_line);
                if !self.is_csv() {
                    paint_indent_guides(
                        ui,
                        &self.text,
                        tab_width,
                        current_line,
                        &output,
                        viewport_rect,
                    );
                }
                self.source_gutter_ui(ui, gutter_rect, line_count, bookmark_lines, &output);

                if output.response.changed() {
                    self.mark_text_changed();
                    self.target_position = None;
                }

                self.editor_context_menu(&output.response);
            });
        });
        self.finish_source_scroll(&scroll_output);
        self.record_textedit_perf(TextEditPerfSample {
            total_ms: elapsed_ms(total_start),
            body_ms: elapsed_ms(body_start),
            total_rows: line_index.line_count(),
            text_len: source_len,
            scroll_before_y,
            search_matches: source_search_match_count,
        });
    }

    /// Runs the find bar for this frame when it is open. Returns its matches.
    pub(super) fn source_search_frame(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<SourceSearchMatch>> {
        if !self.source_search.open {
            self.source_search.match_count = 0;
            return Rc::new(Vec::new());
        }
        let matches = self.cached_source_search_matches(source_len, source_hash);
        self.source_search.match_count = matches.len();
        if self.source_search.current_match >= matches.len() {
            self.source_search.current_match = matches.len().saturating_sub(1);
            self.source_search.scrolled_match = None;
        }
        if focused {
            self.handle_source_search_global_shortcuts(ui, matches.as_slice());
        }
        self.source_search_ui(ui, matches.as_slice());
        if self.source_search.open {
            ui.separator();
        }
        matches
    }

    /// Takes a requested jump and scrolls the target line about a third down the view.
    pub(super) fn take_pending_source_navigation(&mut self, ui: &egui::Ui) -> PendingSourceCursor {
        let pending_navigation = self.pending_navigation.take();
        let position = pending_navigation.map(|navigation| navigation.position);
        if let Some(position) = position {
            let row_height = editor_scroll_row_height(ui);
            self.scroll.y =
                (position.line as f32 * row_height - ui.available_height() * 0.35).max(0.0);
        }
        PendingSourceCursor {
            position,
            focus_editor: pending_navigation.is_some_and(|navigation| navigation.focus_editor),
        }
    }

    /// Clamps the scroll offset and applies PageUp / PageDown and the wheel.
    pub(super) fn prepare_source_scroll(
        &mut self,
        ui: &egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        line_count: usize,
        viewport_height: f32,
    ) {
        let max_scroll_y = if self.scroll.max_y > 0.0 {
            self.scroll.max_y
        } else {
            estimated_textedit_source_max_scroll_y(ui, line_count, viewport_height)
        };
        self.scroll.y = self.scroll.y.clamp(0.0, max_scroll_y);
        if let Some(delta) = editor_page_scroll_delta(ui, focused, text_edit_id, viewport_height) {
            self.scroll.y = (self.scroll.y + delta).clamp(0.0, max_scroll_y);
        }
        self.apply_immediate_wheel_scroll(ui, max_scroll_y);
    }

    pub(super) fn apply_pending_source_cursor(
        &mut self,
        ui: &egui::Ui,
        text_edit_id: egui::Id,
        pending: PendingSourceCursor,
    ) {
        let Some(target_position) = pending.position else {
            return;
        };
        let Some(char_index) = self.text.position_to_char_index(target_position) else {
            return;
        };
        let cursor = CCursor::new(char_index);
        let mut state = TextEditState::load(ui.ctx(), text_edit_id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
        state.store(ui.ctx(), text_edit_id);
        if pending.focus_editor {
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
        }
        self.cursor_position = Some(target_position);
    }

    pub(super) fn source_text_edit(
        &mut self,
        ui: &mut egui::Ui,
        text_edit_id: egui::Id,
        editor_width: f32,
        layouter: &mut dyn FnMut(&egui::Ui, &dyn TextBuffer, f32) -> Arc<egui::Galley>,
    ) -> egui::text_edit::TextEditOutput {
        let interactive = !self.read_only();
        TextEdit::multiline(&mut self.text)
            .id(text_edit_id)
            .font(TextStyle::Monospace)
            .code_editor()
            .frame(egui::Frame::NONE)
            .desired_width(editor_width)
            .desired_rows(TEXTEDIT_SOURCE_MIN_ROWS)
            .layouter(layouter)
            .interactive(interactive)
            .show(ui)
    }

    /// Keeps the render record for the Report. Returns the cursor's line.
    pub(super) fn record_source_render_debug(
        &mut self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        line_count: usize,
    ) -> Option<u32> {
        let current_line = self.cursor_position.map(|position| position.line);
        self.debug.render = Some(capture_editor_render_debug(
            output,
            line_count,
            line_count,
            editor_row_height(ui),
            self.scroll.y,
            ui.clip_rect(),
            current_line,
        ));
        current_line
    }

    pub(super) fn source_gutter_ui(
        &mut self,
        ui: &mut egui::Ui,
        gutter_rect: egui::Rect,
        line_count: usize,
        bookmark_lines: &[u32],
        output: &egui::text_edit::TextEditOutput,
    ) {
        let current_line = self.cursor_position.map(|position| position.line);
        let clicked = paint_line_number_gutter(
            ui,
            gutter_rect,
            line_count,
            current_line,
            bookmark_lines,
            output,
            ui.clip_rect(),
        );
        if let Some(line) = clicked
            && let Some(path) = self.path().map(Path::to_path_buf)
        {
            self.pending_action = Some(EditorAction::ToggleBookmark { path, line });
        }
    }

    pub(super) fn editor_context_menu(&mut self, response: &egui::Response) {
        response.context_menu(|ui| {
            if ui.button("Go to Definition").clicked() {
                self.pending_action = Some(EditorAction::GoToDefinition);
                ui.close();
            }
            if ui.button("Back").clicked() {
                self.pending_action = Some(EditorAction::Back);
                ui.close();
            }
        });
    }

    pub(super) fn finish_source_scroll(
        &mut self,
        scroll_output: &egui::scroll_area::ScrollAreaOutput<()>,
    ) {
        self.scroll.max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll.y = scroll_output.state.offset.y.clamp(0.0, self.scroll.max_y);
    }

    fn record_textedit_perf(&mut self, sample: TextEditPerfSample) {
        let visible_rows = self
            .debug
            .render
            .as_ref()
            .and_then(|debug| debug.anchored_range.clone())
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0);
        self.debug.perf = Some(EditorPerfDebug {
            view: "source-textedit",
            total_ms: sample.total_ms,
            header_ms: 0.0,
            body_ms: sample.body_ms,
            visible_rows,
            total_rows: sample.total_rows,
            text_len: sample.text_len,
            scroll_y: self.scroll.y,
            scroll_x: self.scroll.x,
            scroll_delta_y: self.scroll.y - sample.scroll_before_y,
            source_search_open: self.source_search.open,
            source_search_matches: sample.search_matches,
            syntax_spans: self
                .caches
                .syntax_highlight
                .as_ref()
                .map(|cache| cache.spans.len())
                .unwrap_or(0),
            virtual_cache_entries: self
                .virtual_editor
                .line_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
            line_number_cache_entries: self
                .virtual_editor
                .line_number_layout_cache
                .as_ref()
                .map(|cache| cache.entries.len())
                .unwrap_or(0),
            ..Default::default()
        });
    }
}

pub(super) const TEXTEDIT_SOURCE_MIN_ROWS: usize = 32;

#[derive(Debug, Clone, Default)]
pub(super) struct SourceSearchState {
    pub(super) open: bool,
    pub(super) focus_requested: bool,
    pub(super) query: String,
    pub(super) current_match: usize,
    pub(super) match_count: usize,
    pub(super) scrolled_match: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceSearchMatch {
    pub(super) start: usize,
    pub(super) end: usize,
}

impl TextBuffer for EditorTextBuffer {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.as_str()
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        let byte_index = self
            .char_index_to_byte_index(char_index)
            .unwrap_or(self.text.len());
        self.text.insert_str(byte_index, text);
        self.refresh_metadata();
        text.chars().count()
    }

    fn delete_char_range(&mut self, char_range: std::ops::Range<usize>) {
        if char_range.start > char_range.end {
            return;
        }

        let byte_start = self
            .char_index_to_byte_index(char_range.start)
            .unwrap_or(self.text.len());
        let byte_end = self
            .char_index_to_byte_index(char_range.end)
            .unwrap_or(self.text.len());
        if byte_start <= byte_end {
            self.text.drain(byte_start..byte_end);
            self.refresh_metadata();
        }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.refresh_metadata();
    }

    fn replace_with(&mut self, text: &str) {
        self.replace(text.to_owned());
    }

    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.refresh_metadata();
        text
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BracketPairCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) cursor_position: Option<Position>,
    pub(super) pair: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub(super) struct SourceSearchCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) query: String,
    pub(super) matches: Rc<Vec<SourceSearchMatch>>,
}

pub(super) fn source_search_matches(source: &str, query: &str) -> Vec<SourceSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    source
        .match_indices(query)
        .map(|(start, text)| SourceSearchMatch {
            start,
            end: start + text.len(),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(super) enum InputAssistCommand {
    Insert { text: String, cursor_delta: usize },
    Move { cursor_delta: usize },
}

pub(super) fn paint_current_line_background(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
    current_line: Option<u32>,
) {
    let Some(current_line) = current_line else {
        return;
    };

    let row_rect = galley_line_rect(output, current_line as usize);
    if row_rect.bottom() < viewport_rect.top() || row_rect.top() > viewport_rect.bottom() {
        return;
    }
    let line_rect = egui::Rect::from_min_max(
        egui::pos2(output.text_clip_rect.left(), row_rect.top()),
        egui::pos2(output.text_clip_rect.right(), row_rect.bottom()),
    )
    .intersect(viewport_rect);

    if line_rect.is_positive() {
        ui.painter()
            .rect_filled(line_rect, 0.0, current_line_overlay(ui));
    }
}

pub(super) fn paint_indent_guides(
    ui: &egui::Ui,
    source: &str,
    tab_width: usize,
    current_line: Option<u32>,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
) {
    let tab_width = tab_width.max(1);
    let char_width = editor_char_width(ui);
    let painter = ui.painter_at(output.text_clip_rect.intersect(viewport_rect));
    let guide_color = indent_guide_color(ui);
    let active_guide_color = active_indent_guide_color(ui);
    let line_count = source.lines().count().max(1);

    let visible_lines = visible_galley_row_range(output, line_count, viewport_rect);
    for (index, line) in source
        .lines()
        .enumerate()
        .skip(visible_lines.start)
        .take(visible_lines.end.saturating_sub(visible_lines.start))
    {
        let indent_columns = leading_indent_columns(line, tab_width);
        let guide_count = indent_columns / tab_width;
        if guide_count == 0 || line.trim().is_empty() {
            continue;
        }

        let row_rect = galley_line_rect(output, index);
        let y_range = row_rect.top()..=row_rect.bottom();
        let color = if current_line == Some(index as u32) {
            active_guide_color
        } else {
            guide_color
        };

        for guide_index in 1..=guide_count {
            let column = guide_index * tab_width;
            let x = output.galley_pos.x + char_width * column as f32;
            if !output.text_clip_rect.x_range().contains(x) {
                continue;
            }

            painter.line_segment(
                [
                    egui::pos2(x, *y_range.start()),
                    egui::pos2(x, *y_range.end()),
                ],
                egui::Stroke::new(1.0, color),
            );
        }
    }
}

pub(super) fn visible_galley_row_range(
    output: &egui::text_edit::TextEditOutput,
    line_count: usize,
    viewport_rect: egui::Rect,
) -> std::ops::Range<usize> {
    if line_count == 0 || output.galley.rows.is_empty() {
        return 0..0;
    }

    let row_height = output
        .galley
        .rows
        .first()
        .map_or(16.0, |row| row.rect().height())
        .max(1.0);
    let visible_top = viewport_rect.top() - output.galley_pos.y;
    let visible_bottom = viewport_rect.bottom() - output.galley_pos.y;
    let start = (visible_top / row_height).floor().max(0.0) as usize;
    let end = (visible_bottom / row_height).ceil().max(0.0) as usize + 1;

    start.min(line_count)..end.min(line_count)
}

pub(super) fn galley_line_rect(
    output: &egui::text_edit::TextEditOutput,
    line_index: usize,
) -> egui::Rect {
    output
        .galley
        .rows
        .get(line_index)
        .map(|row| row.rect().translate(output.galley_pos.to_vec2()))
        .unwrap_or_else(|| {
            let row_height = output
                .galley
                .rows
                .last()
                .map_or(16.0, |row| row.rect().height());
            let top = output.galley_pos.y + line_index as f32 * row_height;
            egui::Rect::from_min_size(
                egui::pos2(output.text_clip_rect.left(), top),
                egui::vec2(output.text_clip_rect.width(), row_height),
            )
        })
}

pub(super) fn estimated_textedit_source_max_scroll_y(
    ui: &egui::Ui,
    line_count: usize,
    viewport_height: f32,
) -> f32 {
    let content_height =
        line_count.max(TEXTEDIT_SOURCE_MIN_ROWS) as f32 * editor_scroll_row_height(ui);
    (content_height - viewport_height).max(0.0)
}

pub(super) fn scroll_area_vertical_max_scroll_y(
    content_size: egui::Vec2,
    inner_rect: egui::Rect,
) -> f32 {
    (content_size.y - inner_rect.height()).max(0.0)
}

/// A requested jump, waiting for the text field to exist this frame.
#[derive(Clone, Copy)]
pub(super) struct PendingSourceCursor {
    pub(super) position: Option<Position>,
    pub(super) focus_editor: bool,
}

/// One frame's numbers for the performance report.
struct TextEditPerfSample {
    total_ms: f32,
    body_ms: f32,
    total_rows: usize,
    text_len: usize,
    scroll_before_y: f32,
    search_matches: usize,
}

/// The parts of a search's state its find bar edits.
pub(super) struct FindBarFields<'a> {
    pub(super) query: &'a mut String,
    pub(super) current_match: &'a mut usize,
    pub(super) focus_requested: &'a mut bool,
}

/// What the find bar's frame asked for.
#[derive(Default)]
pub(super) struct FindBarEvents {
    pub(super) close: bool,
    pub(super) go_next: bool,
    pub(super) go_previous: bool,
    pub(super) query_changed: bool,
}

/// The find bar shared by source and rendered Markdown search.
pub(super) fn find_bar_ui(
    ui: &mut egui::Ui,
    search_id: egui::Id,
    hint: &str,
    fields: FindBarFields<'_>,
    match_count: usize,
) -> FindBarEvents {
    let FindBarFields {
        query,
        current_match,
        focus_requested,
    } = fields;
    let mut events = FindBarEvents::default();
    let mut keep_focus = false;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Find").small().strong());
        let input_had_focus = ui.memory(|memory| memory.has_focus(search_id));
        if input_had_focus {
            read_find_bar_enter(ui, &mut events, &mut keep_focus);
        }
        // egui drops focus at the start of the frame that carries a plain Escape, so check
        // who had focus in the previous frame.
        if ui.memory(|memory| memory.had_focus_last_frame(search_id))
            && ui.input(raw_escape_pressed)
        {
            events.close = true;
        }

        let response = ui.add(
            TextEdit::singleline(query)
                .id(search_id)
                .hint_text(hint)
                .desired_width(220.0),
        );
        if response.changed() {
            *current_match = 0;
            events.query_changed = true;
        }
        if *focus_requested {
            response.request_focus();
            *focus_requested = false;
        }
        let input_has_focus =
            response.has_focus() || ui.memory(|memory| memory.has_focus(search_id));
        if !input_had_focus && input_has_focus {
            read_find_bar_enter(ui, &mut events, &mut keep_focus);
            if ui.input(raw_escape_pressed) {
                events.close = true;
            }
        }

        let count_label = if query.trim().is_empty() {
            "0 results".to_owned()
        } else if match_count == 0 {
            "No results".to_owned()
        } else {
            format!("{}/{}", *current_match + 1, match_count)
        };
        ui.label(RichText::new(count_label).small().weak());

        if ui
            .add_enabled(match_count > 0, egui::Button::new("Previous"))
            .on_hover_text("Previous search match")
            .clicked()
        {
            events.go_previous = true;
        }
        if ui
            .add_enabled(match_count > 0, egui::Button::new("Next"))
            .on_hover_text("Next search match")
            .clicked()
        {
            events.go_next = true;
        }
        if ui.button("Close").on_hover_text("Close search").clicked() {
            events.close = true;
        }

        if keep_focus && !events.close {
            ui.memory_mut(|memory| memory.request_focus(search_id));
        }
    });
    events
}

fn read_find_bar_enter(ui: &egui::Ui, events: &mut FindBarEvents, keep_focus: &mut bool) {
    match ui.input(raw_search_enter_direction) {
        Some(1) => {
            events.go_next = true;
            *keep_focus = true;
        }
        Some(-1) => {
            events.go_previous = true;
            *keep_focus = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_search_matches_returns_byte_ranges() {
        let matches = source_search_matches("alpha beta alpha", "alpha");

        assert_eq!(
            matches,
            vec![
                SourceSearchMatch { start: 0, end: 5 },
                SourceSearchMatch { start: 11, end: 16 },
            ]
        );
    }
}
