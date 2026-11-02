//! Keyboard shortcuts: one table of what each context offers, read by the Help window and
//! the status bar, and the key checks shared by the handlers.
//!
//! Changing a binding means changing its handler and its row here. Check the modified
//! combination first: `consume_key(NONE, key)` also matches Shift+key.

use super::*;

/// (keys, what they do)
type Hints = &'static [(&'static str, &'static str)];

/// Where the keyboard is, which decides what the shortcuts do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutContext {
    SearchEverywhere,
    RecentWorkspaces,
    CreateBranch,
    SwitchBranch,
    DiscardChanges,
    UnsavedChanges,
    RenameWorkspace,
    WorkspaceSearch,
    WorkspaceFiles,
    MarkdownSearch,
    SourceSearch,
    MarkdownPreview,
    MarkdownBoth,
    MarkdownPages,
    MarkdownOutline,
    MarkdownSource,
    CsvGrid,
    DiffView,
    GitHistory,
    Source,
}

const CONFIRM_HELP: Hints = &[("Enter", "Confirm"), ("Esc", "Close")];
const FIND_HELP: Hints = &[
    ("Enter", "Next match"),
    ("Shift+Enter", "Previous match"),
    ("F3", "Next match"),
    ("Shift+F3", "Previous match"),
    ("Esc", "Close"),
];
const MARKDOWN_VIEW_HELP: Hints = &[
    ("Alt+1", "Preview mode"),
    ("Alt+2", "Markdown source mode"),
    ("Alt+3", "Both mode"),
    ("Ctrl+PageUp", "Previous page"),
    ("Ctrl+PageDown", "Next page"),
    ("Ctrl+F", "Find rendered Markdown"),
    ("F6 / Shift+F6", "Move to Pages / Outline"),
];
const MARKDOWN_VIEW_STATUS: Hints = &[
    ("Ctrl+PgUp/PgDn", "Page"),
    ("Ctrl+F", "Find"),
    ("Alt+1/2/3", "Mode"),
];
const SCROLLED_VIEW_HELP: Hints = &[("PageUp", "Page up"), ("PageDown", "Page down")];
const SCROLLED_VIEW_STATUS: Hints = &[("PgUp/PgDn", "Scroll"), ("Alt+Left", "Back")];
const SOURCE_STATUS: Hints = &[
    ("F12", "Definition"),
    ("Ctrl+F", "Find"),
    ("Alt+Left/Right", "History"),
    ("Ctrl+S", "Save"),
];
const CSV_SELECTION_STATUS: Hints = &[
    ("Ctrl+C", "Copy Row/Column"),
    ("Ctrl+V", "Paste Row/Column"),
    ("Delete", "Remove Row/Column"),
    ("Ctrl+Z/Y", "Undo/Redo"),
];
const COMMON_HELP: Hints = &[
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
];
const SEARCH_HELP: Hints = &[
    ("Enter", "Next / open selected result"),
    ("Shift+Enter", "Previous match"),
    ("F3", "Next match"),
    ("Shift+F3", "Previous match"),
    ("Esc", "Close"),
    ("Up/Down", "Move selection"),
];

