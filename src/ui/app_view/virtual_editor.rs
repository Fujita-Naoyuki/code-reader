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
        let line_count = line_index.line_count();
        let row_height = editor_scroll_row_height(ui);
        let gutter_width = line_number_gutter_width(ui, line_count);
        let editor_width = line_index.editor_width(ui, ui.available_width());
        let total_width = gutter_width + editor_width;
        let total_height = line_count.max(1) as f32 * row_height;
        let mut current_line = self.cursor_position.map(|position| position.line);
        let bracket_pair = self.cached_bracket_pair(source_len, source_hash);
        let highlight_spans =
            self.cached_syntax_highlight_spans(ui, language_registry, source_len, source_hash);
        let highlight_line_span_ranges =
            self.cached_syntax_highlight_line_span_ranges(source_len, source_hash);

        let viewport_size = ui.available_size();
        let viewport_size = egui::vec2(viewport_size.x.max(1.0), viewport_size.y.max(1.0));
        let (viewport_rect, _) = ui.allocate_exact_size(viewport_size, egui::Sense::hover());
        // The viewport is the focusable widget behind `text_edit_id`.
        let response = ui.interact(viewport_rect, text_edit_id, egui::Sense::click_and_drag());
        lock_virtual_editor_focus(ui, &response, viewport_rect, text_edit_id);
        let scrollbar_layout = virtual_scrollbar_layout(viewport_rect, total_width, total_height);
        let content_rect = scrollbar_layout.content_rect;
        let max_scroll_y = (total_height - content_rect.height()).max(0.0);
        let max_scroll_x = (total_width - content_rect.width()).max(0.0);
        self.scroll.y = self.scroll.y.clamp(0.0, max_scroll_y);
        self.scroll.x = self.scroll.x.clamp(0.0, max_scroll_x);

        self.apply_virtual_wheel(ui, &response, content_rect, max_scroll_x, max_scroll_y);

        let visible_start = (self.scroll.y / row_height).floor().max(0.0) as usize;
        let visible_end = (((self.scroll.y + content_rect.height()) / row_height).ceil() as usize
            + 1)
        .min(line_count);
        let visible_range = visible_start.min(line_count)..visible_end;
        let visible_rows = visible_range.end.saturating_sub(visible_range.start);
        let painter = ui.painter_at(viewport_rect);
        let char_width = editor_char_width(ui);
        let content_left = content_rect.left();
        let text_clip_left = content_left + gutter_width + 1.0;
        let text_clip_rect = egui::Rect::from_min_max(
            egui::pos2(text_clip_left, content_rect.top()),
            content_rect.right_bottom(),
        );
        let text_painter = ui.painter_at(text_clip_rect);
        let text_left = content_left + gutter_width + 8.0 - self.scroll.x;
        let viewport = VirtualViewport {
            content_rect,
            text_clip_rect,
            text_left,
            row_height,
            char_width,
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
        };

        if self.handle_virtual_navigation_input(
            ui,
            focused,
            text_edit_id,
            line_index.as_ref(),
            &VirtualNavMetrics {
                row_height,
                viewport_height: content_rect.height(),
                text_clip_width: text_clip_rect.width(),
                char_width,
                max_scroll_x,
                max_scroll_y,
            },
        ) {
            current_line = self.cursor_position.map(|position| position.line);
            response.request_focus();
            ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            ui.ctx().request_repaint();
        }

        // Navigation may have scrolled to keep the cursor visible.
        let viewport = VirtualViewport {
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
            text_left: content_left + gutter_width + 8.0 - self.scroll.x,
            ..viewport
        };
        if let Some(line) = self.handle_virtual_pointer(
            ui,
            &response,
            (text_edit_id, focused),
            line_index.as_ref(),
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

        let body_start = Instant::now();
        let mut frame_counters = VirtualFrameCounters::default();
        if let Some(current_line) = current_line
            && visible_range.contains(&(current_line as usize))
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
                line_index.as_ref(),
                &self.text,
                visible_range.clone(),
                tab_width,
                current_line,
                &viewport,
            );
        }

        paint_virtual_selection(
            ui,
            line_index.as_ref(),
            &self.text,
            self.virtual_editor
                .selection
                .and_then(|selection| selection.normalized()),
            visible_range.clone(),
            &viewport,
        );

        let line_style = VirtualLineStyle {
            source_len,
            source_hash,
            highlight_spans: highlight_spans.as_slice(),
            highlights: LineHighlights {
                bracket_pair,
                search_matches: source_search_matches.as_slice(),
                active_search_match: active_source_search_match,
            },
        };
        for line_number in visible_range {
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
                    viewport: &viewport,
                    gutter_width,
                    line_count,
                    is_current_line: current_line == Some(line_number as u32),
                    bookmarked: bookmark_lines.contains(&(line_number as u32)),
                    text_edit_id,
                    style: &line_style,
                    highlight_line_span_range: highlight_line_span_ranges
                        .get(line_number)
                        .cloned()
                        .unwrap_or(0..0),
                },
                &mut frame_counters,
            );
        }

        let cursor_rect = virtual_cursor_rect(
            Some(ui),
            line_index.as_ref(),
            &self.text,
            self.cursor_position,
            visible_start..visible_end,
            &viewport,
        );
        self.publish_virtual_ime_area(ui, &response, focused, text_edit_id, cursor_rect);

        paint_virtual_ime_preedit(
            ui,
            self.virtual_editor.ime_preedit.as_deref(),
            cursor_rect,
            text_clip_rect,
            content_rect,
            row_height,
        );
        paint_virtual_cursor(
            ui,
            line_index.as_ref(),
            &self.text,
            self.cursor_position,
            visible_start..visible_end,
            &viewport,
        );

        painter.line_segment(
            [
                egui::pos2(content_left + gutter_width, content_rect.top()),
                egui::pos2(content_left + gutter_width, content_rect.bottom()),
            ],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

        handle_and_paint_virtual_scrollbars(
            ui,
            viewport_rect,
            total_width,
            total_height,
            &mut self.scroll.x,
            &mut self.scroll.y,
        );
        let body_ms = elapsed_ms(body_start);
        self.record_virtual_perf(VirtualPerfSample {
            total_ms: elapsed_ms(total_start),
            body_ms,
            visible_rows,
            total_rows: line_count,
            counters: frame_counters,
            text_len: source_len,
            scroll_before_y,
            search_matches: source_search_matches.len(),
            syntax_spans: highlight_spans.len(),
        });
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
        let VirtualViewport {
            content_rect,
            text_clip_rect,
            text_left,
            row_height,
            char_width,
            ..
        } = *viewport;
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
            visible_range.clone(),
        );
        let click_event_index = click_input
            .event
            .map(|event| self.debug.virtual_click.push_event(event));
        if let Some(pointer_pos) = click_input.position {
            let line = ((pointer_pos.y - content_rect.top() + self.scroll.y) / row_height)
                .floor()
                .max(0.0) as usize;
            let line = line.min(line_count.saturating_sub(1));
            if let Some(line_info) = line_index.line(line) {
                let line_text = self.text.line_text_or_empty(line_info);
                let character = virtual_click_byte_column(
                    Some(ui),
                    line_text,
                    pointer_pos.x,
                    text_left,
                    text_clip_rect.left(),
                    char_width,
                );
                let position = Position::new(line as u32, character);
                if click_input.double_clicked {
                    if let Some(selection) =
                        virtual_word_selection_at_position(line_text, line as u32, character)
                    {
                        self.cursor_position = Some(selection.head);
                        moved_to_line = Some(selection.head.line);
                        self.virtual_editor.selection = Some(selection);
                    } else {
                        self.cursor_position = Some(position);
                        moved_to_line = Some(position.line);
                        self.virtual_editor.selection = None;
                    }
                } else {
                    self.cursor_position = Some(position);
                    moved_to_line = Some(position.line);
                    self.virtual_editor.selection = None;
                }
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
                        text_clip_left: text_clip_rect.left(),
                        x_offset: virtual_click_x_offset(
                            pointer_pos.x,
                            text_left,
                            text_clip_rect.left(),
                        ),
                    });
                }
                response.request_focus();
                ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                ui.ctx().request_repaint();
            }
        }
        moved_to_line
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

