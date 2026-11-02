//! The Markdown pane: Preview / Markdown / Both modes, Pages and Outline, the rendered
//! search box and keyboard focus between the panes.

use super::*;

impl EditorPane {
    pub(super) fn is_markdown(&self) -> bool {
        self.path.as_deref().is_some_and(is_markdown_path)
    }

    pub(super) fn markdown_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        bookmark_lines: &[u32],
    ) {
        let Some(path) = self.path.clone() else {
            ui.label(RichText::new("No Markdown file open.").weak());
            return;
        };

        let document = match self.cached_markdown_document(&path) {
            Ok(document) => document,
            Err(error) => {
                ui.label(
                    RichText::new(format!("Markdown parse error: {error}"))
                        .small()
                        .color(ui.visuals().error_fg_color),
                );
                ui.separator();
                self.markdown_source_ui(ui, focused, tab_width, None, None, bookmark_lines);
                return;
            }
        };

        if self
            .markdown_active_page_id
            .as_deref()
            .and_then(|page_id| document.page_by_id(page_id))
            .is_none()
        {
            self.markdown_active_page_id = document.pages.first().map(|page| page.id.clone());
        }
        if matches!(
            self.markdown_mode,
            MarkdownMode::Markdown | MarkdownMode::Both
        ) {
            self.update_markdown_active_page_from_cursor(&document);
        }

        if focused
            && matches!(
                self.markdown_mode,
                MarkdownMode::Preview | MarkdownMode::Both
            )
        {
            self.handle_markdown_preview_navigation(ui, &document);
        }
        if focused {
            self.handle_markdown_mode_shortcuts(ui, &document);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("MARKDOWN").small().strong());
            ui.separator();
            for mode in MarkdownMode::all() {
                if ui
                    .selectable_label(self.markdown_mode == mode, mode.label())
                    .on_hover_text(format!("Switch to {} mode", mode.label()))
                    .clicked()
                {
                    self.set_markdown_mode(mode, &document);
                }
            }
            if self.markdown_mode != MarkdownMode::Markdown {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.markdown_show_outline, "Outline")
                        .on_hover_text("Show or hide the outline pane")
                        .clicked()
                    {
                        self.markdown_show_outline = !self.markdown_show_outline;
                        self.normalize_markdown_focus_target();
                    }
                    if ui
                        .selectable_label(self.markdown_show_pages, "Pages")
                        .on_hover_text("Show or hide the pages pane")
                        .clicked()
                    {
                        self.markdown_show_pages = !self.markdown_show_pages;
                        self.normalize_markdown_focus_target();
                    }
                });
            }
        });

        let matches = if self.markdown_search.open {
            let matches = markdown_search_matches(&document, &self.markdown_search.query);
            self.markdown_search.match_count = matches.len();
            if self.markdown_search.current_match >= matches.len() {
                self.markdown_search.current_match = matches.len().saturating_sub(1);
            }
            if focused {
                self.handle_markdown_search_global_shortcuts(ui, &document, &matches);
            }
            self.markdown_search_ui(ui, &document, &matches);
            matches
        } else {
            self.markdown_search.match_count = 0;
            Vec::new()
        };

        ui.separator();

        let active_page_range = self.markdown_active_source_line_range(&document);
        let active_search_match = self.markdown_active_search_match(&matches);
        match self.markdown_mode {
            MarkdownMode::Preview => {
                self.markdown_preview_ui(ui, &document, language_registry, active_search_match)
            }
            MarkdownMode::Markdown => self.markdown_source_ui(
                ui,
                focused,
                tab_width,
                active_page_range,
                Some(&document),
                bookmark_lines,
            ),
            MarkdownMode::Both => self.markdown_both_ui(
                ui,
                focused,
                &document,
                language_registry,
                tab_width,
                active_page_range,
                active_search_match,
                bookmark_lines,
            ),
        }
    }

    pub(super) fn cached_markdown_document(
        &mut self,
        path: &Path,
    ) -> Result<Rc<MarkdownDocument>, String> {
        let (source_len, source_hash) = self.text_fingerprint();

        if let Some(cache) = &self.markdown_document_cache
            && cache.path == path
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.document.clone();
        }

        let document =
            MarkdownDocument::parse(path.to_path_buf(), self.text.as_str().to_owned()).map(Rc::new);
        self.markdown_document_cache = Some(MarkdownDocumentCache {
            path: path.to_path_buf(),
            source_len,
            source_hash,
            document: document.clone(),
        });
        document
    }

    pub(super) fn set_markdown_mode(&mut self, mode: MarkdownMode, document: &MarkdownDocument) {
        if self.markdown_mode == mode {
            return;
        }

        if matches!(mode, MarkdownMode::Markdown | MarkdownMode::Both) {
            self.prepare_markdown_source_entry(document);
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
        if mode == MarkdownMode::Both && self.markdown_focus_target == MarkdownFocusTarget::Outline
        {
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
        self.markdown_mode = mode;
    }

    pub(super) fn handle_markdown_mode_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
    ) {
        let mut next_mode = None;
        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::ALT, egui::Key::Num1) {
                next_mode = Some(MarkdownMode::Preview);
            } else if input.consume_key(egui::Modifiers::ALT, egui::Key::Num2) {
                next_mode = Some(MarkdownMode::Markdown);
            } else if input.consume_key(egui::Modifiers::ALT, egui::Key::Num3) {
                next_mode = Some(MarkdownMode::Both);
            }
        });

        if let Some(mode) = next_mode {
            self.set_markdown_mode(mode, document);
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn markdown_search_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
    ) {
        let search_id = ui.id().with(format!("markdown_search_{:?}", self.pane));
        let mut close = false;
        let mut go_next = false;
        let mut go_previous = false;
        let mut keep_focus = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Find").small().strong());
            let input_had_focus = ui.memory(|memory| memory.has_focus(search_id));
            if input_had_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
            }
            // egui drops focus at the start of the frame that carries a plain Escape, so check
            // who had focus in the previous frame.
            if ui.memory(|memory| memory.had_focus_last_frame(search_id))
                && ui.input(raw_escape_pressed)
            {
                close = true;
            }

            let response = ui.add(
                TextEdit::singleline(&mut self.markdown_search.query)
                    .id(search_id)
                    .hint_text("Search rendered markdown")
                    .desired_width(220.0),
            );
            if response.changed() {
                self.markdown_search.current_match = 0;
                self.markdown_search.scrolled_match = None;
            }
            if self.markdown_search.focus_requested {
                response.request_focus();
                self.markdown_search.focus_requested = false;
            }
            let input_has_focus =
                response.has_focus() || ui.memory(|memory| memory.has_focus(search_id));
            if !input_had_focus && input_has_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
                if ui.input(raw_escape_pressed) {
                    close = true;
                }
            }

            let count_label = if self.markdown_search.query.trim().is_empty() {
                "0 results".to_owned()
            } else if matches.is_empty() {
                "No results".to_owned()
            } else {
                format!(
                    "{}/{}",
                    self.markdown_search.current_match + 1,
                    matches.len()
                )
            };
            ui.label(RichText::new(count_label).small().weak());

            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Previous"))
                .on_hover_text("Previous search match")
                .clicked()
            {
                go_previous = true;
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Next"))
                .on_hover_text("Next search match")
                .clicked()
            {
                go_next = true;
            }
            if ui.button("Close").on_hover_text("Close search").clicked() {
                close = true;
            }

            if keep_focus && !close {
                ui.memory_mut(|memory| memory.request_focus(search_id));
            }
        });

        if close {
            self.markdown_search.open = false;
            self.markdown_search.focus_requested = false;
            self.markdown_search.query.clear();
            self.markdown_search.current_match = 0;
            self.markdown_search.match_count = 0;
            self.markdown_search.scrolled_match = None;
            return;
        }

        if go_next {
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        } else {
            self.navigate_to_current_markdown_match(matches);
        }
    }

    pub(super) fn handle_markdown_search_global_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
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
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        }
    }

    pub(super) fn move_markdown_search_match(
        &mut self,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
        delta: isize,
    ) {
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let current = self.markdown_search.current_match as isize;
        self.markdown_search.current_match = (current + delta).rem_euclid(len) as usize;
        self.markdown_search.scrolled_match = None;
        self.navigate_to_current_markdown_match(matches);

        if let Some(search_match) = self.markdown_active_search_match(matches)
            && let Some(page) = document.page_by_id(&search_match.page_id)
        {
            let position = Position::new(page.source_range.start.line as u32, 0);
            self.cursor_position = Some(position);
        }
    }

    pub(super) fn navigate_to_current_markdown_match(&mut self, matches: &[MarkdownSearchMatch]) {
        let Some(search_match) = self.markdown_active_search_match(matches) else {
            return;
        };
        if self.markdown_active_page_id.as_deref() != Some(search_match.page_id.as_str()) {
            self.markdown_active_heading_id = None;
        }
        self.markdown_active_page_id = Some(search_match.page_id.clone());
    }

    pub(super) fn markdown_active_search_match(
        &self,
        matches: &[MarkdownSearchMatch],
    ) -> Option<MarkdownSearchMatch> {
        if self.markdown_search.query.trim().is_empty() {
            return None;
        }
        matches.get(self.markdown_search.current_match).cloned()
    }

    pub(super) fn markdown_preview_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let left_width = if self.markdown_show_pages {
            self.markdown_pages_width =
                clamp_markdown_side_pane_width(self.markdown_pages_width, available_width);
            self.markdown_pages_width
        } else {
            0.0
        };
        let right_width = if self.markdown_show_outline {
            markdown_side_pane_width(available_width)
        } else {
            0.0
        };
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(available_width, available_height),
            egui::Sense::hover(),
        );
        let separator_width = 8.0;
        let separator_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let mut left = rect.left();
        let mut right = rect.right();

        if self.markdown_show_pages {
            let page_rect = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(left_width, rect.height()),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(page_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(page_rect);
                    self.markdown_page_nav_ui(ui, document);
                },
            );
            left = page_rect.right();
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(separator_width, rect.height()),
            );
            self.markdown_pages_resize_handle(ui, handle_rect, rect.left(), available_width);
            left += separator_width;
        }

        if self.markdown_show_outline {
            let outline_left = (right - right_width).max(left);
            let outline_rect = egui::Rect::from_min_size(
                egui::pos2(outline_left, rect.top()),
                egui::vec2((right - outline_left).max(0.0), rect.height()),
            );
            let separator_x = outline_rect.left() - separator_width * 0.5;
            ui.painter().line_segment(
                [
                    egui::pos2(separator_x, rect.top()),
                    egui::pos2(separator_x, rect.bottom()),
                ],
                egui::Stroke::new(1.0, separator_color),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(outline_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.set_clip_rect(outline_rect);
                    self.markdown_outline_ui(ui, document);
                },
            );
            right = outline_rect.left() - separator_width;
        }

        let body_rect = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right.max(left), rect.bottom()),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(body_rect);
                self.markdown_body_ui(ui, document, language_registry, active_search_match);
            },
        );
    }

    pub(super) fn markdown_both_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        active_page_range: Option<(u32, u32)>,
        active_search_match: Option<MarkdownSearchMatch>,
        bookmark_lines: &[u32],
    ) {
        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let separator_width = 8.0;
        let page_width = if self.markdown_show_pages {
            self.markdown_pages_width =
                clamp_markdown_side_pane_width(self.markdown_pages_width, available_width);
            self.markdown_pages_width
        } else {
            0.0
        };
        let outline_width = if self.markdown_show_outline {
            markdown_side_pane_width(available_width)
        } else {
            0.0
        };
        let separator_count =
            1 + usize::from(self.markdown_show_pages) + usize::from(self.markdown_show_outline);
        let remaining_width = (available_width
            - page_width
            - outline_width
            - separator_width * separator_count as f32)
            .max(360.0);
        let preview_width = (remaining_width * 0.48).max(220.0);
        let editor_width = (remaining_width - preview_width - 8.0).max(220.0);

        ui.horizontal_top(|ui| {
            if self.markdown_show_pages {
                ui.allocate_ui_with_layout(
                    egui::vec2(page_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.markdown_page_nav_ui(ui, document),
                );
                let (handle_rect, _) = ui.allocate_exact_size(
                    egui::vec2(separator_width, available_height),
                    egui::Sense::hover(),
                );
                let pane_left = handle_rect.left() - page_width;
                self.markdown_pages_resize_handle(ui, handle_rect, pane_left, available_width);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(preview_width, available_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.markdown_body_ui(ui, document, language_registry, active_search_match),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(editor_width, available_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.label(self.markdown_focus_label(ui, "Markdown", MarkdownFocusTarget::Body));
                    ui.add_space(4.0);
                    self.markdown_source_ui(
                        ui,
                        focused,
                        tab_width,
                        active_page_range,
                        Some(document),
                        bookmark_lines,
                    );
                },
            );
            if self.markdown_show_outline {
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(outline_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.markdown_outline_ui(ui, document),
                );
            }
        });
    }

    pub(super) fn markdown_pages_resize_handle(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pane_left: f32,
        available_width: f32,
    ) {
        let hit_rect = rect.expand(6.0);
        let response = ui.interact(
            hit_rect,
            ui.make_persistent_id(("markdown_pages_resize", self.pane)),
            egui::Sense::click_and_drag(),
        );

        let pointer_position = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
        let primary_down = ui.input(|input| input.pointer.primary_down());
        if primary_pressed && pointer_position.is_some_and(|position| hit_rect.contains(position)) {
            self.markdown_pages_resize_start_width = Some(self.markdown_pages_width);
        }
        if self.markdown_pages_resize_start_width.is_some() && primary_down {
            if let Some(pointer_position) = pointer_position {
                self.markdown_pages_width =
                    clamp_markdown_side_pane_width(pointer_position.x - pane_left, available_width);
            }
            ui.ctx().request_repaint();
        }
        if !primary_down {
            self.markdown_pages_resize_start_width = None;
        }

        let resizing = self.markdown_pages_resize_start_width.is_some();
        if response.hovered() || resizing {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        let stroke_color = if response.hovered() || resizing {
            ui.visuals().widgets.active.bg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        ui.painter().line_segment(
            [
                egui::pos2(rect.center().x, rect.top()),
                egui::pos2(rect.center().x, rect.bottom()),
            ],
            egui::Stroke::new(1.0, stroke_color),
        );
    }

    pub(super) fn markdown_page_nav_ui(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        self.markdown_side_pane_title_ui(ui, "Pages", MarkdownFocusTarget::Pages);
        ui.add_space(4.0);

        ScrollArea::vertical()
            .id_salt(format!("markdown_pages_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if document.pages.is_empty() {
                    ui.label(RichText::new("No pages").weak());
                    return;
                }

                for page in &document.pages {
                    let indent = (page.depth.saturating_sub(1) as f32) * 12.0;
                    let selected =
                        self.markdown_active_page_id.as_deref() == Some(page.id.as_str());
                    let row_height = ui.text_style_height(&TextStyle::Body) + 4.0;
                    let (row_rect, row_response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter()
                            .rect_filled(row_rect, 2.0, ui.visuals().selection.bg_fill);
                    }

                    let text_rect = egui::Rect::from_min_max(
                        egui::pos2(row_rect.left() + indent, row_rect.top()),
                        row_rect.right_bottom(),
                    );
                    let font_id = TextStyle::Body.resolve(ui.style());
                    let text = ellipsize_text_to_width(
                        ui,
                        &page.title,
                        font_id.clone(),
                        text_rect.width().max(0.0),
                    );
                    let color = if selected {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.painter().with_clip_rect(text_rect).text(
                        text_rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font_id,
                        color,
                    );

                    if row_response.on_hover_text(&page.title).clicked() {
                        self.select_markdown_page(page.id.clone());
                        if self.markdown_mode == MarkdownMode::Both {
                            self.move_markdown_source_to_page(page);
                        } else {
                            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                        }
                    }
                }
            });
    }

    pub(super) fn markdown_body_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let Some(page) = document.active_or_first_page(self.markdown_active_page_id.as_deref())
        else {
            ui.label(RichText::new("Empty Markdown document").weak());
            return;
        };
        let search_query = active_search_match
            .as_ref()
            .filter(|search_match| search_match.page_id == page.id)
            .map(|_| self.markdown_search.query.trim().to_owned());

        self.markdown_page_header_ui(ui, document, page, search_query.as_deref());
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt(format!("markdown_body_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let body_id = self.markdown_focus_id(MarkdownFocusTarget::Body);
                let body_response = ui.interact(ui.max_rect(), body_id, egui::Sense::click());
                if body_response.clicked() {
                    self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                }
                let page_body_start = markdown_page_body_start_offset(document, page);
                let page_source =
                    document.source_text_between(page_body_start, page.source_range.end.offset);
                let outline = document.page_outline(&page.id);
                if page_source.trim().is_empty() {
                    ui.label(RichText::new("No rendered text for this page.").weak());
                } else if outline.is_empty() {
                    self.markdown_render_source_fragment_ui(
                        ui,
                        document,
                        language_registry,
                        page_body_start,
                        page_source,
                        search_query.as_deref(),
                    );
                } else {
                    self.markdown_body_sections_ui(
                        ui,
                        document,
                        language_registry,
                        page,
                        &outline,
                        search_query.as_deref(),
                    );
                }
                self.markdown_footnotes_ui(
                    ui,
                    document,
                    language_registry,
                    page,
                    search_query.as_deref(),
                );
            });
    }

    pub(super) fn markdown_body_sections_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        page: &crate::core::markdown::MarkdownPage,
        outline: &[&crate::core::markdown::MarkdownHeading],
        search_query: Option<&str>,
    ) {
        let page_body_start = markdown_page_body_start_offset(document, page);
        if let Some(first_heading) = outline.first() {
            self.markdown_render_source_fragment_ui(
                ui,
                document,
                language_registry,
                page_body_start,
                document
                    .source_text_between(page_body_start, first_heading.source_range.start.offset),
                search_query,
            );
        }

        for (index, heading) in outline.iter().enumerate() {
            let selected =
                self.markdown_active_heading_id.as_deref() == Some(heading.slug.as_str());
            ui.add_space(4.0);
            let heading_job = markdown_plain_text_layout_job(
                ui,
                &heading.title,
                search_query,
                markdown_heading_text_format(ui, heading.level),
            );
            let response = ui.selectable_label(selected, heading_job);
            if response.clicked() {
                self.markdown_active_heading_id = Some(heading.slug.clone());
            }
            if selected || self.should_scroll_to_markdown_search_match(search_query, &heading.title)
            {
                response.scroll_to_me(Some(egui::Align::Center));
            }

            let section_end = outline
                .get(index + 1)
                .map(|next| next.source_range.start.offset)
                .unwrap_or(page.source_range.end.offset);
            self.markdown_render_source_fragment_ui(
                ui,
                document,
                language_registry,
                heading.source_range.end.offset,
                document.source_text_between(heading.source_range.end.offset, section_end),
                search_query,
            );
        }
    }

    pub(super) fn should_scroll_to_markdown_search_match(
        &mut self,
        search_query: Option<&str>,
        text: &str,
    ) -> bool {
        let Some(query) = search_query.filter(|query| !query.is_empty()) else {
            return false;
        };
        if self.markdown_search.scrolled_match == Some(self.markdown_search.current_match) {
            return false;
        }
        if text.to_lowercase().contains(&query.to_lowercase()) {
            self.markdown_search.scrolled_match = Some(self.markdown_search.current_match);
            return true;
        }
        false
    }

    pub(super) fn markdown_page_header_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        page: &crate::core::markdown::MarkdownPage,
        search_query: Option<&str>,
    ) {
        let Some(index) = document.page_index_by_id(&page.id) else {
            return;
        };
        let previous = index
            .checked_sub(1)
            .and_then(|index| document.pages.get(index));
        let next = document.pages.get(index + 1);

        ui.horizontal(|ui| {
            let mut title_format = markdown_heading_text_format(ui, 2);
            if self.markdown_focus_target == MarkdownFocusTarget::Body {
                title_format.color = markdown_focus_color(ui);
            }
            let title_job =
                markdown_plain_text_layout_job(ui, &page.title, search_query, title_format);
            let title_response = ui.add(egui::Label::new(title_job));
            if self.should_scroll_to_markdown_search_match(search_query, &page.title) {
                title_response.scroll_to_me(Some(egui::Align::Center));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(next.is_some(), egui::Button::new("Next"))
                    .on_hover_text("Go to next page")
                    .clicked()
                    && let Some(next) = next
                {
                    self.select_markdown_page(next.id.clone());
                    if self.markdown_mode == MarkdownMode::Both {
                        self.move_markdown_source_to_page(next);
                    } else {
                        self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                    }
                }

                ui.label(RichText::new(format!(
                    "{}/{}",
                    index + 1,
                    document.pages.len()
                )));

                if ui
                    .add_enabled(previous.is_some(), egui::Button::new("Previous"))
                    .on_hover_text("Go to previous page")
                    .clicked()
                    && let Some(previous) = previous
                {
                    self.select_markdown_page(previous.id.clone());
                    if self.markdown_mode == MarkdownMode::Both {
                        self.move_markdown_source_to_page(previous);
                    } else {
                        self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                    }
                }
            });
        });
    }

    pub(super) fn markdown_outline_ui(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        self.markdown_side_pane_title_ui(ui, "Outline", MarkdownFocusTarget::Outline);
        ui.add_space(4.0);

        let Some(page) = document.active_or_first_page(self.markdown_active_page_id.as_deref())
        else {
            ui.label(RichText::new("No outline").weak());
            return;
        };

        ScrollArea::vertical()
            .id_salt(format!("markdown_outline_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let outline = document.page_outline(&page.id);
                if outline.is_empty() {
                    ui.label(RichText::new("No headings").weak());
                    return;
                }

                for heading in outline {
                    ui.horizontal(|ui| {
                        ui.add_space((heading.level.saturating_sub(1) as f32) * 8.0);
                        let selected = self.markdown_active_heading_id.as_deref()
                            == Some(heading.slug.as_str());
                        if ui
                            .selectable_label(selected, RichText::new(&heading.title).small())
                            .clicked()
                        {
                            self.markdown_active_heading_id = Some(heading.slug.clone());
                            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
                            ui.ctx().request_repaint();
                        }
                    });
                }
            });
    }

    pub(super) fn handle_markdown_preview_navigation(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
    ) {
        if document.pages.is_empty() {
            return;
        }

        // Shift first: consume_key(NONE, ..) ignores an extra Shift.
        let (previous_pane, next_pane) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::SHIFT, egui::Key::F6),
                input.consume_key(egui::Modifiers::NONE, egui::Key::F6),
            )
        });
        if next_pane || previous_pane {
            self.cycle_markdown_focus_target(ui, next_pane);
            ui.ctx().request_repaint();
            return;
        }

        let mut next_index = None;
        let mut next_heading_index = None;
        let mut focus_body = false;

        ui.input_mut(|input| {
            if self.markdown_focus_target == MarkdownFocusTarget::Pages {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    next_index = self.markdown_page_index_with_delta(document, -1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    next_index = self.markdown_page_index_with_delta(document, 1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    next_index = document
                        .active_or_first_page_index(self.markdown_active_page_id.as_deref());
                    focus_body = true;
                    return;
                }
            }

            if self.markdown_focus_target == MarkdownFocusTarget::Outline {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    next_heading_index = self.markdown_outline_index_with_delta(document, -1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    next_heading_index = self.markdown_outline_index_with_delta(document, 1);
                    return;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    focus_body = true;
                    return;
                }
            }

            if input.consume_key(egui::Modifiers::CTRL, egui::Key::PageUp) {
                next_index = self.markdown_page_index_with_delta(document, -1);
                focus_body = true;
                return;
            }
            if input.consume_key(egui::Modifiers::CTRL, egui::Key::PageDown) {
                next_index = self.markdown_page_index_with_delta(document, 1);
                focus_body = true;
            }
        });

        if let Some(next_index) = next_index
            && let Some(page) = document.pages.get(next_index)
        {
            self.select_markdown_page(page.id.clone());
            if self.markdown_mode == MarkdownMode::Both && focus_body {
                self.move_markdown_source_to_page(page);
            } else if focus_body {
                self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
            }
            ui.ctx().request_repaint();
        }
        if let Some(next_heading_index) = next_heading_index
            && let Some(page) =
                document.active_or_first_page(self.markdown_active_page_id.as_deref())
            && let Some(heading) = document.page_outline(&page.id).get(next_heading_index)
        {
            self.markdown_active_heading_id = Some(heading.slug.clone());
            ui.ctx().request_repaint();
        }
        if focus_body && next_index.is_none() {
            self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
        }
    }

    pub(super) fn markdown_page_index_with_delta(
        &self,
        document: &MarkdownDocument,
        delta: isize,
    ) -> Option<usize> {
        let index = document.active_or_first_page_index(self.markdown_active_page_id.as_deref())?;
        let max_index = document.pages.len().saturating_sub(1) as isize;
        Some((index as isize + delta).clamp(0, max_index) as usize)
    }

    pub(super) fn markdown_outline_index_with_delta(
        &self,
        document: &MarkdownDocument,
        delta: isize,
    ) -> Option<usize> {
        let page = document.active_or_first_page(self.markdown_active_page_id.as_deref())?;
        let outline = document.page_outline(&page.id);
        if outline.is_empty() {
            return None;
        }
        let index = self
            .markdown_active_heading_id
            .as_deref()
            .and_then(|heading_id| {
                outline
                    .iter()
                    .position(|heading| heading.slug.as_str() == heading_id)
            })
            .unwrap_or(0);
        let max_index = outline.len().saturating_sub(1) as isize;
        Some((index as isize + delta).clamp(0, max_index) as usize)
    }

    pub(super) fn select_markdown_page(&mut self, page_id: String) {
        if self.markdown_active_page_id.as_deref() != Some(page_id.as_str()) {
            self.markdown_active_heading_id = None;
        }
        self.markdown_active_page_id = Some(page_id);
    }

    pub(super) fn move_markdown_source_to_page(
        &mut self,
        page: &crate::core::markdown::MarkdownPage,
    ) {
        let position = Position::new(page.source_range.start.line as u32, 0);
        self.cursor_position = Some(position);
        self.target_position = Some(position);
        self.pending_navigation = Some(PendingEditorNavigation {
            position,
            focus_editor: false,
        });
        self.markdown_focus_target = MarkdownFocusTarget::Body;
    }

    /// Independent of the calling `ui`, so a request from the page header or a shortcut
    /// handler focuses the same widget the body registers with `ui.interact`.
    pub(super) fn markdown_focus_id(&self, target: MarkdownFocusTarget) -> egui::Id {
        egui::Id::new(("markdown_focus", self.pane, target))
    }

    pub(super) fn request_markdown_focus(
        &mut self,
        ui: &mut egui::Ui,
        target: MarkdownFocusTarget,
    ) {
        self.markdown_focus_target = target;
        if target != MarkdownFocusTarget::Body {
            // Pages / Outline keys work from `markdown_focus_target` alone. Nothing holds egui
            // focus meanwhile, so egui does not turn their Up/Down into focus moves.
            ui.memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
            return;
        }
        let focus_id = self.markdown_focus_id(target);
        ui.memory_mut(|memory| memory.request_focus(focus_id));
    }

    /// Title of the Pages / Outline pane; clicking it gives that pane the keyboard.
    pub(super) fn markdown_side_pane_title_ui(
        &mut self,
        ui: &mut egui::Ui,
        label: &'static str,
        target: MarkdownFocusTarget,
    ) {
        let title = egui::Label::new(self.markdown_focus_label(ui, label, target))
            .sense(egui::Sense::click());
        if ui.add(title).clicked() {
            self.request_markdown_focus(ui, target);
        }
    }

    /// F6 / Shift+F6 move the keyboard between the visible Pages, Body and Outline panes.
    pub(super) fn cycle_markdown_focus_target(&mut self, ui: &mut egui::Ui, forward: bool) {
        let targets = self.visible_markdown_focus_targets();
        let index = targets
            .iter()
            .position(|target| *target == self.markdown_focus_target)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % targets.len()
        } else {
            (index + targets.len() - 1) % targets.len()
        };
        self.request_markdown_focus(ui, targets[next]);
    }

    pub(super) fn visible_markdown_focus_targets(&self) -> Vec<MarkdownFocusTarget> {
        if self.markdown_mode == MarkdownMode::Markdown {
            return vec![MarkdownFocusTarget::Body];
        }

        let mut targets = Vec::new();
        if self.markdown_show_pages {
            targets.push(MarkdownFocusTarget::Pages);
        }
        targets.push(MarkdownFocusTarget::Body);
        if self.markdown_show_outline {
            targets.push(MarkdownFocusTarget::Outline);
        }
        targets
    }

    pub(super) fn normalize_markdown_focus_target(&mut self) {
        let targets = self.visible_markdown_focus_targets();
        if !targets.contains(&self.markdown_focus_target) {
            self.markdown_focus_target = MarkdownFocusTarget::Body;
        }
    }

    pub(super) fn markdown_focus_label(
        &self,
        ui: &egui::Ui,
        label: &'static str,
        target: MarkdownFocusTarget,
    ) -> RichText {
        let text = RichText::new(label).small().strong();
        if self.markdown_focus_target == target {
            text.color(markdown_focus_color(ui))
                .background_color(markdown_focus_background(ui))
        } else {
            text
        }
    }
}

