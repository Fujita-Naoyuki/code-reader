//! Keyboard editing in the large-file editor: commands, cursor movement, selection and
//! undo. It takes keys only while it holds egui focus (`virtual_editor_can_handle_key`).

use super::*;

/// What keyboard movement needs to keep the cursor in view.
#[derive(Debug, Clone, Copy)]
pub(super) struct VirtualNavMetrics {
    pub(super) row_height: f32,
    pub(super) viewport_height: f32,
    pub(super) text_clip_width: f32,
    pub(super) char_width: f32,
    pub(super) max_scroll_x: f32,
    pub(super) max_scroll_y: f32,
}

impl EditorPane {
    pub(super) fn has_virtual_text_selection(&self) -> bool {
        self.virtual_editor
            .selection
            .and_then(|selection| selection.normalized())
            .is_some()
    }

    pub(super) fn virtual_selection_text(&self) -> Option<String> {
        let selection = self
            .virtual_editor
            .selection
            .and_then(|selection| selection.normalized())?;
        virtual_selection_text(&self.text, selection)
    }

    pub(super) fn handle_virtual_text_input(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        tab_width: usize,
    ) -> bool {
        if self.read_only()
            || !focused
            || !virtual_editor_can_handle_key(ui, text_edit_id)
            || self.source_search.open
        {
            return false;
        }

        let (ime_command, ime_changed) = self.consume_virtual_ime_input(ui);
        let command = ime_command.or_else(|| self.consume_virtual_edit_command(ui, tab_width));
        let Some(command) = command else {
            if ime_changed {
                ui.ctx().request_repaint();
            }
            return false;
        };

        let edited = match command {
            VirtualEditCommand::Insert(text) => self.apply_virtual_insert(&text),
            VirtualEditCommand::InsertPair(open, close) => {
                self.apply_virtual_insert_pair(open, close)
            }
            VirtualEditCommand::SkipClosing(character) => {
                self.skip_virtual_closing(character);
                false
            }
            VirtualEditCommand::Backspace => self.apply_virtual_backspace(),
            VirtualEditCommand::Delete => self.apply_virtual_delete(),
            VirtualEditCommand::Undo => self.apply_virtual_undo(),
            VirtualEditCommand::Redo => self.apply_virtual_redo(),
        };

        if edited {
            self.mark_text_changed();
            self.target_position = None;
            self.virtual_editor.ime_preedit = None;
            ui.ctx().request_repaint();
        } else if ime_changed {
            ui.ctx().request_repaint();
        }
        edited
    }

    pub(super) fn handle_virtual_navigation_input(
        &mut self,
        ui: &mut egui::Ui,
        focused: bool,
        text_edit_id: egui::Id,
        line_index: &LineIndex,
        metrics: &VirtualNavMetrics,
    ) -> bool {
        if !focused || !virtual_editor_can_handle_key(ui, text_edit_id) || self.source_search.open {
            return false;
        }

        let Some(command) = consume_virtual_navigation_command(ui) else {
            return false;
        };

        self.apply_virtual_navigation(command, line_index, metrics)
    }

    pub(super) fn consume_virtual_ime_input(
        &mut self,
        ui: &mut egui::Ui,
    ) -> (Option<VirtualEditCommand>, bool) {
        let mut command = None;
        let mut changed = false;

        ui.input_mut(|input| {
            input.events.retain(|event| {
                let egui::Event::Ime(ime_event) = event else {
                    return true;
                };

                changed = true;
                if let Some(next_command) = self.apply_virtual_ime_event(ime_event) {
                    command = Some(next_command);
                }
                false
            });
        });

        (command, changed)
    }

    pub(super) fn apply_virtual_ime_event(
        &mut self,
        ime_event: &egui::ImeEvent,
    ) -> Option<VirtualEditCommand> {
        match ime_event {
            egui::ImeEvent::Enabled => {
                self.virtual_editor
                    .ime_preedit
                    .get_or_insert_with(String::new);
                None
            }
            egui::ImeEvent::Preedit(text) => {
                if text == "\n" || text == "\r" || text.is_empty() {
                    self.virtual_editor.ime_preedit = None;
                } else {
                    self.virtual_editor.ime_preedit = Some(text.clone());
                }
                None
            }
            egui::ImeEvent::Commit(text) => {
                self.virtual_editor.ime_preedit = None;
                if text != "\n" && text != "\r" && !text.is_empty() {
                    Some(VirtualEditCommand::Insert(text.clone()))
                } else {
                    None
                }
            }
            egui::ImeEvent::Disabled => {
                self.virtual_editor.ime_preedit = None;
                None
            }
        }
    }

