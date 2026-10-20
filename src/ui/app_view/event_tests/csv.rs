//! CSV grid: cell editing and navigation, row / column selection by clicking and dragging
//! headers, copy / paste / delete, undo / redo, Grid and Source modes and saving.

use eframe::egui;

use super::super::{CsvCellPosition, CsvMode, CsvSelection};
use super::support::{CTRL, TestApp};

const CSV: &str = "name,qty,note\napple,1,red\nbanana,22,\"yellow, long\"\n";

/// Grid geometry derived from the selected cell's editor (the only widget in the grid).
struct Grid {
    origin: egui::Pos2,
    widths: Vec<f32>,
    row_height: f32,
    header_height: f32,
    row_number_width: f32,
}

impl Grid {
    const SPACING: egui::Vec2 = egui::vec2(8.0, 3.0);

    fn cell(&self, row: usize, column: usize) -> egui::Pos2 {
        let x = self.origin.x
            + self.widths[..column].iter().sum::<f32>()
            + Self::SPACING.x * column as f32
            + self.widths[column] * 0.5;
        let y = self.origin.y
            + (self.row_height + Self::SPACING.y) * row as f32
            + self.row_height * 0.5;
        egui::pos2(x, y)
    }

    fn row_header(&self, row: usize) -> egui::Pos2 {
        let cell = self.cell(row, 0);
        egui::pos2(
            self.origin.x - Self::SPACING.x - self.row_number_width * 0.5,
            cell.y,
        )
    }

    fn column_header(&self, column: usize) -> egui::Pos2 {
        let cell = self.cell(0, column);
        egui::pos2(
            cell.x,
            self.origin.y - Self::SPACING.y - self.header_height * 0.5,
        )
    }
}

fn grid(t: &TestApp) -> Grid {
    let editor = t.app().left.text.as_str().to_owned();
    let document = super::super::parse_csv(&editor).unwrap();
    let selected = t
        .app()
        .left
        .selected_csv_cell
        .unwrap_or(CsvCellPosition { row: 0, column: 0 });
    let (font_row_height, _) = t.editor_metrics();
    let column_count = document.column_count();
    let widths = (0..column_count)
        .map(|column| {
            let max_chars = document
                .rows
                .iter()
                .filter_map(|row| row.get(column))
                .flat_map(|cell| cell.lines())
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
                .max(super::super::csv_column_label(column).chars().count());
            (max_chars as f32 * font_row_height * 0.58 + 28.0).clamp(72.0, 360.0)
        })
        .collect::<Vec<_>>();
    let row_height = font_row_height + 8.0;
    let header_height = font_row_height + 10.0;
    let row_number_width =
        ((document.rows.len() + 1).to_string().len() as f32 * 10.0 + 22.0).max(44.0);
    // The selected cell's text field is the cell rect shrunk by (3, 2).
    let field = *t
        .text_field_rects()
        .iter()
        .find(|rect| rect.height() < row_height + 1.0)
        .expect("selected cell editor");
    let cell_left = field.left() - 3.0;
    let cell_top = field.top() - 2.0;
    let origin = egui::pos2(
        cell_left
            - widths[..selected.column].iter().sum::<f32>()
            - Grid::SPACING.x * selected.column as f32,
        cell_top - (row_height + Grid::SPACING.y) * selected.row as f32,
    );
    Grid {
        origin,
        widths,
        row_height,
        header_height,
        row_number_width,
    }
}

fn csv_app() -> TestApp {
    let mut t = TestApp::builder().file("data.csv", CSV).build();
    t.click_first("data.csv");
    t.steps(3);
    t
}

fn text(t: &TestApp) -> String {
    t.app().left.text.as_str().to_owned()
}