pub(super) const MARKDOWN_SIDE_PANE_DEFAULT_WIDTH: f32 = 180.0;

pub(super) const MARKDOWN_SIDE_PANE_MIN_WIDTH: f32 = 120.0;

pub(super) const MARKDOWN_SIDE_PANE_MAX_WIDTH: f32 = 560.0;

pub(super) const MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH: f32 = 360.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkdownMode {
    Preview,
    Markdown,
    Both,
}

impl MarkdownMode {
    pub(super) fn all() -> [Self; 3] {
        [Self::Preview, Self::Markdown, Self::Both]
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Preview => "Preview",
            Self::Markdown => "Markdown",
            Self::Both => "Both",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct MarkdownSearchState {
    pub(super) open: bool,
    pub(super) focus_requested: bool,
    pub(super) query: String,
    pub(super) current_match: usize,
    pub(super) match_count: usize,
    pub(super) scrolled_match: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownSearchMatch {
    pub(super) page_id: String,
    pub(super) page_index: usize,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkdownFootnoteScrollTarget {
    Reference(String),
    Definition(String),
}

#[derive(Debug, Clone)]
pub(super) struct MarkdownDocumentCache {
    pub(super) path: PathBuf,
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) document: Result<Rc<MarkdownDocument>, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum MarkdownFocusTarget {
    Pages,
    Body,
    Outline,
}

pub(super) fn markdown_search_matches(
    document: &MarkdownDocument,
    query: &str,
) -> Vec<MarkdownSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    for (page_index, page) in document.pages.iter().enumerate() {
        let page_text = document.page_rendered_text(page);
        let page_text_lower = page_text.to_lowercase();
        let mut cursor = 0;

        while let Some(relative_start) = page_text_lower[cursor..].find(&query_lower) {
            let start = cursor + relative_start;
            let end = start + query_lower.len();
            matches.push(MarkdownSearchMatch {
                page_id: page.id.clone(),
                page_index,
                start,
                end,
            });
            cursor = end.max(start + 1);
        }
    }

    matches
}

pub(super) fn markdown_side_pane_width(available_width: f32) -> f32 {
    clamp_markdown_side_pane_width(MARKDOWN_SIDE_PANE_DEFAULT_WIDTH, available_width)
}

pub(super) fn clamp_markdown_side_pane_width(width: f32, available_width: f32) -> f32 {
    let max_width = (available_width - MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH)
        .clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, MARKDOWN_SIDE_PANE_MAX_WIDTH);
    width.clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, max_width)
}

pub(super) fn markdown_search_match_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(87, 72, 24)
    } else {
        Color32::from_rgb(255, 238, 160)
    }
}