    pub(super) fn consume_virtual_edit_command(
        &self,
        ui: &mut egui::Ui,
        tab_width: usize,
    ) -> Option<VirtualEditCommand> {
        let cursor_index = self
            .cursor_position
            .and_then(|position| self.text.position_to_char_index(position))
            .unwrap_or(0);

        ui.input_mut(|input| {
            if consume_undo_shortcut(input) {
                return Some(VirtualEditCommand::Undo);
            }
            if consume_redo_shortcut(input) {
                return Some(VirtualEditCommand::Redo);
            }
            if let Some(text) = consume_paste_text(input) {
                return Some(VirtualEditCommand::Insert(text));
            }
            if consume_paste_shortcut(input)
                && let Ok(text) = read_text_from_system_clipboard()
            {
                return Some(VirtualEditCommand::Insert(text));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                return Some(VirtualEditCommand::Insert(" ".repeat(tab_width)));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                return Some(VirtualEditCommand::Insert(smart_newline(
                    &self.text,
                    cursor_index,
                    tab_width,
                )));
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) {
                return Some(VirtualEditCommand::Backspace);
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Delete) {
                return Some(VirtualEditCommand::Delete);
            }

            let has_selection = self
                .virtual_editor
                .selection
                .is_some_and(|selection| selection.anchor != selection.head);
            // AltGr arrives as Ctrl+Alt on Windows and types characters (`@`, `{`, ...);
            // plain Ctrl / Cmd text is a shortcut, not input.
            let shortcut_modifiers =
                (input.modifiers.command || input.modifiers.ctrl) && !input.modifiers.alt;
            let mut command = None;
            input.events.retain(|event| {
                let egui::Event::Text(text) = event else {
                    return true;
                };
                if text.is_empty() || shortcut_modifiers {
                    return true;
                }

                // Same assistance as the regular editor: pair brackets and quotes, step over
                // a closing one, never with a selection or for multi-character input.
                let mut chars = text.chars();
                if let (Some(character), None, false) = (chars.next(), chars.next(), has_selection)
                {
                    if let Some(close) = closing_pair(character) {
                        command = Some(VirtualEditCommand::InsertPair(character, close));
                        return false;
                    }
                    if is_closing_bracket(character)
                        && self.text.next_char_at(cursor_index) == Some(character)
                    {
                        command = Some(VirtualEditCommand::SkipClosing(character));
                        return false;
                    }
                }

                command = Some(VirtualEditCommand::Insert(text.to_string()));
                false
            });
            command
        })
    }

    pub(super) fn apply_virtual_navigation(
        &mut self,
        command: VirtualNavigationCommand,
        line_index: &LineIndex,
        metrics: &VirtualNavMetrics,
    ) -> bool {
        let VirtualNavMetrics {
            row_height,
            viewport_height,
            max_scroll_y,
            ..
        } = *metrics;

        if line_index.line_count() == 0 {
            self.cursor_position = Some(Position::new(0, 0));
            self.virtual_editor.selection = None;
            return true;
        }

        let current = self
            .cursor_position
            .map(|position| clamp_virtual_position(line_index, position))
            .unwrap_or_else(|| Position::new(0, 0));
        let anchor = self
            .virtual_editor
            .selection
            .map(|selection| selection.anchor)
            .unwrap_or(current);
        let goal_column = match self.virtual_editor.vertical_goal {
            Some((position, column)) if position == current => column,
            _ => current.character,
        };
        let vertical_from = Position::new(current.line, goal_column);
        let next = match command.target {
            VirtualNavigationTarget::Left => virtual_position_left(&self.text, current),
            VirtualNavigationTarget::Right => virtual_position_right(&self.text, current),
            VirtualNavigationTarget::Up => virtual_position_vertical(line_index, vertical_from, -1),
            VirtualNavigationTarget::Down => {
                virtual_position_vertical(line_index, vertical_from, 1)
            }
            VirtualNavigationTarget::Home => virtual_position_line_home(line_index, current),
            VirtualNavigationTarget::End => virtual_position_line_end(line_index, current),
            VirtualNavigationTarget::PageUp => {
                let rows = virtual_page_row_delta(row_height, viewport_height);
                self.scroll.y = (self.scroll.y - rows as f32 * row_height).clamp(0.0, max_scroll_y);
                virtual_position_vertical(line_index, vertical_from, -rows)
            }
            VirtualNavigationTarget::PageDown => {
                let rows = virtual_page_row_delta(row_height, viewport_height);
                self.scroll.y = (self.scroll.y + rows as f32 * row_height).clamp(0.0, max_scroll_y);
                virtual_position_vertical(line_index, vertical_from, rows)
            }
            VirtualNavigationTarget::SelectAll => {
                let start = Position::new(0, 0);
                let end = virtual_document_end(line_index);
                self.cursor_position = Some(end);
                self.virtual_editor.selection = Some(VirtualTextSelection {
                    anchor: start,
                    head: end,
                });
                self.virtual_editor.ime_preedit = None;
                self.ensure_virtual_cursor_visible(line_index, metrics);
                self.target_position = None;
                return true;
            }
        };

        self.cursor_position = Some(next);
        let vertical = matches!(
            command.target,
            VirtualNavigationTarget::Up
                | VirtualNavigationTarget::Down
                | VirtualNavigationTarget::PageUp
                | VirtualNavigationTarget::PageDown
        );
        self.virtual_editor.vertical_goal = vertical.then_some((next, goal_column));
        if command.extend_selection {
            self.virtual_editor.selection = Some(VirtualTextSelection { anchor, head: next });
        } else {
            self.virtual_editor.selection = None;
        }
        self.virtual_editor.ime_preedit = None;
        self.ensure_virtual_cursor_visible(line_index, metrics);
        self.target_position = None;
        true
    }

