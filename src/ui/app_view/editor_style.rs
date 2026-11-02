//! What the source editors share: syntax highlighting caches and jobs, the line number
//! gutter, colors and font metrics.

use super::*;

impl EditorPane {
    pub(super) fn cached_syntax_highlight_spans(
        &mut self,
        ui: &egui::Ui,
        language_registry: &LanguageRegistry,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<HighlightSpan>> {
        self.poll_syntax_highlight(ui);

        let path = self.path().map(Path::to_path_buf);
        if let Some(cache) = &self.caches.syntax_highlight
            && cache.path == path
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.spans.clone();
        }

        if path.as_deref().is_some_and(is_external_rust_path)
            && source_len > EXTERNAL_RUST_HIGHLIGHT_MAX_BYTES
        {
            let spans = Rc::new(Vec::new());
            self.caches.syntax_highlight = Some(SyntaxHighlightCache {
                path,
                source_len,
                source_hash,
                spans: spans.clone(),
                line_span_ranges: Rc::new(vec![0..0; self.text.line_index().line_count()]),
            });
            return spans;
        }

        if path
            .as_deref()
            .and_then(|path| language_registry.language_for_path(path))
            .is_none()
        {
            let spans = Rc::new(Vec::new());
            self.caches.syntax_highlight = Some(SyntaxHighlightCache {
                path,
                source_len,
                source_hash,
                spans: spans.clone(),
                line_span_ranges: Rc::new(vec![0..0; self.text.line_index().line_count()]),
            });
            return spans;
        }

        let pending_matches =
            self.caches
                .syntax_highlight_pending
                .as_ref()
                .is_some_and(|pending| {
                    pending.path == path
                        && pending.source_len == source_len
                        && pending.source_hash == source_hash
                });
        if !pending_matches {
            self.caches.syntax_highlight_receiver = path.as_ref().map(|path| {
                start_syntax_highlighting(
                    path.clone(),
                    self.text.as_str().to_owned(),
                    language_registry.clone(),
                    source_len,
                    source_hash,
                )
            });
            self.caches.syntax_highlight_pending = Some(SyntaxHighlightPending {
                path: path.clone(),
                source_len,
                source_hash,
            });
        }

        ui.ctx().request_repaint_after(Duration::from_millis(30));
        Rc::new(Vec::new())
    }

    pub(super) fn poll_syntax_highlight(&mut self, ui: &egui::Ui) {
        let Some(receiver) = self.caches.syntax_highlight_receiver.take() else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                let (current_source_len, current_source_hash) = self.text_fingerprint();
                let current_path = self.path().map(Path::to_path_buf);
                let pending_matches =
                    self.caches
                        .syntax_highlight_pending
                        .as_ref()
                        .is_some_and(|pending| {
                            pending.path == result.path
                                && pending.source_len == result.source_len
                                && pending.source_hash == result.source_hash
                        });
                let current_matches = current_path == result.path
                    && current_source_len == result.source_len
                    && current_source_hash == result.source_hash;
                if pending_matches && current_matches {
                    self.caches.syntax_highlight = Some(SyntaxHighlightCache {
                        path: result.path,
                        source_len: result.source_len,
                        source_hash: result.source_hash,
                        line_span_ranges: Rc::new(build_highlight_line_span_ranges(
                            &result.spans,
                            self.text.line_index().as_ref(),
                        )),
                        spans: Rc::new(result.spans),
                    });
                    self.caches.syntax_highlight_pending = None;
                    self.virtual_editor.line_layout_cache = None;
                    ui.ctx().request_repaint();
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.caches.syntax_highlight_receiver = Some(receiver);
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.caches.syntax_highlight_pending = None;
            }
        }
    }

    pub(super) fn cached_syntax_highlight_line_span_ranges(
        &self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<std::ops::Range<usize>>> {
        self.caches
            .syntax_highlight
            .as_ref()
            .filter(|cache| {
                cache.path.as_deref() == self.path()
                    && cache.source_len == source_len
                    && cache.source_hash == source_hash
            })
            .map(|cache| cache.line_span_ranges.clone())
            .unwrap_or_else(|| Rc::new(vec![0..0; self.text.line_index().line_count()]))
    }

    pub(super) fn cached_editor_metrics(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> EditorMetricsCache {
        if let Some(cache) = self.caches.editor_metrics
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache;
        }

        let line_index = self.cached_line_index(source_len, source_hash);

        let cache = EditorMetricsCache {
            source_len,
            source_hash,
            line_count: line_index.line_count(),
            longest_line_chars: line_index.longest_line_chars,
        };
        self.caches.editor_metrics = Some(cache);
        cache
    }

    pub(super) fn cached_line_index(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<LineIndex> {
        let line_index = self.text.line_index();
        debug_assert_eq!(line_index.source_len, source_len);
        debug_assert_eq!(line_index.source_hash, source_hash);
        line_index
    }
}

pub(super) const EDITOR_PAGE_SCROLL_RATIO: f32 = 0.85;

pub(super) const VIRTUAL_SOURCE_MIN_BYTES: usize = 512 * 1024;

pub(super) const VIRTUAL_SOURCE_MIN_LINES: usize = 10_000;

pub(super) const EXTERNAL_RUST_HIGHLIGHT_MAX_BYTES: usize = 512 * 1024;

pub(super) const BOOKMARK_GUTTER_WIDTH: f32 = 18.0;

#[derive(Debug, Clone)]
pub(super) struct SyntaxHighlightCache {
    pub(super) path: Option<PathBuf>,
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) spans: Rc<Vec<HighlightSpan>>,
    pub(super) line_span_ranges: Rc<Vec<std::ops::Range<usize>>>,
}

