//! From pointer positions to text positions in the large-file editor: clicks, double
//! clicks and drag selection.

use super::*;

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
