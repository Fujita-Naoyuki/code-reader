//! Painting the large-file editor's pieces: indent guides, selection, cursor, IME text,
//! highlighted lines, and its scrollbars.

use super::*;

/// Where the large-file editor's text sits on screen this frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct VirtualViewport {
    /// The text area without the scrollbars.
    pub(super) content_rect: egui::Rect,
    pub(super) text_clip_rect: egui::Rect,
    pub(super) text_left: f32,
    pub(super) row_height: f32,
    pub(super) char_width: f32,
    pub(super) scroll_x: f32,
    pub(super) scroll_y: f32,
}

/// The brackets and search matches a line's layout has to mark.
#[derive(Clone, Copy)]
pub(super) struct LineHighlights<'a> {
    pub(super) bracket_pair: Option<(usize, usize)>,
    pub(super) search_matches: &'a [SourceSearchMatch],
    pub(super) active_search_match: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct VirtualScrollbarLayout {
    pub(super) content_rect: egui::Rect,
    pub(super) vertical_rect: Option<egui::Rect>,
    pub(super) horizontal_rect: Option<egui::Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VirtualIndentGuideLine {
    pub(super) line_number: usize,
    pub(super) guide_count: usize,
    pub(super) active: bool,
}

pub(super) fn virtual_indent_guide_lines(
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    visible_range: std::ops::Range<usize>,
    tab_width: usize,
    current_line: Option<u32>,
) -> Vec<VirtualIndentGuideLine> {
    let tab_width = tab_width.max(1);
    visible_range
        .filter_map(|line_number| {
            let line_info = line_index.line(line_number)?;
            if line_info.is_blank {
                return None;
            }

            let line = text.line_text_or_empty(line_info);
            let leading_len = line_info
                .leading_end
                .saturating_sub(line_info.start)
                .min(line.len());
            let indent_columns = leading_indent_columns(&line[..leading_len], tab_width);
            let guide_count = indent_columns / tab_width;
            (guide_count > 0).then_some(VirtualIndentGuideLine {
                line_number,
                guide_count,
                active: current_line == Some(line_number as u32),
            })
        })
        .collect()
}

pub(super) fn paint_virtual_indent_guides(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    visible_range: std::ops::Range<usize>,
    tab_width: usize,
    current_line: Option<u32>,
    viewport: &VirtualViewport,
) {
    let VirtualViewport {
        text_clip_rect: clip_rect,
        text_left,
        row_height,
        scroll_y,
        ..
    } = *viewport;

    let guide_lines =
        virtual_indent_guide_lines(line_index, text, visible_range, tab_width, current_line);
    let max_guide_count = guide_lines
        .iter()
        .map(|line| line.guide_count)
        .max()
        .unwrap_or(0);
    if max_guide_count == 0 {
        return;
    }

    let tab_width = tab_width.max(1);
    let char_width = editor_char_width(ui);
    let painter = ui.painter_at(clip_rect);
    let guide_color = indent_guide_color(ui);
    let active_guide_color = active_indent_guide_color(ui);

    for guide_index in 1..=max_guide_count {
        let x = text_left + char_width * (guide_index * tab_width) as f32;
        if !clip_rect.x_range().contains(x) {
            continue;
        }

        let mut run_start = None;
        let mut previous_line = None;
        for guide_line in guide_lines.iter().copied() {
            let line_has_guide = guide_line.guide_count >= guide_index;
            let continues_run =
                previous_line.is_some_and(|previous| guide_line.line_number == previous + 1);

            if (!line_has_guide || guide_line.active || !continues_run)
                && let (Some(start), Some(end)) = (run_start.take(), previous_line)
            {
                paint_virtual_indent_guide_run(
                    &painter,
                    x,
                    start..end,
                    row_height,
                    scroll_y,
                    clip_rect,
                    guide_color,
                );
            }

            if line_has_guide && guide_line.active {
                paint_virtual_indent_guide_run(
                    &painter,
                    x,
                    guide_line.line_number..guide_line.line_number,
                    row_height,
                    scroll_y,
                    clip_rect,
                    active_guide_color,
                );
            } else if line_has_guide && run_start.is_none() {
                run_start = Some(guide_line.line_number);
            }

            previous_line = Some(guide_line.line_number);
        }

        if let (Some(start), Some(end)) = (run_start, previous_line) {
            paint_virtual_indent_guide_run(
                &painter,
                x,
                start..end,
                row_height,
                scroll_y,
                clip_rect,
                guide_color,
            );
        }
    }
}

pub(super) fn paint_virtual_indent_guide_run(
    painter: &egui::Painter,
    x: f32,
    lines: std::ops::Range<usize>,
    row_height: f32,
    scroll_y: f32,
    clip_rect: egui::Rect,
    color: Color32,
) {
    let (start_line, end_line) = (lines.start, lines.end);

    let top = clip_rect.top() + start_line as f32 * row_height - scroll_y;
    let bottom = clip_rect.top() + (end_line + 1) as f32 * row_height - scroll_y;
    painter.line_segment(
        [
            egui::pos2(x, top.max(clip_rect.top())),
            egui::pos2(x, bottom.min(clip_rect.bottom())),
        ],
        egui::Stroke::new(1.0, color),
    );
}

pub(super) fn paint_virtual_selection(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    selection: Option<NormalizedVirtualSelection>,
    visible_range: std::ops::Range<usize>,
    viewport: &VirtualViewport,
) {
    let VirtualViewport {
        content_rect: viewport_rect,
        text_clip_rect,
        text_left,
        row_height,
        char_width,
        scroll_y,
        ..
    } = *viewport;

    let Some(selection) = selection else {
        return;
    };

    let start_line = selection.start.line as usize;
    let end_line = selection.end.line as usize;
    let visible_start = visible_range.start.max(start_line);
    let visible_end = visible_range.end.min(end_line.saturating_add(1));
    if visible_start >= visible_end {
        return;
    }

    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    let fill = ui.visuals().selection.bg_fill;
    for line_number in visible_start..visible_end {
        let Some(line_info) = line_index.line(line_number) else {
            continue;
        };
        let line = text.line_text_or_empty(line_info);
        let line_len = line.len();
        let start_byte = if line_number == start_line {
            selection.start.character as usize
        } else {
            0
        }
        .min(line_len);
        let end_byte = if line_number == end_line {
            selection.end.character as usize
        } else {
            line_len
        }
        .min(line_len);
        if line_number == start_line && line_number == end_line && start_byte >= end_byte {
            continue;
        }

        let x1 = text_left + visual_x_for_byte_column(Some(ui), line, start_byte, char_width);
        let mut x2 = text_left + visual_x_for_byte_column(Some(ui), line, end_byte, char_width);
        if x2 <= x1 && line_number != end_line {
            x2 = x1 + char_width;
        }

        let top = viewport_rect.top() + line_number as f32 * row_height - scroll_y;
        let rect = egui::Rect::from_min_max(egui::pos2(x1, top), egui::pos2(x2, top + row_height))
            .intersect(text_clip_rect)
            .intersect(viewport_rect);
        if rect.is_positive() {
            painter.rect_filled(rect, 0.0, fill);
        }
    }
}

pub(super) fn paint_virtual_cursor(
    ui: &egui::Ui,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    cursor_position: Option<Position>,
    visible_range: std::ops::Range<usize>,
    viewport: &VirtualViewport,
) {
    let VirtualViewport {
        content_rect: viewport_rect,
        text_clip_rect,
        ..
    } = *viewport;
    let Some(rect) = virtual_cursor_rect(
        Some(ui),
        line_index,
        text,
        cursor_position,
        visible_range,
        viewport,
    ) else {
        return;
    };

    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    painter.line_segment(
        [rect.left_top(), rect.left_bottom()],
        egui::Stroke::new(1.5, ui.visuals().widgets.active.fg_stroke.color),
    );
}

pub(super) fn virtual_cursor_rect(
    ui: Option<&egui::Ui>,
    line_index: &LineIndex,
    text: &EditorTextBuffer,
    cursor_position: Option<Position>,
    visible_range: std::ops::Range<usize>,
    viewport: &VirtualViewport,
) -> Option<egui::Rect> {
    let VirtualViewport {
        content_rect: viewport_rect,
        text_clip_rect,
        text_left,
        row_height,
        char_width,
        scroll_y,
        ..
    } = *viewport;

    let position = cursor_position?;
    let line_number = position.line as usize;
    if !visible_range.contains(&line_number) {
        return None;
    }
    let line_info = line_index.line(line_number)?;
    let line = text.line_text_or_empty(line_info);
    let x = text_left + visual_x_for_byte_column(ui, line, position.character as usize, char_width);
    if !text_clip_rect.x_range().contains(x) {
        return None;
    }

    let top = viewport_rect.top() + line_number as f32 * row_height - scroll_y + 2.0;
    let bottom = (top + row_height - 4.0).min(viewport_rect.bottom());
    if bottom <= viewport_rect.top() || top >= viewport_rect.bottom() {
        return None;
    }

    Some(egui::Rect::from_min_max(
        egui::pos2(x, top.max(viewport_rect.top())),
        egui::pos2(x + 1.5, bottom),
    ))
}

pub(super) fn paint_virtual_ime_preedit(
    ui: &egui::Ui,
    preedit: Option<&str>,
    cursor_rect: Option<egui::Rect>,
    text_clip_rect: egui::Rect,
    viewport_rect: egui::Rect,
    row_height: f32,
) {
    let Some(preedit) = preedit.filter(|text| !text.is_empty()) else {
        return;
    };
    let Some(cursor_rect) = cursor_rect else {
        return;
    };
    let painter = ui.painter_at(text_clip_rect.intersect(viewport_rect));
    let position = cursor_rect.left_top();
    let font_id = TextStyle::Monospace.resolve(ui.style());
    painter.text(
        position,
        egui::Align2::LEFT_TOP,
        preedit,
        font_id,
        ui.visuals().text_color(),
    );
    let width = preedit.chars().count().max(1) as f32 * editor_char_width(ui);
    let underline_y = (position.y + row_height - 3.0).min(viewport_rect.bottom());
    painter.line_segment(
        [
            egui::pos2(position.x, underline_y),
            egui::pos2(position.x + width, underline_y),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.active.fg_stroke.color),
    );
}

pub(super) fn highlighted_line_layout_job(
    ui: &egui::Ui,
    source: &str,
    line: LineInfo,
    spans: &[HighlightSpan],
    line_span_range: std::ops::Range<usize>,
    highlights: LineHighlights<'_>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = line.start;
    let default_format = text_format(ui, None);

    let span_start = line_span_range.start.min(spans.len());
    let span_end = line_span_range.end.min(spans.len());
    for span in &spans[span_start..span_end] {
        let start = clamp_to_char_boundary(source, span.range.start.max(line.start));
        let end = clamp_to_char_boundary(source, span.range.end.min(line.content_end));

        if start < cursor || start >= end || end > line.content_end {
            continue;
        }

        if cursor < start {
            append_text_segment(
                &mut job,
                source,
                cursor,
                start,
                default_format.clone(),
                highlights,
                ui,
            );
        }

        append_text_segment(
            &mut job,
            source,
            start,
            end,
            text_format(ui, Some(span.kind)),
            highlights,
            ui,
        );
        cursor = end;
    }

    if cursor < line.content_end {
        append_text_segment(
            &mut job,
            source,
            cursor,
            line.content_end,
            default_format,
            highlights,
            ui,
        );
    }

    job
}

pub(super) fn handle_and_paint_virtual_scrollbars(
    ui: &egui::Ui,
    viewport_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
    scroll_x: &mut f32,
    scroll_y: &mut f32,
) {
    let painter = ui.painter_at(viewport_rect);
    let track_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let layout = virtual_scrollbar_layout(viewport_rect, total_width, total_height);

    if let Some(track_rect) = layout.vertical_rect {
        let ratio = (layout.content_rect.height() / total_height).clamp(0.02, 1.0);
        let handle_height = (layout.content_rect.height() * ratio).max(24.0);
        let max_handle_top = (track_rect.height() - handle_height).max(0.0);
        let max_scroll = (total_height - layout.content_rect.height()).max(1.0);
        let id = ui.make_persistent_id("virtual_source_vertical_scrollbar");
        let response = ui.interact(track_rect, id, egui::Sense::click_and_drag());
        let mut handle_top =
            track_rect.top() + max_handle_top * ((*scroll_y / max_scroll).clamp(0.0, 1.0));
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pointer_ratio = ((pointer.y - track_rect.top() - handle_height * 0.5)
                / max_handle_top.max(1.0))
            .clamp(0.0, 1.0);
            *scroll_y = pointer_ratio * max_scroll;
            handle_top = track_rect.top() + max_handle_top * pointer_ratio;
            ui.ctx().request_repaint();
        }
        let handle_color = virtual_scrollbar_handle_color(ui, &response);
        let rect = egui::Rect::from_min_size(
            egui::pos2(track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH, handle_top),
            egui::vec2(VIRTUAL_SCROLLBAR_BAR_WIDTH, handle_height),
        );
        painter.rect_filled(rect, 3.0, handle_color);
        painter.line_segment(
            [
                egui::pos2(
                    track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                    track_rect.top(),
                ),
                egui::pos2(
                    track_rect.right() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                    track_rect.bottom(),
                ),
            ],
            egui::Stroke::new(1.0, track_color),
        );
    }

    if let Some(track_rect) = layout.horizontal_rect {
        let ratio = (layout.content_rect.width() / total_width).clamp(0.02, 1.0);
        let handle_width = (layout.content_rect.width() * ratio).max(24.0);
        let max_handle_left = (track_rect.width() - handle_width).max(0.0);
        let max_scroll = (total_width - layout.content_rect.width()).max(1.0);
        let id = ui.make_persistent_id("virtual_source_horizontal_scrollbar");
        let response = ui.interact(track_rect, id, egui::Sense::click_and_drag());
        let mut handle_left =
            track_rect.left() + max_handle_left * ((*scroll_x / max_scroll).clamp(0.0, 1.0));
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pointer_ratio = ((pointer.x - track_rect.left() - handle_width * 0.5)
                / max_handle_left.max(1.0))
            .clamp(0.0, 1.0);
            *scroll_x = pointer_ratio * max_scroll;
            handle_left = track_rect.left() + max_handle_left * pointer_ratio;
            ui.ctx().request_repaint();
        }
        let handle_color = virtual_scrollbar_handle_color(ui, &response);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                handle_left,
                track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
            ),
            egui::vec2(handle_width, VIRTUAL_SCROLLBAR_BAR_WIDTH),
        );
        painter.rect_filled(rect, 3.0, handle_color);
        painter.line_segment(
            [
                egui::pos2(
                    track_rect.left(),
                    track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                ),
                egui::pos2(
                    track_rect.right(),
                    track_rect.bottom() - VIRTUAL_SCROLLBAR_BAR_WIDTH,
                ),
            ],
            egui::Stroke::new(1.0, track_color),
        );
    }
}