impl ShortcutContext {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::SearchEverywhere => "Search Everywhere",
            Self::RecentWorkspaces => "Recent Workspaces",
            Self::CreateBranch => "Create Branch",
            Self::SwitchBranch => "Switch Branch",
            Self::DiscardChanges => "Discard Changes",
            Self::UnsavedChanges => "Unsaved Changes",
            Self::RenameWorkspace => "Rename Workspace",
            Self::WorkspaceSearch => "Workspace Search",
            Self::WorkspaceFiles => "Workspace Files",
            Self::MarkdownSearch => "Markdown Search",
            Self::SourceSearch => "Source Search",
            Self::MarkdownPreview => "Markdown Preview",
            Self::MarkdownBoth => "Markdown Both",
            Self::MarkdownPages => "Markdown Pages",
            Self::MarkdownOutline => "Markdown Outline",
            Self::MarkdownSource => "Markdown Source",
            Self::CsvGrid => "CSV Grid",
            Self::DiffView => "Diff View",
            Self::GitHistory => "Git History",
            Self::Source => "Source",
        }
    }

    /// The full list in the Help window.
    fn help(self) -> Hints {
        match self {
            Self::SearchEverywhere => &[
                ("Enter", "Open selected result"),
                ("Up/Down", "Move selection"),
                ("Esc", "Close"),
            ],
            Self::RecentWorkspaces => &[("Esc", "Close")],
            Self::SwitchBranch => &[
                ("Up/Down", "Select branch"),
                ("Enter", "Switch"),
                ("Esc", "Close"),
            ],
            Self::CreateBranch
            | Self::DiscardChanges
            | Self::UnsavedChanges
            | Self::RenameWorkspace => CONFIRM_HELP,
            Self::WorkspaceSearch => &[("Enter", "Run search"), ("Esc", "Clear")],
            Self::WorkspaceFiles => &[
                ("F2", "Rename selected file"),
                ("Delete", "Delete selected file"),
                ("Esc", "Close dialogs"),
            ],
            Self::MarkdownSearch | Self::SourceSearch => FIND_HELP,
            Self::MarkdownPreview | Self::MarkdownBoth => MARKDOWN_VIEW_HELP,
            Self::MarkdownPages => &[
                ("Up/Down", "Select page"),
                ("Enter", "Open page"),
                ("Ctrl+PageUp", "Previous page"),
                ("Ctrl+PageDown", "Next page"),
                ("F6 / Shift+F6", "Next / previous pane"),
            ],
            Self::MarkdownOutline => &[
                ("Up/Down", "Select heading"),
                ("Enter", "Jump to heading"),
                ("Ctrl+F", "Find rendered Markdown"),
                ("F6 / Shift+F6", "Next / previous pane"),
            ],
            Self::MarkdownSource => &[
                ("Alt+1/2/3", "Change Markdown mode"),
                ("Ctrl+F", "Find source"),
                ("F12", "Go to Definition"),
                ("Ctrl+S", "Save"),
            ],
            Self::CsvGrid => &[
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
            ],
            Self::DiffView | Self::GitHistory => SCROLLED_VIEW_HELP,
            Self::Source => &[
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
            ],
        }
    }

    /// The few hints that fit in the status bar.
    fn status(self) -> Hints {
        match self {
            Self::SearchEverywhere => &[("Enter", "Open"), ("Up/Down", "Select"), ("Esc", "Close")],
            Self::RecentWorkspaces => &[("Esc", "Close")],
            Self::CreateBranch
            | Self::SwitchBranch
            | Self::DiscardChanges
            | Self::UnsavedChanges
            | Self::RenameWorkspace => CONFIRM_HELP,
            Self::WorkspaceSearch => &[
                ("Enter", "Search"),
                ("Esc", "Clear"),
                ("Ctrl+Shift+F", "Workspace"),
            ],
            Self::WorkspaceFiles => &[("F2", "Rename"), ("Delete", "Delete")],
            // A find bar's keys are listed in Help; `status_bar_shortcuts` shows the view's.
            Self::MarkdownSearch | Self::SourceSearch | Self::Source => SOURCE_STATUS,
            Self::MarkdownPreview | Self::MarkdownBoth => MARKDOWN_VIEW_STATUS,
            Self::MarkdownPages => &[("Up/Down", "Page"), ("Enter", "Open"), ("Ctrl+F", "Find")],
            Self::MarkdownOutline => &[
                ("Up/Down", "Heading"),
                ("Enter", "Jump"),
                ("Ctrl+F", "Find"),
            ],
            Self::MarkdownSource => &[
                ("Alt+1/2/3", "Mode"),
                ("Ctrl+F", "Find"),
                ("Ctrl+S", "Save"),
                ("F12", "Definition"),
            ],
            Self::CsvGrid => &[
                ("Tab", "Next Cell"),
                ("Enter", "Next Row"),
                ("Arrows", "Move Cell"),
                ("Ctrl+Z/Y", "Undo/Redo"),
            ],
            Self::DiffView | Self::GitHistory => SCROLLED_VIEW_STATUS,
        }
    }
}

