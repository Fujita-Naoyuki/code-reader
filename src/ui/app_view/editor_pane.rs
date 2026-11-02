//! `EditorPane`: one side of the editor area, its file, and the mode-independent parts
//! (constructing, saving, unsaved-change tracking, dispatching to the view for its mode).

use super::*;

impl EditorPane {
    fn new(pane: Pane, title: String, content: EditorContent, text: String) -> Self {
        Self {
            pane,
            title,
            content,
            text: EditorTextBuffer::new(text),
            cursor_position: None,
            target_position: None,
            pending_navigation: None,
            scroll: ScrollState::default(),
            source_search: SourceSearchState::default(),
            virtual_editor: VirtualEditorState::default(),
            csv: CsvPaneState::default(),
            markdown: MarkdownPaneState::default(),
            diff: DiffPaneState::default(),
            caches: EditorCaches::default(),
            debug: EditorDebugState::default(),
            pending_action: None,
            saved_fingerprint: (0, 0),
        }
    }

    pub(super) fn empty(pane: Pane) -> Self {
        let text = "Select a file from the workspace sidebar.".to_owned();
        Self::new(pane, "No file open".to_owned(), EditorContent::Empty, text)
    }

    pub(super) fn from_path(pane: Pane, path: PathBuf) -> Self {
        let mut editor = Self::read_path(pane, path);
        editor.saved_fingerprint = editor.text_fingerprint();
        editor
    }

    pub(super) fn read_path(pane: Pane, path: PathBuf) -> Self {
        match fs::read_to_string(&path) {
            Ok(text) => {
                let title = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<invalid>")
                    .to_owned();
                let read_only = is_external_path(&path);
                Self::new(pane, title, EditorContent::File { path, read_only }, text)
            }
            Err(error) => {
                let text = format!("Failed to read file: {error}");
                Self::new(
                    pane,
                    "Unable to open file".to_owned(),
                    EditorContent::Unreadable { path },
                    text,
                )
            }
        }
    }

    pub(super) fn from_diff(pane: Pane, diff_view: GitDiffView, text: String) -> Self {
        let title = diff_view.title.clone();
        Self::new(pane, title, EditorContent::Diff(Box::new(diff_view)), text)
    }

    pub(super) fn from_git_history(pane: Pane, history_view: GitHistoryView) -> Self {
        let title = format!("{}: Git Graph", history_view.repository_name);
        let content = EditorContent::GitHistory(Box::new(history_view));
        Self::new(pane, title, content, String::new())
    }

    /// The file on screen, readable or not. Diffs and history have none.
    pub(super) fn path(&self) -> Option<&Path> {
        match &self.content {
            EditorContent::File { path, .. } | EditorContent::Unreadable { path } => Some(path),
            EditorContent::Empty | EditorContent::Diff(_) | EditorContent::GitHistory(_) => None,
        }
    }

    pub(super) fn read_only(&self) -> bool {
        !matches!(
            self.content,
            EditorContent::File {
                read_only: false,
                ..
            }
        )
    }

    pub(super) fn diff_view(&self) -> Option<&GitDiffView> {
        match &self.content {
            EditorContent::Diff(view) => Some(view),
            _ => None,
        }
    }

    pub(super) fn diff_view_mut(&mut self) -> Option<&mut GitDiffView> {
        match &mut self.content {
            EditorContent::Diff(view) => Some(view),
            _ => None,
        }
    }

    pub(super) fn git_history_view(&self) -> Option<&GitHistoryView> {
        match &self.content {
            EditorContent::GitHistory(view) => Some(view),
            _ => None,
        }
    }

    pub(super) fn git_history_view_mut(&mut self) -> Option<&mut GitHistoryView> {
        match &mut self.content {
            EditorContent::GitHistory(view) => Some(view),
            _ => None,
        }
    }

