//! Painting and pointer input of the large-file editor: only visible lines are laid out
//! and cached, with its own scrollbars and immediate wheel scrolling.

use super::*;

/// What every line of a large file is laid out with this frame.
#[derive(Clone, Copy)]
pub(super) struct VirtualLineStyle<'a> {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) highlight_spans: &'a [HighlightSpan],
    pub(super) highlights: LineHighlights<'a>,
}

/// The arguments of `virtual_source_editor_ui` that come from the pane's frame.
pub(super) struct VirtualSourceFrame<'a> {
    pub(super) language_registry: &'a LanguageRegistry,
    pub(super) tab_width: usize,
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) search_matches: Rc<Vec<SourceSearchMatch>>,
    pub(super) active_search_match: Option<usize>,
    pub(super) pending_position: Option<Position>,
    pub(super) bookmark_lines: &'a [u32],
}

impl EditorPane {
    pub(super) fn cached_virtual_line_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_info: LineInfo,
        style: &VirtualLineStyle<'_>,
        highlight_line_span_range: std::ops::Range<usize>,
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let VirtualLineStyle {
            source_len,
            source_hash,
            highlight_spans,
            highlights,
        } = *style;
        let LineHighlights {
            bracket_pair,
            search_matches: source_search_matches,
            ..
        } = highlights;

        let mut frame_counters = frame_counters;
        let can_cache = source_search_matches.is_empty()
            && !line_contains_bracket_pair(line_info, bracket_pair);
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();

        if can_cache {
            let cache = self
                .virtual_editor
                .line_layout_cache
                .get_or_insert_with(|| VirtualLineLayoutCache {
                    source_len,
                    source_hash,
                    dark_mode,
                    entries: HashMap::new(),
                });
            if cache.source_len != source_len
                || cache.source_hash != source_hash
                || cache.dark_mode != dark_mode
            {
                *cache = VirtualLineLayoutCache {
                    source_len,
                    source_hash,
                    dark_mode,
                    entries: HashMap::new(),
                };
            }

            if let Some(entry) = cache.entries.get_mut(&line_number)
                && entry.line_start == line_info.start
                && entry.line_content_end == line_info.content_end
            {
                entry.last_used_frame = frame;
                if let Some(counters) = frame_counters.as_deref_mut() {
                    counters.galley_hits += 1;
                }
                return entry.galley.clone();
            }
        }
        if let Some(counters) = frame_counters {
            counters.galley_misses += 1;
        }