#[derive(Debug, Clone)]
pub(super) struct SyntaxHighlightPending {
    pub(super) path: Option<PathBuf>,
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
}

#[derive(Debug, Clone)]
pub(super) struct SyntaxHighlightResult {
    pub(super) path: Option<PathBuf>,
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) spans: Vec<HighlightSpan>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EditorMetricsCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) line_count: usize,
    pub(super) longest_line_chars: usize,
}

impl EditorMetricsCache {
    pub(super) fn editor_width(self, ui: &egui::Ui, available_width: f32) -> f32 {
        let char_width = editor_char_width(ui);
        (self.longest_line_chars as f32 * char_width + 64.0).max(available_width)
    }
}

impl LineIndex {
    pub(super) fn editor_width(&self, ui: &egui::Ui, available_width: f32) -> f32 {
        let char_width = editor_char_width(ui);
        (self.longest_line_chars as f32 * char_width + 64.0).max(available_width)
    }
}

pub(super) fn syntax_layout_job(
    ui: &egui::Ui,
    path: Option<&Path>,
    source: &str,
    language_registry: &LanguageRegistry,
    bracket_pair: Option<(usize, usize)>,
) -> LayoutJob {
    let mut spans = path
        .and_then(|path| {
            let language = language_registry.language_for_path(path)?;
            let document = Document::new(path.to_path_buf(), source.to_owned());
            Some(language.highlight_spans(&document))
        })
        .unwrap_or_default();
    spans.sort_by_key(|span| (span.range.start, span.range.end));

    highlighted_layout_job(ui, source, &spans, bracket_pair, &[], None)
}

pub(super) fn build_highlight_line_span_ranges(
    spans: &[HighlightSpan],
    line_index: &LineIndex,
) -> Vec<std::ops::Range<usize>> {
    line_index
        .lines
        .iter()
        .map(|line| {
            let start = spans.partition_point(|span| span.range.end <= line.start);
            let end =
                start + spans[start..].partition_point(|span| span.range.start < line.content_end);
            start..end
        })
        .collect()
}

pub(super) fn highlighted_layout_job(
    ui: &egui::Ui,
    source: &str,
    spans: &[HighlightSpan],
    bracket_pair: Option<(usize, usize)>,
    search_matches: &[SourceSearchMatch],
    active_search_match: Option<usize>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = 0;
    let default_format = text_format(ui, None);

    for span in spans {
        let start = clamp_to_char_boundary(source, span.range.start);
        let end = clamp_to_char_boundary(source, span.range.end);

        if start < cursor || start >= end || end > source.len() {
            continue;
        }

        if cursor < start {
            append_text_segment(
                &mut job,
                source,
                cursor,
                start,
                default_format.clone(),
                LineHighlights {
                    bracket_pair,
                    search_matches,
                    active_search_match,
                },
                ui,
            );
        }

        append_text_segment(
            &mut job,
            source,
            start,
            end,
            text_format(ui, Some(span.kind)),
            LineHighlights {
                bracket_pair,
                search_matches,
                active_search_match,
            },
            ui,
        );
        cursor = end;
    }

    if cursor < source.len() {
        append_text_segment(
            &mut job,
            source,
            cursor,
            source.len(),
            default_format,
            LineHighlights {
                bracket_pair,
                search_matches,
                active_search_match,
            },
            ui,
        );
    }

    job
}

