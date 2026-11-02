//! The Help window. What it lists comes from `keymap`.

use super::*;

impl CodeReaderApp {
    pub(super) fn show_shortcuts_help_window(&mut self, ui: &mut egui::Ui) {
        if !self.layout.shortcuts_help_open {
            return;
        }

        let groups = self.shortcut_help_groups();
        egui::Window::new("Help")
            .id(egui::Id::new("shortcuts_help_window"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 56.0))
            .fixed_size(egui::vec2(520.0, 560.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.shortcut_context_title()).strong());
                    ui.separator();
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                ScrollArea::vertical()
                    .id_salt("shortcuts_help_groups")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (group_index, group) in groups.iter().enumerate() {
                            if group_index > 0 {
                                ui.add_space(10.0);
                            }
                            help_group_ui(ui, group);
                            ui.add_space(8.0);
                        }
                    });
            });
    }
}

const HELP_ROW_HEIGHT: f32 = 24.0;

/// A Help row split into its Shortcut and Action cells.
fn help_row_cells(ui: &mut egui::Ui) -> (egui::Rect, egui::Rect) {
    const SHORTCUT_WIDTH: f32 = 164.0;
    const COLUMN_GAP: f32 = 20.0;
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HELP_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let shortcut_rect =
        egui::Rect::from_min_size(row_rect.min, egui::vec2(SHORTCUT_WIDTH, HELP_ROW_HEIGHT));
    let action_rect = egui::Rect::from_min_max(
        egui::pos2(shortcut_rect.right() + COLUMN_GAP, row_rect.top()),
        row_rect.right_bottom(),
    );
    (shortcut_rect, action_rect)
}

fn help_group_ui(ui: &mut egui::Ui, group: &ShortcutGroup) {
    let body_font = TextStyle::Body.resolve(ui.style());
    let monospace_font = TextStyle::Monospace.resolve(ui.style());
    let text_color = ui.visuals().text_color();
    let weak_text_color = ui.visuals().weak_text_color();

    let (group_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HELP_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(
        group_rect.shrink2(egui::vec2(0.0, 2.0)),
        2.0,
        ui.visuals().extreme_bg_color,
    );
    paint_row_text(
        ui,
        group_rect.shrink2(egui::vec2(6.0, 0.0)),
        group.title,
        &body_font,
        text_color,
    );
    ui.add_space(4.0);

    let (shortcut_header, action_header) = help_row_cells(ui);
    paint_row_text(ui, shortcut_header, "Shortcut", &body_font, weak_text_color);
    paint_row_text(ui, action_header, "Action", &body_font, weak_text_color);
    ui.separator();

    for shortcut in &group.shortcuts {
        let (shortcut_rect, action_rect) = help_row_cells(ui);
        paint_row_text(
            ui,
            shortcut_rect,
            shortcut.keys,
            &monospace_font,
            text_color,
        );
        paint_row_text(ui, action_rect, shortcut.label, &body_font, text_color);
    }
}
