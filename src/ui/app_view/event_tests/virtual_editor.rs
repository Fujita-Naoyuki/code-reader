//! Virtualized editor used for large files (>= 10,000 lines or >= 512 KiB): mouse
//! selection, keyboard navigation and editing, clipboard, undo / redo, IME, scrolling,
//! scrollbars, bookmarks and the context menu.

use eframe::egui;

use crate::core::location::Position;

use super::support::{CTRL, TestApp};

const LINES: usize = 12_000;

fn big_rust() -> String {
    let mut text = String::from(
        "fn first_word() {
    let alpha = (beta + 1);
}
",
    );
    for index in 3..LINES {
        text.push_str(&format!(
            "fn f{index}() {{}}
"
        ));
    }
    text
}

fn big_text() -> String {
    let mut text = String::new();
    text.push_str("fn first_word() {\n");
    text.push_str("    let alpha = (beta + 1);\n");
    text.push_str("}\n");
    for index in 3..LINES {
        text.push_str(&format!("line {index} content\n"));
    }
    text
}

fn virtual_app(name: &str) -> TestApp {
    let mut t = TestApp::builder().file(name, &big_text()).build();
    t.click_first(name);
    t.steps(2);
    assert_eq!(
        t.app()
            .left
            .editor_perf_debug
            .as_ref()
            .map(|perf| perf.view),
        Some("source-virtual")
    );
    t
}

fn text(t: &TestApp) -> String {
    t.app().left.text.as_str().to_owned()
}

#[test]
fn clicking_places_the_cursor_and_typing_edits() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(1, 8);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(1, 8)));
    t.type_text("XY");
    assert!(text(&t).starts_with("fn first_word() {\n    let XYalpha"));
    t.key(egui::Key::Backspace);
    assert!(text(&t).contains("let Xalpha"));
    t.key(egui::Key::Delete);
    assert!(text(&t).contains("let Xlpha"));
    t.key(egui::Key::Enter);
    assert!(
        text(&t).contains("let X\n    lpha"),
        "smart newline keeps indentation"
    );

    t.ctrl(egui::Key::Z);
    assert!(text(&t).contains("let Xlpha"));
    t.ctrl(egui::Key::Y);
    assert!(text(&t).contains("let X\n    lpha"));
    t.ctrl(egui::Key::Z);
    assert!(text(&t).contains("let Xlpha"));
    t.ctrl_shift(egui::Key::Z);
    // DISCREPANCY(D10): Ctrl+Shift+Z is documented as Redo, but the undo check runs
    // first and egui's consume_key ignores the extra Shift, so it undoes once more.
    assert!(text(&t).contains("let Xalpha"));
    t.ctrl(egui::Key::Y);
    t.ctrl(egui::Key::Y);
    assert!(text(&t).contains("let X\n    lpha"));

    // Ctrl+S saves the large file too.
    t.ctrl(egui::Key::S);
    t.assert_status_containing("Saved file:");
    assert!(t.read("big.txt").contains("let X\n    lpha"));
}

#[test]
fn undo_redo_and_edits_at_document_edges() {
    let mut t = virtual_app("big.txt");
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Y);
    assert_eq!(text(&t), big_text(), "nothing to undo or redo");

    t.click_virtual(0, 0);
    t.key(egui::Key::Backspace);
    assert_eq!(text(&t), big_text(), "backspace at start does nothing");

    t.ctrl(egui::Key::End);
    t.key(egui::Key::End);
    let last = (LINES) as u32;
    t.app_mut().left.cursor_position = Some(Position::new(last, 0));
    t.key(egui::Key::Delete);
    assert_eq!(text(&t), big_text(), "delete at end does nothing");
}

