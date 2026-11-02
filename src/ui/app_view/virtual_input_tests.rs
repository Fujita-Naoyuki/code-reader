//! Unit tests of the large-file editor's edit commands and cursor movement.

use super::*;

#[test]
fn virtual_text_selection_normalizes_drag_direction() {
    let selection = VirtualTextSelection {
        anchor: Position::new(3, 8),
        head: Position::new(1, 2),
    };

    assert_eq!(
        selection.normalized(),
        Some(NormalizedVirtualSelection {
            start: Position::new(1, 2),
            end: Position::new(3, 8),
        })
    );
}

#[test]
fn virtual_text_selection_ignores_empty_selection() {
    let selection = VirtualTextSelection {
        anchor: Position::new(3, 8),
        head: Position::new(3, 8),
    };

    assert_eq!(selection.normalized(), None);
}

#[test]
fn virtual_selection_text_copies_single_line_range() {
    let buffer = EditorTextBuffer::new("alpha beta\ngamma".to_owned());
    let selection = NormalizedVirtualSelection {
        start: Position::new(0, 6),
        end: Position::new(0, 10),
    };

    assert_eq!(
        virtual_selection_text(&buffer, selection),
        Some("beta".to_owned())
    );
}

#[test]
fn virtual_selection_text_copies_multi_line_range_with_newline() {
    let buffer = EditorTextBuffer::new("alpha beta\ngamma delta\nomega".to_owned());
    let selection = NormalizedVirtualSelection {
        start: Position::new(0, 6),
        end: Position::new(1, 5),
    };

    assert_eq!(
        virtual_selection_text(&buffer, selection),
        Some("beta\ngamma".to_owned())
    );
}

#[test]
fn virtual_selection_text_clamps_to_utf8_boundary() {
    let buffer = EditorTextBuffer::new("aあb\nnext".to_owned());
    let selection = NormalizedVirtualSelection {
        start: Position::new(0, 1),
        end: Position::new(0, 4),
    };

    assert_eq!(
        virtual_selection_text(&buffer, selection),
        Some("あ".to_owned())
    );
}

#[test]
fn virtual_word_selection_selects_identifier() {
    let selection = virtual_word_selection_at_position("text::{CCursor, LayoutJob}", 4, 9)
        .and_then(|selection| selection.normalized());

    assert_eq!(
        selection,
        Some(NormalizedVirtualSelection {
            start: Position::new(4, 7),
            end: Position::new(4, 14),
        })
    );
}

#[test]
fn virtual_word_selection_uses_previous_word_at_right_edge() {
    let selection = virtual_word_selection_at_position("fmt.Sprint(", 2, 10)
        .and_then(|selection| selection.normalized());

    assert_eq!(
        selection,
        Some(NormalizedVirtualSelection {
            start: Position::new(2, 4),
            end: Position::new(2, 10),
        })
    );
}

#[test]
fn virtual_word_selection_ignores_punctuation_only_position() {
    assert_eq!(virtual_word_selection_at_position("::", 0, 1), None);
}

#[test]
fn virtual_insert_inserts_at_cursor() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("abcd".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 2));

    assert!(editor.apply_virtual_insert("X"));
    assert_eq!(editor.text.as_str(), "abXcd");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 3)));
    assert_eq!(editor.virtual_editor.selection, None);
}

#[test]
fn virtual_insert_replaces_selection() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("abcd".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 3));
    editor.virtual_editor.selection = Some(VirtualTextSelection {
        anchor: Position::new(0, 1),
        head: Position::new(0, 3),
    });

    assert!(editor.apply_virtual_insert("X"));
    assert_eq!(editor.text.as_str(), "aXd");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
    assert_eq!(editor.virtual_editor.selection, None);
}

#[test]
fn virtual_backspace_removes_previous_character_across_lines() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("a\nb".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(1, 0));

    assert!(editor.apply_virtual_backspace());
    assert_eq!(editor.text.as_str(), "ab");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
}