    pub(super) fn navigation_point(&self) -> Option<NavigationPoint> {
        Some(NavigationPoint {
            path: self.path().map(Path::to_path_buf)?,
            position: self.cursor_position.unwrap_or(Position::new(0, 0)),
        })
    }

    pub(super) fn take_pending_action(&mut self) -> Option<EditorAction> {
        self.pending_action.take()
    }

    pub(super) fn text_fingerprint(&self) -> (usize, u64) {
        self.text.fingerprint()
    }

    /// An editable file whose text differs from what was last read or saved.
    pub(super) fn has_unsaved_changes(&self) -> bool {
        !self.read_only() && self.text_fingerprint() != self.saved_fingerprint
    }

    /// Takes over `other`'s unsaved text when `self` is a fresh copy of the same file, so
    /// moving within that file (definition jump, Back/Forward) keeps the edits.
    pub(super) fn adopt_unsaved_text(&mut self, other: &mut EditorPane) {
        self.text = std::mem::replace(&mut other.text, EditorTextBuffer::new(String::new()));
        self.saved_fingerprint = other.saved_fingerprint;
        self.mark_text_changed();
    }

    pub(super) fn mark_text_changed(&mut self) {
        self.text.refresh_metadata();
        self.caches.syntax_highlight_pending = None;
        self.caches.syntax_highlight_receiver = None;
        self.virtual_editor.line_layout_cache = None;
        self.virtual_editor.line_number_layout_cache = None;
    }

    pub(super) fn replace_text(&mut self, text: String) {
        self.text.replace(text);
        self.caches.syntax_highlight_pending = None;
        self.caches.syntax_highlight_receiver = None;
        self.virtual_editor.line_layout_cache = None;
        self.virtual_editor.line_number_layout_cache = None;
        self.virtual_editor.undo_stack.clear();
        self.virtual_editor.redo_stack.clear();
    }