#[test]
fn grid_shows_cells_and_edits_the_selected_one() {
    let mut t = csv_app();
    assert!(t.has_label("CSV VIEW"));
    assert!(t.has_label("Grid"));
    assert_eq!(
        t.app().left.selected_csv_cell,
        Some(CsvCellPosition { row: 0, column: 0 })
    );
    let g = grid(&t);
    t.click_at(g.cell(1, 1));
    assert_eq!(
        t.app().left.selected_csv_cell,
        Some(CsvCellPosition { row: 1, column: 1 })
    );
    t.assert_status_containing("Indexed");
    t.ctrl(egui::Key::A);
    t.type_text("5");
    // DISCREPANCY(D14): any grid edit re-serializes the file without its final newline.
    assert_eq!(
        text(&t),
        "name,qty,note\napple,5,red\nbanana,22,\"yellow, long\""
    );

    t.ctrl(egui::Key::Z);
    t.assert_status("CSV undo.");
    assert!(text(&t).contains("apple,1,red"));
    t.ctrl(egui::Key::Y);
    t.assert_status("CSV redo.");
    assert!(text(&t).contains("apple,5,red"));
    t.ctrl(egui::Key::Y);
    t.assert_status("No CSV edit to redo.");
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Z);
    t.assert_status("No CSV edit to undo.");

    t.ctrl(egui::Key::S);
    t.assert_status_containing("Saved file:");
}

#[test]
fn keyboard_moves_between_cells() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.cell(1, 0));
    let at = |t: &TestApp| t.app().left.selected_csv_cell.unwrap();
    t.key(egui::Key::Tab);
    assert_eq!(at(&t), CsvCellPosition { row: 1, column: 1 });
    t.key(egui::Key::End);
    assert_eq!(at(&t), CsvCellPosition { row: 1, column: 2 });
    t.key(egui::Key::Home);
    assert_eq!(at(&t), CsvCellPosition { row: 1, column: 0 });
    t.key(egui::Key::Enter);
    assert_eq!(at(&t), CsvCellPosition { row: 2, column: 0 });
    t.key(egui::Key::ArrowUp);
    assert_eq!(at(&t), CsvCellPosition { row: 1, column: 0 });
    t.key(egui::Key::ArrowDown);
    assert_eq!(at(&t), CsvCellPosition { row: 2, column: 0 });
    // A newly focused cell has its caret at the end, so ArrowRight leaves it at once;
    // ArrowLeft first walks the caret back to the start of the text.
    t.key(egui::Key::ArrowRight);
    assert_eq!(at(&t), CsvCellPosition { row: 2, column: 1 });
    t.key(egui::Key::ArrowLeft);
    t.key(egui::Key::ArrowLeft);
    assert_eq!(at(&t), CsvCellPosition { row: 2, column: 1 });
    t.key(egui::Key::ArrowLeft);
    assert_eq!(at(&t), CsvCellPosition { row: 2, column: 0 });
    assert_eq!(
        text(&t),
        CSV,
        "navigation inside the grid does not change the file"
    );
}

#[test]
fn navigating_past_the_edge_adds_rows_and_columns_to_the_file() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.cell(0, 2));
    t.key(egui::Key::Tab);
    assert_eq!(
        t.app().left.selected_csv_cell,
        Some(CsvCellPosition { row: 0, column: 3 })
    );
    t.ctrl(egui::Key::S);
    // DISCREPANCY(D13): moving the cursor past the last column (Tab / ArrowRight) adds an
    // empty column, and it is written to disk on save although nothing was typed.
    assert_eq!(
        t.read("data.csv"),
        "name,qty,note,\napple,1,red,\nbanana,22,\"yellow, long\","
    );

    // Past the last row: a new row appears in the buffer (trailing empty rows are trimmed
    // on save).
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::Enter);
    assert_eq!(t.app().left.selected_csv_cell.unwrap().row, 4);
    t.key(egui::Key::End);
    assert_eq!(t.app().left.selected_csv_cell.unwrap().column, 3);
    t.key(egui::Key::ArrowRight);
    assert_eq!(t.app().left.selected_csv_cell.unwrap().column, 4);
}

#[test]
fn row_and_column_headers_select_and_copy() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.row_header(1));
    assert_eq!(
        t.app().left.csv_selection,
        Some(CsvSelection::Rows { start: 1, end: 1 })
    );
    assert!(t.has_label("Selected row 2"));
    t.copy_event();
    assert_eq!(t.platform.clipboard_text().as_deref(), Some("apple,1,red"));
    t.assert_status("Copied CSV selection: 11 chars.");

    t.drag(g.row_header(1), g.row_header(2));
    assert_eq!(
        t.app().left.csv_selection,
        Some(CsvSelection::Rows { start: 1, end: 2 })
    );
    assert!(t.has_label("Selected rows 2-3"));

    t.click_at(g.column_header(1));
    assert_eq!(
        t.app().left.csv_selection,
        Some(CsvSelection::Columns { start: 1, end: 1 })
    );
    assert!(t.has_label("Selected column B"));
    t.drag(g.column_header(0), g.column_header(2));
    assert_eq!(
        t.app().left.csv_selection,
        Some(CsvSelection::Columns { start: 0, end: 2 })
    );
    assert!(t.has_label("Selected columns A-C"));
    t.copy_event();
    assert_eq!(t.platform.clipboard_text().as_deref(), Some(CSV.trim_end()));

    t.platform.clipboard_fails.set(true);
    t.copy_event();
    t.assert_status("CSV copy requested, but system clipboard failed: clipboard unavailable");

    // Clicking a cell clears the header selection.
    t.click_at(g.cell(2, 2));
    assert!(t.app().left.csv_selection.is_none());
}