pub(super) struct VirtualEditorClickInput {
    pub(super) position: Option<egui::Pos2>,
    pub(super) double_clicked: bool,
    pub(super) event: Option<VirtualClickEventDebug>,
}

pub(super) fn virtual_editor_drag_selection(
    ui: &egui::Ui,
    response: &egui::Response,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    viewport: &VirtualViewport,
) -> Option<(Position, Position)> {
    let viewport_rect = viewport.content_rect;

    if !(response.drag_started_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Primary))
    {
        return None;
    }

    let pointer_pos = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|input| input.pointer.interact_pos()))?;
    let press_origin = ui
        .input(|input| input.pointer.press_origin())
        .unwrap_or(pointer_pos);
    // `viewport_rect` is the text area without the scrollbars, so presses on a scrollbar
    // never start a selection.
    if !viewport_rect.contains(press_origin) {
        return None;
    }

    let anchor =
        virtual_pointer_to_text_position(Some(ui), press_origin, line_index, text, viewport)?;
    let head = virtual_pointer_to_text_position(Some(ui), pointer_pos, line_index, text, viewport)?;
    Some((anchor, head))
}

pub(super) fn visual_column_for_position(
    ui: Option<&egui::Ui>,
    text: &EditorTextBuffer,
    line_index: &LineIndex,
    position: Position,
    fallback_char_width: f32,
) -> f32 {
    let position = clamp_virtual_position(line_index, position);
    let Some(line) = line_index.line(position.line as usize) else {
        return 0.0;
    };
    visual_x_for_byte_column(
        ui,
        text.line_text_or_empty(line),
        position.character as usize,
        fallback_char_width,
    )
}