    pub(super) fn save_to_disk(&mut self) -> Result<PathBuf, String> {
        if self.read_only() {
            return Err("Focused file is read-only.".to_owned());
        }

        let Some(path) = self.path().map(Path::to_path_buf) else {
            return Err("No file is open in the focused pane.".to_owned());
        };

        if self.is_csv() {
            let text = parse_csv(&self.text)
                .map(|document| {
                    serialize_csv_for_save(&document, CsvTextStyle::of(self.text.as_str()))
                })
                .unwrap_or_else(|_| self.text.as_str().to_owned());
            self.replace_text(text);
        } else {
            let text = strip_trailing_whitespace(&self.text);
            self.replace_text(text);
        }
        fs::write(&path, self.text.as_str())
            .map_err(|error| format!("Failed to save file: {error}"))?;
        self.saved_fingerprint = self.text_fingerprint();
        Ok(path)
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        language_registry: &LanguageRegistry,
        tab_width: usize,
        navigation_history: &[NavigationPoint],
        bookmark_lines: &[u32],
    ) {
        let frame = egui::Frame::group(ui.style()).fill(if focused {
            ui.visuals().faint_bg_color
        } else {
            ui.visuals().extreme_bg_color
        });

        frame.show(ui, |ui| {
            ui.set_min_height(ui.available_height());

            ui.horizontal(|ui| {
                if self.has_unsaved_changes() {
                    ui.label(RichText::new(format!("● {}", self.title)).strong())
                        .on_hover_text("Unsaved changes");
                } else {
                    ui.label(RichText::new(&self.title).strong());
                }
                paint_navigation_breadcrumb(
                    ui,
                    navigation_history,
                    self.navigation_point(),
                    |index| {
                        self.pending_action = Some(EditorAction::OpenNavigationHistory { index });
                    },
                );

                if self.read_only() {
                    ui.label(
                        RichText::new("READ ONLY")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                } else if ui.button("Save").clicked() {
                    self.pending_action = Some(EditorAction::SaveFile);
                }
            });

            ui.separator();

            if self.diff_view().is_some() {
                self.diff_view_ui(ui, focused);
                return;
            }

            if self.git_history_view().is_some() {
                self.git_history_ui(ui, focused);
                return;
            }

            if self.is_csv() {
                self.csv_editor_ui(ui, focused, language_registry, tab_width);
                return;
            }

            if self.is_markdown() {
                self.markdown_ui(ui, focused, language_registry, tab_width, bookmark_lines);
                return;
            }

            self.source_editor_ui(ui, focused, language_registry, tab_width, bookmark_lines);
        });
    }
}

pub(super) const DEFAULT_TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingEditorNavigation {
    pub(super) position: Position,
    pub(super) focus_editor: bool,
}

/// What a pane shows. `EditorPane::text` holds the file's text, the diff text, or the message
/// shown for `Empty` and `Unreadable`.
pub(super) enum EditorContent {
    Empty,
    File {
        path: PathBuf,
        read_only: bool,
    },
    /// Reading the file failed; the text is the error message.
    Unreadable {
        path: PathBuf,
    },
    Diff(Box<GitDiffView>),
    GitHistory(Box<GitHistoryView>),
}

pub(super) struct EditorPane {
    pub(super) pane: Pane,
    pub(super) title: String,
    pub(super) content: EditorContent,
    pub(super) text: EditorTextBuffer,
    pub(super) cursor_position: Option<Position>,
    pub(super) target_position: Option<Position>,
    pub(super) pending_navigation: Option<PendingEditorNavigation>,
    pub(super) scroll: ScrollState,
    pub(super) source_search: SourceSearchState,
    pub(super) virtual_editor: VirtualEditorState,
    pub(super) csv: CsvPaneState,
    pub(super) markdown: MarkdownPaneState,
    pub(super) diff: DiffPaneState,
    pub(super) caches: EditorCaches,
    pub(super) debug: EditorDebugState,
    pub(super) pending_action: Option<EditorAction>,
    /// Text fingerprint when the file was last read or saved; see `has_unsaved_changes`.
    pub(super) saved_fingerprint: (usize, u64),
}

#[derive(Default)]
pub(super) struct ScrollState {
    pub(super) y: f32,
    pub(super) max_y: f32,
    pub(super) x: f32,
}

/// Results computed from the pane's text and reused until the text changes.
#[derive(Default)]
pub(super) struct EditorCaches {
    pub(super) syntax_highlight: Option<SyntaxHighlightCache>,
    pub(super) syntax_highlight_pending: Option<SyntaxHighlightPending>,
    pub(super) syntax_highlight_receiver: Option<Receiver<SyntaxHighlightResult>>,
    pub(super) editor_metrics: Option<EditorMetricsCache>,
    pub(super) bracket_pair: Option<BracketPairCache>,
    pub(super) source_search: Option<SourceSearchCache>,
}

pub(super) fn editor_pane_from_session_path(
    pane: Pane,
    path: PathBuf,
    text_position: TextPosition,
) -> EditorPane {
    let mut editor = EditorPane::from_path(pane, path);
    let position = Position::new(text_position.line, text_position.character);
    editor.target_position = Some(position);
    editor.pending_navigation = Some(PendingEditorNavigation {
        position,
        focus_editor: true,
    });
    editor.cursor_position = Some(position);
    editor
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditorAction {
    GoToDefinition,
    Back,
    OpenNavigationHistory {
        index: usize,
    },
    OpenGitCommitDetails {
        repository_name: String,
        root: PathBuf,
        commit: GitCommitNode,
    },
    SaveFile,
    OpenMarkdownLink {
        path: PathBuf,
        fragment: Option<String>,
    },
    OpenExternalUrl(String),
    ToggleBookmark {
        path: PathBuf,
        line: u32,
    },
    ShowStatus(String),
}

#[derive(Debug, Clone)]
pub(super) enum BookmarkAction {
    Open(SessionBookmark),
    Remove(usize),
}
