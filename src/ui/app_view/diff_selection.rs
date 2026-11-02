//! Text selection in the diff view: from pointer to selected cells, and painting it.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_diff_selection_input(
    ui: &mut egui::Ui,
    rows: &[DiffRenderRow<'_>],
    mode: DiffDisplayMode,
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    side_selection: &mut Option<DiffSideSelection>,
    unified_selection: &mut Option<DiffUnifiedSelection>,
    side_scroll: DiffSideBySideScroll,
) {
    if start >= end || !visible_rect.is_positive() {
        return;
    }
    let _response = ui.interact(
        visible_rect,
        ui.auto_id_with("diff_visible_text_selection"),
        egui::Sense::drag(),
    );
    let Some(pointer_pos) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
    let primary_down = ui.input(|input| input.pointer.primary_down());
    let char_width = editor_char_width(ui);
    let pointer_in_visible = visible_rect.intersect(ui.clip_rect()).contains(pointer_pos);
    match mode {
        DiffDisplayMode::Unified => handle_unified_diff_selection_input(
            ui,
            rows,
            start,
            end,
            visible_rect,
            row_height,
            gutter_width,
            unified_selection,
            char_width,
            pointer_pos,
            primary_pressed && pointer_in_visible,
            primary_down,
        ),
        DiffDisplayMode::SideBySide => handle_side_by_side_diff_selection_input(
            ui,
            rows,
            start,
            end,
            visible_rect,
            row_height,
            gutter_width,
            side_selection,
            side_scroll,
            char_width,
            pointer_pos,
            primary_pressed && pointer_in_visible,
            primary_down,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_unified_diff_selection_input(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    unified_selection: &mut Option<DiffUnifiedSelection>,
    char_width: f32,
    pointer_pos: egui::Pos2,
    start_selection: bool,
    primary_down: bool,
) {
    if start_selection {
        let Some((row_index, row_rect)) =
            diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, false)
        else {
            return;
        };
        let Some((text, content_rect)) =
            unified_diff_selection_target(rows, row_index, row_rect, gutter_width)
        else {
            return;
        };
        if !content_rect.contains(pointer_pos) {
            return;
        }
        let point = diff_selection_point_for_pointer(
            row_index,
            &text,
            content_rect.left(),
            pointer_pos.x,
            char_width,
        );
        *unified_selection = Some(DiffUnifiedSelection {
            anchor: point,
            head: point,
            dragging: true,
        });
        ui.ctx().request_repaint();
        return;
    }

    if !primary_down || !unified_selection.is_some_and(|selection| selection.dragging) {
        return;
    }
    let Some((row_index, row_rect)) =
        diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, true)
    else {
        return;
    };
    let Some((text, content_rect)) =
        unified_diff_selection_target(rows, row_index, row_rect, gutter_width)
    else {
        return;
    };
    if let Some(selection) = unified_selection.as_mut() {
        selection.head = diff_selection_point_for_pointer(
            row_index,
            &text,
            content_rect.left(),
            pointer_pos.x,
            char_width,
        );
        ui.ctx().request_repaint();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_side_by_side_diff_selection_input(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
    gutter_width: f32,
    side_selection: &mut Option<DiffSideSelection>,
    side_scroll: DiffSideBySideScroll,
    char_width: f32,
    pointer_pos: egui::Pos2,
    start_selection: bool,
    primary_down: bool,
) {
    if start_selection {
        let Some((row_index, row_rect)) =
            diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, false)
        else {
            return;
        };
        let Some((side, content, content_rect, text_left)) = side_by_side_diff_selection_target(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            pointer_pos.x,
        ) else {
            return;
        };
        if !content_rect.contains(pointer_pos) {
            return;
        }
        let point = diff_selection_point_for_pointer(
            row_index,
            &content,
            text_left,
            pointer_pos.x,
            char_width,
        );
        *side_selection = Some(DiffSideSelection {
            side,
            anchor: point,
            head: point,
            dragging: true,
        });
        ui.ctx().request_repaint();
        return;
    }

    let Some(selection) = side_selection.as_mut() else {
        return;
    };
    if !primary_down || !selection.dragging {
        return;
    }
    let Some((row_index, row_rect)) =
        diff_row_at_pointer(visible_rect, start, end, row_height, pointer_pos.y, true)
    else {
        return;
    };
    let Some((content, _content_rect, text_left)) = side_by_side_diff_selection_target_for_side(
        rows,
        row_index,
        row_rect,
        gutter_width,
        side_scroll,
        selection.side,
    ) else {
        return;
    };
    selection.head =
        diff_selection_point_for_pointer(row_index, &content, text_left, pointer_pos.x, char_width);
    ui.ctx().request_repaint();
}

pub(super) fn diff_row_at_pointer(
    visible_rect: egui::Rect,
    start: usize,
    end: usize,
    row_height: f32,
    pointer_y: f32,
    clamp: bool,
) -> Option<(usize, egui::Rect)> {
    let y = if clamp {
        pointer_y.clamp(visible_rect.top(), visible_rect.bottom() - 0.1)
    } else if (visible_rect.top()..=visible_rect.bottom()).contains(&pointer_y) {
        pointer_y
    } else {
        return None;
    };
    let row_offset = ((y - visible_rect.top()) / row_height).floor().max(0.0) as usize;
    if row_offset >= end.saturating_sub(start) {
        return None;
    }
    Some((
        start + row_offset,
        diff_visible_row_rect(visible_rect, row_offset, row_height),
    ))
}

pub(super) fn unified_diff_selection_target(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
) -> Option<(String, egui::Rect)> {
    let DiffRenderRow::UnifiedLine {
        marker, content, ..
    } = rows.get(row_index)?
    else {
        return None;
    };
    let text = unified_diff_visible_text(marker, content);
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(row_rect.left() + gutter_width + 8.0, row_rect.top()),
        row_rect.max,
    );
    Some((text, content_rect))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn side_by_side_diff_selection_target(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
    side_scroll: DiffSideBySideScroll,
    pointer_x: f32,
) -> Option<(DiffSide, String, egui::Rect, f32)> {
    let width = row_rect.width().max(1.0);
    let center_x = row_rect.left() + width * 0.5;
    if pointer_x < center_x - 1.0 {
        let (content, content_rect, text_left) = side_by_side_diff_selection_target_for_side(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            DiffSide::Old,
        )?;
        Some((DiffSide::Old, content, content_rect, text_left))
    } else if pointer_x > center_x + 1.0 {
        let (content, content_rect, text_left) = side_by_side_diff_selection_target_for_side(
            rows,
            row_index,
            row_rect,
            gutter_width,
            side_scroll,
            DiffSide::New,
        )?;
        Some((DiffSide::New, content, content_rect, text_left))
    } else {
        None
    }
}

pub(super) fn side_by_side_diff_selection_target_for_side(
    rows: &[DiffRenderRow<'_>],
    row_index: usize,
    row_rect: egui::Rect,
    gutter_width: f32,
    side_scroll: DiffSideBySideScroll,
    side: DiffSide,
) -> Option<(String, egui::Rect, f32)> {
    let DiffRenderRow::SideBySide { old, new } = rows.get(row_index)? else {
        return None;
    };
    let width = row_rect.width().max(1.0);
    let center_x = row_rect.left() + width * 0.5;
    let (cell, rect, scroll_x) = match side {
        DiffSide::Old => (
            old,
            egui::Rect::from_min_max(row_rect.min, egui::pos2(center_x - 1.0, row_rect.max.y)),
            side_scroll.left_x,
        ),
        DiffSide::New => (
            new,
            egui::Rect::from_min_max(egui::pos2(center_x + 1.0, row_rect.min.y), row_rect.max),
            side_scroll.right_x,
        ),
    };
    let content_rect = egui::Rect::from_min_max(
        egui::pos2((rect.left() + gutter_width).min(rect.right()), rect.top()),
        rect.max,
    );
    let text_left = rect.left() + gutter_width + 8.0 - scroll_x;
    Some((cell.content.to_owned(), content_rect, text_left))
}

pub(super) fn diff_selection_point_for_pointer(
    row_index: usize,
    content: &str,
    text_left: f32,
    pointer_x: f32,
    char_width: f32,
) -> DiffSideSelectionPoint {
    let byte_column =
        byte_column_for_visual_x(None, content, (pointer_x - text_left).max(0.0), char_width)
            as usize;
    DiffSideSelectionPoint {
        row: row_index,
        column: visual_column_for_byte_column(content, byte_column),
    }
}

pub(super) fn paint_diff_side_selection_for_cell(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    content: &str,
    row_index: usize,
    side: DiffSide,
    selection: Option<&DiffSideSelection>,
    text_left: f32,
    char_width: f32,
) {
    let Some((start_column, end_column)) =
        diff_side_selection_columns_for_cell(selection, side, row_index, content.chars().count())
    else {
        return;
    };

    let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
    let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
    let x1 = text_left + visual_x_for_byte_column(Some(ui), content, start_byte, char_width);
    let mut x2 = text_left + visual_x_for_byte_column(Some(ui), content, end_byte, char_width);
    if x2 <= x1 {
        x2 = x1 + char_width;
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(x1, content_rect.top()),
        egui::pos2(x2, content_rect.bottom()),
    )
    .intersect(content_rect);
    if rect.is_positive() {
        ui.painter_at(content_rect)
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
}

pub(super) fn paint_diff_unified_selection_for_line(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    content: &str,
    row_index: usize,
    selection: Option<&DiffUnifiedSelection>,
    text_left: f32,
    char_width: f32,
) {
    let Some((start_column, end_column)) =
        diff_unified_selection_columns_for_line(selection, row_index, content.chars().count())
    else {
        return;
    };

    let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
    let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
    let x1 = text_left + visual_x_for_byte_column(Some(ui), content, start_byte, char_width);
    let mut x2 = text_left + visual_x_for_byte_column(Some(ui), content, end_byte, char_width);
    if x2 <= x1 {
        x2 = x1 + char_width;
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(x1, content_rect.top()),
        egui::pos2(x2, content_rect.bottom()),
    )
    .intersect(content_rect);
    if rect.is_positive() {
        ui.painter_at(content_rect)
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
}
