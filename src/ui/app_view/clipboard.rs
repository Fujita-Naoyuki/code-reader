//! Copy, paste and delete for selections that egui widgets do not handle themselves
//! (large-file editor, diff view, CSV grid).

use super::*;

impl CodeReaderApp {
    pub(super) fn has_csv_selection(&self) -> bool {
        self.left.has_active_csv_selection() || self.right.has_active_csv_selection()
    }

    pub(super) fn copy_virtual_selection_to_clipboard(&mut self, ui: &egui::Ui) -> bool {
        let Some(text) = self.focused_editor().virtual_selection_text() else {
            return false;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied virtual selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message = format!(
                    "Virtual selection copy requested, but system clipboard failed: {error}"
                );
            }
        }
        true
    }

    pub(super) fn copy_diff_selection_to_clipboard(&mut self, ui: &egui::Ui) -> bool {
        let Some(text) = self.focused_editor().diff_selection_text() else {
            return false;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied diff selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message =
                    format!("Diff selection copy requested, but system clipboard failed: {error}");
            }
        }
        true
    }

    pub(super) fn copy_selected_csv_to_clipboard(&mut self, ui: &egui::Ui) {
        let Some(text) = self
            .focused_editor()
            .csv_selection_text()
            .or_else(|| self.left.csv_selection_text())
            .or_else(|| self.right.csv_selection_text())
        else {
            self.status_message = "No CSV row or column selection to copy.".to_owned();
            return;
        };

        ui.ctx().copy_text(text.clone());

        match copy_text_to_system_clipboard(&text) {
            Ok(()) => {
                self.status_message =
                    format!("Copied CSV selection: {} chars.", text.chars().count());
            }
            Err(error) => {
                self.status_message =
                    format!("CSV copy requested, but system clipboard failed: {error}");
            }
        }
    }

    pub(super) fn paste_csv_text_from_clipboard(&mut self, text: &str) {
        let pane = if self.focused_editor().has_active_csv_selection() {
            Some(self.focused_pane)
        } else if self.left.has_active_csv_selection() {
            Some(Pane::Left)
        } else if self.right.has_active_csv_selection() {
            Some(Pane::Right)
        } else {
            None
        };

        let Some(pane) = pane else {
            self.status_message = "No CSV row or column selection to paste into.".to_owned();
            return;
        };

        let result = match pane {
            Pane::Left => self.left.paste_csv_text(text),
            Pane::Right => self.right.paste_csv_text(text),
        };

        match result {
            Ok(cell_count) => {
                self.focused_pane = pane;
                self.status_message = format!("Pasted CSV into {cell_count} cells.");
            }
            Err(error) => {
                self.status_message = format!("CSV paste failed: {error}");
            }
        }
    }

    pub(super) fn delete_selected_csv(&mut self) {
        let pane = if self.focused_editor().has_active_csv_selection() {
            Some(self.focused_pane)
        } else if self.left.has_active_csv_selection() {
            Some(Pane::Left)
        } else if self.right.has_active_csv_selection() {
            Some(Pane::Right)
        } else {
            None
        };

        let Some(pane) = pane else {
            self.status_message = "No CSV row or column selection to delete.".to_owned();
            return;
        };

        let result = match pane {
            Pane::Left => self.left.delete_csv_selection(),
            Pane::Right => self.right.delete_csv_selection(),
        };

        match result {
            Ok(label) => {
                self.focused_pane = pane;
                self.status_message = format!("Deleted CSV {label}.");
            }
            Err(error) => {
                self.status_message = format!("CSV delete failed: {error}");
            }
        }
    }
}
