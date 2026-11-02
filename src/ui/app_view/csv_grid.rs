//! The CSV grid: painting cells and headers, selection, keyboard movement and undo.
//! The document logic itself is in `core::csv`.

use super::*;

impl EditorPane {
    pub(super) fn capture_csv_history_entry(&self) -> CsvEditHistoryEntry {
        CsvEditHistoryEntry {
            text: self.text.as_str().to_owned(),
            csv_selection: self.csv.selection,
            selected_csv_cell: self.csv.selected_cell,
            scroll_y: self.scroll.y,
            scroll_x: self.scroll.x,
        }
    }

    pub(super) fn push_csv_undo_entry(&mut self) {
        let entry = self.capture_csv_history_entry();
        self.push_csv_undo_entry_value(entry);
    }

    pub(super) fn push_csv_undo_entry_value(&mut self, entry: CsvEditHistoryEntry) {
        if self.csv.undo_stack.last() != Some(&entry) {
            self.csv.undo_stack.push(entry);
            limit_csv_edit_history(&mut self.csv.undo_stack);
        }
        self.csv.redo_stack.clear();
    }

    pub(super) fn restore_csv_history_entry(&mut self, entry: CsvEditHistoryEntry) {
        self.text.replace(entry.text);
        self.text.refresh_metadata();
        self.caches.syntax_highlight_pending = None;
        self.caches.syntax_highlight_receiver = None;
        self.virtual_editor.line_layout_cache = None;
        self.virtual_editor.line_number_layout_cache = None;
        self.caches.editor_metrics = None;
        self.caches.source_search = None;
        self.csv.selection = entry.csv_selection;
        self.csv.selected_cell = entry.selected_csv_cell;
        self.csv.drag = None;
        self.csv.edit_history_cell = None;
        self.scroll.y = entry.scroll_y;
        self.scroll.x = entry.scroll_x;
        self.cursor_position = entry
            .selected_csv_cell
            .map(|cell| Position::new(cell.row as u32, cell.column as u32));
    }

    pub(super) fn apply_csv_undo(&mut self) -> bool {
        let Some(entry) = self.csv.undo_stack.pop() else {
            return false;
        };
        let current = self.capture_csv_history_entry();
        self.restore_csv_history_entry(entry);
        self.csv.redo_stack.push(current);
        limit_csv_edit_history(&mut self.csv.redo_stack);
        self.csv.debug_message = Some("csv undo".to_owned());
        true
    }

    pub(super) fn apply_csv_redo(&mut self) -> bool {
        let Some(entry) = self.csv.redo_stack.pop() else {
            return false;
        };
        let current = self.capture_csv_history_entry();
        self.restore_csv_history_entry(entry);
        self.csv.undo_stack.push(current);
        limit_csv_edit_history(&mut self.csv.undo_stack);
        self.csv.debug_message = Some("csv redo".to_owned());
        true
    }

    pub(super) fn is_csv(&self) -> bool {
        self.path().is_some_and(is_csv_path)
    }

    pub(super) fn is_csv_grid(&self) -> bool {
        self.is_csv() && self.csv.mode == CsvMode::Grid
    }

    pub(super) fn has_active_csv_selection(&self) -> bool {
        self.is_csv_grid() && self.csv.selection.is_some()
    }

    pub(super) fn csv_selection_text(&self) -> Option<String> {
        if !self.has_active_csv_selection() {
            return None;
        }
        let selection = self.csv.selection?;
        let document = parse_csv(&self.text).ok()?;
        Some(copy_csv_selection_as_csv(&document, selection))
    }

    pub(super) fn paste_csv_text(&mut self, text: &str) -> Result<usize, String> {
        if self.read_only() {
            return Err("Focused file is read-only.".to_owned());
        }

        let selection = self
            .csv
            .selection
            .ok_or_else(|| "No CSV row or column selection.".to_owned())?;
        let mut document = parse_csv(&self.text)?;
        self.push_csv_undo_entry();
        self.csv.edit_history_cell = None;
        let cell_count = paste_csv_selection_from_text(&mut document, selection, text)?;
        self.replace_text(serialize_csv(
            &document,
            CsvTextStyle::of(self.text.as_str()),
        ));
        self.csv.debug_message = Some(format!("csv pasted {cell_count} cells"));
        Ok(cell_count)
    }

