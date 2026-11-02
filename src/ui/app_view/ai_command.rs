//! The AI Cmd window: the CLI agent command for the current workspace.

use super::*;

impl CodeReaderApp {
    pub(super) fn cli_agent_command(&self) -> Result<CliAgentCommand, String> {
        let roots = self
            .current_session()
            .map(|session| session.root_folders.as_slice())
            .unwrap_or(&[]);
        build_cli_agent_command(&self.app_state.cli_agent, roots)
    }

    pub(super) fn copy_cli_agent_command(&mut self) {
        match self.cli_agent_command() {
            Ok(command) => match copy_text_to_system_clipboard(&command.command) {
                Ok(()) => {
                    self.status_message = format!(
                        "Copied {} command: base {}.",
                        command.agent_name,
                        command.base_dir.display()
                    );
                }
                Err(error) => {
                    self.status_message = format!("AI command copy failed: {error}");
                }
            },
            Err(error) => {
                self.status_message = format!("AI command unavailable: {error}");
            }
        }
    }

    pub(super) fn show_ai_command_window(&mut self, ui: &mut egui::Ui) {
        if !self.ai_command_open {
            return;
        }

        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        let mut close = false;
        let mut copy = false;
        let mut changed = false;

        egui::Window::new("AI Cmd")
            .id(egui::Id::new("ai_command_window"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 88.0))
            .fixed_size(egui::vec2(520.0, 300.0))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Esc: close").weak());
                });
                ui.separator();

                {
                    let settings = &mut self.app_state.cli_agent;
                    ui.label("Command");
                    changed |= ui
                        .add(
                            TextEdit::singleline(&mut settings.executable)
                                .desired_width(f32::INFINITY),
                        )
                        .changed();
                    ui.add_space(6.0);

                    ui.label("Add dir option");
                    changed |= ui
                        .add(
                            TextEdit::singleline(&mut settings.add_dir_arg)
                                .desired_width(f32::INFINITY),
                        )
                        .changed();
                }

                ui.add_space(8.0);
                ui.label(RichText::new("Preview").strong());
                match build_cli_agent_command(&self.app_state.cli_agent, &roots) {
                    Ok(command) => {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(command.preview).monospace());
                        });
                    }
                    Err(error) => {
                        ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_copy =
                        build_cli_agent_command(&self.app_state.cli_agent, &roots).is_ok();
                    if ui
                        .add_enabled(can_copy, egui::Button::new("Copy"))
                        .clicked()
                    {
                        copy = true;
                    }
                });
            });

        if changed {
            self.save_state_silent();
        }
        if copy {
            self.copy_cli_agent_command();
        }
        if close {
            self.ai_command_open = false;
        }
    }
}
