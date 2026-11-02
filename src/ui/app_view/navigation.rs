//! Back / Forward through each pane's history, and bookmarks.

use super::*;

impl CodeReaderApp {
    pub(super) fn focused_navigation_history_mut(&mut self) -> &mut NavigationHistory {
        self.navigation_history_mut(self.focused_pane)
    }

    pub(super) fn navigation_history_mut(&mut self, pane: Pane) -> &mut NavigationHistory {
        match pane {
            Pane::Left => &mut self.navigation.left_history,
            Pane::Right => &mut self.navigation.right_history,
        }
    }

    pub(super) fn push_focused_navigation_history(&mut self, point: NavigationPoint) {
        self.focused_navigation_history_mut().push_back(point);
    }

    pub(super) fn open_navigation_point_in_focused_pane(&mut self, point: NavigationPoint) -> bool {
        let mut next = EditorPane::from_path(self.focused_pane, point.path.clone());
        next.target_position = Some(point.position);
        next.pending_navigation = Some(PendingEditorNavigation {
            position: point.position,
            focus_editor: true,
        });
        next.cursor_position = Some(point.position);
        let opened_path = next.path().map(Path::to_path_buf);

        if let Some(path) = opened_path.as_deref() {
            if is_external_rust_path(path) {
                // Rust definition navigation is handled by rust-analyzer. Indexing external
                // Rust sources here can be very expensive and is no longer needed for F12.
            } else if is_external_path(path) {
                self.search
                    .symbol_index
                    .add_external_file(path, &self.language_registry);
            } else {
                self.search
                    .symbol_index
                    .add_file(path, &self.language_registry);
            }
            self.remember_recent_file(path.to_path_buf());
            self.reveal_workspace_file_in_tree(path);
        }

        self.replace_editor(self.focused_pane, next, true, None)
    }

    pub(super) fn go_back(&mut self) {
        let current = self.focused_editor().navigation_point();
        let Some(previous) = self.focused_navigation_history_mut().go_back(current) else {
            self.status_message = "No previous navigation.".to_owned();
            return;
        };

        if self.open_navigation_point_in_focused_pane(previous) {
            self.status_message = "Returned to previous location.".to_owned();
        }
    }

    pub(super) fn go_forward(&mut self) {
        let current = self.focused_editor().navigation_point();
        let Some(next) = self.focused_navigation_history_mut().go_forward(current) else {
            self.status_message = "No next navigation.".to_owned();
            return;
        };

        if self.open_navigation_point_in_focused_pane(next) {
            self.status_message = "Moved to next location.".to_owned();
        }
    }

    pub(super) fn open_navigation_history_entry(&mut self, index: usize) {
        let current = self.focused_editor().navigation_point();
        let Some(point) = self
            .focused_navigation_history_mut()
            .open_back_entry(index, current)
        else {
            self.status_message = "Navigation history entry is no longer available.".to_owned();
            return;
        };

        let label = navigation_point_label(&point);
        self.open_navigation_point_in_focused_pane(point);
        self.status_message = format!("Opened navigation history: {label}");
    }

    pub(super) fn bookmark_lines_for_path(&self, path: Option<&Path>) -> Vec<u32> {
        let Some(path) = path else {
            return Vec::new();
        };
        let mut lines = self
            .current_session()
            .map(|session| {
                session
                    .bookmarks
                    .iter()
                    .filter(|bookmark| bookmark.path == path)
                    .map(|bookmark| bookmark.position.line)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    pub(super) fn toggle_bookmark_at_line(&mut self, path: PathBuf, line: u32) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<invalid>");
        let label = format!("{file_name}:{}", line + 1);
        let mut removed_count = 0;

        if let Some(session) = self.current_session_mut() {
            let before = session.bookmarks.len();
            session
                .bookmarks
                .retain(|bookmark| bookmark.path != path || bookmark.position.line != line);
            removed_count = before.saturating_sub(session.bookmarks.len());

            if removed_count == 0 {
                session.bookmarks.insert(
                    0,
                    SessionBookmark {
                        label: label.clone(),
                        path,
                        position: TextPosition::new(line, 0),
                        temporary: false,
                    },
                );
            }
            session.touch();
            self.bump_search_index_revision();
        }

        if removed_count == 0 {
            self.status_message = format!("Added bookmark: {label}");
        } else {
            self.status_message = format!("Removed bookmark: {label}");
        }
        self.save_state();
    }

    pub(super) fn open_bookmark(&mut self, bookmark: SessionBookmark) {
        let position = Position::new(bookmark.position.line, bookmark.position.character);
        let location = Location::new(bookmark.path, Range::new(position, position));
        self.open_location_in_focused_pane(location, true);
        self.status_message = format!("Opened bookmark: {}", bookmark.label);
    }

    pub(super) fn remove_bookmark(&mut self, index: usize) {
        let mut removed = None;
        if let Some(session) = self.current_session_mut()
            && index < session.bookmarks.len()
        {
            removed = Some(session.bookmarks.remove(index));
            session.touch();
        }

        if let Some(removed) = removed {
            self.bump_search_index_revision();
            self.status_message = format!("Removed bookmark: {}", removed.label);
            if !removed.temporary {
                self.save_state();
            }
        }
    }
}

pub(super) const NAVIGATION_BREADCRUMB_LIMIT: usize = 5;

pub(super) fn paint_navigation_breadcrumb(
    ui: &mut egui::Ui,
    navigation_history: &[NavigationPoint],
    current: Option<NavigationPoint>,
    mut open_history_entry: impl FnMut(usize),
) {
    if navigation_history.is_empty() {
        return;
    }

    let Some(current) = current else {
        return;
    };

    ui.separator();

    let visible_previous_count = NAVIGATION_BREADCRUMB_LIMIT.saturating_sub(1);
    let visible_start = navigation_history
        .len()
        .saturating_sub(visible_previous_count);
    if visible_start > 0 {
        ui.label(RichText::new("...").small().weak());
        ui.label(RichText::new(">").small().weak());
    }

    for (index, point) in navigation_history.iter().enumerate().skip(visible_start) {
        let response = ui
            .small_button(RichText::new(navigation_point_label(point)).monospace())
            .on_hover_text(navigation_point_hover_text(point));
        if response.clicked() {
            open_history_entry(index);
        }
        ui.label(RichText::new(">").small().weak());
    }

    ui.label(
        RichText::new(navigation_point_label(&current))
            .small()
            .strong()
            .monospace(),
    )
    .on_hover_text(navigation_point_hover_text(&current));
}