#[test]
fn virtual_delete_removes_next_character_across_lines() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("a\nb".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert!(editor.apply_virtual_delete());
    assert_eq!(editor.text.as_str(), "ab");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
}

#[test]
fn virtual_insert_supports_multi_line_paste() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert!(editor.apply_virtual_insert("x\ny"));
    assert_eq!(editor.text.as_str(), "ax\nyb");
    assert_eq!(editor.cursor_position, Some(Position::new(1, 1)));
}

#[test]
fn virtual_edit_rejects_read_only_file() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: true,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert!(!editor.apply_virtual_insert("X"));
    assert!(!editor.apply_virtual_backspace());
    assert!(!editor.apply_virtual_delete());
    assert_eq!(editor.text.as_str(), "ab");
}

#[test]
fn virtual_undo_redo_restores_insert() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert!(editor.apply_virtual_insert("X"));
    assert_eq!(editor.text.as_str(), "aXb");
    assert_eq!(editor.virtual_editor.undo_stack.len(), 1);
    assert_eq!(editor.virtual_editor.redo_stack.len(), 0);

    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.text.as_str(), "ab");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
    assert_eq!(editor.virtual_editor.undo_stack.len(), 0);
    assert_eq!(editor.virtual_editor.redo_stack.len(), 1);

    assert!(editor.apply_virtual_redo());
    assert_eq!(editor.text.as_str(), "aXb");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
    assert_eq!(editor.virtual_editor.undo_stack.len(), 1);
    assert_eq!(editor.virtual_editor.redo_stack.len(), 0);
}

#[test]
fn virtual_undo_redo_restores_selection_replacement() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("abcd".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 3));
    editor.virtual_editor.selection = Some(VirtualTextSelection {
        anchor: Position::new(0, 1),
        head: Position::new(0, 3),
    });

    assert!(editor.apply_virtual_insert("X"));
    assert_eq!(editor.text.as_str(), "aXd");
    assert_eq!(editor.virtual_editor.selection, None);

    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.text.as_str(), "abcd");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 3)));
    assert_eq!(
        editor.virtual_editor.selection,
        Some(VirtualTextSelection {
            anchor: Position::new(0, 1),
            head: Position::new(0, 3),
        })
    );

    assert!(editor.apply_virtual_redo());
    assert_eq!(editor.text.as_str(), "aXd");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
    assert_eq!(editor.virtual_editor.selection, None);
}

#[test]
fn virtual_undo_restores_delete_and_backspace() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("a\nb".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(1, 0));

    assert!(editor.apply_virtual_backspace());
    assert_eq!(editor.text.as_str(), "ab");
    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.text.as_str(), "a\nb");
    assert_eq!(editor.cursor_position, Some(Position::new(1, 0)));

    editor.cursor_position = Some(Position::new(0, 1));
    assert!(editor.apply_virtual_delete());
    assert_eq!(editor.text.as_str(), "ab");
    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.text.as_str(), "a\nb");
    assert_eq!(editor.cursor_position, Some(Position::new(0, 1)));
}

#[test]
fn virtual_new_edit_clears_redo_stack() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert!(editor.apply_virtual_insert("X"));
    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.virtual_editor.redo_stack.len(), 1);

    assert!(editor.apply_virtual_insert("Y"));
    assert_eq!(editor.text.as_str(), "aYb");
    assert_eq!(editor.virtual_editor.redo_stack.len(), 0);
    assert!(!editor.apply_virtual_redo());
}

#[test]
fn virtual_ime_preedit_does_not_change_text_until_commit() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));

    assert_eq!(
        editor.apply_virtual_ime_event(&egui::ImeEvent::Preedit("に".to_owned())),
        None
    );
    assert_eq!(editor.text.as_str(), "ab");
    assert_eq!(editor.virtual_editor.ime_preedit, Some("に".to_owned()));

    assert_eq!(
        editor.apply_virtual_ime_event(&egui::ImeEvent::Commit("日".to_owned())),
        Some(VirtualEditCommand::Insert("日".to_owned()))
    );
    assert_eq!(editor.virtual_editor.ime_preedit, None);
}

