//! Which shortcuts apply right now: the Help window and the status bar hints.

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

    pub(super) fn shortcut_context_title(&self) -> &'static str {
        match self.top_dialog() {
            Some(Modal::SearchEverywhere) => return "Search Everywhere",
            Some(Modal::RecentWorkspaces) => return "Recent Workspaces",
            Some(Modal::GitCreateBranch) => return "Create Branch",
            Some(Modal::GitSwitchBranch) => return "Switch Branch",
            Some(Modal::GitDiscard) => return "Discard Changes",
            Some(Modal::UnsavedChanges) => return "Unsaved Changes",
            Some(Modal::RenameWorkspace) => return "Rename Workspace",
            _ => {}
        }
        if self.sidebar_view == SidebarView::Search && self.workspace_search.open {
            return "Workspace Search";
        }
        if self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
        {
            return "Workspace Files";
        }

        let editor = self.focused_editor();
        if editor.markdown.search.open {
            "Markdown Search"
        } else if editor.source_search.open {
            "Source Search"
        } else if editor.is_markdown() {
            match editor.markdown.mode {
                MarkdownMode::Preview => match editor.markdown.focus_target {
                    MarkdownFocusTarget::Pages => "Markdown Pages",
                    MarkdownFocusTarget::Outline => "Markdown Outline",
                    MarkdownFocusTarget::Body => "Markdown Preview",
                },
                MarkdownMode::Markdown => "Markdown Source",
                MarkdownMode::Both => match editor.markdown.focus_target {
                    MarkdownFocusTarget::Pages => "Markdown Pages",
                    MarkdownFocusTarget::Outline => "Markdown Outline",
                    MarkdownFocusTarget::Body => "Markdown Both",
                },
            }
        } else if editor.is_csv() && editor.csv.mode == CsvMode::Grid {
            "CSV Grid"
        } else if editor.diff_view().is_some() {
            "Diff View"
        } else if editor.git_history_view().is_some() {
            "Git History"
        } else {
            "Source"
        }
    }

    pub(super) fn status_bar_shortcuts(&self) -> Vec<ShortcutHint> {
        match self.top_dialog() {
            Some(Modal::SearchEverywhere) => {
                return shortcuts(&[("Enter", "Open"), ("Up/Down", "Select"), ("Esc", "Close")]);
            }
            Some(Modal::RecentWorkspaces) => return shortcuts(&[("Esc", "Close")]),
            Some(
                Modal::GitCreateBranch
                | Modal::GitSwitchBranch
                | Modal::GitDiscard
                | Modal::UnsavedChanges
                | Modal::RenameWorkspace,
            ) => return shortcuts(&[("Enter", "Confirm"), ("Esc", "Close")]),
            _ => {}
        }
        if self.sidebar_view == SidebarView::Search && self.workspace_search.open {
            return shortcuts(&[
                ("Enter", "Search"),
                ("Esc", "Clear"),
                ("Ctrl+Shift+F", "Workspace"),
            ]);
        }
        if self.show_workspace_sidebar
            && self.sidebar_view == SidebarView::Workspace
            && self.workspace_sidebar_active
        {
            return shortcuts(&[("F2", "Rename"), ("Delete", "Delete")]);
        }

        let editor = self.focused_editor();
        if editor.is_markdown() {
            return match editor.markdown.mode {
                MarkdownMode::Preview => match editor.markdown.focus_target {
                    MarkdownFocusTarget::Pages => {
                        shortcuts(&[("Up/Down", "Page"), ("Enter", "Open"), ("Ctrl+F", "Find")])
                    }
                    MarkdownFocusTarget::Outline => shortcuts(&[
                        ("Up/Down", "Heading"),
                        ("Enter", "Jump"),
                        ("Ctrl+F", "Find"),
                    ]),
                    MarkdownFocusTarget::Body => shortcuts(&[
                        ("Ctrl+PgUp/PgDn", "Page"),
                        ("Ctrl+F", "Find"),
                        ("Alt+1/2/3", "Mode"),
                    ]),
                },
                MarkdownMode::Markdown => shortcuts(&[
                    ("Alt+1/2/3", "Mode"),
                    ("Ctrl+F", "Find"),
                    ("Ctrl+S", "Save"),
                    ("F12", "Definition"),
                ]),
                MarkdownMode::Both => shortcuts(&[
                    ("Ctrl+PgUp/PgDn", "Page"),
                    ("Ctrl+F", "Find"),
                    ("Alt+1/2/3", "Mode"),
                ]),
            };
        }
        if editor.is_csv() && editor.csv.mode == CsvMode::Grid {
            return if editor.has_active_csv_selection() {
                shortcuts(&[
                    ("Ctrl+C", "Copy Row/Column"),
                    ("Ctrl+V", "Paste Row/Column"),
                    ("Delete", "Remove Row/Column"),
                    ("Ctrl+Z/Y", "Undo/Redo"),
                ])
            } else {
                shortcuts(&[
                    ("Tab", "Next Cell"),
                    ("Enter", "Next Row"),
                    ("Arrows", "Move Cell"),
                    ("Ctrl+Z/Y", "Undo/Redo"),
                ])
            };
        }
        if editor.diff_view().is_some() || editor.git_history_view().is_some() {
            return shortcuts(&[("PgUp/PgDn", "Scroll"), ("Alt+Left", "Back")]);
        }

        shortcuts(&[
            ("F12", "Definition"),
            ("Ctrl+F", "Find"),
            ("Alt+Left/Right", "History"),
            ("Ctrl+S", "Save"),
        ])
    }

    pub(super) fn shortcut_help_groups(&self) -> Vec<ShortcutGroup> {
        let mut groups = vec![ShortcutGroup {
            title: self.shortcut_context_title(),
            shortcuts: self.context_shortcuts(),
        }];
        groups.push(ShortcutGroup {
            title: "Common",
            shortcuts: shortcuts(&[
                ("Ctrl+F", "Find current file"),
                ("Ctrl+Shift+F", "Workspace Search"),
                ("Ctrl+P", "Search Everywhere"),
                ("Ctrl+Shift+O", "Search Everywhere"),
                ("Shift x2", "Search Everywhere"),
                ("Ctrl+Shift+A", "Copy AI command"),
                ("Alt+Left", "Back"),
                ("Alt+Right", "Forward"),
                ("F12", "Go to Definition"),
                ("Ctrl+S", "Save"),
                ("F1", "Show Help"),
            ]),
        });
        groups.push(ShortcutGroup {
            title: "Search",
            shortcuts: shortcuts(&[
                ("Enter", "Next / open selected result"),
                ("Shift+Enter", "Previous match"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Esc", "Close"),
                ("Up/Down", "Move selection"),
            ]),
        });
        groups
    }

    pub(super) fn context_shortcuts(&self) -> Vec<ShortcutHint> {
        match self.shortcut_context_title() {
            "Search Everywhere" => shortcuts(&[
                ("Enter", "Open selected result"),
                ("Up/Down", "Move selection"),
                ("Esc", "Close"),
            ]),
            "Recent Workspaces" => shortcuts(&[("Esc", "Close")]),
            "Switch Branch" => shortcuts(&[
                ("Up/Down", "Select branch"),
                ("Enter", "Switch"),
                ("Esc", "Close"),
            ]),
            "Create Branch" | "Discard Changes" | "Unsaved Changes" | "Rename Workspace" => {
                shortcuts(&[("Enter", "Confirm"), ("Esc", "Close")])
            }
            "Workspace Search" => shortcuts(&[("Enter", "Run search"), ("Esc", "Clear")]),
            "Workspace Files" => shortcuts(&[
                ("F2", "Rename selected file"),
                ("Delete", "Delete selected file"),
                ("Esc", "Close dialogs"),
            ]),
            "Markdown Search" | "Source Search" => shortcuts(&[
                ("Enter", "Next match"),
                ("Shift+Enter", "Previous match"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Esc", "Close"),
            ]),
            "Markdown Preview" | "Markdown Both" => shortcuts(&[
                ("Alt+1", "Preview mode"),
                ("Alt+2", "Markdown source mode"),
                ("Alt+3", "Both mode"),
                ("Ctrl+PageUp", "Previous page"),
                ("Ctrl+PageDown", "Next page"),
                ("Ctrl+F", "Find rendered Markdown"),
                ("F6 / Shift+F6", "Move to Pages / Outline"),
            ]),
            "Markdown Pages" => shortcuts(&[
                ("Up/Down", "Select page"),
                ("Enter", "Open page"),
                ("Ctrl+PageUp", "Previous page"),
                ("Ctrl+PageDown", "Next page"),
                ("F6 / Shift+F6", "Next / previous pane"),
            ]),
            "Markdown Outline" => shortcuts(&[
                ("Up/Down", "Select heading"),
                ("Enter", "Jump to heading"),
                ("Ctrl+F", "Find rendered Markdown"),
                ("F6 / Shift+F6", "Next / previous pane"),
            ]),
            "Markdown Source" => shortcuts(&[
                ("Alt+1/2/3", "Change Markdown mode"),
                ("Ctrl+F", "Find source"),
                ("F12", "Go to Definition"),
                ("Ctrl+S", "Save"),
            ]),
            "CSV Grid" => shortcuts(&[
                ("Ctrl+Z", "Undo CSV grid edit"),
                ("Ctrl+Y", "Redo CSV grid edit"),
                ("Ctrl+Shift+Z", "Redo CSV grid edit"),
                ("Ctrl+C", "Copy selected row or column as CSV"),
                ("Ctrl+V", "Paste CSV into selected row or column"),
                ("Delete", "Delete selected row or column"),
                ("Backspace", "Delete selected row or column"),
                ("Tab", "Next cell"),
                ("Enter", "Next row"),
                ("Home/End", "First / last column"),
                ("Arrow keys", "Move between cells"),
            ]),
            "Diff View" | "Git History" => {
                shortcuts(&[("PageUp", "Page up"), ("PageDown", "Page down")])
            }
            _ => shortcuts(&[
                ("F12", "Go to Definition"),
                ("Ctrl+F", "Find source"),
                ("F3", "Next match"),
                ("Shift+F3", "Previous match"),
                ("Ctrl+A", "Select all"),
                ("Ctrl+C", "Copy selection"),
                ("Ctrl+V", "Paste"),
                ("Ctrl+Z", "Undo"),
                ("Ctrl+Y", "Redo"),
                ("Ctrl+Shift+Z", "Redo"),
                ("Arrows", "Move cursor"),
                ("Shift+Arrows", "Extend selection"),
                ("PageUp/PageDown", "Page scroll"),
            ]),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ShortcutHint {
    pub(super) keys: &'static str,
    pub(super) label: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct ShortcutGroup {
    pub(super) title: &'static str,
    pub(super) shortcuts: Vec<ShortcutHint>,
}

pub(super) fn shortcuts(items: &[(&'static str, &'static str)]) -> Vec<ShortcutHint> {
    items
        .iter()
        .map(|(keys, label)| ShortcutHint { keys, label })
        .collect()
}

pub(super) fn consume_copy_shortcut(input: &mut egui::InputState) -> bool {
    let mut copied = false;
    input.events.retain(|event| {
        let is_copy_event = matches!(
            event,
            egui::Event::Copy
                | egui::Event::Key {
                    key: egui::Key::Copy,
                    pressed: true,
                    ..
                }
        );
        copied |= is_copy_event;
        !is_copy_event
    });

    copied
        || input.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
        || input.consume_key(egui::Modifiers::CTRL, egui::Key::C)
}

pub(super) fn consume_paste_text(input: &mut egui::InputState) -> Option<String> {
    let mut pasted = None;
    input.events.retain(|event| {
        if let egui::Event::Paste(text) = event {
            pasted = Some(text.clone());
            false
        } else {
            true
        }
    });
    pasted
}

pub(super) fn consume_paste_shortcut(input: &mut egui::InputState) -> bool {
    let mut pasted = false;
    input.events.retain(|event| {
        let is_paste_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Paste,
                pressed: true,
                ..
            }
        );
        pasted |= is_paste_key;
        !is_paste_key
    });

    pasted
        || input.consume_key(egui::Modifiers::COMMAND, egui::Key::V)
        || input.consume_key(egui::Modifiers::CTRL, egui::Key::V)
}

pub(super) fn consume_undo_shortcut(input: &mut egui::InputState) -> bool {
    let mut undo = false;
    input.events.retain(|event| {
        let is_undo_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Z,
                pressed: true,
                modifiers,
                ..
            } if (modifiers.ctrl || modifiers.command) && !modifiers.shift
        );
        undo |= is_undo_key;
        !is_undo_key
    });

    // No consume_key fallback: it ignores extra Shift and would swallow Ctrl+Shift+Z (redo).
    undo
}

pub(super) fn consume_redo_shortcut(input: &mut egui::InputState) -> bool {
    let mut redo = false;
    input.events.retain(|event| {
        let is_redo_key = matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Y,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command
        ) || matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Z,
                pressed: true,
                modifiers,
                ..
            } if (modifiers.ctrl || modifiers.command) && modifiers.shift
        );
        redo |= is_redo_key;
        !is_redo_key
    });

    redo
}

pub(super) fn consume_delete_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
        || input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
}

pub(super) fn consume_navigation_back_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft)
        || input.consume_key(egui::Modifiers::NONE, egui::Key::BrowserBack)
}

pub(super) fn consume_navigation_forward_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight)
}

pub(super) fn consume_ai_command_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(
        egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
        egui::Key::A,
    )
}

pub(super) fn raw_enter_pressed(input: &egui::InputState) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                ..
            }
        )
    })
}

pub(super) fn raw_search_enter_direction(input: &egui::InputState) -> Option<isize> {
    input.events.iter().find_map(|event| match event {
        egui::Event::Key {
            key: egui::Key::Enter,
            pressed: true,
            modifiers,
            ..
        } => Some(if modifiers.shift { -1 } else { 1 }),
        _ => None,
    })
}

pub(super) fn raw_escape_pressed(input: &egui::InputState) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        )
    })
}