    pub(super) fn delete_csv_selection(&mut self) -> Result<String, String> {
        if self.read_only() {
            return Err("Focused file is read-only.".to_owned());
        }

        let selection = self
            .csv
            .selection
            .ok_or_else(|| "No CSV row or column selection.".to_owned())?;
        let mut document = parse_csv(&self.text)?;
        self.push_csv_undo_entry();
        self.csv.edit_history_cell = None;
        let (next_selection, label) = delete_csv_selection(&mut document, selection);
        self.replace_text(serialize_csv(
            &document,
            CsvTextStyle::of(self.text.as_str()),
        ));
        self.csv.selection = Some(next_selection);
        self.csv.selected_cell = None;
        self.csv.drag = None;
        self.csv.debug_message = Some(format!("csv deleted {label}"));
        Ok(label)
    }

    pub(super) fn set_csv_mode(&mut self, mode: CsvMode) {
        if self.csv.mode == mode {
            return;
        }
        self.csv.mode = mode;
        self.csv.edit_history_cell = None;
        if mode == CsvMode::Grid {
            self.close_source_search();
        }
    }

    pub(super) fn csv_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
    ) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("CSV").small().strong());
            ui.separator();
            for mode in CsvMode::all() {
                if ui
                    .selectable_label(self.csv.mode == mode, mode.label())
                    .on_hover_text(format!("Switch to {} mode", mode.label()))
                    .clicked()
                {
                    self.set_csv_mode(mode);
                }
            }
        });
        ui.separator();

        match self.csv.mode {
            CsvMode::Grid => self.csv_ui(ui, focused),
            CsvMode::Source => {
                self.source_editor_ui(ui, focused, language_registry, tab_width, &[])
            }
        }
    }

    pub(super) fn csv_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let mut document = match parse_csv(&self.text) {
            Ok(document) => document,
            Err(error) => {
                ui.label(
                    RichText::new(format!("CSV parse error: {error}"))
                        .color(ui.visuals().warn_fg_color),
                );
                return;
            }
        };

        let column_count = document.column_count();
        if column_count == 0 {
            ui.label(RichText::new("Empty CSV").weak());
            return;
        }

        let column_widths = csv_column_widths(ui, &document, column_count);
        let header_height = editor_row_height(ui) + 10.0;
        let row_height = editor_row_height(ui) + 8.0;
        let row_number_width =
            ((document.rows.len() + 1).to_string().len() as f32 * 10.0 + 22.0).max(44.0);
        let mut changed = false;
        let mut next_focus = None;

        if self.csv.selection.is_none()
            && self.csv.selected_cell.is_none()
            && !document.rows.is_empty()
        {
            self.csv.selected_cell = Some(CsvCellPosition { row: 0, column: 0 });
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("CSV VIEW").small().strong());
            if let Some(selection) = self.csv.selection {
                ui.label(RichText::new(csv_selection_label(selection)).small().weak());
            }
            if let Some(message) = &self.csv.debug_message {
                ui.label(RichText::new(message).small().weak());
            }
        });

        ScrollArea::both()
            .id_salt(format!("csv_grid_{:?}", self.pane))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(action) = paint_csv_header(
                    ui,
                    row_number_width,
                    header_height,
                    &column_widths,
                    self.csv.selection,
                    self.csv.drag,
                ) {
                    self.apply_csv_header_action(ui, CsvSelectionKind::Columns, action);
                }

                for row in 0..document.rows.len() {
                    ui.horizontal(|ui| {
                        let active_row = self.csv.selected_cell.is_some_and(|cell| cell.row == row)
                            && self.csv.selection.is_none();
                        let selected_row = self
                            .csv
                            .selection
                            .is_some_and(|selection| selection.contains(row, 0));
                        let row_header_response = paint_csv_row_header(
                            ui,
                            row + 1,
                            row_number_width,
                            row_height,
                            active_row || selected_row,
                        );
                        if let Some(action) = csv_pointer_action_for_rect(
                            ui,
                            row_header_response.rect,
                            row,
                            CsvSelectionKind::Rows,
                            self.csv.drag,
                        ) {
                            self.apply_csv_header_action(ui, CsvSelectionKind::Rows, action);
                        }

                        for (column, width) in column_widths.iter().enumerate().take(column_count) {
                            let outcome = self.csv_cell_ui(
                                ui,
                                &mut document,
                                CsvCellPosition { row, column },
                                (*width, row_height),
                                (focused, active_row),
                            );
                            changed |= outcome.changed;
                            next_focus = outcome.next_focus.or(next_focus);
                        }
                    });
                }
            });

        if !ui.input(|input| input.pointer.primary_down()) {
            self.csv.drag = None;
        }

        if changed {
            self.replace_text(serialize_csv(
                &document,
                CsvTextStyle::of(self.text.as_str()),
            ));
        }

        if let Some(target) = next_focus
            && self.csv.selected_cell != Some(target)
        {
            let needs_new_cell = document
                .rows
                .get(target.row)
                .and_then(|row| row.get(target.column))
                .is_none();
            if needs_new_cell {
                self.push_csv_undo_entry();
                document.ensure_cell(target.row, target.column);
                self.replace_text(serialize_csv(
                    &document,
                    CsvTextStyle::of(self.text.as_str()),
                ));
            }
            self.csv.selected_cell = Some(target);
            self.csv.edit_history_cell = None;
            self.cursor_position = Some(Position::new(target.row as u32, target.column as u32));
            // The target cell's TextEdit takes focus when it is built in the next frame.
            ui.ctx().request_repaint();
        }
    }

    /// Selects columns or rows from a press or drag on their header.
    fn apply_csv_header_action(
        &mut self,
        ui: &egui::Ui,
        kind: CsvSelectionKind,
        action: CsvHeaderAction,
    ) {
        let (start, end) = match action {
            CsvHeaderAction::Press(index) => {
                self.csv.drag = Some(CsvSelectionDrag {
                    kind,
                    anchor: index,
                });
                (index, index)
            }
            CsvHeaderAction::ExtendDrag(index) => {
                let start = self
                    .csv
                    .drag
                    .filter(|drag| drag.kind == kind)
                    .map(|drag| drag.anchor)
                    .unwrap_or(index);
                (start, index)
            }
        };
        let (selection, name) = match kind {
            CsvSelectionKind::Columns => (CsvSelection::Columns { start, end }, "column"),
            CsvSelectionKind::Rows => (CsvSelection::Rows { start, end }, "row"),
        };
        self.csv.selection = Some(selection);
        self.csv.debug_message = Some(format!(
            "csv {name} action: {:?} -> {:?}",
            action, self.csv.selection
        ));
        self.csv.selected_cell = None;
        self.csv.edit_history_cell = None;
        ui.ctx().request_repaint();
    }

    /// Paints one cell; the selected one is a text field.
    fn csv_cell_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut CsvDocument,
        position: CsvCellPosition,
        (width, row_height): (f32, f32),
        (focused, active_row): (bool, bool),
    ) -> CsvCellOutcome {
        let CsvCellPosition { row, column } = position;
        let column_count = document.column_count();
        let mut outcome = CsvCellOutcome::default();
        let id = ui
            .id()
            .with(format!("csv_cell_{:?}_{row}_{column}", self.pane));
        let row_count = document.rows.len();
        let is_selected = self.csv.selection.is_none()
            && self.csv.selected_cell == Some(CsvCellPosition { row, column });
        let is_range_selected = self
            .csv
            .selection
            .is_some_and(|selection| selection.contains(row, column));
        let text = document
            .rows
            .get(row)
            .and_then(|fields| fields.get(column))
            .cloned()
            .unwrap_or_default();
        let response = paint_csv_cell(
            ui,
            &text,
            width,
            row_height,
            active_row || is_range_selected,
            is_selected || is_range_selected,
        );

        if response.clicked() {
            self.csv.selected_cell = Some(CsvCellPosition { row, column });
            self.csv.selection = None;
            self.csv.drag = None;
            self.csv.edit_history_cell = None;
            self.csv.debug_message =
                Some(format!("csv cell selected: R{}C{}", row + 1, column + 1));
            self.cursor_position = Some(Position::new(row as u32, column as u32));
            // The cell's TextEdit is only built from the next frame on;
            // it takes focus itself then (see below).
            ui.ctx().request_repaint();
        }

        if is_selected {
            let edit_rect = response.rect.shrink2(egui::vec2(3.0, 2.0));
            let cell_position = CsvCellPosition { row, column };
            let before_edit = self.capture_csv_history_entry();
            let cell = document.cell_mut(row, column);
            if focused
                && let Some(target) = csv_navigation_target(
                    ui,
                    id,
                    row,
                    column,
                    row_count,
                    column_count,
                    cell.chars().count(),
                )
            {
                outcome.next_focus = Some(target);
            }

            let output = ui
                .scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                    TextEdit::singleline(cell)
                        .id(id)
                        .font(TextStyle::Monospace)
                        .desired_width(edit_rect.width())
                        .interactive(!self.read_only())
                        .show(ui)
                })
                .inner;

            if output.response.changed() {
                if self.csv.edit_history_cell != Some(cell_position) {
                    self.push_csv_undo_entry_value(before_edit);
                    self.csv.edit_history_cell = Some(cell_position);
                }
                outcome.changed = true;
                self.target_position = None;
            }

            if output.response.has_focus() {
                self.cursor_position = Some(Position::new(row as u32, column as u32));
            } else if focused {
                ui.memory_mut(|memory| memory.request_focus(id));
            }
        }
        outcome
    }
}