pub(super) fn append_text_segment(
    job: &mut LayoutJob,
    source: &str,
    start: usize,
    end: usize,
    format: TextFormat,
    highlights: LineHighlights<'_>,
    ui: &egui::Ui,
) {
    let LineHighlights {
        bracket_pair,
        search_matches,
        active_search_match,
    } = highlights;

    let mut cursor = start;
    while cursor < end {
        let mut next = end;
        let mut background = None;

        if let Some((left, right)) = bracket_pair {
            for bracket in [left, right] {
                if bracket < cursor || bracket >= end {
                    continue;
                }
                if cursor < bracket {
                    next = next.min(bracket);
                } else {
                    next = next.min(next_char_boundary(source, bracket));
                    background.get_or_insert_with(|| matching_bracket_background(ui));
                }
            }
        }

        let search_index =
            search_matches.partition_point(|search_match| search_match.end <= cursor);
        if let Some(search_match) = search_matches.get(search_index)
            && search_match.start < end
        {
            if cursor < search_match.start {
                next = next.min(search_match.start);
            } else if cursor < search_match.end {
                next = next.min(search_match.end);
                background = Some(if active_search_match == Some(search_index) {
                    active_source_search_match_background(ui)
                } else {
                    source_search_match_background(ui)
                });
            }
        }

        if next <= cursor {
            next = next_char_boundary(source, cursor);
        }

        let mut next_format = format.clone();
        if let Some(background) = background {
            next_format.background = background;
        }
        job.append(&source[cursor..next], 0.0, next_format);
        cursor = next;
    }
}

pub(super) fn text_format(ui: &egui::Ui, kind: Option<HighlightKind>) -> TextFormat {
    let color = match kind {
        Some(HighlightKind::Comment) => Color32::from_rgb(0, 128, 0),
        Some(HighlightKind::String) => Color32::from_rgb(163, 21, 21),
        Some(HighlightKind::Number) => Color32::from_rgb(9, 134, 88),
        Some(HighlightKind::Keyword) => Color32::from_rgb(0, 0, 255),
        Some(HighlightKind::Function) => Color32::from_rgb(121, 94, 38),
        Some(HighlightKind::Type) => Color32::from_rgb(38, 127, 153),
        None => ui.visuals().text_color(),
    };

    TextFormat {
        font_id: editor_font_id(),
        color,
        ..Default::default()
    }
}

pub(super) fn allocate_line_number_gutter(ui: &mut egui::Ui, line_count: usize) -> egui::Rect {
    let row_height = editor_scroll_row_height(ui);
    let visible_rows = line_count.max(TEXTEDIT_SOURCE_MIN_ROWS);
    let gutter_width = line_number_gutter_width(ui, line_count);
    let height = visible_rows as f32 * row_height;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(gutter_width, height), egui::Sense::hover());

    rect
}

pub(super) fn line_number_gutter_width(ui: &egui::Ui, line_count: usize) -> f32 {
    let row_height = editor_scroll_row_height(ui);
    let digits = line_count.max(1).to_string().len();
    (digits as f32 * row_height * 0.6 + 22.0 + BOOKMARK_GUTTER_WIDTH).max(56.0)
}

pub(super) fn paint_line_number_gutter(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    line_count: usize,
    current_line: Option<u32>,
    bookmark_lines: &[u32],
    output: &egui::text_edit::TextEditOutput,
    viewport_rect: egui::Rect,
) -> Option<u32> {
    let painter = ui.painter_at(rect.intersect(viewport_rect));
    let font_id = editor_font_id();
    let inactive_color = ui.visuals().weak_text_color();
    let active_color = ui.visuals().text_color();
    let bookmark_color = ui.visuals().selection.bg_fill;
    let mut clicked_bookmark_line = None;

    for index in visible_galley_row_range(output, line_count, viewport_rect) {
        let row_rect = galley_line_rect(output, index);
        let line_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), row_rect.top()),
            egui::vec2(rect.width(), row_rect.height()),
        )
        .intersect(viewport_rect);

        if current_line == Some(index as u32) {
            painter.rect_filled(line_rect, 0.0, current_line_background(ui));
        }

        let bookmark_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), line_rect.top()),
            egui::pos2(
                (rect.left() + BOOKMARK_GUTTER_WIDTH).min(rect.right()),
                line_rect.bottom(),
            ),
        );
        let bookmark_response = ui.interact(
            bookmark_rect,
            output.response.id.with(("bookmark_gutter", index)),
            egui::Sense::click(),
        );
        let bookmarked = bookmark_lines.contains(&(index as u32));
        if bookmarked || bookmark_response.hovered() || current_line == Some(index as u32) {
            painter.text(
                bookmark_rect.center(),
                egui::Align2::CENTER_CENTER,
                if bookmarked { "★" } else { "☆" },
                bookmark_gutter_font_id(line_rect.height()),
                if bookmarked {
                    bookmark_color
                } else {
                    inactive_color
                },
            );
        }
        if bookmark_response.clicked() {
            clicked_bookmark_line = Some(index as u32);
        }

        painter.text(
            egui::pos2(rect.right() - 8.0, row_rect.top()),
            egui::Align2::RIGHT_TOP,
            (index + 1).to_string(),
            font_id.clone(),
            if current_line == Some(index as u32) {
                active_color
            } else {
                inactive_color
            },
        );
    }

    painter.line_segment(
        [
            egui::pos2(rect.right(), viewport_rect.top()),
            egui::pos2(rect.right(), viewport_rect.bottom()),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    clicked_bookmark_line
}

pub(super) fn current_line_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(45, 45, 48)
    } else {
        Color32::from_rgb(232, 242, 255)
    }
}