pub(super) fn virtual_scrollbar_layout(
    viewport_rect: egui::Rect,
    total_width: f32,
    total_height: f32,
) -> VirtualScrollbarLayout {
    let mut content_rect = viewport_rect;
    let mut needs_vertical = total_height > content_rect.height();
    let mut needs_horizontal = total_width > content_rect.width();

    if needs_vertical {
        content_rect.max.x =
            (content_rect.max.x - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.x + 1.0);
    }
    if needs_horizontal {
        content_rect.max.y =
            (content_rect.max.y - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.y + 1.0);
    }

    if !needs_vertical && total_height > content_rect.height() {
        needs_vertical = true;
        content_rect.max.x =
            (content_rect.max.x - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.x + 1.0);
    }
    if !needs_horizontal && total_width > content_rect.width() {
        needs_horizontal = true;
        content_rect.max.y =
            (content_rect.max.y - VIRTUAL_SCROLLBAR_HIT_WIDTH).max(content_rect.min.y + 1.0);
    }

    let vertical_rect = needs_vertical.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.right(), content_rect.top()),
            egui::pos2(viewport_rect.right(), content_rect.bottom()),
        )
    });
    let horizontal_rect = needs_horizontal.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.left(), content_rect.bottom()),
            egui::pos2(content_rect.right(), viewport_rect.bottom()),
        )
    });

    VirtualScrollbarLayout {
        content_rect,
        vertical_rect,
        horizontal_rect,
    }
}

pub(super) fn virtual_scrollbar_handle_color(ui: &egui::Ui, response: &egui::Response) -> Color32 {
    if response.dragged() {
        ui.visuals().widgets.active.fg_stroke.color
    } else if response.hovered() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    }
}