    pub(super) fn ensure_virtual_cursor_visible(
        &mut self,
        line_index: &LineIndex,
        metrics: &VirtualNavMetrics,
    ) {
        let VirtualNavMetrics {
            row_height,
            viewport_height,
            text_clip_width,
            char_width,
            max_scroll_x,
            max_scroll_y,
        } = *metrics;

        let Some(position) = self.cursor_position else {
            return;
        };
        let position = clamp_virtual_position(line_index, position);
        let cursor_top = position.line as f32 * row_height;
        let cursor_bottom = cursor_top + row_height;
        if cursor_top < self.scroll.y {
            self.scroll.y = cursor_top.clamp(0.0, max_scroll_y);
        } else if cursor_bottom > self.scroll.y + viewport_height {
            self.scroll.y = (cursor_bottom - viewport_height).clamp(0.0, max_scroll_y);
        }

        let cursor_x =
            visual_column_for_position(None, &self.text, line_index, position, char_width);
        let horizontal_padding = 8.0 * char_width;
        if cursor_x < self.scroll.x {
            self.scroll.x = cursor_x.clamp(0.0, max_scroll_x);
        } else if cursor_x + horizontal_padding > self.scroll.x + text_clip_width {
            self.scroll.x =
                (cursor_x + horizontal_padding - text_clip_width).clamp(0.0, max_scroll_x);
        }
    }

    pub(super) fn apply_virtual_insert(&mut self, text: &str) -> bool {
        if self.read_only() {
            return false;
        }
        let Some(range) = self.virtual_edit_range_for_insert() else {
            return false;
        };
        self.replace_virtual_byte_range(range, text)
    }