#[test]
fn paste_and_delete_rows_and_columns() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.row_header(2));
    t.paste_event("x,y");
    t.assert_status("Pasted CSV into 2 cells.");
    assert!(text(&t).contains("x,y,\"yellow, long\""));

    t.platform.set_clipboard("p,q,r");
    t.key(egui::Key::Paste);
    t.assert_status("Pasted CSV into 3 cells.");
    t.platform.clipboard_fails.set(true);
    t.key(egui::Key::Paste);
    t.assert_status("CSV paste requested, but clipboard read failed: clipboard unavailable");
    t.paste_event("\"broken");
    t.assert_status("CSV paste failed: unterminated quoted field");

    t.key(egui::Key::Delete);
    t.assert_status("Deleted CSV row 3.");
    assert_eq!(text(&t), "name,qty,note\napple,1,red");

    t.click_at(g.column_header(1));
    t.key(egui::Key::Backspace);
    t.assert_status("Deleted CSV column B.");
    assert_eq!(text(&t), "name,note\napple,red");

    t.drag(g.column_header(0), g.column_header(1));
    t.key(egui::Key::Delete);
    t.assert_status("Deleted CSV columns A-B.");

    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Z);
    assert_eq!(text(&t), CSV);
}

#[test]
fn delete_all_rows_keeps_one_empty_row() {
    let mut t = TestApp::builder().file("one.csv", "a,b\n").build();
    t.click_first("one.csv");
    t.steps(2);
    let g = grid(&t);
    t.click_at(g.row_header(0));
    t.key(egui::Key::Delete);
    t.assert_status("Deleted CSV row 1.");
    t.drag(g.row_header(0), g.row_header(0));
}

#[test]
fn read_only_and_unparsable_csv() {
    let mut t = TestApp::builder()
        .file("bad.csv", "a,\"unterminated\n")
        .file("empty.csv", "")
        .build();
    t.click_first("bad.csv");
    t.steps(2);
    assert!(t.has_label("CSV parse error: unterminated quoted field"));
    // Saving keeps the original text when it does not parse.
    t.ctrl(egui::Key::S);
    t.assert_status_containing("Saved file:");
    assert_eq!(t.read("bad.csv"), "a,\"unterminated\n");

    t.click_first("empty.csv");
    t.steps(2);
    assert!(!t.has_label("Empty CSV"), "an empty file is one empty cell");
}

#[test]
fn csv_files_in_library_folders_are_read_only() {
    let mut t = TestApp::plain();
    let library = t.tmp.join(".cargo").join("registry").join("data");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("ro.csv"), "a,b\n1,2\n").unwrap();
    t.platform.queue_folder_pick(&library);
    t.click_nth("Add", 0);
    t.click_first("ro.csv");
    t.steps(2);
    assert!(t.app().left.read_only);
    let g = grid(&t);
    t.click_at(g.row_header(0));
    t.paste_event("x");
    t.assert_status("CSV paste failed: Focused file is read-only.");
    t.key(egui::Key::Delete);
    t.assert_status("CSV delete failed: Focused file is read-only.");
}

#[test]
fn source_mode_edits_text_and_searches() {
    let mut t = csv_app();
    t.click("Source");
    assert_eq!(t.app().left.csv_mode, CsvMode::Source);
    assert!(t.has_label("Source"));
    t.ctrl(egui::Key::F);
    assert!(t.app().left.source_search.open);
    t.type_text("banana");
    assert!(t.has_label("1/1"));
    // Switching back to Grid closes the source search.
    t.click("Grid");
    assert_eq!(t.app().left.csv_mode, CsvMode::Grid);
    assert!(!t.app().left.source_search.open);
    t.click("Grid");
    // Ctrl+F in grid mode falls back to Search Everywhere.
    t.ctrl(egui::Key::F);
    assert!(t.app().search_overlay.open);
}

