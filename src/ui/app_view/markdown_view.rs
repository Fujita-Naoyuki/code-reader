//! The Markdown pane: Preview / Markdown / Both modes, Pages and Outline, the rendered
//! search box and keyboard focus between the panes.

use super::*;

/// What a pane's views take from the app each frame.
#[derive(Clone, Copy)]
pub(super) struct EditorEnv<'a> {
    pub(super) language_registry: &'a LanguageRegistry,
    pub(super) tab_width: usize,
    pub(super) bookmark_lines: &'a [u32],
}

impl EditorPane {
    pub(super) fn is_markdown(&self) -> bool {
        self.path().is_some_and(is_markdown_path)
    }

    /// Keeps the active page valid and, while the source is visible, on the cursor's page.
    fn sync_markdown_active_page(&mut self, document: &MarkdownDocument) {
        if self
            .markdown
            .active_page_id
            .as_deref()
            .and_then(|page_id| document.page_by_id(page_id))
            .is_none()
        {
            self.markdown.active_page_id = document.pages.first().map(|page| page.id.clone());
        }
        if matches!(
            self.markdown.mode,
            MarkdownMode::Markdown | MarkdownMode::Both
        ) {
            self.update_markdown_active_page_from_cursor(document);
        }
    }

    /// The mode buttons and the Pages / Outline toggles.
    fn markdown_toolbar_ui(&mut self, ui: &mut egui::Ui, document: &MarkdownDocument) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MARKDOWN").small().strong());
            ui.separator();
            for mode in MarkdownMode::all() {
                if ui
                    .selectable_label(self.markdown.mode == mode, mode.label())
                    .on_hover_text(format!("Switch to {} mode", mode.label()))
                    .clicked()
                {
                    self.set_markdown_mode(mode, document);
                }
            }
            if self.markdown.mode != MarkdownMode::Markdown {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.markdown.show_outline, "Outline")
                        .on_hover_text("Show or hide the outline pane")
                        .clicked()
                    {
                        self.markdown.show_outline = !self.markdown.show_outline;
                        self.normalize_markdown_focus_target();
                    }
                    if ui
                        .selectable_label(self.markdown.show_pages, "Pages")
                        .on_hover_text("Show or hide the pages pane")
                        .clicked()
                    {
                        self.markdown.show_pages = !self.markdown.show_pages;
                        self.normalize_markdown_focus_target();
                    }
                });
            }
        });
    }

    pub(super) fn markdown_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        bookmark_lines: &[u32],
    ) {
        let Some(path) = self.path().map(Path::to_path_buf) else {
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

        self.sync_markdown_active_page(&document);

        if focused
            && matches!(
                self.markdown.mode,
                MarkdownMode::Preview | MarkdownMode::Both
            )
        {
            self.handle_markdown_preview_navigation(ui, &document);
        }
        if focused {
            self.handle_markdown_mode_shortcuts(ui, &document);
        }

        self.markdown_toolbar_ui(ui, &document);

        let matches = if self.markdown.search.open {
            let matches = markdown_search_matches(&document, &self.markdown.search.query);
            self.markdown.search.match_count = matches.len();
            if self.markdown.search.current_match >= matches.len() {
                self.markdown.search.current_match = matches.len().saturating_sub(1);
            }
            if focused {
                self.handle_markdown_search_global_shortcuts(ui, &document, &matches);
            }
            self.markdown_search_ui(ui, &document, &matches);
            matches
        } else {
            self.markdown.search.match_count = 0;
            Vec::new()
        };

        ui.separator();

        let active_page_range = self.markdown_active_source_line_range(&document);
        let active_search_match = self.markdown_active_search_match(&matches);
        match self.markdown.mode {
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
                EditorEnv {
                    language_registry,
                    tab_width,
                    bookmark_lines,
                },
                active_page_range,
                active_search_match,
            ),
        }
    }

    pub(super) fn cached_markdown_document(
        &mut self,
        path: &Path,
    ) -> Result<Rc<MarkdownDocument>, String> {
        let (source_len, source_hash) = self.text_fingerprint();

        if let Some(cache) = &self.markdown.document_cache
            && cache.path == path
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.document.clone();
        }

        let document =
            MarkdownDocument::parse(path.to_path_buf(), self.text.as_str().to_owned()).map(Rc::new);
        self.markdown.document_cache = Some(MarkdownDocumentCache {
            path: path.to_path_buf(),
            source_len,
            source_hash,
            document: document.clone(),
        });
        document
    }

    pub(super) fn set_markdown_mode(&mut self, mode: MarkdownMode, document: &MarkdownDocument) {
        if self.markdown.mode == mode {
            return;
        }

        if matches!(mode, MarkdownMode::Markdown | MarkdownMode::Both) {
            self.prepare_markdown_source_entry(document);
            self.markdown.focus_target = MarkdownFocusTarget::Body;
        }
        if mode == MarkdownMode::Both && self.markdown.focus_target == MarkdownFocusTarget::Outline
        {
            self.markdown.focus_target = MarkdownFocusTarget::Body;
        }
        self.markdown.mode = mode;
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

    pub(super) fn markdown_preview_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let left_width = if self.markdown.show_pages {
            self.markdown.pages_width =
                clamp_markdown_side_pane_width(self.markdown.pages_width, available_width);
            self.markdown.pages_width
        } else {
            0.0
        };
        let right_width = if self.markdown.show_outline {
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

        if self.markdown.show_pages {
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

        if self.markdown.show_outline {
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
        env: EditorEnv<'_>,
        active_page_range: Option<(u32, u32)>,
        active_search_match: Option<MarkdownSearchMatch>,
    ) {
        let EditorEnv {
            language_registry,
            tab_width,
            bookmark_lines,
        } = env;

        let available_height = ui.available_height();
        let available_width = ui.available_width();
        let separator_width = 8.0;
        let page_width = if self.markdown.show_pages {
            self.markdown.pages_width =
                clamp_markdown_side_pane_width(self.markdown.pages_width, available_width);
            self.markdown.pages_width
        } else {
            0.0
        };
        let outline_width = if self.markdown.show_outline {
            markdown_side_pane_width(available_width)
        } else {
            0.0
        };
        let separator_count =
            1 + usize::from(self.markdown.show_pages) + usize::from(self.markdown.show_outline);
        let remaining_width = (available_width
            - page_width
            - outline_width
            - separator_width * separator_count as f32)
            .max(360.0);
        let preview_width = (remaining_width * 0.48).max(220.0);
        let editor_width = (remaining_width - preview_width - 8.0).max(220.0);

        ui.horizontal_top(|ui| {
            if self.markdown.show_pages {
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
            if self.markdown.show_outline {
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
            self.markdown.pages_resize_start_width = Some(self.markdown.pages_width);
        }
        if self.markdown.pages_resize_start_width.is_some() && primary_down {
            if let Some(pointer_position) = pointer_position {
                self.markdown.pages_width =
                    clamp_markdown_side_pane_width(pointer_position.x - pane_left, available_width);
            }
            ui.ctx().request_repaint();
        }
        if !primary_down {
            self.markdown.pages_resize_start_width = None;
        }

        let resizing = self.markdown.pages_resize_start_width.is_some();
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
                        self.markdown.active_page_id.as_deref() == Some(page.id.as_str());
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
                        if self.markdown.mode == MarkdownMode::Both {
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
        let Some(page) = document.active_or_first_page(self.markdown.active_page_id.as_deref())
        else {
            ui.label(RichText::new("Empty Markdown document").weak());
            return;
        };
        let search_query = active_search_match
            .as_ref()
            .filter(|search_match| search_match.page_id == page.id)
            .map(|_| self.markdown.search.query.trim().to_owned());

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
                self.markdown.active_heading_id.as_deref() == Some(heading.slug.as_str());
            ui.add_space(4.0);
            let heading_job = markdown_plain_text_layout_job(
                ui,
                &heading.title,
                search_query,
                markdown_heading_text_format(ui, heading.level),
            );
            let response = ui.selectable_label(selected, heading_job);
            if response.clicked() {
                self.markdown.active_heading_id = Some(heading.slug.clone());
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
            if self.markdown.focus_target == MarkdownFocusTarget::Body {
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
                    if self.markdown.mode == MarkdownMode::Both {
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
                    if self.markdown.mode == MarkdownMode::Both {
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

        let Some(page) = document.active_or_first_page(self.markdown.active_page_id.as_deref())
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
                        let selected = self.markdown.active_heading_id.as_deref()
                            == Some(heading.slug.as_str());
                        if ui
                            .selectable_label(selected, RichText::new(&heading.title).small())
                            .clicked()
                        {
                            self.markdown.active_heading_id = Some(heading.slug.clone());
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
            if self.markdown.focus_target == MarkdownFocusTarget::Pages {
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
                        .active_or_first_page_index(self.markdown.active_page_id.as_deref());
                    focus_body = true;
                    return;
                }
            }

            if self.markdown.focus_target == MarkdownFocusTarget::Outline {
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
            if self.markdown.mode == MarkdownMode::Both && focus_body {
                self.move_markdown_source_to_page(page);
            } else if focus_body {
                self.request_markdown_focus(ui, MarkdownFocusTarget::Body);
            }
            ui.ctx().request_repaint();
        }
        if let Some(next_heading_index) = next_heading_index
            && let Some(page) =
                document.active_or_first_page(self.markdown.active_page_id.as_deref())
            && let Some(heading) = document.page_outline(&page.id).get(next_heading_index)
        {
            self.markdown.active_heading_id = Some(heading.slug.clone());
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
        let index = document.active_or_first_page_index(self.markdown.active_page_id.as_deref())?;
        let max_index = document.pages.len().saturating_sub(1) as isize;
        Some((index as isize + delta).clamp(0, max_index) as usize)
    }

    pub(super) fn markdown_outline_index_with_delta(
        &self,
        document: &MarkdownDocument,
        delta: isize,
    ) -> Option<usize> {
        let page = document.active_or_first_page(self.markdown.active_page_id.as_deref())?;
        let outline = document.page_outline(&page.id);
        if outline.is_empty() {
            return None;
        }
        let index = self
            .markdown
            .active_heading_id
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
        if self.markdown.active_page_id.as_deref() != Some(page_id.as_str()) {
            self.markdown.active_heading_id = None;
        }
        self.markdown.active_page_id = Some(page_id);
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
        self.markdown.focus_target = MarkdownFocusTarget::Body;
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
        self.markdown.focus_target = target;
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
            .position(|target| *target == self.markdown.focus_target)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % targets.len()
        } else {
            (index + targets.len() - 1) % targets.len()
        };
        self.request_markdown_focus(ui, targets[next]);
    }

    pub(super) fn visible_markdown_focus_targets(&self) -> Vec<MarkdownFocusTarget> {
        if self.markdown.mode == MarkdownMode::Markdown {
            return vec![MarkdownFocusTarget::Body];
        }

        let mut targets = Vec::new();
        if self.markdown.show_pages {
            targets.push(MarkdownFocusTarget::Pages);
        }
        targets.push(MarkdownFocusTarget::Body);
        if self.markdown.show_outline {
            targets.push(MarkdownFocusTarget::Outline);
        }
        targets
    }

    pub(super) fn normalize_markdown_focus_target(&mut self) {
        let targets = self.visible_markdown_focus_targets();
        if !targets.contains(&self.markdown.focus_target) {
            self.markdown.focus_target = MarkdownFocusTarget::Body;
        }
    }

    pub(super) fn markdown_focus_label(
        &self,
        ui: &egui::Ui,
        label: &'static str,
        target: MarkdownFocusTarget,
    ) -> RichText {
        let text = RichText::new(label).small().strong();
        if self.markdown.focus_target == target {
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

/// State only the Markdown views use.
pub(super) struct MarkdownPaneState {
    pub(super) mode: MarkdownMode,
    pub(super) active_page_id: Option<String>,
    pub(super) active_heading_id: Option<String>,
    pub(super) focus_target: MarkdownFocusTarget,
    pub(super) show_pages: bool,
    pub(super) pages_width: f32,
    pub(super) pages_resize_start_width: Option<f32>,
    pub(super) show_outline: bool,
    pub(super) search: MarkdownSearchState,
    pub(super) document_cache: Option<MarkdownDocumentCache>,
    pub(super) image_cache: HashMap<PathBuf, Result<egui::TextureHandle, String>>,
    pub(super) mermaid_cache: HashMap<String, Result<String, String>>,
    pub(super) pending_footnote_scroll: Option<MarkdownFootnoteScrollTarget>,
}

impl Default for MarkdownPaneState {
    fn default() -> Self {
        Self {
            mode: MarkdownMode::Preview,
            active_page_id: None,
            active_heading_id: None,
            focus_target: MarkdownFocusTarget::Body,
            show_pages: true,
            pages_width: MARKDOWN_SIDE_PANE_DEFAULT_WIDTH,
            pages_resize_start_width: None,
            show_outline: false,
            search: MarkdownSearchState::default(),
            document_cache: None,
            image_cache: HashMap::new(),
            mermaid_cache: HashMap::new(),
            pending_footnote_scroll: None,
        }
    }
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

pub(super) fn markdown_side_pane_width(available_width: f32) -> f32 {
    clamp_markdown_side_pane_width(MARKDOWN_SIDE_PANE_DEFAULT_WIDTH, available_width)
}

pub(super) fn clamp_markdown_side_pane_width(width: f32, available_width: f32) -> f32 {
    let max_width = (available_width - MARKDOWN_SIDE_PANE_MIN_REMAINING_WIDTH)
        .clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, MARKDOWN_SIDE_PANE_MAX_WIDTH);
    width.clamp(MARKDOWN_SIDE_PANE_MIN_WIDTH, max_width)
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