    pub(super) fn apply_virtual_insert_pair(&mut self, open: char, close: char) -> bool {
        if !self.apply_virtual_insert(&format!("{open}{close}")) {
            return false;
        }
        // Put the cursor between the pair, for the edit and for its undo entry.
        if let Some(cursor) = self.cursor_position
            && let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor)
        {
            self.cursor_position = self
                .text
                .byte_index_to_position(cursor_byte - close.len_utf8());
            if let Some(entry) = self.virtual_editor.undo_stack.last_mut() {
                entry.after_cursor = self.cursor_position;
            }
        }
        true
    }

    pub(super) fn skip_virtual_closing(&mut self, character: char) {
        if let Some(cursor) = self.cursor_position
            && let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor)
        {
            self.cursor_position = self
                .text
                .byte_index_to_position(cursor_byte + character.len_utf8());
        }
    }

    pub(super) fn apply_virtual_backspace(&mut self) -> bool {
        if self.read_only() {
            return false;
        }
        if let Some(range) = self.virtual_selection_byte_range() {
            return self.replace_virtual_byte_range(range, "");
        }

        let Some(cursor) = self.cursor_position else {
            return false;
        };
        let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor) else {
            return false;
        };
        if cursor_byte == 0 {
            return false;
        }
        let start = previous_char_boundary(self.text.as_str(), cursor_byte);
        self.replace_virtual_byte_range(start..cursor_byte, "")
    }

    pub(super) fn apply_virtual_delete(&mut self) -> bool {
        if self.read_only() {
            return false;
        }
        if let Some(range) = self.virtual_selection_byte_range() {
            return self.replace_virtual_byte_range(range, "");
        }

        let Some(cursor) = self.cursor_position else {
            return false;
        };
        let Some(cursor_byte) = virtual_position_to_byte_index(&self.text, cursor) else {
            return false;
        };
        if cursor_byte >= self.text.len() {
            return false;
        }
        let end = next_char_boundary(self.text.as_str(), cursor_byte);
        self.replace_virtual_byte_range(cursor_byte..end, "")
    }

    pub(super) fn apply_virtual_undo(&mut self) -> bool {
        if self.read_only() {
            return false;
        }
        let Some(entry) = self.virtual_editor.undo_stack.pop() else {
            return false;
        };
        let start = entry.start;
        let end = start + entry.inserted.len();
        self.text.replace_byte_range(start..end, &entry.removed);
        self.cursor_position = entry.before_cursor;
        self.virtual_editor.selection = entry.before_selection;
        self.virtual_editor.redo_stack.push(entry);
        limit_virtual_edit_history(&mut self.virtual_editor.redo_stack);
        true
    }

    pub(super) fn apply_virtual_redo(&mut self) -> bool {
        if self.read_only() {
            return false;
        }
        let Some(entry) = self.virtual_editor.redo_stack.pop() else {
            return false;
        };
        let start = entry.start;
        let end = start + entry.removed.len();
        self.text.replace_byte_range(start..end, &entry.inserted);
        self.cursor_position = entry.after_cursor;
        self.virtual_editor.selection = entry.after_selection;
        self.virtual_editor.undo_stack.push(entry);
        limit_virtual_edit_history(&mut self.virtual_editor.undo_stack);
        true
    }

    pub(super) fn virtual_edit_range_for_insert(&self) -> Option<std::ops::Range<usize>> {
        self.virtual_selection_byte_range().or_else(|| {
            let cursor = self.cursor_position.unwrap_or(Position::new(0, 0));
            let byte = virtual_position_to_byte_index(&self.text, cursor)?;
            Some(byte..byte)
        })
    }

    pub(super) fn virtual_selection_byte_range(&self) -> Option<std::ops::Range<usize>> {
        let selection = self
            .virtual_editor
            .selection
            .and_then(|selection| selection.normalized())?;
        virtual_selection_byte_range(&self.text, selection)
    }

    pub(super) fn replace_virtual_byte_range(
        &mut self,
        range: std::ops::Range<usize>,
        replacement: &str,
    ) -> bool {
        let Some(removed) = self.text.as_str().get(range.clone()).map(ToOwned::to_owned) else {
            return false;
        };
        if removed == replacement {
            return false;
        }
        let before_cursor = self.cursor_position;
        let before_selection = self.virtual_editor.selection;
        let start = range.start;
        self.text.replace_byte_range(range, replacement);
        let cursor_byte = start + replacement.len();
        self.cursor_position = self.text.byte_index_to_position(cursor_byte);
        self.virtual_editor.selection = None;
        self.virtual_editor
            .undo_stack
            .push(VirtualEditHistoryEntry {
                start,
                removed,
                inserted: replacement.to_owned(),
                before_cursor,
                after_cursor: self.cursor_position,
                before_selection,
                after_selection: self.virtual_editor.selection,
            });
        limit_virtual_edit_history(&mut self.virtual_editor.undo_stack);
        self.virtual_editor.redo_stack.clear();
        true
    }
}