        let mut job = highlighted_line_layout_job(
            ui,
            &self.text,
            line_info,
            highlight_spans,
            highlight_line_span_range,
            highlights,
        );
        job.wrap.max_width = f32::INFINITY;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));

        if can_cache && let Some(cache) = &mut self.virtual_editor.line_layout_cache {
            cache.entries.insert(
                line_number,
                CachedVirtualLineLayout {
                    line_start: line_info.start,
                    line_content_end: line_info.content_end,
                    galley: galley.clone(),
                    last_used_frame: frame,
                },
            );
            prune_virtual_line_layout_cache(cache, frame);
        }

        galley
    }

    pub(super) fn cached_virtual_line_number_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_count: usize,
        active: bool,
        (inactive_color, active_color): (Color32, Color32),
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let mut frame_counters = frame_counters;
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();
        let cache = self
            .virtual_editor
            .line_number_layout_cache
            .get_or_insert_with(|| VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            });
        if cache.line_count != line_count
            || cache.dark_mode != dark_mode
            || cache.inactive_color != inactive_color
            || cache.active_color != active_color
        {
            *cache = VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            };
        }

        let key = (line_number, active);
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used_frame = frame;
            if let Some(counters) = frame_counters.as_deref_mut() {
                counters.line_number_hits += 1;
            }
            return entry.galley.clone();
        }
        if let Some(counters) = frame_counters {
            counters.line_number_misses += 1;
        }

        let color = if active { active_color } else { inactive_color };
        let galley =
            ui.painter()
                .layout_no_wrap((line_number + 1).to_string(), editor_font_id(), color);
        cache.entries.insert(
            key,
            CachedVirtualLineNumberLayout {
                galley: galley.clone(),
                last_used_frame: frame,
            },
        );
        prune_virtual_line_number_layout_cache(cache, frame);

        galley
    }

    pub(super) fn virtual_source_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        frame: VirtualSourceFrame<'_>,
    ) {
        let VirtualSourceFrame {
            language_registry,
            tab_width,
            mut source_len,
            mut source_hash,
            search_matches: source_search_matches,
            active_search_match: active_source_search_match,
            pending_position,
            bookmark_lines,
        } = frame;

        let total_start = Instant::now();
        let scroll_before_y = self.scroll.y;
        if !focused {
            self.virtual_editor.ime_preedit = None;
        }
        if let Some(position) = pending_position {
            self.cursor_position = Some(position);
            self.virtual_editor.selection = None;
            self.virtual_editor.ime_preedit = None;
        }
        if self.handle_virtual_text_input(ui, focused, text_edit_id, tab_width) {
            (source_len, source_hash) = self.text_fingerprint();
        }

        let line_index = self.cached_line_index(source_len, source_hash);
        let bracket_pair = self.cached_bracket_pair(source_len, source_hash);
        let highlight_spans =
            self.cached_syntax_highlight_spans(ui, language_registry, source_len, source_hash);
        let highlight_line_span_ranges =
            self.cached_syntax_highlight_line_span_ranges(source_len, source_hash);

        let layout = VirtualLayout::allocate(ui, line_index.as_ref());
        let interaction = self.virtual_editor_interaction(
            ui,
            (focused, text_edit_id),
            line_index.as_ref(),
            &layout,
        );

        let body_start = Instant::now();
        let mut frame_counters = VirtualFrameCounters::default();
        self.paint_virtual_body(
            ui,
            &VirtualBodyFrame {
                layout: &layout,
                viewport: &interaction.viewport,
                line_index: line_index.as_ref(),
                visible_range: interaction.visible_range.clone(),
                current_line: interaction.current_line,
                tab_width,
                bookmark_lines,
                text_edit_id,
                line_style: VirtualLineStyle {
                    source_len,
                    source_hash,
                    highlight_spans: highlight_spans.as_slice(),
                    highlights: LineHighlights {
                        bracket_pair,
                        search_matches: source_search_matches.as_slice(),
                        active_search_match: active_source_search_match,
                    },
                },
                highlight_line_span_ranges: highlight_line_span_ranges.as_slice(),
            },
            &mut frame_counters,
        );
        self.finish_virtual_frame(
            ui,
            (focused, text_edit_id),
            line_index.as_ref(),
            &layout,
            &interaction,
        );
        let body_ms = elapsed_ms(body_start);
        self.record_virtual_perf(VirtualPerfSample {
            total_ms: elapsed_ms(total_start),
            body_ms,
            visible_rows: interaction.visible_range.len(),
            total_rows: line_index.line_count(),
            counters: frame_counters,
            text_len: source_len,
            scroll_before_y,
            search_matches: source_search_matches.len(),
            syntax_spans: highlight_spans.len(),
        });
    }

    /// Everything the pointer and keys do this frame, before anything is painted.
    fn virtual_editor_interaction(
        &mut self,
        ui: &mut egui::Ui,
        (focused, text_edit_id): (bool, egui::Id),
        line_index: &LineIndex,
        layout: &VirtualLayout,
    ) -> VirtualInteraction {
        let line_count = line_index.line_count();
        let mut current_line = self.cursor_position.map(|position| position.line);
        // The viewport is the focusable widget behind `text_edit_id`.
        let response = ui.interact(
            layout.viewport_rect,
            text_edit_id,
            egui::Sense::click_and_drag(),
        );
        lock_virtual_editor_focus(ui, &response, layout.viewport_rect, text_edit_id);
        let (content_rect, row_height) = (layout.content_rect, layout.row_height);
        self.scroll.y = self.scroll.y.clamp(0.0, layout.max_scroll_y);
        self.scroll.x = self.scroll.x.clamp(0.0, layout.max_scroll_x);
        self.apply_virtual_wheel(
            ui,
            &response,
            content_rect,
            layout.max_scroll_x,
            layout.max_scroll_y,
        );

        let visible_start = (self.scroll.y / row_height).floor().max(0.0) as usize;
        let visible_end = (((self.scroll.y + content_rect.height()) / row_height).ceil() as usize
            + 1)
        .min(line_count);
        let visible_range = visible_start.min(line_count)..visible_end;

        if self.handle_virtual_navigation_input(
            ui,
            focused,
            text_edit_id,
            line_index,
            &layout.nav_metrics(),
        ) {
            current_line = self.cursor_position.map(|position| position.line);
            response.request_focus();
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            ui.ctx().request_repaint();
        }

        // After navigation, which may have scrolled to keep the cursor visible.
        let viewport = layout.viewport(self.scroll.x, self.scroll.y);
        if let Some(line) = self.handle_virtual_pointer(
            ui,
            &response,
            (text_edit_id, focused),
            line_index,
            &viewport,
            visible_range.clone(),
        ) {
            current_line = Some(line);
        }

        self.debug.render = Some(capture_virtual_editor_render_debug(
            line_count,
            row_height,
            self.scroll.y,
            content_rect,
            visible_range.clone(),
            current_line,
        ));
        self.editor_context_menu(&response);
        VirtualInteraction {
            response,
            viewport,
            current_line,
            visible_range,
        }
    }

    /// The current line, indent guides, selection, the visible lines and the gutter's edge.
    fn paint_virtual_body(
        &mut self,
        ui: &egui::Ui,
        body: &VirtualBodyFrame<'_>,
        frame_counters: &mut VirtualFrameCounters,
    ) {
        let VirtualBodyFrame {
            layout,
            viewport,
            line_index,
            current_line,
            tab_width,
            bookmark_lines,
            text_edit_id,
            highlight_line_span_ranges,
            ..
        } = *body;
        let (content_rect, row_height) = (layout.content_rect, layout.row_height);
        let painter = ui.painter_at(layout.viewport_rect);
        let text_painter = ui.painter_at(viewport.text_clip_rect);
        if let Some(current_line) = current_line
            && body.visible_range.contains(&(current_line as usize))
        {
            let top = content_rect.top() + current_line as f32 * row_height - self.scroll.y;
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(content_rect.left(), top),
                egui::vec2(content_rect.width(), row_height),
            );
            painter.rect_filled(
                row_rect.intersect(content_rect),
                0.0,
                current_line_overlay(ui),
            );
        }

        if !self.is_csv() {
            paint_virtual_indent_guides(
                ui,
                line_index,
                &self.text,
                body.visible_range.clone(),
                tab_width,
                current_line,
                viewport,
            );
        }

        paint_virtual_selection(
            ui,
            line_index,
            &self.text,
            self.virtual_editor
                .selection
                .and_then(|selection| selection.normalized()),
            body.visible_range.clone(),
            viewport,
        );

        for line_number in body.visible_range.clone() {
            let Some(line_info) = line_index.line(line_number) else {
                continue;
            };
            self.paint_virtual_line(
                ui,
                line_number,
                line_info,
                &VirtualLinePaint {
                    painter: &painter,
                    text_painter: &text_painter,
                    viewport,
                    gutter_width: layout.gutter_width,
                    line_count: line_index.line_count(),
                    is_current_line: current_line == Some(line_number as u32),
                    bookmarked: bookmark_lines.contains(&(line_number as u32)),
                    text_edit_id,
                    style: &body.line_style,
                    highlight_line_span_range: highlight_line_span_ranges
                        .get(line_number)
                        .cloned()
                        .unwrap_or(0..0),
                },
                frame_counters,
            );
        }

        let gutter_right = content_rect.left() + layout.gutter_width;
        painter.line_segment(
            [
                egui::pos2(gutter_right, content_rect.top()),
                egui::pos2(gutter_right, content_rect.bottom()),
            ],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }

    /// The IME area and uncommitted IME text, the cursor on top of them, then the scrollbars.
    fn finish_virtual_frame(
        &mut self,
        ui: &egui::Ui,
        (focused, text_edit_id): (bool, egui::Id),
        line_index: &LineIndex,
        layout: &VirtualLayout,
        interaction: &VirtualInteraction,
    ) {
        let VirtualInteraction {
            response,
            viewport,
            visible_range,
            ..
        } = interaction;
        let cursor_rect = virtual_cursor_rect(
            Some(ui),
            line_index,
            &self.text,
            self.cursor_position,
            visible_range.clone(),
            viewport,
        );
        self.publish_virtual_ime_area(ui, response, focused, text_edit_id, cursor_rect);
        paint_virtual_ime_preedit(
            ui,
            self.virtual_editor.ime_preedit.as_deref(),
            cursor_rect,
            viewport.text_clip_rect,
            viewport.content_rect,
            viewport.row_height,
        );
        paint_virtual_cursor(
            ui,
            line_index,
            &self.text,
            self.cursor_position,
            visible_range.clone(),
            viewport,
        );
        handle_and_paint_virtual_scrollbars(
            ui,
            layout.viewport_rect,
            layout.total_width,
            layout.total_height,
            &mut self.scroll.x,
            &mut self.scroll.y,
        );
    }

    fn record_virtual_perf(&mut self, sample: VirtualPerfSample) {
        self.debug.perf = Some(EditorPerfDebug {
            view: "source-virtual",
            total_ms: sample.total_ms,
            header_ms: 0.0,
            body_ms: sample.body_ms,
            visible_rows: sample.visible_rows,
            total_rows: sample.total_rows,
            galley_hits: sample.counters.galley_hits,
            galley_misses: sample.counters.galley_misses,
            line_number_hits: sample.counters.line_number_hits,
            line_number_misses: sample.counters.line_number_misses,
            text_len: sample.text_len,
            scroll_y: self.scroll.y,
            scroll_x: self.scroll.x,
            scroll_delta_y: self.scroll.y - sample.scroll_before_y,
            source_search_open: self.source_search.open,
            source_search_matches: sample.search_matches,
            syntax_spans: sample.syntax_spans,
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
        });
    }

    /// Scrolls for this frame's wheel input while the pointer is over the editor.
    fn apply_virtual_wheel(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        content_rect: egui::Rect,
        max_scroll_x: f32,
        max_scroll_y: f32,
    ) {
        if response.hovered() {
            let (immediate_scroll, smooth_scroll, shift) = ui.input(|input| {
                (
                    immediate_wheel_scroll_delta(&input.raw.events, content_rect.height()),
                    input.smooth_scroll_delta(),
                    input.modifiers.shift,
                )
            });
            // Wheel notches are applied at once; egui smooths the same notch over the next
            // frames, so that later smoothed delta is skipped instead of scrolling twice.
            let scroll_delta = match immediate_scroll {
                Some(raw) => {
                    let raw = if shift {
                        egui::vec2(raw.x + raw.y, 0.0)
                    } else {
                        raw
                    };
                    self.virtual_editor.wheel_lead += raw - smooth_scroll;
                    raw
                }
                None => consume_wheel_lead(&mut self.virtual_editor.wheel_lead, smooth_scroll),
            };
            if scroll_delta != egui::Vec2::ZERO {
                self.scroll.y = (self.scroll.y - scroll_delta.y).clamp(0.0, max_scroll_y);
                self.scroll.x = (self.scroll.x - scroll_delta.x).clamp(0.0, max_scroll_x);
            }
            if smooth_scroll != egui::Vec2::ZERO {
                ui.input_mut(|input| {
                    input.smooth_scroll_delta = egui::Vec2::ZERO;
                });
                ui.ctx().request_repaint();
            }
        } else {
            self.virtual_editor.wheel_lead = egui::Vec2::ZERO;
        }
    }

    /// Drag selection and clicks. Returns the line the cursor moved to, if it did.
    fn handle_virtual_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        (text_edit_id, focused): (egui::Id, bool),
        line_index: &LineIndex,
        viewport: &VirtualViewport,
        visible_range: std::ops::Range<usize>,
    ) -> Option<u32> {
        let line_count = line_index.line_count();
        let mut moved_to_line = None;
        if let Some((anchor, head)) =
            virtual_editor_drag_selection(ui, response, line_index, &self.text, viewport)
        {
            self.virtual_editor.selection = Some(VirtualTextSelection { anchor, head });
            self.cursor_position = Some(head);
            self.virtual_editor.ime_preedit = None;
            moved_to_line = Some(head.line);
            self.target_position = None;
            response.request_focus();
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            ui.ctx().request_repaint();
        }

        let click_input = virtual_editor_click_input(
            ui,
            response,
            text_edit_id,
            focused,
            viewport,
            line_count,
            visible_range,
        );
        let click_event_index = click_input
            .event
            .map(|event| self.debug.virtual_click.push_event(event));
        if let Some(pointer_pos) = click_input.position {
            let line = ((pointer_pos.y - viewport.content_rect.top() + self.scroll.y)
                / viewport.row_height)
                .floor()
                .max(0.0) as usize;
            let line = line.min(line_count.saturating_sub(1));
            if let Some(line_info) = line_index.line(line) {
                moved_to_line = Some(self.apply_virtual_click(
                    ui,
                    (line, line_info),
                    pointer_pos,
                    click_input.double_clicked,
                    viewport,
                    click_event_index,
                ));
                response.request_focus();
                ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                ui.ctx().request_repaint();
            }
        }
        moved_to_line
    }

    /// Puts the cursor where the click landed; a double click selects the word there.
    /// Returns the cursor's line.
    fn apply_virtual_click(
        &mut self,
        ui: &egui::Ui,
        (line, line_info): (usize, LineInfo),
        pointer_pos: egui::Pos2,
        double_clicked: bool,
        viewport: &VirtualViewport,
        click_event_index: Option<u64>,
    ) -> u32 {
        let (text_left, text_clip_left) = (viewport.text_left, viewport.text_clip_rect.left());
        let line_text = self.text.line_text_or_empty(line_info);
        let character = virtual_click_byte_column(
            Some(ui),
            line_text,
            pointer_pos.x,
            text_left,
            text_clip_left,
            viewport.char_width,
        );
        let position = Position::new(line as u32, character);
        let word = double_clicked
            .then(|| virtual_word_selection_at_position(line_text, line as u32, character))
            .flatten();
        self.cursor_position = Some(word.map_or(position, |selection| selection.head));
        self.virtual_editor.selection = word;
        self.virtual_editor.ime_preedit = None;
        self.target_position = None;
        if let Some(event_index) = click_event_index {
            self.debug.virtual_click.last_accept = Some(VirtualClickAcceptDebug {
                event_index,
                pointer_pos,
                line,
                character,
                cursor_position: position,
                text_left,
                text_clip_left,
                x_offset: virtual_click_x_offset(pointer_pos.x, text_left, text_clip_left),
            });
        }
        word.map_or(position.line, |selection| selection.head.line)
    }

    fn paint_virtual_line(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_info: LineInfo,
        paint: &VirtualLinePaint<'_>,
        frame_counters: &mut VirtualFrameCounters,
    ) {
        let VirtualLinePaint {
            painter,
            text_painter,
            viewport,
            gutter_width,
            line_count,
            is_current_line,
            bookmarked,
            text_edit_id,
            style,
            ref highlight_line_span_range,
        } = *paint;
        let highlight_line_span_range = highlight_line_span_range.clone();
        let VirtualViewport {
            content_rect,
            text_left,
            row_height,
            ..
        } = *viewport;
        let content_left = content_rect.left();
        let inactive_color = ui.visuals().weak_text_color();
        let active_color = ui.visuals().text_color();
        let top = content_rect.top() + line_number as f32 * row_height - self.scroll.y;
        let bookmark_rect = egui::Rect::from_min_max(
            egui::pos2(content_left, top),
            egui::pos2(
                (content_left + BOOKMARK_GUTTER_WIDTH).min(content_rect.right()),
                top + row_height,
            ),
        );
        let bookmark_response = ui.interact(
            bookmark_rect,
            text_edit_id.with(("virtual_bookmark_gutter", line_number)),
            egui::Sense::click(),
        );
        if bookmarked || bookmark_response.hovered() || is_current_line {
            painter.text(
                bookmark_rect.center(),
                egui::Align2::CENTER_CENTER,
                if bookmarked { "★" } else { "☆" },
                bookmark_gutter_font_id(row_height),
                if bookmarked {
                    ui.visuals().selection.bg_fill
                } else {
                    inactive_color
                },
            );
        }
        if bookmark_response.clicked()
            && let Some(path) = self.path().map(Path::to_path_buf)
        {
            self.pending_action = Some(EditorAction::ToggleBookmark {
                path,
                line: line_number as u32,
            });
        }
        let line_number_galley = self.cached_virtual_line_number_galley(
            ui,
            line_number,
            line_count,
            is_current_line,
            (inactive_color, active_color),
            Some(&mut *frame_counters),
        );
        painter.galley(
            egui::pos2(
                content_left + gutter_width - 8.0 - line_number_galley.size().x,
                top,
            ),
            line_number_galley,
            if is_current_line {
                active_color
            } else {
                inactive_color
            },
        );

        let galley = self.cached_virtual_line_galley(
            ui,
            line_number,
            line_info,
            style,
            highlight_line_span_range,
            Some(frame_counters),
        );
        text_painter.galley(
            egui::pos2(text_left, top),
            galley,
            ui.visuals().text_color(),
        );
    }

    /// Tells the platform where the IME window goes, and that text under the pointer is editable.
    fn publish_virtual_ime_area(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        focused: bool,
        text_edit_id: egui::Id,
        cursor_rect: Option<egui::Rect>,
    ) {
        if focused
            && !self.read_only()
            && virtual_editor_can_handle_key(ui, text_edit_id)
            && let Some(cursor_rect) = cursor_rect
        {
            let to_global = ui
                .ctx()
                .layer_transform_to_global(ui.layer_id())
                .unwrap_or_default();
            ui.output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    rect: to_global * response.rect,
                    cursor_rect: to_global * cursor_rect,
                });
            });
        }
        if response.hovered() && !self.read_only() {
            ui.output_mut(|output| {
                output.mutable_text_under_cursor = true;
            });
        }
    }
}