pub(super) fn markdown_focus_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(116, 190, 255)
    } else {
        Color32::from_rgb(0, 92, 160)
    }
}

pub(super) fn markdown_focus_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(34, 54, 72)
    } else {
        Color32::from_rgb(214, 236, 252)
    }
}

pub(super) fn ellipsize_text_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: egui::FontId,
    max_width: f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }

    let fits = |value: &str| {
        ui.painter()
            .layout_no_wrap(value.to_owned(), font_id.clone(), Color32::WHITE)
            .size()
            .x
            <= max_width
    };

    if fits(text) {
        return text.to_owned();
    }
    if !fits("...") {
        return String::new();
    }

    let char_count = text.chars().count();
    let mut low = 0;
    let mut high = char_count;
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}...", text.chars().take(mid).collect::<String>());
        if fits(&candidate) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    format!("{}...", text.chars().take(low).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_markdown_document_reuses_until_source_changes() {
        let mut editor = EditorPane::empty(Pane::Left);
        let path = PathBuf::from("guide.md");
        editor.replace_text("# First\n\nText\n".to_owned());

        let first = editor.cached_markdown_document(&path).unwrap();
        let second = editor.cached_markdown_document(&path).unwrap();

        assert!(Rc::ptr_eq(&first, &second));

        editor
            .text
            .insert_str(editor.text.len(), "\n## Second\n\nMore\n");
        editor.mark_text_changed();
        let third = editor.cached_markdown_document(&path).unwrap();

        assert!(!Rc::ptr_eq(&first, &third));
        assert_eq!(third.pages.len(), 2);
    }

    #[test]
    fn markdown_search_matches_rendered_text_across_pages() {
        let document = MarkdownDocument::parse(
            "guide.md".into(),
            "# First\n\nFind **needle** here.\n\n## Second\n\n`needle` again.\n".to_owned(),
        )
        .unwrap();

        let matches = markdown_search_matches(&document, "needle");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].page_id, "first");
        assert_eq!(matches[1].page_id, "second");
        assert!(markdown_search_matches(&document, "**needle**").is_empty());
    }
}