impl CodeReaderApp {
    /// Front to back: dialogs, the sidebar, then the focused pane's search box and view.
    pub(super) fn shortcut_context(&self) -> ShortcutContext {
        match self.top_dialog() {
            Some(Modal::SearchEverywhere) => return ShortcutContext::SearchEverywhere,
            Some(Modal::RecentWorkspaces) => return ShortcutContext::RecentWorkspaces,
            Some(Modal::GitCreateBranch) => return ShortcutContext::CreateBranch,
            Some(Modal::GitSwitchBranch) => return ShortcutContext::SwitchBranch,
            Some(Modal::GitDiscard) => return ShortcutContext::DiscardChanges,
            Some(Modal::UnsavedChanges) => return ShortcutContext::UnsavedChanges,
            Some(Modal::RenameWorkspace) => return ShortcutContext::RenameWorkspace,
            _ => {}
        }
        if self.layout.sidebar_view == SidebarView::Search && self.search.workspace.open {
            return ShortcutContext::WorkspaceSearch;
        }
        if self.layout.show_workspace_sidebar
            && self.layout.sidebar_view == SidebarView::Workspace
            && self.workspace.sidebar_active
        {
            return ShortcutContext::WorkspaceFiles;
        }

        let editor = self.focused_editor();
        if editor.markdown.search.open {
            ShortcutContext::MarkdownSearch
        } else if editor.source_search.open {
            ShortcutContext::SourceSearch
        } else {
            self.view_shortcut_context()
        }
    }

    /// The focused pane's view, ignoring its search box.
    fn view_shortcut_context(&self) -> ShortcutContext {
        let editor = self.focused_editor();
        if editor.is_markdown() {
            return match (editor.markdown.mode, editor.markdown.focus_target) {
                (MarkdownMode::Markdown, _) => ShortcutContext::MarkdownSource,
                (_, MarkdownFocusTarget::Pages) => ShortcutContext::MarkdownPages,
                (_, MarkdownFocusTarget::Outline) => ShortcutContext::MarkdownOutline,
                (MarkdownMode::Preview, MarkdownFocusTarget::Body) => {
                    ShortcutContext::MarkdownPreview
                }
                (MarkdownMode::Both, MarkdownFocusTarget::Body) => ShortcutContext::MarkdownBoth,
            };
        }
        if editor.is_csv() && editor.csv.mode == CsvMode::Grid {
            return ShortcutContext::CsvGrid;
        }
        match editor.content {
            EditorContent::Diff(_) => ShortcutContext::DiffView,
            EditorContent::GitHistory(_) => ShortcutContext::GitHistory,
            _ => ShortcutContext::Source,
        }
    }

    pub(super) fn shortcut_context_title(&self) -> &'static str {
        self.shortcut_context().title()
    }

    pub(super) fn status_bar_shortcuts(&self) -> Vec<ShortcutHint> {
        let context = match self.shortcut_context() {
            ShortcutContext::MarkdownSearch | ShortcutContext::SourceSearch => {
                self.view_shortcut_context()
            }
            context => context,
        };
        if context == ShortcutContext::CsvGrid && self.focused_editor().has_active_csv_selection() {
            return shortcuts(CSV_SELECTION_STATUS);
        }
        shortcuts(context.status())
    }

    pub(super) fn context_shortcuts(&self) -> Vec<ShortcutHint> {
        shortcuts(self.shortcut_context().help())
    }

    pub(super) fn shortcut_help_groups(&self) -> Vec<ShortcutGroup> {
        vec![
            ShortcutGroup {
                title: self.shortcut_context_title(),
                shortcuts: self.context_shortcuts(),
            },
            ShortcutGroup {
                title: "Common",
                shortcuts: shortcuts(COMMON_HELP),
            },
            ShortcutGroup {
                title: "Search",
                shortcuts: shortcuts(SEARCH_HELP),
            },
        ]
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

pub(super) fn shortcuts(items: Hints) -> Vec<ShortcutHint> {
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