/// Where the large-file editor's parts go this frame. Allocates its space in the `Ui`.
struct VirtualLayout {
    viewport_rect: egui::Rect,
    content_rect: egui::Rect,
    text_clip_rect: egui::Rect,
    gutter_width: f32,
    total_width: f32,
    total_height: f32,
    max_scroll_x: f32,
    max_scroll_y: f32,
    row_height: f32,
    char_width: f32,
}

impl VirtualLayout {
    fn allocate(ui: &mut egui::Ui, line_index: &LineIndex) -> Self {
        let line_count = line_index.line_count();
        let row_height = editor_scroll_row_height(ui);
        let gutter_width = line_number_gutter_width(ui, line_count);
        let editor_width = line_index.editor_width(ui, ui.available_width());
        let total_width = gutter_width + editor_width;
        let total_height = line_count.max(1) as f32 * row_height;
        let viewport_size = ui.available_size();
        let viewport_size = egui::vec2(viewport_size.x.max(1.0), viewport_size.y.max(1.0));
        let (viewport_rect, _) = ui.allocate_exact_size(viewport_size, egui::Sense::hover());
        let content_rect =
            virtual_scrollbar_layout(viewport_rect, total_width, total_height).content_rect;
        let text_clip_rect = egui::Rect::from_min_max(
            egui::pos2(content_rect.left() + gutter_width + 1.0, content_rect.top()),
            content_rect.right_bottom(),
        );
        Self {
            viewport_rect,
            content_rect,
            text_clip_rect,
            gutter_width,
            total_width,
            total_height,
            max_scroll_x: (total_width - content_rect.width()).max(0.0),
            max_scroll_y: (total_height - content_rect.height()).max(0.0),
            row_height,
            char_width: editor_char_width(ui),
        }
    }