#[test]
fn keyboard_navigation_and_selection() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(1, 4);
    t.key(egui::Key::ArrowRight);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(1, 5)));
    t.key(egui::Key::ArrowLeft);
    t.key(egui::Key::ArrowLeft);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(1, 3)));
    t.key(egui::Key::ArrowDown);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(2, 1)));
    // The column is clamped to the short line and not remembered (no "desired column").
    t.key(egui::Key::ArrowUp);
    t.key(egui::Key::ArrowUp);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 1)));
    t.key(egui::Key::End);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 17)));
    t.key(egui::Key::Home);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 0)));
    t.key(egui::Key::ArrowLeft);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 0)));

    t.shift_held(&[egui::Key::End]);
    assert!(t.app().left.has_virtual_text_selection());
    t.copy_event();
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("fn first_word() {")
    );
    t.assert_status("Copied virtual selection: 17 chars.");

    // Typing replaces the selection.
    t.type_text("// replaced");
    assert!(text(&t).starts_with("// replaced\n"));

    t.key(egui::Key::PageDown);
    assert!(t.app().left.scroll_y > 0.0);
    let line_after_page = t.app().left.cursor_position.unwrap().line;
    assert!(line_after_page > 10);
    t.shift_held(&[egui::Key::PageUp]);
    assert!(t.app().left.has_virtual_text_selection());
    t.key(egui::Key::PageUp);
    assert_eq!(t.app().left.scroll_y, 0.0);

    t.key_with(CTRL, egui::Key::A);
    let selection = t.app().left.virtual_selection.unwrap();
    assert_eq!(selection.anchor, Position::new(0, 0));
    assert_eq!(selection.head.line as usize, LINES);
    assert!(
        t.app().left.scroll_y > 1000.0,
        "select all scrolls to the end"
    );

    // Ctrl+other keys are left for other shortcuts.
    t.key_with(CTRL, egui::Key::ArrowDown);
}

#[test]
fn copy_failure_and_backspace_delete_with_selection() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(0, 0);
    t.shift_held(&[egui::Key::ArrowRight, egui::Key::ArrowRight]);
    t.platform.clipboard_fails.set(true);
    t.copy_event();
    t.assert_status(
        "Virtual selection copy requested, but system clipboard failed: clipboard unavailable",
    );
    t.key(egui::Key::Backspace);
    assert!(text(&t).starts_with(" first_word"));
    t.shift_held(&[egui::Key::ArrowRight]);
    t.key(egui::Key::Delete);
    assert!(text(&t).starts_with("first_word"));
}

#[test]
fn paste_from_events_and_from_the_clipboard() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(0, 0);
    t.paste_event("PASTED ");
    assert!(text(&t).starts_with("PASTED fn"));

    t.platform.set_clipboard("CLIP ");
    t.key(egui::Key::Paste);
    assert!(text(&t).starts_with("PASTED CLIP fn"));

    // An unreadable clipboard inserts nothing.
    t.platform.clipboard_fails.set(true);
    t.key(egui::Key::Paste);
    assert!(text(&t).starts_with("PASTED CLIP fn"));
}

#[test]
fn ime_preedit_and_commit() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(0, 0);
    t.harness.event(egui::Event::Ime(egui::ImeEvent::Enabled));
    t.step();
    assert_eq!(t.app().left.virtual_ime_preedit.as_deref(), Some(""));
    t.harness.event(egui::Event::Ime(egui::ImeEvent::Preedit(
        "にほん".to_owned(),
    )));
    t.step();
    assert_eq!(t.app().left.virtual_ime_preedit.as_deref(), Some("にほん"));
    t.harness
        .event(egui::Event::Ime(egui::ImeEvent::Preedit(String::new())));
    t.step();
    assert!(t.app().left.virtual_ime_preedit.is_none());
    t.harness
        .event(egui::Event::Ime(egui::ImeEvent::Preedit("ご".to_owned())));
    t.step();
    t.harness
        .event(egui::Event::Ime(egui::ImeEvent::Commit("日本".to_owned())));
    t.step();
    assert!(text(&t).starts_with("日本fn"));
    t.harness
        .event(egui::Event::Ime(egui::ImeEvent::Commit("\n".to_owned())));
    t.step();
    assert!(text(&t).starts_with("日本fn"));
    t.harness.event(egui::Event::Ime(egui::ImeEvent::Disabled));
    t.step();
    assert!(t.app().left.virtual_ime_preedit.is_none());
}

