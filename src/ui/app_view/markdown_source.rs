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
        self.source_search_frame(ui, focused, source_len, source_hash);

        let pending = self.take_pending_source_navigation(ui);
        let viewport_height = ui.available_height();
        let metrics = self.cached_editor_metrics(source_len, source_hash);
        let editor_width = metrics.editor_width(ui, ui.available_width());
        let line_count = metrics.line_count;
        self.prepare_source_scroll(ui, focused, text_edit_id, line_count, viewport_height);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("markdown_source_scroll_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll.y);

        let scroll_output = scroll_area.show(ui, |ui| {
            self.apply_pending_source_cursor(ui, text_edit_id, pending);

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
                let output = self.source_text_edit(ui, text_edit_id, editor_width, &mut layouter);

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

                let current_line = self.record_source_render_debug(ui, &output, line_count);
                let viewport_rect = ui.clip_rect();
                paint_markdown_page_range_background(ui, &output, viewport_rect, active_page_range);
                paint_current_line_background(ui, &output, viewport_rect, current_line);
                self.source_gutter_ui(ui, gutter_rect, line_count, bookmark_lines, &output);

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
        self.finish_source_scroll(&scroll_output);
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