    fn nav_metrics(&self) -> VirtualNavMetrics {
        VirtualNavMetrics {
            row_height: self.row_height,
            viewport_height: self.content_rect.height(),
            text_clip_width: self.text_clip_rect.width(),
            char_width: self.char_width,
            max_scroll_x: self.max_scroll_x,
            max_scroll_y: self.max_scroll_y,
        }
    }

    fn viewport(&self, scroll_x: f32, scroll_y: f32) -> VirtualViewport {
        VirtualViewport {
            content_rect: self.content_rect,
            text_clip_rect: self.text_clip_rect,
            text_left: self.content_rect.left() + self.gutter_width + 8.0 - scroll_x,
            row_height: self.row_height,
            char_width: self.char_width,
            scroll_x,
            scroll_y,
        }
    }
}

/// What the frame's input left for painting.
struct VirtualInteraction {
    response: egui::Response,
    viewport: VirtualViewport,
    current_line: Option<u32>,
    visible_range: std::ops::Range<usize>,
}

/// What `paint_virtual_body` draws from.
struct VirtualBodyFrame<'a> {
    layout: &'a VirtualLayout,
    viewport: &'a VirtualViewport,
    line_index: &'a LineIndex,
    visible_range: std::ops::Range<usize>,
    current_line: Option<u32>,
    tab_width: usize,
    bookmark_lines: &'a [u32],
    text_edit_id: egui::Id,
    line_style: VirtualLineStyle<'a>,
    highlight_line_span_ranges: &'a [std::ops::Range<usize>],
}