#[test]
fn csv_selection_in_the_unfocused_pane_still_handles_shortcuts() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.row_header(1));
    t.click("Right Pane");
    let right = t.nth_label_rect("No file open", 0);
    t.click_at(right.center() + egui::vec2(0.0, 80.0));
    assert_eq!(t.app().focused_pane, super::super::Pane::Right);
    t.copy_event();
    assert_eq!(t.platform.clipboard_text().as_deref(), Some("apple,1,red"));
    t.paste_event("z");
    t.assert_status("Pasted CSV into 1 cells.");
    assert_eq!(t.app().focused_pane, super::super::Pane::Left);
    t.click_at(right.center() + egui::vec2(0.0, 80.0));
    t.key_with(egui::Modifiers::NONE, egui::Key::Delete);
    t.assert_status("Deleted CSV row 2.");
    let _ = CTRL;
}

#[test]
fn ctrl_shift_z_undoes_in_the_grid() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.cell(1, 1));
    t.ctrl(egui::Key::A);
    t.type_text("5");
    t.ctrl(egui::Key::Z);
    assert!(text(&t).contains("apple,1,red"));
    t.ctrl_shift(egui::Key::Z);
    // DISCREPANCY(D10): Help lists Ctrl+Shift+Z as "Redo CSV grid edit", but it undoes.
    t.assert_status("No CSV edit to undo.");
    assert!(text(&t).contains("apple,1,red"));
}

#[test]
fn crlf_files_and_escaped_quotes_parse() {
    let mut t = TestApp::builder()
        .file("crlf.csv", "a,\"say \"\"hi\"\"\"\r\n1,2\r\n")
        .build();
    t.click_first("crlf.csv");
    t.steps(2);
    let g = grid(&t);
    t.click_at(g.row_header(0));
    t.copy_event();
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("a,\"say \"\"hi\"\"\"")
    );
    // Pasting a wider row grows it.
    t.paste_event("w,x,y,z");
    t.assert_status("Pasted CSV into 4 cells.");
    // Dragging upwards over the row headers selects the range in either direction.
    t.drag(g.row_header(1), g.row_header(0));
    assert!(t.has_label("Selected rows 1-2"));
}

#[test]
fn copy_when_the_grid_no_longer_parses() {
    let mut t = csv_app();
    let g = grid(&t);
    t.click_at(g.row_header(1));
    // Break the text from Source mode; the row selection survives the switch back.
    t.click("Source");
    t.click_text(0, 0);
    t.ctrl(egui::Key::End);
    // Paste a lone quote at the end (typing one would be auto-paired).
    t.paste_event("\"");
    assert!(super::super::parse_csv(&text(&t)).is_err());
    t.click("Grid");
    t.copy_event();
    t.assert_status("No CSV row or column selection to copy.");
}

#[test]
fn csv_selection_in_the_right_pane_while_the_left_is_focused() {
    let mut t = TestApp::builder()
        .file("left.txt", "left\n")
        .file("right.csv", "a,b\n1,2\n")
        .build();
    t.click_first("left.txt");
    let from = t.label_rect("right.csv").center();
    t.drag(from, egui::pos2(1250.0, 400.0));
    t.steps(2);
    let g = grid(&t);
    t.click_at(g.row_header(1));
    // Focus the left pane by clicking into its text.
    t.click_at(egui::pos2(500.0, 300.0));
    assert_eq!(t.app().focused_pane, super::super::Pane::Left);
    t.paste_event("9,9");
    t.assert_status("Pasted CSV into 2 cells.");
    assert_eq!(t.app().focused_pane, super::super::Pane::Right);
    t.click_at(egui::pos2(500.0, 300.0));
    t.key(egui::Key::Delete);
    t.assert_status("Deleted CSV row 2.");
}

#[test]
fn csv_edit_history_is_capped() {
    let mut t = csv_app();
    let g = grid(&t);
    for index in 0..260 {
        t.click_at(g.row_header(index % 2));
        t.paste_event(&format!("{index}"));
    }
    assert_eq!(t.app().left.csv_undo_stack.len(), 256);
}