pub(super) fn virtual_editor_click_input(
    ui: &egui::Ui,
    response: &egui::Response,
    text_edit_id: egui::Id,
    pane_focused: bool,
    viewport: &VirtualViewport,
    line_count: usize,
    visible_range: std::ops::Range<usize>,
) -> VirtualEditorClickInput {
    let VirtualViewport {
        content_rect: viewport_rect,
        text_clip_rect,
        scroll_x,
        scroll_y,
        row_height,
        ..
    } = *viewport;

    let response_pointer_pos = response.interact_pointer_pos();
    let response_clicked = response.clicked();
    let response_primary_clicked = response.clicked_by(egui::PointerButton::Primary);
    let response_double_clicked = response.double_clicked_by(egui::PointerButton::Primary);
    let response_hovered = response.hovered();
    let response_contains_pointer = response.contains_pointer();
    let text_edit_focused = ui.memory(|memory| memory.has_focus(text_edit_id));

    let (
        input_pointer_pos,
        primary_pressed,
        primary_released,
        primary_clicked,
        any_pressed,
        any_released,
        any_click,
        dragging,
    ) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.primary_pressed(),
            input.pointer.primary_released(),
            input.pointer.primary_clicked(),
            input.pointer.any_pressed(),
            input.pointer.any_released(),
            input.pointer.any_click(),
            input.pointer.is_decidedly_dragging(),
        )
    });

    let input_activity = response_clicked
        || response_primary_clicked
        || response_double_clicked
        || primary_pressed
        || primary_released
        || primary_clicked
        || any_pressed
        || any_released
        || any_click;

    let (source, candidate_pos) = if response_double_clicked {
        ("response_double_clicked", response_pointer_pos)
    } else if response_primary_clicked {
        ("response_primary_clicked", response_pointer_pos)
    } else if primary_released {
        ("primary_released", input_pointer_pos)
    } else if primary_pressed {
        ("primary_pressed", input_pointer_pos)
    } else {
        ("none", response_pointer_pos.or(input_pointer_pos))
    };

    let pointer_pos = candidate_pos.or(response_pointer_pos).or(input_pointer_pos);
    let response_hit = pointer_pos.is_some_and(|position| response.rect.contains(position));
    let viewport_hit = pointer_pos.is_some_and(|position| viewport_rect.contains(position));
    let text_hit = pointer_pos.is_some_and(|position| text_clip_rect.contains(position));
    // The response covers the whole editor; `viewport_rect` leaves out the scrollbars.
    let scrollbar_hit = pointer_pos.is_some_and(|position| {
        response.rect.contains(position) && !viewport_rect.contains(position)
    });

    let reason = if source == "none" {
        "no_primary_press_or_release"
    } else if candidate_pos.is_none() {
        "no_pointer_position"
    } else if source != "response_primary_clicked" && dragging {
        "dragging"
    } else if scrollbar_hit {
        "scrollbar"
    } else if !viewport_hit {
        "outside_viewport"
    } else {
        "accepted"
    };
    let accepted = reason == "accepted";

    let event = input_activity.then_some(VirtualClickEventDebug {
        index: 0,
        source,
        accepted,
        reason,
        pointer_pos,
        response_rect: response.rect,
        viewport_rect,
        text_clip_rect,
        response_hovered,
        response_contains_pointer,
        response_clicked,
        response_primary_clicked,
        primary_pressed,
        primary_released,
        primary_clicked,
        any_pressed,
        any_released,
        any_click,
        dragging,
        response_hit,
        viewport_hit,
        text_hit,
        scrollbar_hit,
        pane_focused,
        text_edit_focused,
        scroll_x,
        scroll_y,
        row_height,
        line_count,
        visible_range,
    });

    VirtualEditorClickInput {
        position: accepted.then_some(candidate_pos).flatten(),
        double_clicked: accepted && response_double_clicked,
        event,
    }
}