/// One frame's numbers for the performance report.
struct VirtualPerfSample {
    total_ms: f32,
    body_ms: f32,
    visible_rows: usize,
    total_rows: usize,
    counters: VirtualFrameCounters,
    text_len: usize,
    scroll_before_y: f32,
    search_matches: usize,
    syntax_spans: usize,
}

/// How to paint one line of the large-file editor.
struct VirtualLinePaint<'a> {
    painter: &'a egui::Painter,
    text_painter: &'a egui::Painter,
    viewport: &'a VirtualViewport,
    gutter_width: f32,
    line_count: usize,
    is_current_line: bool,
    bookmarked: bool,
    text_edit_id: egui::Id,
    style: &'a VirtualLineStyle<'a>,
    highlight_line_span_range: std::ops::Range<usize>,
}

/// Keys reach the editor only while it holds focus, and the lock keeps Tab and arrows from
/// moving focus away. A press elsewhere gives the focus up.
fn lock_virtual_editor_focus(
    ui: &egui::Ui,
    response: &egui::Response,
    viewport_rect: egui::Rect,
    text_edit_id: egui::Id,
) {
    if response.has_focus() {
        let pressed_outside = ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|position| !viewport_rect.contains(position))
        });
        if pressed_outside {
            response.surrender_focus();
        } else {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    text_edit_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                );
            });
        }
    }
}

