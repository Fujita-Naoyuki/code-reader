//! The Help window. What it lists comes from `keymap`.

use super::*;

impl CodeReaderApp {
    pub(super) fn show_shortcuts_help_window(&mut self, ui: &mut egui::Ui) {
        if !self.shortcuts_help_open {
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

                let row_height = 24.0;
                let shortcut_width = 164.0;
                let column_gap = 20.0;
                let body_font = TextStyle::Body.resolve(ui.style());
                let monospace_font = TextStyle::Monospace.resolve(ui.style());
                let weak_text_color = ui.visuals().weak_text_color();
                let draw_left_text = |ui: &egui::Ui,
                                      rect: egui::Rect,
                                      text: &str,
                                      font: egui::FontId,
                                      color: egui::Color32| {
                    ui.painter().with_clip_rect(rect).text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font,
                        color,
                    );
                };

                ScrollArea::vertical()
                    .id_salt("shortcuts_help_groups")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (group_index, group) in groups.into_iter().enumerate() {
                            let row_width = ui.available_width();
                            if group_index > 0 {
                                ui.add_space(10.0);
                            }

                            let (group_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                group_rect.shrink2(egui::vec2(0.0, 2.0)),
                                2.0,
                                ui.visuals().extreme_bg_color,
                            );
                            draw_left_text(
                                ui,
                                group_rect.shrink2(egui::vec2(6.0, 0.0)),
                                group.title,
                                body_font.clone(),
                                ui.visuals().text_color(),
                            );
                            ui.add_space(4.0);

                            let (header_rect, _) = ui.allocate_exact_size(
                                egui::vec2(row_width, row_height),
                                egui::Sense::hover(),
                            );
                            let shortcut_header_rect = egui::Rect::from_min_size(
                                header_rect.min,
                                egui::vec2(shortcut_width, row_height),
                            );
                            let action_header_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    shortcut_header_rect.right() + column_gap,
                                    header_rect.top(),
                                ),
                                header_rect.right_bottom(),
                            );
                            draw_left_text(
                                ui,
                                shortcut_header_rect,
                                "Shortcut",
                                body_font.clone(),
                                weak_text_color,
                            );
                            draw_left_text(
                                ui,
                                action_header_rect,
                                "Action",
                                body_font.clone(),
                                weak_text_color,
                            );
                            ui.separator();

                            for shortcut in group.shortcuts {
                                let (row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(row_width, row_height),
                                    egui::Sense::hover(),
                                );
                                let shortcut_rect = egui::Rect::from_min_size(
                                    row_rect.min,
                                    egui::vec2(shortcut_width, row_height),
                                );
                                let action_rect = egui::Rect::from_min_max(
                                    egui::pos2(shortcut_rect.right() + column_gap, row_rect.top()),
                                    row_rect.right_bottom(),
                                );
                                draw_left_text(
                                    ui,
                                    shortcut_rect,
                                    shortcut.keys,
                                    monospace_font.clone(),
                                    ui.visuals().text_color(),
                                );
                                draw_left_text(
                                    ui,
                                    action_rect,
                                    shortcut.label,
                                    body_font.clone(),
                                    ui.visuals().text_color(),
                                );
                            }
                            ui.add_space(8.0);
                        }
                    });
            });
    }
}
