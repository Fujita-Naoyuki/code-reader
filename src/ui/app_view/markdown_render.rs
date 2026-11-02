//! Rendering Markdown nodes in the preview: blocks, inline runs, tables, code, Mermaid,
//! images and footnotes.

use super::*;

impl EditorPane {
    pub(super) fn markdown_render_source_fragment_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        fragment_start_offset: usize,
        source: &str,
        search_query: Option<&str>,
    ) {
        // A fragment is parsed on its own, so add the definitions it does not contain;
        // otherwise its reference links and footnote markers stay literal text.
        let definitions = markdown_definition_sources_outside(
            document,
            fragment_start_offset..fragment_start_offset + source.len(),
        );
        let source_with_definitions;
        let parse_source = if definitions.is_empty() {
            source
        } else {
            source_with_definitions = format!("{source}\n\n{definitions}");
            source_with_definitions.as_str()
        };
        let Ok(ast) = markdown::to_mdast(parse_source, &ParseOptions::gfm()) else {
            let text = MarkdownDocument::rendered_text_for_source(source);
            if !text.trim().is_empty() {
                let job = markdown_plain_text_layout_job(
                    ui,
                    text.trim(),
                    search_query,
                    markdown_inline_format(ui, MarkdownInlineStyle::default()),
                );
                let response = ui.add(egui::Label::new(job).wrap());
                if self.should_scroll_to_markdown_search_match(search_query, text.trim()) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
            }
            return;
        };

        self.markdown_render_node_ui(
            ui,
            document,
            language_registry,
            &ast,
            fragment_start_offset,
            0,
            0,
            search_query,
        );
    }

    pub(super) fn markdown_render_nodes_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        nodes: &[Node],
        fragment_start_offset: usize,
        depth: usize,
        ordered_start: u32,
        search_query: Option<&str>,
    ) {
        for (index, node) in nodes.iter().enumerate() {
            self.markdown_render_node_ui(
                ui,
                document,
                language_registry,
                node,
                fragment_start_offset,
                depth,
                ordered_start + index as u32,
                search_query,
            );
        }
    }

    pub(super) fn markdown_render_node_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        node: &Node,
        fragment_start_offset: usize,
        depth: usize,
        list_index: u32,
        search_query: Option<&str>,
    ) {
        match node {
            Node::Root(root) => self.markdown_render_nodes_ui(
                ui,
                document,
                language_registry,
                &root.children,
                fragment_start_offset,
                depth,
                0,
                search_query,
            ),
            Node::Paragraph(paragraph) => {
                let plain_text = markdown_inline_plain_text(&paragraph.children);
                if markdown_nodes_need_widgets(&paragraph.children) {
                    let response = ui
                        .horizontal_wrapped(|ui| {
                            self.markdown_inline_widgets_ui(
                                ui,
                                document,
                                &paragraph.children,
                                MarkdownInlineStyle::default(),
                                search_query,
                            );
                        })
                        .response;
                    if self.should_scroll_to_markdown_search_match(search_query, &plain_text) {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    ui.add_space(4.0);
                } else {
                    let job = markdown_inline_layout_job(ui, &paragraph.children, search_query);
                    if !job.text.trim().is_empty() {
                        let response = ui.add(egui::Label::new(job).wrap());
                        if self.should_scroll_to_markdown_search_match(search_query, &plain_text) {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        ui.add_space(4.0);
                    }
                }
            }
            Node::Heading(heading) => {
                let title = markdown_inline_plain_text(&heading.children);
                let job = markdown_plain_text_layout_job(
                    ui,
                    &title,
                    search_query,
                    markdown_heading_text_format(ui, heading.depth),
                );
                let response = ui.add(egui::Label::new(job).wrap());
                if self.should_scroll_to_markdown_search_match(search_query, &title) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                ui.add_space(4.0);
            }
            Node::Blockquote(blockquote) => {
                egui::Frame::new()
                    .fill(markdown_blockquote_background(ui))
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &blockquote.children,
                            fragment_start_offset,
                            depth + 1,
                            0,
                            search_query,
                        );
                    });
                ui.add_space(6.0);
            }
            Node::List(list) => {
                let start = list.start.unwrap_or(1);
                for (index, child) in list.children.iter().enumerate() {
                    let marker = if list.ordered {
                        start + index as u32
                    } else {
                        0
                    };
                    self.markdown_render_node_ui(
                        ui,
                        document,
                        language_registry,
                        child,
                        fragment_start_offset,
                        depth,
                        marker,
                        search_query,
                    );
                }
                ui.add_space(4.0);
            }
            Node::ListItem(item) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    if let Some(checked) = item.checked {
                        let mut checked = checked;
                        ui.add_enabled(false, egui::Checkbox::new(&mut checked, ""));
                    } else if list_index > 0 {
                        ui.label(RichText::new(format!("{list_index}.")).monospace());
                    } else {
                        ui.label(RichText::new("-").monospace());
                    }
                    ui.vertical(|ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &item.children,
                            fragment_start_offset,
                            depth + 1,
                            0,
                            search_query,
                        );
                    });
                });
            }
            Node::ThematicBreak(_) => {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 10.0),
                    egui::Sense::hover(),
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left(), rect.center().y),
                        egui::pos2(rect.right(), rect.center().y),
                    ],
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );
                ui.add_space(6.0);
            }
            Node::Code(code) => {
                let code_id = markdown_node_ui_id(fragment_start_offset, node, "code");
                if code.lang.as_deref().is_some_and(is_mermaid_language) {
                    self.markdown_mermaid_block_ui(ui, language_registry, &code_id, &code.value);
                } else {
                    self.markdown_code_block_ui(
                        ui,
                        language_registry,
                        &code_id,
                        code.lang.as_deref(),
                        &code.value,
                    );
                }
            }
            Node::Table(table) => {
                let table_id = markdown_node_ui_id(fragment_start_offset, node, "table");
                self.markdown_table_ui(ui, &table_id, &table.children, &table.align, search_query);
            }
            Node::Image(image) => {
                self.markdown_image_ui(ui, document, &image.url, &image.alt);
            }
            Node::ImageReference(image) => {
                self.markdown_image_ui(ui, document, &image.identifier, &image.alt);
            }
            Node::Html(html) => {
                if !html.value.trim().is_empty() {
                    ui.label(RichText::new(&html.value).monospace().weak());
                    ui.add_space(4.0);
                }
            }
            Node::FootnoteDefinition(_) => {}
            _ => {
                if let Some(children) = node.children() {
                    self.markdown_render_nodes_ui(
                        ui,
                        document,
                        language_registry,
                        children,
                        fragment_start_offset,
                        depth,
                        0,
                        search_query,
                    );
                }
            }
        }
    }

    pub(super) fn markdown_inline_widgets_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        children: &[Node],
        style: MarkdownInlineStyle,
        search_query: Option<&str>,
    ) {
        for child in children {
            match child {
                Node::Text(text) => {
                    let job = markdown_plain_text_layout_job(
                        ui,
                        &text.value,
                        search_query,
                        markdown_inline_format(ui, style),
                    );
                    if !job.text.is_empty() {
                        ui.add(egui::Label::new(job).wrap());
                    }
                }
                Node::InlineCode(code) => {
                    let mut format = markdown_inline_format(ui, style);
                    format.font_id = editor_font_id();
                    format.background = markdown_code_background(ui);
                    format.expand_bg = 3.0;
                    let job = markdown_plain_text_layout_job(ui, &code.value, search_query, format);
                    ui.add(egui::Label::new(job).wrap());
                }
                Node::Break(_) => {
                    ui.end_row();
                }
                Node::Emphasis(emphasis) => {
                    let mut style = style;
                    style.emphasis = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &emphasis.children,
                        style,
                        search_query,
                    );
                }
                Node::Strong(strong) => {
                    let mut style = style;
                    style.strong = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &strong.children,
                        style,
                        search_query,
                    );
                }
                Node::Delete(delete) => {
                    let mut style = style;
                    style.delete = true;
                    self.markdown_inline_widgets_ui(
                        ui,
                        document,
                        &delete.children,
                        style,
                        search_query,
                    );
                }
                Node::Link(link) => {
                    let label = markdown_inline_plain_text(&link.children);
                    let label = if label.trim().is_empty() {
                        link.url.as_str()
                    } else {
                        label.as_str()
                    };
                    let mut job = LayoutJob::default();
                    let mut link_style = style;
                    link_style.link = true;
                    append_markdown_text_with_search(
                        ui,
                        &mut job,
                        label,
                        markdown_inline_format(ui, link_style),
                        search_query,
                    );
                    let response = ui
                        .add(egui::Label::new(job).sense(egui::Sense::click()).wrap())
                        .on_hover_text(&link.url);
                    if response.hovered() {
                        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        self.activate_markdown_link(document, &link.url);
                    }
                }
                Node::LinkReference(link) => {
                    let label = markdown_inline_plain_text(&link.children);
                    let mut job = LayoutJob::default();
                    let mut link_style = style;
                    link_style.link = true;
                    append_markdown_text_with_search(
                        ui,
                        &mut job,
                        &label,
                        markdown_inline_format(ui, link_style),
                        search_query,
                    );
                    let response = ui
                        .add(egui::Label::new(job).sense(egui::Sense::click()).wrap())
                        .on_hover_text(&link.identifier);
                    if response.hovered() {
                        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        match markdown_definition_url(document, &link.identifier) {
                            Some(url) => self.activate_markdown_link(document, &url),
                            None => {
                                self.pending_action = Some(EditorAction::ShowStatus(format!(
                                    "Reference link has no definition: {}",
                                    link.identifier
                                )));
                            }
                        }
                    }
                }
                Node::Image(image) => {
                    self.markdown_image_ui(ui, document, &image.url, &image.alt);
                }
                Node::ImageReference(image) => {
                    self.markdown_image_ui(ui, document, &image.identifier, &image.alt);
                }
                Node::FootnoteReference(footnote) => {
                    let number = markdown_footnote_number(document, &footnote.identifier)
                        .map(|number| number.to_string())
                        .unwrap_or_else(|| {
                            footnote
                                .label
                                .clone()
                                .unwrap_or_else(|| footnote.identifier.clone())
                        });
                    let response = ui
                        .link(RichText::new(format!("[{number}]")).small().raised())
                        .on_hover_text(format!("Jump to footnote {}", footnote.identifier));
                    if response.clicked() {
                        self.markdown.pending_footnote_scroll = Some(
                            MarkdownFootnoteScrollTarget::Definition(footnote.identifier.clone()),
                        );
                    }
                    if self
                        .markdown.pending_footnote_scroll
                        .as_ref()
                        .is_some_and(|target| matches!(target, MarkdownFootnoteScrollTarget::Reference(identifier) if identifier == &footnote.identifier))
                    {
                        response.scroll_to_me(Some(egui::Align::Center));
                        self.markdown.pending_footnote_scroll = None;
                    }
                }
                Node::InlineMath(math) => {
                    let mut format = markdown_inline_format(ui, style);
                    format.font_id = editor_font_id();
                    let job = markdown_plain_text_layout_job(ui, &math.value, search_query, format);
                    ui.add(egui::Label::new(job).wrap());
                }
                _ => {
                    if let Some(children) = child.children() {
                        self.markdown_inline_widgets_ui(
                            ui,
                            document,
                            children,
                            style,
                            search_query,
                        );
                    }
                }
            }
        }
    }

    pub(super) fn activate_markdown_link(&mut self, document: &MarkdownDocument, target: &str) {
        let target = target.trim();
        if target.is_empty() {
            self.pending_action = Some(EditorAction::ShowStatus(
                "Markdown link target is empty.".to_owned(),
            ));
            return;
        }
        if is_external_markdown_link(target) {
            self.pending_action = Some(EditorAction::OpenExternalUrl(target.to_owned()));
            return;
        }

        let (path_part, fragment) = split_markdown_link_target(target);
        let path = if path_part.is_empty() {
            document.path.clone()
        } else {
            resolve_markdown_link_path(&document.path, &path_part)
        };

        self.pending_action = Some(EditorAction::OpenMarkdownLink { path, fragment });
    }

    pub(super) fn markdown_code_block_ui(
        &mut self,
        ui: &mut egui::Ui,
        language_registry: &LanguageRegistry,
        block_id: &str,
        language: Option<&str>,
        code: &str,
    ) {
        egui::Frame::new()
            .fill(markdown_code_background(ui))
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(language.unwrap_or("code"))
                            .small()
                            .monospace()
                            .weak(),
                    );
                    let copy_response = ui.small_button("Copy").on_hover_text("Copy code block");
                    if copy_response.clicked()
                        || (copy_response.has_focus() && ui.input_mut(consume_copy_shortcut))
                    {
                        copy_code_block_to_clipboard(ui, code);
                    }
                });
                ui.separator();
                ScrollArea::horizontal()
                    .id_salt(format!("markdown_code_{:?}_{block_id}", self.pane))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let path = language.and_then(markdown_code_path_for_language);
                        let mut job =
                            syntax_layout_job(ui, path.as_deref(), code, language_registry, None);
                        job.wrap.max_width = f32::INFINITY;
                        ui.label(job);
                    });
            });
        ui.add_space(8.0);
    }

    pub(super) fn markdown_mermaid_block_ui(
        &mut self,
        ui: &mut egui::Ui,
        language_registry: &LanguageRegistry,
        block_id: &str,
        code: &str,
    ) {
        let render_result = self
            .markdown
            .mermaid_cache
            .entry(code.to_owned())
            .or_insert_with(|| render_mermaid_svg(code))
            .clone();

        let Ok(svg) = render_result else {
            self.markdown_code_block_ui(ui, language_registry, block_id, Some("mermaid"), code);
            if let Some(Err(error)) = self.markdown.mermaid_cache.get(code) {
                ui.label(
                    RichText::new(format!("Mermaid rendering failed: {error}"))
                        .small()
                        .weak(),
                );
            }
            return;
        };

        egui::Frame::new()
            .fill(markdown_code_background(ui))
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("mermaid").small().monospace().weak());
                    let copy_response =
                        ui.small_button("Copy").on_hover_text("Copy Mermaid source");
                    if copy_response.clicked()
                        || (copy_response.has_focus() && ui.input_mut(consume_copy_shortcut))
                    {
                        copy_code_block_to_clipboard(ui, code);
                    }
                });
                ui.separator();
                ScrollArea::horizontal()
                    .id_salt(format!("markdown_mermaid_{:?}_{block_id}", self.pane))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let uri = format!("bytes://markdown_mermaid_{}.svg", stable_hash(&svg));
                        ui.add(egui::Image::from_bytes(uri, svg.into_bytes()));
                    });
            });
        ui.add_space(8.0);
    }

    pub(super) fn markdown_footnotes_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        language_registry: &LanguageRegistry,
        page: &crate::core::markdown::MarkdownPage,
        search_query: Option<&str>,
    ) {
        let definitions = markdown_footnote_definitions_for_page(document, page);
        if definitions.is_empty() {
            return;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("Footnotes").strong());
        ui.add_space(4.0);

        for definition in definitions {
            let number = markdown_footnote_number(document, &definition.identifier).unwrap_or(0);
            let response = ui
                .horizontal_top(|ui| {
                    ui.label(RichText::new(format!("{number}.")).small().monospace());
                    ui.vertical(|ui| {
                        self.markdown_render_nodes_ui(
                            ui,
                            document,
                            language_registry,
                            &definition.children,
                            definition
                                .position
                                .as_ref()
                                .map(|position| position.start.offset)
                                .unwrap_or(page.source_range.start.offset),
                            0,
                            0,
                            search_query,
                        );
                    });
                    if ui
                        .small_button("Back")
                        .on_hover_text("Back to footnote reference")
                        .clicked()
                    {
                        self.markdown.pending_footnote_scroll = Some(
                            MarkdownFootnoteScrollTarget::Reference(definition.identifier.clone()),
                        );
                    }
                })
                .response;
            if self
                .markdown.pending_footnote_scroll
                .as_ref()
                .is_some_and(|target| matches!(target, MarkdownFootnoteScrollTarget::Definition(identifier) if identifier == &definition.identifier))
            {
                response.scroll_to_me(Some(egui::Align::Center));
                self.markdown.pending_footnote_scroll = None;
            }
        }
        ui.add_space(8.0);
    }

    pub(super) fn markdown_table_ui(
        &self,
        ui: &mut egui::Ui,
        table_id: &str,
        rows: &[Node],
        align: &[AlignKind],
        search_query: Option<&str>,
    ) {
        ScrollArea::horizontal()
            .id_salt(format!("markdown_table_{:?}_{table_id}", self.pane))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new(format!("markdown_table_grid_{:?}_{table_id}", self.pane))
                    .striped(true)
                    .spacing(egui::vec2(14.0, 6.0))
                    .show(ui, |ui| {
                        for (row_index, row) in rows.iter().enumerate() {
                            let Node::TableRow(row) = row else {
                                continue;
                            };
                            for (cell_index, cell) in row.children.iter().enumerate() {
                                let Node::TableCell(cell) = cell else {
                                    continue;
                                };
                                let mut job =
                                    markdown_inline_layout_job(ui, &cell.children, search_query);
                                if row_index == 0 {
                                    for section in &mut job.sections {
                                        section.format.color = ui.visuals().strong_text_color();
                                    }
                                }
                                ui.with_layout(
                                    markdown_table_cell_layout(
                                        align.get(cell_index).copied().unwrap_or(AlignKind::None),
                                    ),
                                    |ui| {
                                        ui.label(job);
                                    },
                                );
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(8.0);
    }

    pub(super) fn markdown_image_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        url: &str,
        alt: &str,
    ) {
        let Some(path) = markdown_resolve_local_image_path(document, url) else {
            ui.label(RichText::new(format!("[image: {alt}] {url}")).weak());
            return;
        };

        let result = self
            .markdown
            .image_cache
            .entry(path.clone())
            .or_insert_with(|| load_markdown_image_texture(ui.ctx(), &path));

        match result {
            Ok(texture) => {
                let response = ui.add(
                    egui::Image::from_texture(&*texture)
                        .max_width(ui.available_width())
                        .maintain_aspect_ratio(true),
                );
                if !alt.trim().is_empty() {
                    response.on_hover_text(alt);
                }
            }
            Err(error) => {
                ui.label(RichText::new(format!("[image: {alt}] {error}")).weak());
            }
        }
        ui.add_space(6.0);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct MarkdownInlineStyle {
    pub(super) emphasis: bool,
    pub(super) strong: bool,
    pub(super) link: bool,
    pub(super) delete: bool,
}

pub(super) fn markdown_inline_layout_job(
    ui: &egui::Ui,
    children: &[Node],
    search_query: Option<&str>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for child in children {
        append_markdown_inline(
            ui,
            &mut job,
            child,
            MarkdownInlineStyle::default(),
            search_query,
        );
    }
    job.wrap.max_width = ui.available_width();
    job
}

pub(super) fn markdown_plain_text_layout_job(
    ui: &egui::Ui,
    text: &str,
    search_query: Option<&str>,
    format: TextFormat,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    append_markdown_text_with_search(ui, &mut job, text, format, search_query);
    job.wrap.max_width = ui.available_width();
    job
}

pub(super) fn append_markdown_inline(
    ui: &egui::Ui,
    job: &mut LayoutJob,
    node: &Node,
    style: MarkdownInlineStyle,
    search_query: Option<&str>,
) {
    match node {
        Node::Text(text) => append_markdown_text_with_search(
            ui,
            job,
            &text.value,
            markdown_inline_format(ui, style),
            search_query,
        ),
        Node::InlineCode(code) => {
            let mut format = markdown_inline_format(ui, style);
            format.font_id = editor_font_id();
            format.background = markdown_code_background(ui);
            format.expand_bg = 3.0;
            append_markdown_text_with_search(ui, job, &code.value, format, search_query);
        }
        Node::Break(_) => job.append("\n", 0.0, markdown_inline_format(ui, style)),
        Node::Emphasis(emphasis) => {
            let mut style = style;
            style.emphasis = true;
            for child in &emphasis.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Strong(strong) => {
            let mut style = style;
            style.strong = true;
            for child in &strong.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Delete(delete) => {
            let mut style = style;
            style.delete = true;
            for child in &delete.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Link(link) => {
            let mut style = style;
            style.link = true;
            for child in &link.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
            let mut format = markdown_inline_format(ui, style);
            format.color = ui.visuals().weak_text_color();
            job.append(&format!(" ({})", link.url), 0.0, format);
        }
        Node::LinkReference(link) => {
            let mut style = style;
            style.link = true;
            for child in &link.children {
                append_markdown_inline(ui, job, child, style, search_query);
            }
        }
        Node::Image(image) => {
            job.append(
                &format!("[image: {}]", image.alt),
                0.0,
                markdown_inline_format(ui, style),
            );
        }
        Node::ImageReference(image) => {
            job.append(
                &format!("[image: {}]", image.alt),
                0.0,
                markdown_inline_format(ui, style),
            );
        }
        Node::InlineMath(math) => {
            let mut format = markdown_inline_format(ui, style);
            format.font_id = editor_font_id();
            job.append(&math.value, 0.0, format);
        }
        _ => {
            if let Some(children) = node.children() {
                for child in children {
                    append_markdown_inline(ui, job, child, style, search_query);
                }
            }
        }
    }
}

pub(super) fn append_markdown_text_with_search(
    ui: &egui::Ui,
    job: &mut LayoutJob,
    text: &str,
    format: TextFormat,
    search_query: Option<&str>,
) {
    let Some(query) = search_query.filter(|query| !query.is_empty()) else {
        job.append(text, 0.0, format);
        return;
    };

    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut cursor = 0;

    while let Some(relative_start) = text_lower[cursor..].find(&query_lower) {
        let start = cursor + relative_start;
        let end = start + query_lower.len();
        let start = clamp_to_char_boundary(text, start.min(text.len()));
        let end = clamp_to_char_boundary(text, end.min(text.len()));

        if cursor < start {
            job.append(&text[cursor..start], 0.0, format.clone());
        }

        let mut match_format = format.clone();
        match_format.background = markdown_search_match_background(ui);
        match_format.expand_bg = 3.0;
        match_format.color = ui.visuals().strong_text_color();
        job.append(&text[start..end], 0.0, match_format);
        cursor = end.max(start + 1).min(text.len());
    }

    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, format);
    }
}

pub(super) fn markdown_inline_format(ui: &egui::Ui, style: MarkdownInlineStyle) -> TextFormat {
    let mut format = TextFormat {
        font_id: FontId::new(14.0, egui::FontFamily::Proportional),
        color: if style.strong {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        },
        ..Default::default()
    };
    format.italics = style.emphasis;
    if style.link {
        format.color = Color32::from_rgb(56, 132, 214);
        format.underline = egui::Stroke::new(1.0, format.color);
    }
    if style.delete {
        format.strikethrough = egui::Stroke::new(1.0, ui.visuals().weak_text_color());
    }
    format
}

pub(super) fn markdown_heading_text_format(ui: &egui::Ui, depth: u8) -> TextFormat {
    TextFormat {
        font_id: FontId::new(markdown_heading_size(depth), egui::FontFamily::Proportional),
        color: ui.visuals().strong_text_color(),
        ..Default::default()
    }
}

pub(super) fn markdown_inline_plain_text(children: &[Node]) -> String {
    let mut text = String::new();
    for child in children {
        match child {
            Node::Text(node) => text.push_str(&node.value),
            Node::InlineCode(node) => text.push_str(&node.value),
            Node::InlineMath(node) => text.push_str(&node.value),
            Node::Break(_) => text.push('\n'),
            Node::Image(node) => text.push_str(&node.alt),
            Node::ImageReference(node) => text.push_str(&node.alt),
            _ => {
                if let Some(children) = child.children() {
                    text.push_str(&markdown_inline_plain_text(children));
                }
            }
        }
    }
    text
}

pub(super) fn markdown_nodes_need_widgets(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Link(_)
        | Node::LinkReference(_)
        | Node::Image(_)
        | Node::ImageReference(_)
        | Node::FootnoteReference(_) => true,
        _ => node
            .children()
            .is_some_and(|children| markdown_nodes_need_widgets(children)),
    })
}

pub(super) fn markdown_heading_size(depth: u8) -> f32 {
    match depth {
        1 => 24.0,
        2 => 21.0,
        3 => 18.0,
        4 => 16.0,
        5 => 15.0,
        _ => 14.0,
    }
}

pub(super) fn markdown_table_cell_layout(align: AlignKind) -> egui::Layout {
    match align {
        AlignKind::Right => egui::Layout::right_to_left(egui::Align::Center),
        AlignKind::Center => egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        AlignKind::Left | AlignKind::None => egui::Layout::left_to_right(egui::Align::Center),
    }
}

pub(super) fn markdown_node_ui_id(
    fragment_start_offset: usize,
    node: &Node,
    fallback_kind: &str,
) -> String {
    node.position()
        .map(|position| {
            format!(
                "{}_{}_{}",
                fallback_kind,
                fragment_start_offset + position.start.offset,
                fragment_start_offset + position.end.offset
            )
        })
        .unwrap_or_else(|| format!("{fallback_kind}_{fragment_start_offset}"))
}

pub(super) fn copy_code_block_to_clipboard(ui: &egui::Ui, code: &str) {
    ui.ctx().copy_text(code.to_owned());
    let _ = copy_text_to_system_clipboard(code);
}

pub(super) fn load_markdown_image_texture(
    context: &egui::Context,
    path: &Path,
) -> Result<egui::TextureHandle, String> {
    if !path.exists() {
        return Err(format!("not found: {}", path.display()));
    }

    let image = image::ImageReader::open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("decode {}: {error}", path.display()))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

    Ok(context.load_texture(
        format!("markdown_image:{}", path.display()),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

pub(super) fn markdown_code_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(36, 38, 42)
    } else {
        Color32::from_rgb(244, 246, 248)
    }
}

pub(super) fn markdown_blockquote_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(38, 42, 46)
    } else {
        Color32::from_rgb(248, 249, 250)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_node_ui_id_stays_stable_across_reparses() {
        let source = "| A | B |\n| - | - |\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n";
        let ast_a = markdown::to_mdast(source, &ParseOptions::gfm()).unwrap();
        let ast_b = markdown::to_mdast(source, &ParseOptions::gfm()).unwrap();
        let children_a = ast_a.children().unwrap();
        let children_b = ast_b.children().unwrap();

        assert!(matches!(children_a.first(), Some(Node::Table(_))));
        assert!(matches!(children_a.get(1), Some(Node::Code(_))));
        assert_eq!(
            markdown_node_ui_id(120, &children_a[0], "table"),
            markdown_node_ui_id(120, &children_b[0], "table")
        );
        assert_eq!(
            markdown_node_ui_id(120, &children_a[1], "code"),
            markdown_node_ui_id(120, &children_b[1], "code")
        );
    }
}