pub(super) const VIRTUAL_LINE_LAYOUT_CACHE_LIMIT: usize = 4096;

pub(super) const VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES: u64 = 180;

pub(super) const VIRTUAL_WHEEL_LINE_SCROLL_SPEED: f32 = 40.0;

pub(super) const VIRTUAL_SCROLLBAR_BAR_WIDTH: f32 = 6.0;

pub(super) const VIRTUAL_SCROLLBAR_HIT_WIDTH: f32 = 12.0;

pub(super) struct CachedVirtualLineLayout {
    pub(super) line_start: usize,
    pub(super) line_content_end: usize,
    pub(super) galley: Arc<egui::Galley>,
    pub(super) last_used_frame: u64,
}

pub(super) struct VirtualLineLayoutCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) dark_mode: bool,
    pub(super) entries: HashMap<usize, CachedVirtualLineLayout>,
}

pub(super) struct CachedVirtualLineNumberLayout {
    pub(super) galley: Arc<egui::Galley>,
    pub(super) last_used_frame: u64,
}

pub(super) struct VirtualLineNumberLayoutCache {
    pub(super) line_count: usize,
    pub(super) dark_mode: bool,
    pub(super) inactive_color: Color32,
    pub(super) active_color: Color32,
    pub(super) entries: HashMap<(usize, bool), CachedVirtualLineNumberLayout>,
}