pub(super) fn virtual_pointer_to_text_position(
    ui: Option<&egui::Ui>,
    pointer_pos: egui::Pos2,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    viewport: &VirtualViewport,
) -> Option<Position> {
    let VirtualViewport {
        content_rect: viewport_rect,
        text_clip_rect,
        text_left,
        row_height,
        scroll_y,
        char_width,
        ..
    } = *viewport;

    let line_count = line_index.line_count();
    if line_count == 0 {
        return None;
    }

    let clamped_y = pointer_pos.y.clamp(
        viewport_rect.top(),
        (viewport_rect.bottom() - 1.0).max(viewport_rect.top()),
    );
    let line = ((clamped_y - viewport_rect.top() + scroll_y) / row_height)
        .floor()
        .max(0.0) as usize;
    let line = line.min(line_count.saturating_sub(1));
    let line_info = line_index.line(line)?;
    let line_text = text.line_text_or_empty(line_info);
    let character = virtual_click_byte_column(
        ui,
        line_text,
        pointer_pos.x,
        text_left,
        text_clip_rect.left(),
        char_width,
    );
    Some(Position::new(line as u32, character))
}

pub(super) fn byte_column_for_visual_x(
    ui: Option<&egui::Ui>,
    line: &str,
    x_offset: f32,
    fallback_char_width: f32,
) -> u32 {
    let target_x = x_offset.max(0.0);
    let mut current_x = 0.0;
    for (byte_index, character) in line.char_indices() {
        let character_width = visual_width_for_char(ui, character, fallback_char_width);
        if target_x < current_x + character_width * 0.5 {
            return byte_index as u32;
        }
        current_x += character_width;
    }
    line.len() as u32
}

pub(super) fn visual_x_for_byte_column(
    ui: Option<&egui::Ui>,
    line: &str,
    byte_column: usize,
    fallback_char_width: f32,
) -> f32 {
    let byte_column = clamp_to_char_boundary(line, byte_column.min(line.len()));
    line[..byte_column]
        .chars()
        .map(|character| visual_width_for_char(ui, character, fallback_char_width))
        .sum()
}

pub(super) fn visual_column_for_byte_column(line: &str, byte_column: usize) -> usize {
    let byte_column = clamp_to_char_boundary(line, byte_column.min(line.len()));
    line[..byte_column].chars().count()
}

pub(super) fn visual_width_for_char(
    ui: Option<&egui::Ui>,
    character: char,
    fallback_char_width: f32,
) -> f32 {
    if character == '\t' {
        return fallback_char_width * DEFAULT_TAB_WIDTH as f32;
    }
    ui.map(|ui| ui.fonts_mut(|fonts| fonts.glyph_width(&editor_font_id(), character)))
        .filter(|width| *width > 0.0)
        .unwrap_or(fallback_char_width)
}

pub(super) fn virtual_click_x_offset(pointer_x: f32, text_left: f32, text_clip_left: f32) -> f32 {
    if pointer_x < text_clip_left {
        0.0
    } else {
        pointer_x - text_left
    }
}

pub(super) fn virtual_click_byte_column(
    ui: Option<&egui::Ui>,
    line: &str,
    pointer_x: f32,
    text_left: f32,
    text_clip_left: f32,
    char_width: f32,
) -> u32 {
    let x_offset = virtual_click_x_offset(pointer_x, text_left, text_clip_left);
    byte_column_for_visual_x(ui, line, x_offset, char_width)
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
