//! Markdown source mode: the colored text editor and the link between cursor and page.

use super::*;

impl EditorPane {
    pub(super) fn markdown_source_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        tab_width: usize,
        active_page_range: Option<(u32, u32)>,
        document: Option<&MarkdownDocument>,
        bookmark_lines: &[u32],
    ) {
        let text_edit_id = ui.id().with(format!("markdown_source_{:?}", self.pane));
        let (source_len, source_hash) = self.text_fingerprint();
        let source_search_matches = if self.source_search.open {
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
            matches
        } else {
            self.source_search.match_count = 0;
            Rc::new(Vec::new())
        };
        if self.source_search.open {
            ui.separator();
        }

        let pending_navigation = self.pending_navigation.take();
        let pending_position = pending_navigation.map(|navigation| navigation.position);
        let pending_focus_editor =
            pending_navigation.is_some_and(|navigation| navigation.focus_editor);
        let pending_scroll_y = pending_position.map(|position| {
            let row_height = editor_scroll_row_height(ui);
            (position.line as f32 * row_height - ui.available_height() * 0.35).max(0.0)
        });
        let viewport_height = ui.available_height();
        if let Some(offset) = pending_scroll_y {
            self.scroll.y = offset;
        }
        let metrics = self.cached_editor_metrics(source_len, source_hash);
        let editor_width = metrics.editor_width(ui, ui.available_width());
        let line_count = metrics.line_count;
        let estimated_max_scroll_y =
            estimated_textedit_source_max_scroll_y(ui, line_count, viewport_height);
        let max_scroll_y = if self.scroll.max_y > 0.0 {
            self.scroll.max_y
        } else {
            estimated_max_scroll_y
        };
        self.scroll.y = self.scroll.y.clamp(0.0, max_scroll_y);
        if let Some(delta) = editor_page_scroll_delta(ui, focused, text_edit_id, viewport_height) {
            self.scroll.y = (self.scroll.y + delta).clamp(0.0, max_scroll_y);
        }
        self.apply_immediate_wheel_scroll(ui, max_scroll_y);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("markdown_source_scroll_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll.y);

        let scroll_output = scroll_area.show(ui, |ui| {
            if let Some(target_position) = pending_position
                && let Some(char_index) = self.text.position_to_char_index(target_position)
            {
                let cursor = CCursor::new(char_index);
                let mut state = TextEditState::load(ui.ctx(), text_edit_id).unwrap_or_default();
                state.cursor.set_char_range(Some(CCursorRange::one(cursor)));
                state.store(ui.ctx(), text_edit_id);
                if pending_focus_editor {
                    ui.memory_mut(|memory| memory.request_focus(text_edit_id));
                }
                self.cursor_position = Some(target_position);
            }

            if focused && !self.read_only() {
                self.handle_input_assistance(ui, text_edit_id, tab_width);
            }
            if focused
                && !self.read_only()
                && !self.markdown.search.open
                && !self.source_search.open
                && !ui.memory(|memory| memory.has_focus(text_edit_id))
            {
                ui.memory_mut(|memory| memory.request_focus(text_edit_id));
            }

            ui.horizontal_top(|ui| {
                let gutter_rect = allocate_line_number_gutter(ui, line_count);

                let mut layouter = |ui: &egui::Ui, text: &dyn TextBuffer, _wrap_width: f32| {
                    let mut job = markdown_source_layout_job(ui, text.as_str());
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };

                let interactive = !self.read_only();
                let output = TextEdit::multiline(&mut self.text)
                    .id(text_edit_id)
                    .font(TextStyle::Monospace)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(editor_width)
                    .desired_rows(TEXTEDIT_SOURCE_MIN_ROWS)
                    .layouter(&mut layouter)
                    .interactive(interactive)
                    .show(ui);

                if let Some(cursor_range) = output.cursor_range {
                    let next_position =
                        self.text.char_index_to_position(cursor_range.primary.index);
                    if self.cursor_position != next_position {
                        self.cursor_position = next_position;
                        if let Some(document) = document {
                            self.update_markdown_active_page_from_cursor(document);
                        }
                        ui.ctx().request_repaint();
                    }
                }

                let current_line = self.cursor_position.map(|position| position.line);
                let row_height = editor_row_height(ui);
                let viewport_rect = ui.clip_rect();
                self.debug.render = Some(capture_editor_render_debug(
                    &output,
                    line_count,
                    line_count,
                    row_height,
                    self.scroll.y,
                    viewport_rect,
                    current_line,
                ));
                paint_markdown_page_range_background(ui, &output, viewport_rect, active_page_range);
                paint_current_line_background(ui, &output, viewport_rect, current_line);
                if let Some(line) = paint_line_number_gutter(
                    ui,
                    gutter_rect,
                    line_count,
                    current_line,
                    bookmark_lines,
                    &output,
                    viewport_rect,
                ) && let Some(path) = self.path().map(Path::to_path_buf)
                {
                    self.pending_action = Some(EditorAction::ToggleBookmark { path, line });
                }

                if output.response.changed() {
                    self.mark_text_changed();
                    if self.source_search.open {
                        self.source_search.scrolled_match = None;
                    }
                    self.target_position = None;
                    self.markdown.active_heading_id = None;
                    if let Some(document) = document {
                        self.update_markdown_active_page_from_cursor(document);
                    }
                }
            });
        });
        self.scroll.max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll.y = scroll_output.state.offset.y.clamp(0.0, self.scroll.max_y);
        drop(source_search_matches);
    }

    pub(super) fn prepare_markdown_source_entry(&mut self, document: &MarkdownDocument) {
        let Some(page) = document.active_or_first_page(self.markdown.active_page_id.as_deref())
        else {
            return;
        };
        let position = Position::new(
            page.source_range.start.line as u32,
            page.source_range.start.column as u32,
        );
        self.pending_navigation = Some(PendingEditorNavigation {
            position,
            focus_editor: true,
        });
        self.cursor_position = Some(position);
    }

    pub(super) fn markdown_active_source_line_range(
        &self,
        document: &MarkdownDocument,
    ) -> Option<(u32, u32)> {
        let page = document.active_or_first_page(self.markdown.active_page_id.as_deref())?;
        let start = page.source_range.start.line as u32;
        let mut end = page.source_range.end.line as u32;

        if page.source_range.end.column == 0 && end > start {
            end -= 1;
        }

        Some((start, end.max(start)))
    }

    pub(super) fn update_markdown_active_page_from_cursor(&mut self, document: &MarkdownDocument) {
        let Some(position) = self.cursor_position else {
            return;
        };
        let cursor_line = position.line as usize;
        let Some(page) = document.pages.iter().find(|page| {
            let start = page.source_range.start.line;
            let mut end = page.source_range.end.line;
            if page.source_range.end.column == 0 && end > start {
                end -= 1;
            }
            (start..=end.max(start)).contains(&cursor_line)
        }) else {
            return;
        };

        if self.markdown.active_page_id.as_deref() != Some(page.id.as_str()) {
            self.markdown.active_heading_id = None;
            self.markdown.active_page_id = Some(page.id.clone());
        }
    }
}