/// Removes from `smooth` the part already applied ahead of time (`lead`, per axis and only
/// in the same direction) and returns what is left to scroll.
pub(super) fn consume_wheel_lead(lead: &mut egui::Vec2, smooth: egui::Vec2) -> egui::Vec2 {
    let mut remaining = smooth;
    for axis in 0..2 {
        if smooth[axis] == 0.0 {
            continue;
        }
        if lead[axis].signum() != smooth[axis].signum() {
            lead[axis] = 0.0;
            continue;
        }
        let skipped = if lead[axis] > 0.0 {
            smooth[axis].min(lead[axis])
        } else {
            smooth[axis].max(lead[axis])
        };
        lead[axis] -= skipped;
        remaining[axis] -= skipped;
    }
    remaining
}

pub(super) fn immediate_wheel_scroll_delta(
    events: &[egui::Event],
    viewport_height: f32,
) -> Option<egui::Vec2> {
    let mut scroll_delta = egui::Vec2::ZERO;
    for event in events {
        let egui::Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        } = event
        else {
            continue;
        };
        if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) || modifiers.command {
            continue;
        }

        scroll_delta += match unit {
            egui::MouseWheelUnit::Point => *delta,
            egui::MouseWheelUnit::Line => VIRTUAL_WHEEL_LINE_SCROLL_SPEED * *delta,
            egui::MouseWheelUnit::Page => viewport_height * *delta,
        };
    }

    (scroll_delta != egui::Vec2::ZERO).then_some(scroll_delta)
}

pub(super) fn line_contains_bracket_pair(
    line: LineInfo,
    bracket_pair: Option<(usize, usize)>,
) -> bool {
    let Some((left, right)) = bracket_pair else {
        return false;
    };
    [left, right]
        .into_iter()
        .any(|index| (line.start..line.content_end).contains(&index))
}

pub(super) fn prune_virtual_line_layout_cache(cache: &mut VirtualLineLayoutCache, frame: u64) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(line_number, entry)| (*line_number, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (line_number, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&line_number);
    }
}