/// What one cell's frame asks of the grid.
#[derive(Default)]
struct CsvCellOutcome {
    changed: bool,
    next_focus: Option<CsvCellPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum CsvMode {
    #[default]
    Grid,
    Source,
}

impl CsvMode {
    pub(super) fn all() -> [Self; 2] {
        [Self::Grid, Self::Source]
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::Source => "Source",
        }
    }
}

pub(super) const CSV_EDIT_HISTORY_LIMIT: usize = 256;

/// State only the CSV views use.
#[derive(Default)]
pub(super) struct CsvPaneState {
    pub(super) selected_cell: Option<CsvCellPosition>,
    pub(super) selection: Option<CsvSelection>,
    pub(super) drag: Option<CsvSelectionDrag>,
    pub(super) debug_message: Option<String>,
    pub(super) mode: CsvMode,
    pub(super) undo_stack: Vec<CsvEditHistoryEntry>,
    pub(super) redo_stack: Vec<CsvEditHistoryEntry>,
    pub(super) edit_history_cell: Option<CsvCellPosition>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CsvEditHistoryEntry {
    pub(super) text: String,
    pub(super) csv_selection: Option<CsvSelection>,
    pub(super) selected_csv_cell: Option<CsvCellPosition>,
    pub(super) scroll_y: f32,
    pub(super) scroll_x: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CsvHeaderAction {
    Press(usize),
    ExtendDrag(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CsvSelectionDrag {
    pub(super) kind: CsvSelectionKind,
    pub(super) anchor: usize,
}

pub(super) fn csv_column_widths(
    ui: &egui::Ui,
    document: &CsvDocument,
    column_count: usize,
) -> Vec<f32> {
    let row_height = editor_row_height(ui);
    let char_width = row_height * 0.58;

    (0..column_count)
        .map(|column| {
            let max_chars = document
                .rows
                .iter()
                .filter_map(|row| row.get(column))
                .flat_map(|cell| cell.lines())
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
                .max(csv_column_label(column).chars().count());

            (max_chars as f32 * char_width + 28.0).clamp(72.0, 360.0)
        })
        .collect()
}

pub(super) fn limit_csv_edit_history(stack: &mut Vec<CsvEditHistoryEntry>) {
    let overflow = stack.len().saturating_sub(CSV_EDIT_HISTORY_LIMIT);
    if overflow > 0 {
        stack.drain(0..overflow);
    }
}

pub(super) fn paint_csv_header(
    ui: &mut egui::Ui,
    row_number_width: f32,
    header_height: f32,
    column_widths: &[f32],
    selection: Option<CsvSelection>,
    drag: Option<CsvSelectionDrag>,
) -> Option<CsvHeaderAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        let (corner_rect, _) = ui.allocate_exact_size(
            egui::vec2(row_number_width, header_height),
            egui::Sense::hover(),
        );
        paint_csv_box(
            ui,
            corner_rect,
            "",
            csv_header_background(ui),
            ui.visuals().weak_text_color(),
            egui::Align2::CENTER_CENTER,
            false,
        );

        for (column, width) in column_widths.iter().enumerate() {
            let selected = selection.is_some_and(|selection| selection.contains(0, column));
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(*width, header_height),
                egui::Sense::click_and_drag(),
            );
            paint_csv_box(
                ui,
                rect,
                &csv_column_label(column),
                if selected {
                    current_line_background(ui)
                } else {
                    csv_header_background(ui)
                },
                if selected {
                    ui.visuals().text_color()
                } else {
                    ui.visuals().weak_text_color()
                },
                egui::Align2::CENTER_CENTER,
                true,
            );

            if let Some(next_action) =
                csv_pointer_action_for_rect(ui, rect, column, CsvSelectionKind::Columns, drag)
            {
                action = Some(next_action);
            }
        }
    });