#[test]
fn double_click_selects_a_word_and_drag_selects_text() {
    let mut t = virtual_app("big.txt");
    t.steps(25);
    let pos = t.virtual_pos(1, 9);
    t.double_click_at(pos);
    let selection = t
        .app()
        .left
        .virtual_selection
        .unwrap()
        .normalized()
        .unwrap();
    assert_eq!(selection.start, Position::new(1, 8));
    assert_eq!(selection.end, Position::new(1, 13));

    // Double-clicking punctuation only places the cursor.
    t.steps(25);
    let paren = t.virtual_pos(1, 16);
    t.double_click_at(paren);
    assert!(!t.app().left.has_virtual_text_selection());

    t.steps(25);
    let from = t.virtual_pos(3, 0);
    let to = t.virtual_pos(4, 4);
    t.drag(from, to);
    let selection = t
        .app()
        .left
        .virtual_selection
        .unwrap()
        .normalized()
        .unwrap();
    assert_eq!(selection.start, Position::new(3, 0));
    assert_eq!(selection.end, Position::new(4, 4));
    t.copy_event();
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("line 3 content\nline")
    );
}

#[test]
fn wheel_and_scrollbars_scroll_the_view() {
    let mut t = virtual_app("big.txt");
    let pos = t.virtual_pos(5, 5);
    t.scroll(pos, egui::vec2(0.0, -400.0));
    assert!(t.app().left.scroll_y >= 400.0);
    t.scroll(pos, egui::vec2(0.0, 2000.0));
    t.steps(30);
    assert_eq!(t.app().left.scroll_y, 0.0);

    // Line-based wheel units scroll by lines.
    t.move_pointer(pos);
    t.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -3.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    t.steps(2);
    assert!(t.app().left.scroll_y > 0.0);
    t.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Page,
        delta: egui::vec2(0.0, -1.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    t.steps(2);

    // Dragging the vertical scrollbar thumb.
    let g = t.virtual_geometry();
    let thumb = egui::pos2(g.full_rect.right() - 3.0, g.full_rect.top() + 6.0);
    let before = t.app().left.scroll_y;
    t.drag(thumb, thumb + egui::vec2(0.0, 200.0));
    assert!(t.app().left.scroll_y > before);
    // Clicking the scrollbar track does not move the text cursor.
    let cursor = t.app().left.cursor_position;
    t.click_at(egui::pos2(
        g.full_rect.right() - 3.0,
        g.full_rect.bottom() - 40.0,
    ));
    assert_eq!(t.app().left.cursor_position, cursor);
}

#[test]
fn tab_moves_keyboard_focus_away_from_the_large_file_editor() {
    let mut t = virtual_app("big.txt");
    t.click_virtual(1, 8);
    assert!(t.harness.ctx.memory(|memory| memory.focused()).is_none());
    t.key(egui::Key::Tab);
    // DISCREPANCY(D08): nothing holds focus in the virtual editor, so egui gives Tab's
    // focus move to another widget instead of inserting spaces; typing then goes nowhere.
    assert!(text(&t).starts_with("fn first_word() {\n    let alpha"));
    assert!(t.harness.ctx.memory(|memory| memory.focused()).is_some());
    t.type_text("Q");
    assert!(!text(&t).contains('Q'));
}

#[test]
fn one_wheel_event_keeps_scrolling_on_later_frames() {
    let mut t = virtual_app("big.txt");
    let pos = t.virtual_pos(5, 5);
    t.move_pointer(pos);
    t.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    t.step();
    assert_eq!(t.app().left.scroll_y, 100.0);
    t.steps(30);
    // DISCREPANCY(D09): the raw wheel delta is applied immediately, but egui's smoothed
    // remainder keeps arriving on later frames, so one 100px notch ends up past 100px.
    assert!(t.app().left.scroll_y > 150.0, "{}", t.app().left.scroll_y);
}

#[test]
fn long_lines_scroll_horizontally() {
    let mut text = "x".repeat(600);
    text.push('\n');
    text.push_str(&big_text());
    let mut t = TestApp::builder().file("wide.txt", &text).build();
    t.click_first("wide.txt");
    t.steps(2);
    t.click_virtual(0, 0);
    t.key(egui::Key::End);
    assert!(t.app().left.scroll_x > 0.0, "cursor kept visible");
    t.key(egui::Key::Home);
    assert_eq!(t.app().left.scroll_x, 0.0);

    let pos = t.virtual_pos(2, 2);
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.scroll(pos, egui::vec2(0.0, -200.0));
    t.set_modifiers(egui::Modifiers::NONE);
    assert!(t.app().left.scroll_x > 0.0);

    // Horizontal scrollbar.
    let g = t.virtual_geometry();
    let thumb = egui::pos2(g.full_rect.left() + 80.0, g.full_rect.bottom() - 3.0);
    t.drag(thumb, thumb + egui::vec2(300.0, 0.0));
}

#[test]
fn bookmark_gutter_and_context_menu() {
    let mut t = virtual_app("big.txt");
    let g = t.virtual_geometry();
    // The star column is the first 18px of the gutter, at the left of the viewport.
    let star = egui::pos2(
        g.viewport.left() + 9.0,
        g.viewport.top() + g.row_height * 2.5,
    );
    t.steps(25);
    t.click_at(star);
    t.assert_status("Added bookmark: big.txt:3");

    let pos = t.virtual_pos(1, 9);
    t.secondary_click_at(pos);
    assert!(t.has_label("Go to Definition"));
    t.click("Back");
    t.assert_status("No previous navigation.");
    t.secondary_click_at(pos);
    t.click("Go to Definition");
    t.assert_status("No language support for this file.");
}

#[test]
fn find_in_a_large_file_highlights_and_jumps() {
    let mut t = virtual_app("big.txt");
    t.ctrl(egui::Key::F);
    t.type_text("line 11999");
    assert!(t.has_label("1/1"));
    t.steps(3);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(11999, 0)));
    assert!(t.app().left.scroll_y > 1000.0);
    // While the find box is open the editor ignores typing.
    t.click("Close");
    assert!(!t.app().left.source_search.open);
}