/// State only the large-file editor uses.
#[derive(Default)]
pub(super) struct VirtualEditorState {
    pub(super) selection: Option<VirtualTextSelection>,
    pub(super) ime_preedit: Option<String>,
    pub(super) undo_stack: Vec<VirtualEditHistoryEntry>,
    pub(super) redo_stack: Vec<VirtualEditHistoryEntry>,
    /// Wheel scrolling applied immediately but egui is still going to deliver as smoothed
    /// deltas on later frames; those are skipped up to this amount.
    pub(super) wheel_lead: egui::Vec2,
    /// Where Up/Down last left the cursor and the column it was aiming for, so passing
    /// through shorter lines does not lose the column (only while the cursor stays there).
    pub(super) vertical_goal: Option<(Position, u32)>,
    pub(super) line_layout_cache: Option<VirtualLineLayoutCache>,
    pub(super) line_number_layout_cache: Option<VirtualLineNumberLayoutCache>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VirtualTextSelection {
    pub(super) anchor: Position,
    pub(super) head: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NormalizedVirtualSelection {
    pub(super) start: Position,
    pub(super) end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VirtualEditCommand {
    Insert(String),
    /// An opening bracket or quote and its closing partner, with the cursor between them.
    InsertPair(char, char),
    /// Typing a closing bracket right before the same bracket steps over it.
    SkipClosing(char),
    Backspace,
    Delete,
    Undo,
    Redo,
}

pub(super) const VIRTUAL_EDIT_HISTORY_LIMIT: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VirtualEditHistoryEntry {
    pub(super) start: usize,
    pub(super) removed: String,
    pub(super) inserted: String,
    pub(super) before_cursor: Option<Position>,
    pub(super) after_cursor: Option<Position>,
    pub(super) before_selection: Option<VirtualTextSelection>,
    pub(super) after_selection: Option<VirtualTextSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VirtualNavigationTarget {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    SelectAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VirtualNavigationCommand {
    pub(super) target: VirtualNavigationTarget,
    pub(super) extend_selection: bool,
}

impl VirtualTextSelection {
    pub(super) fn normalized(self) -> Option<NormalizedVirtualSelection> {
        if self.anchor == self.head {
            return None;
        }
        let (start, end) = if position_leq(self.anchor, self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        Some(NormalizedVirtualSelection { start, end })
    }
}

pub(super) fn virtual_selection_text(
    text: &EditorTextBuffer,
    selection: NormalizedVirtualSelection,
) -> Option<String> {
    let range = virtual_selection_byte_range(text, selection)?;
    text.as_str().get(range).map(ToOwned::to_owned)
}

pub(super) fn virtual_selection_byte_range(
    text: &EditorTextBuffer,
    selection: NormalizedVirtualSelection,
) -> Option<std::ops::Range<usize>> {
    let start = virtual_position_to_byte_index(text, selection.start)?;
    let end = virtual_position_to_byte_index(text, selection.end)?;
    if start >= end {
        return None;
    }
    Some(start..end)
}

pub(super) fn virtual_word_selection_at_position(
    line: &str,
    line_number: u32,
    byte_column: u32,
) -> Option<VirtualTextSelection> {
    let byte_column = clamp_to_char_boundary(line, (byte_column as usize).min(line.len()));
    let target = if line[byte_column..]
        .chars()
        .next()
        .is_some_and(is_editor_word_char)
    {
        byte_column
    } else if byte_column > 0 {
        let previous = previous_char_boundary(line, byte_column);
        if line[previous..]
            .chars()
            .next()
            .is_some_and(is_editor_word_char)
        {
            previous
        } else {
            return None;
        }
    } else {
        return None;
    };

    let mut start = target;
    while start > 0 {
        let previous = previous_char_boundary(line, start);
        let Some(character) = line[previous..].chars().next() else {
            break;
        };
        if !is_editor_word_char(character) {
            break;
        }
        start = previous;
    }

    let mut end = target;
    while end < line.len() {
        let Some(character) = line[end..].chars().next() else {
            break;
        };
        if !is_editor_word_char(character) {
            break;
        }
        end += character.len_utf8();
    }

    (start < end).then_some(VirtualTextSelection {
        anchor: Position::new(line_number, start as u32),
        head: Position::new(line_number, end as u32),
    })
}

pub(super) fn is_editor_word_char(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

pub(super) fn virtual_position_to_byte_index(
    text: &EditorTextBuffer,
    position: Position,
) -> Option<usize> {
    let line = text.line_index.line(position.line as usize)?;
    let target = (line.start + position.character as usize).min(line.content_end);
    Some(clamp_to_char_boundary(text.as_str(), target))
}

pub(super) fn virtual_editor_can_handle_key(ui: &egui::Ui, text_edit_id: egui::Id) -> bool {
    ui.memory(|memory| memory.has_focus(text_edit_id))
}

pub(super) fn consume_virtual_navigation_command(
    ui: &mut egui::Ui,
) -> Option<VirtualNavigationCommand> {
    ui.input_mut(|input| {
        if input.modifiers.ctrl || input.modifiers.command {
            if input.consume_key(egui::Modifiers::CTRL, egui::Key::A)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)
            {
                return Some(VirtualNavigationCommand {
                    target: VirtualNavigationTarget::SelectAll,
                    extend_selection: false,
                });
            }
            return None;
        }

        let extend_selection = input.modifiers.shift;
        let modifiers = if extend_selection {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };

        let target = if input.consume_key(modifiers, egui::Key::ArrowLeft) {
            Some(VirtualNavigationTarget::Left)
        } else if input.consume_key(modifiers, egui::Key::ArrowRight) {
            Some(VirtualNavigationTarget::Right)
        } else if input.consume_key(modifiers, egui::Key::ArrowUp) {
            Some(VirtualNavigationTarget::Up)
        } else if input.consume_key(modifiers, egui::Key::ArrowDown) {
            Some(VirtualNavigationTarget::Down)
        } else if input.consume_key(modifiers, egui::Key::Home) {
            Some(VirtualNavigationTarget::Home)
        } else if input.consume_key(modifiers, egui::Key::End) {
            Some(VirtualNavigationTarget::End)
        } else if input.consume_key(modifiers, egui::Key::PageUp) {
            Some(VirtualNavigationTarget::PageUp)
        } else if input.consume_key(modifiers, egui::Key::PageDown) {
            Some(VirtualNavigationTarget::PageDown)
        } else {
            None
        }?;

        Some(VirtualNavigationCommand {
            target,
            extend_selection,
        })
    })
}

pub(super) fn virtual_page_row_delta(row_height: f32, viewport_height: f32) -> i32 {
    ((viewport_height * EDITOR_PAGE_SCROLL_RATIO) / row_height.max(1.0))
        .round()
        .max(1.0) as i32
}

pub(super) fn clamp_virtual_position(line_index: &LineIndex, position: Position) -> Position {
    let line_count = line_index.line_count().max(1);
    let line_number = (position.line as usize).min(line_count.saturating_sub(1));
    let character = line_index
        .line(line_number)
        .map(|line| (position.character as usize).min(line.content_end - line.start))
        .unwrap_or(0);
    Position::new(line_number as u32, character as u32)
}

pub(super) fn virtual_document_end(line_index: &LineIndex) -> Position {
    let line_number = line_index.line_count().saturating_sub(1);
    virtual_position_line_end(line_index, Position::new(line_number as u32, 0))
}

pub(super) fn virtual_position_line_home(line_index: &LineIndex, position: Position) -> Position {
    let line_number = (position.line as usize).min(line_index.line_count().saturating_sub(1));
    Position::new(line_number as u32, 0)
}

pub(super) fn virtual_position_line_end(line_index: &LineIndex, position: Position) -> Position {
    let line_number = (position.line as usize).min(line_index.line_count().saturating_sub(1));
    let character = line_index
        .line(line_number)
        .map(|line| line.content_end - line.start)
        .unwrap_or(0);
    Position::new(line_number as u32, character as u32)
}

pub(super) fn virtual_position_vertical(
    line_index: &LineIndex,
    position: Position,
    row_delta: i32,
) -> Position {
    let line_count = line_index.line_count().max(1);
    let next_line = (position.line as i32 + row_delta).clamp(0, line_count as i32 - 1) as usize;
    let character = line_index
        .line(next_line)
        .map(|line| (position.character as usize).min(line.content_end - line.start))
        .unwrap_or(0);
    Position::new(next_line as u32, character as u32)
}

pub(super) fn virtual_position_left(text: &EditorTextBuffer, position: Position) -> Position {
    let Some(byte_index) = virtual_position_to_byte_index(text, position) else {
        return Position::new(0, 0);
    };
    if byte_index == 0 {
        return Position::new(0, 0);
    }
    text.byte_index_to_position(previous_char_boundary(text.as_str(), byte_index))
        .unwrap_or(Position::new(0, 0))
}

pub(super) fn virtual_position_right(text: &EditorTextBuffer, position: Position) -> Position {
    let Some(byte_index) = virtual_position_to_byte_index(text, position) else {
        return Position::new(0, 0);
    };
    if byte_index >= text.len() {
        return text
            .byte_index_to_position(text.len())
            .unwrap_or(Position::new(0, 0));
    }
    text.byte_index_to_position(next_char_boundary(text.as_str(), byte_index))
        .unwrap_or(Position::new(0, 0))
}

pub(super) fn position_leq(left: Position, right: Position) -> bool {
    (left.line, left.character) <= (right.line, right.character)
}

pub(super) fn limit_virtual_edit_history(stack: &mut Vec<VirtualEditHistoryEntry>) {
    let overflow = stack.len().saturating_sub(VIRTUAL_EDIT_HISTORY_LIMIT);
    if overflow > 0 {
        stack.drain(0..overflow);
    }
}