pub(super) fn markdown_source_layout_job(ui: &egui::Ui, source: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = 0;
    let mut spans = markdown_source_spans(source);
    let default_format = markdown_source_text_format(ui, None);

    spans.sort_by_key(|span| (span.range.start, span.range.end));

    for span in spans {
        let start = clamp_to_char_boundary(source, span.range.start);
        let end = clamp_to_char_boundary(source, span.range.end);

        if start < cursor || start >= end || end > source.len() {
            continue;
        }

        if cursor < start {
            job.append(&source[cursor..start], 0.0, default_format.clone());
        }
        job.append(
            &source[start..end],
            0.0,
            markdown_source_text_format(ui, Some(span.kind)),
        );
        cursor = end;
    }

    if cursor < source.len() {
        job.append(&source[cursor..], 0.0, default_format);
    }

    job
}

pub(super) fn markdown_source_text_format(
    ui: &egui::Ui,
    kind: Option<MarkdownSourceSpanKind>,
) -> TextFormat {
    let mut format = TextFormat {
        font_id: editor_font_id(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };

    match kind {
        Some(MarkdownSourceSpanKind::Heading) => {
            format.color = Color32::from_rgb(128, 0, 0);
        }
        Some(MarkdownSourceSpanKind::Strong) => {
            format.color = ui.visuals().strong_text_color();
        }
        Some(MarkdownSourceSpanKind::Emphasis) => {
            format.italics = true;
        }
        Some(MarkdownSourceSpanKind::InlineCode) => {
            format.color = Color32::from_rgb(163, 21, 21);
            format.background = markdown_code_background(ui);
            format.expand_bg = 2.0;
        }
        Some(MarkdownSourceSpanKind::CodeBlock) => {
            format.color = Color32::from_rgb(163, 21, 21);
        }
        Some(MarkdownSourceSpanKind::Link) => {
            format.color = Color32::from_rgb(4, 81, 165);
            format.underline = egui::Stroke::new(1.0, format.color);
        }
        Some(MarkdownSourceSpanKind::Blockquote) => {
            format.color = Color32::from_rgb(0, 128, 0);
        }
        Some(MarkdownSourceSpanKind::ListMarker) => {
            format.color = Color32::from_rgb(4, 81, 165);
        }
        None => {}
    }

    format
}

pub(super) fn paint_markdown_page_range_background(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
    active_page_range: Option<(u32, u32)>,
) {
    let Some((start_line, end_line)) = active_page_range else {
        return;
    };

    let visible_lines = visible_galley_row_range(output, end_line as usize + 1, viewport_rect);
    let start = (start_line as usize).max(visible_lines.start);
    let end = (end_line as usize + 1).min(visible_lines.end);

    for line in start..end {
        let row_rect = galley_line_rect(output, line);
        let line_rect = egui::Rect::from_min_max(
            egui::pos2(output.text_clip_rect.left(), row_rect.top()),
            egui::pos2(output.text_clip_rect.right(), row_rect.bottom()),
        )
        .intersect(viewport_rect);

        if line_rect.is_positive() {
            ui.painter()
                .rect_filled(line_rect, 0.0, markdown_page_range_overlay(ui));
        }
    }
}

pub(super) fn markdown_page_range_overlay(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(120, 150, 90, 24)
    } else {
        Color32::from_rgba_unmultiplied(180, 210, 120, 28)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_active_source_line_range_uses_active_page_boundaries() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nText\n\n## Second\n\nMore\n\n## Third\n\nEnd\n".to_owned(),
        )
        .unwrap();
        let mut editor = EditorPane::empty(Pane::Left);
        editor.markdown.active_page_id = Some("second".to_owned());

        assert_eq!(
            editor.markdown_active_source_line_range(&document),
            Some((4, 7))
        );

        editor.prepare_markdown_source_entry(&document);
        assert_eq!(
            editor.pending_navigation,
            Some(PendingEditorNavigation {
                position: Position::new(4, 0),
                focus_editor: true,
            })
        );
    }
}