    action
}

pub(super) fn csv_pointer_action_for_rect(
    ui: &egui::Ui,
    rect: egui::Rect,
    index: usize,
    kind: CsvSelectionKind,
    drag: Option<CsvSelectionDrag>,
) -> Option<CsvHeaderAction> {
    ui.input(|input| {
        if input.pointer.button_pressed(egui::PointerButton::Primary)
            && input
                .pointer
                .press_origin()
                .or_else(|| input.pointer.latest_pos())
                .is_some_and(|position| rect.contains(position))
        {
            return Some(CsvHeaderAction::Press(index));
        }

        if input.pointer.primary_down()
            && drag.is_some_and(|drag| drag.kind == kind)
            && input
                .pointer
                .latest_pos()
                .is_some_and(|position| rect.contains(position))
        {
            return Some(CsvHeaderAction::ExtendDrag(index));
        }

        None
    })
}

pub(super) fn paint_csv_row_header(
    ui: &mut egui::Ui,
    row_number: usize,
    width: f32,
    height: f32,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    paint_csv_box(
        ui,
        rect,
        &row_number.to_string(),
        if selected {
            current_line_background(ui)
        } else {
            csv_header_background(ui)
        },
        if selected {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        },
        egui::Align2::RIGHT_CENTER,
        selected,
    );
    response
}