#[test]
fn rust_large_files_are_highlighted_in_the_background() {
    let mut t = TestApp::builder().file("big.rs", &big_rust()).build();
    t.click_first("big.rs");
    t.steps(2);
    t.wait_until("syntax highlight", |app| {
        app.left.syntax_highlight_cache.is_some()
    });
    t.steps(3);
    t.click_virtual(1, 16);
    t.steps(2);
    // Matching bracket highlighting near the cursor.
    assert!(t.app().left.bracket_pair_cache.is_some());
}

#[test]
fn read_only_large_files_ignore_edits() {
    let mut t = virtual_app("big.txt");
    t.app_mut().left.read_only = true;
    t.click_virtual(0, 0);
    t.type_text("nope");
    t.key(egui::Key::Backspace);
    t.key(egui::Key::Delete);
    t.ctrl(egui::Key::Z);
    t.ctrl(egui::Key::Y);
    assert_eq!(text(&t), big_text());
}

#[test]
fn unfocused_right_pane_virtual_editor_drops_preedit() {
    let mut t = virtual_app("big.txt");
    t.click("Right Pane");
    t.app_mut().left.virtual_ime_preedit = Some("x".to_owned());
    // Focus the right pane: the left one is no longer focused.
    let right = t.nth_label_rect("No file open", 0);
    t.click_at(right.center() + egui::vec2(0.0, 60.0));
    assert!(t.app().left.virtual_ime_preedit.is_none());
}