pub(super) fn prune_virtual_line_number_layout_cache(
    cache: &mut VirtualLineNumberLayoutCache,
    frame: u64,
) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(key, entry)| (*key, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (key, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_click_byte_column_treats_gutter_click_as_line_start() {
        assert_eq!(
            virtual_click_byte_column(None, "abcdef", 42.0, -120.0, 80.0, 10.0),
            0
        );
    }

    #[test]
    fn virtual_click_byte_column_uses_visible_text_offset() {
        assert_eq!(
            virtual_click_byte_column(None, "abcdef", 110.0, 80.0, 80.0, 10.0),
            3
        );
    }

    #[test]
    fn visual_column_for_byte_column_counts_characters_before_byte_column() {
        assert_eq!(visual_column_for_byte_column("abcdef", 3), 3);
        assert_eq!(visual_column_for_byte_column("aあb", 4), 2);
        assert_eq!(visual_column_for_byte_column("aあb", 2), 1);
    }

    #[test]
    fn virtual_scrollbar_layout_reserves_space_below_content() {
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 320.0));
        let layout = virtual_scrollbar_layout(viewport, 400.0, 800.0);
        let horizontal = layout.horizontal_rect.expect("horizontal scrollbar");
        let vertical = layout.vertical_rect.expect("vertical scrollbar");

        assert!(layout.content_rect.bottom() <= horizontal.top());
        assert!(layout.content_rect.right() <= vertical.left());
        assert_eq!(horizontal.bottom(), viewport.bottom());
        assert_eq!(vertical.right(), viewport.right());
    }

    #[test]
    fn virtual_pointer_to_text_position_uses_scroll_and_text_offset() {
        let buffer = EditorTextBuffer::new("zero\none two\nthree".to_owned());
        let line_index = buffer.line_index();
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 120.0));
        let text_clip = egui::Rect::from_min_max(egui::pos2(50.0, 20.0), egui::pos2(210.0, 120.0));

        assert_eq!(
            virtual_pointer_to_text_position(
                None,
                egui::pos2(80.0, 37.0),
                &line_index,
                &buffer,
                &VirtualViewport {
                    content_rect: viewport,
                    text_clip_rect: text_clip,
                    text_left: 50.0,
                    row_height: 17.0,
                    char_width: 10.0,
                    scroll_x: 0.0,
                    scroll_y: 17.0,
                }
            ),
            Some(Position::new(2, 3))
        );
    }

    #[test]
    fn virtual_indent_guide_lines_skip_blank_lines_and_track_active_line() {
        let buffer = EditorTextBuffer::new(
            "root\n    child\n        grandchild\n    \n\tchild\n".to_owned(),
        );
        let line_index = buffer.line_index();

        assert_eq!(
            virtual_indent_guide_lines(&line_index, &buffer, 0..5, 4, Some(2)),
            vec![
                VirtualIndentGuideLine {
                    line_number: 1,
                    guide_count: 1,
                    active: false,
                },
                VirtualIndentGuideLine {
                    line_number: 2,
                    guide_count: 2,
                    active: true,
                },
                VirtualIndentGuideLine {
                    line_number: 4,
                    guide_count: 1,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn immediate_wheel_scroll_delta_converts_line_and_page_units() {
        let events = vec![
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 2.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Page,
                delta: egui::vec2(1.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::default(),
            },
        ];

        assert_eq!(
            immediate_wheel_scroll_delta(&events, 300.0),
            Some(egui::vec2(300.0, 80.0))
        );
    }

    #[test]
    fn immediate_wheel_scroll_delta_ignores_zoom_wheel_events() {
        let events = vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers {
                command: true,
                ..Default::default()
            },
        }];

        assert_eq!(immediate_wheel_scroll_delta(&events, 300.0), None);
    }

    #[test]
    fn consume_wheel_lead_skips_only_what_was_applied_ahead() {
        let mut lead = egui::vec2(0.0, -60.0);
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, -40.0)),
            egui::Vec2::ZERO
        );
        assert_eq!(lead, egui::vec2(0.0, -20.0));
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::Vec2::ZERO),
            egui::Vec2::ZERO
        );
        assert_eq!(lead, egui::vec2(0.0, -20.0), "an idle frame keeps the lead");
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, -30.0)),
            egui::vec2(0.0, -10.0)
        );
        assert_eq!(lead, egui::Vec2::ZERO);

        // Scrolling the other way drops the lead.
        let mut lead = egui::vec2(0.0, -60.0);
        assert_eq!(
            consume_wheel_lead(&mut lead, egui::vec2(0.0, 10.0)),
            egui::vec2(0.0, 10.0)
        );
        assert_eq!(lead, egui::Vec2::ZERO);
    }
}