#[test]
fn virtual_ime_commit_uses_edit_history() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab".to_owned());
    editor.content = EditorContent::File {
        path: PathBuf::from("test.txt"),
        read_only: false,
    };
    editor.cursor_position = Some(Position::new(0, 1));
    let command = editor.apply_virtual_ime_event(&egui::ImeEvent::Commit("日".to_owned()));

    if let Some(VirtualEditCommand::Insert(text)) = command {
        assert!(editor.apply_virtual_insert(&text));
    } else {
        panic!("expected IME commit to produce insert command");
    }

    assert_eq!(editor.text.as_str(), "a日b");
    assert_eq!(editor.virtual_editor.undo_stack.len(), 1);
    assert!(editor.apply_virtual_undo());
    assert_eq!(editor.text.as_str(), "ab");
}

#[test]
fn virtual_ime_disabled_clears_preedit() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.virtual_editor.ime_preedit = Some("変換中".to_owned());

    assert_eq!(
        editor.apply_virtual_ime_event(&egui::ImeEvent::Disabled),
        None
    );
    assert_eq!(editor.virtual_editor.ime_preedit, None);
}

#[test]
fn virtual_navigation_moves_across_line_boundaries() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("ab\ncde".to_owned());
    editor.cursor_position = Some(Position::new(0, 2));
    let line_index = editor.text.line_index();

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::Right,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(1, 0)));

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::Left,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
}

#[test]
fn virtual_navigation_extends_selection_with_shift() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("abcd\nef".to_owned());
    editor.cursor_position = Some(Position::new(0, 1));
    let line_index = editor.text.line_index();

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::Right,
            extend_selection: true,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(0, 2)));
    assert_eq!(
        editor.virtual_editor.selection,
        Some(VirtualTextSelection {
            anchor: Position::new(0, 1),
            head: Position::new(0, 2),
        })
    );

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::Down,
            extend_selection: true,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(
        editor.virtual_editor.selection,
        Some(VirtualTextSelection {
            anchor: Position::new(0, 1),
            head: Position::new(1, 2),
        })
    );
}

#[test]
fn virtual_navigation_home_end_and_select_all() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new("abcd\nef".to_owned());
    editor.cursor_position = Some(Position::new(0, 2));
    let line_index = editor.text.line_index();

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::End,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(0, 4)));

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::Home,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(0, 0)));

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::SelectAll,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 17.0,
            viewport_height: 170.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 1000.0,
        }
    ));
    assert_eq!(
        editor.virtual_editor.selection,
        Some(VirtualTextSelection {
            anchor: Position::new(0, 0),
            head: Position::new(1, 2),
        })
    );
}

#[test]
fn virtual_navigation_page_moves_cursor_and_scroll() {
    let mut editor = EditorPane::empty(Pane::Left);
    editor.text = EditorTextBuffer::new((0..100).map(|i| format!("{i}\n")).collect());
    editor.cursor_position = Some(Position::new(0, 0));
    let line_index = editor.text.line_index();

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::PageDown,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 10.0,
            viewport_height: 100.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 900.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(9, 0)));
    assert_eq!(editor.scroll.y, 90.0);

    assert!(editor.apply_virtual_navigation(
        VirtualNavigationCommand {
            target: VirtualNavigationTarget::PageUp,
            extend_selection: false,
        },
        line_index.as_ref(),
        &VirtualNavMetrics {
            row_height: 10.0,
            viewport_height: 100.0,
            text_clip_width: 200.0,
            char_width: 10.0,
            max_scroll_x: 0.0,
            max_scroll_y: 900.0,
        }
    ));
    assert_eq!(editor.cursor_position, Some(Position::new(0, 0)));
    assert_eq!(editor.scroll.y, 0.0);
}