pub(super) fn paint_csv_cell(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    height: f32,
    selected_row: bool,
    selected_cell: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    paint_csv_box(
        ui,
        rect,
        &csv_cell_display_text(text),
        if selected_row {
            current_line_background(ui)
        } else {
            ui.visuals().extreme_bg_color
        },
        ui.visuals().text_color(),
        egui::Align2::LEFT_CENTER,
        selected_cell,
    );
    response
}

pub(super) fn paint_csv_box(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    fill: Color32,
    text_color: Color32,
    align: egui::Align2,
    strong_border: bool,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, fill);

    let stroke_color = if strong_border {
        ui.visuals().widgets.active.bg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    let stroke_width = if strong_border { 1.5 } else { 1.0 };
    let stroke = egui::Stroke::new(stroke_width, stroke_color);
    painter.line_segment([rect.left_top(), rect.right_top()], stroke);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
    painter.line_segment([rect.left_top(), rect.left_bottom()], stroke);
    painter.line_segment([rect.right_top(), rect.right_bottom()], stroke);

    let x = match align {
        egui::Align2::RIGHT_CENTER => rect.right() - 6.0,
        egui::Align2::CENTER_CENTER => rect.center().x,
        _ => rect.left() + 6.0,
    };
    painter.text(
        egui::pos2(x, rect.center().y),
        align,
        text,
        editor_font_id(),
        text_color,
    );
}