pub(super) fn current_line_overlay(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(90, 140, 220, 48)
    } else {
        Color32::from_rgba_unmultiplied(80, 150, 255, 44)
    }
}

pub(super) fn indent_guide_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(190, 190, 190, 42)
    } else {
        Color32::from_rgba_unmultiplied(80, 80, 80, 38)
    }
}

pub(super) fn active_indent_guide_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(220, 220, 220, 72)
    } else {
        Color32::from_rgba_unmultiplied(45, 45, 45, 70)
    }
}

pub(super) fn source_search_match_background(ui: &egui::Ui) -> Color32 {
    markdown_search_match_background(ui)
}

pub(super) fn active_source_search_match_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(128, 92, 24)
    } else {
        Color32::from_rgb(255, 190, 80)
    }
}

pub(super) fn matching_bracket_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(75, 75, 84)
    } else {
        Color32::from_rgb(210, 226, 255)
    }
}

pub(super) fn editor_font_id() -> FontId {
    FontId::new(14.0, egui::FontFamily::Monospace)
}

pub(super) fn bookmark_gutter_font_id(row_height: f32) -> FontId {
    FontId::new(
        (row_height * 1.25).clamp(18.0, 24.0),
        egui::FontFamily::Proportional,
    )
}

pub(super) fn editor_row_height(ui: &egui::Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.row_height(&editor_font_id()))
}

pub(super) fn editor_scroll_row_height(ui: &egui::Ui) -> f32 {
    editor_row_height(ui).ceil()
}

pub(super) fn editor_char_width(ui: &egui::Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.glyph_width(&editor_font_id(), ' '))
}

pub(super) fn should_use_virtual_source_view(source_len: usize, line_count: usize) -> bool {
    source_len >= VIRTUAL_SOURCE_MIN_BYTES || line_count >= VIRTUAL_SOURCE_MIN_LINES
}

pub(super) fn editor_page_scroll_delta(
    ui: &egui::Ui,
    focused: bool,
    text_edit_id: egui::Id,
    viewport_height: f32,
) -> Option<f32> {
    if !focused {
        return None;
    }

    let editor_can_handle_key =
        ui.memory(|memory| memory.focused().is_none() || memory.has_focus(text_edit_id));
    if !editor_can_handle_key {
        return None;
    }

    let amount = (viewport_height * EDITOR_PAGE_SCROLL_RATIO).max(editor_row_height(ui));
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown) {
            Some(amount)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp) {
            Some(-amount)
        } else {
            None
        }
    })
}

pub(super) fn start_syntax_highlighting(
    path: PathBuf,
    text: String,
    language_registry: LanguageRegistry,
    source_len: usize,
    source_hash: u64,
) -> Receiver<SyntaxHighlightResult> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let mut spans = language_registry
            .language_for_path(&path)
            .map(|language| {
                let document = Document::new(path.clone(), text);
                language.highlight_spans(&document)
            })
            .unwrap_or_default();
        spans.sort_by_key(|span| (span.range.start, span.range.end));

        let _ = sender.send(SyntaxHighlightResult {
            path: Some(path),
            source_len,
            source_hash,
            spans,
        });
    });

    receiver
}

/// Left-aligned text clipped to one cell of a list row.
pub(super) fn paint_row_text(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    font: &FontId,
    color: Color32,
) {
    ui.painter().with_clip_rect(rect).text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        font.clone(),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::build_line_index;

    #[test]
    fn highlight_line_span_ranges_map_spans_to_lines() {
        let source = "fn main() {\n    value\n}\n";
        let line_index = build_line_index(source, source.len(), stable_hash(source));
        let value_start = source.find("value").unwrap();
        let spans = vec![
            HighlightSpan::new(0..2, HighlightKind::Keyword),
            HighlightSpan::new(
                value_start..value_start + "value".len(),
                HighlightKind::Function,
            ),
        ];

        let ranges = build_highlight_line_span_ranges(&spans, &line_index);

        assert_eq!(ranges[0], 0..1);
        assert_eq!(ranges[1], 1..2);
        assert_eq!(ranges[2], 2..2);
    }
}