pub(super) fn csv_header_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(58, 58, 62)
    } else {
        Color32::from_rgb(215, 218, 222)
    }
}

pub(super) fn csv_navigation_target(
    ui: &mut egui::Ui,
    text_edit_id: egui::Id,
    row: usize,
    column: usize,
    row_count: usize,
    column_count: usize,
    cell_len: usize,
) -> Option<CsvCellPosition> {
    if !ui.memory(|memory| memory.has_focus(text_edit_id)) {
        return None;
    }

    let cursor_index = TextEditState::load(ui.ctx(), text_edit_id)
        .and_then(|state| state.cursor.char_range().map(|range| range.primary.index))
        .unwrap_or(0);

    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
            return Some(CsvCellPosition { row, column: 0 });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
            return Some(CsvCellPosition {
                row,
                column: column_count.saturating_sub(1),
            });
        }

        // Moving never grows the table; Enter on the last row is the one explicit way to
        // append a row.
        let last_row = row_count.saturating_sub(1);
        let last_column = column_count.saturating_sub(1);
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            return Some(CsvCellPosition {
                row: row + 1,
                column,
            });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            return Some(CsvCellPosition {
                row,
                column: (column + 1).min(last_column),
            });
        }

        if row > 0 && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            return Some(CsvCellPosition {
                row: row - 1,
                column,
            });
        }

        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            return Some(CsvCellPosition {
                row: (row + 1).min(last_row),
                column,
            });
        }

        if cursor_index == 0
            && column > 0
            && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
        {
            return Some(CsvCellPosition {
                row,
                column: column - 1,
            });
        }

        if cursor_index >= cell_len
            && column < last_column
            && input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
        {
            return Some(CsvCellPosition {
                row,
                column: column + 1,
            });
        }

        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_undo_redo_restores_paste() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.content = EditorContent::File {
            path: PathBuf::from("data.csv"),
            read_only: false,
        };
        editor.text = EditorTextBuffer::new("a,b\nc,d".to_owned());
        editor.csv.mode = CsvMode::Grid;
        editor.csv.selection = Some(CsvSelection::Columns { start: 1, end: 1 });

        let cell_count = editor.paste_csv_text("x\ny").unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(editor.text.as_str(), "a,x\nc,y");
        assert!(editor.apply_csv_undo());
        assert_eq!(editor.text.as_str(), "a,b\nc,d");
        assert!(editor.apply_csv_redo());
        assert_eq!(editor.text.as_str(), "a,x\nc,y");
    }

    #[test]
    fn csv_undo_redo_restores_delete_selection() {
        let mut editor = EditorPane::empty(Pane::Left);
        editor.content = EditorContent::File {
            path: PathBuf::from("data.csv"),
            read_only: false,
        };
        editor.text = EditorTextBuffer::new("a,b\nc,d\ne,f".to_owned());
        editor.csv.mode = CsvMode::Grid;
        editor.csv.selection = Some(CsvSelection::Rows { start: 1, end: 1 });

        let label = editor.delete_csv_selection().unwrap();

        assert_eq!(label, "row 2");
        assert_eq!(editor.text.as_str(), "a,b\ne,f");
        assert!(editor.apply_csv_undo());
        assert_eq!(editor.text.as_str(), "a,b\nc,d\ne,f");
        assert!(editor.apply_csv_redo());
        assert_eq!(editor.text.as_str(), "a,b\ne,f");
    }
}
