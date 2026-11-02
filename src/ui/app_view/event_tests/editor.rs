//! Source editor for normal-sized files (egui TextEdit): typing with input assistance,
//! save, find in file, bookmarks gutter, scrolling and unreadable files.

use eframe::egui;

use crate::core::location::Position;
use crate::core::lsp::LspRequestKind;

use super::support::{CTRL, TestApp, location};

fn editor_app() -> TestApp {
    let mut t = TestApp::builder()
        .file(
            "notes.txt",
            "alpha beta\n    indented\nalpha again   \nlast alpha\n",
        )
        .file("bin.dat.txt", "")
        .build();
    t.click_first("notes.txt");
    t
}

#[test]
fn typing_edits_the_buffer_with_bracket_and_indent_assistance() {
    let mut t = editor_app();
    t.click_text(0, 5);
    t.type_text("(");
    assert!(t.app().left.text.as_str().starts_with("alpha() beta"));
    t.type_text("x");
    // Typing the closing bracket steps over the auto-inserted one.
    t.type_text(")");
    assert!(t.app().left.text.as_str().starts_with("alpha(x) beta"));
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 8)));

    t.type_text("\"");
    assert!(t.app().left.text.as_str().starts_with("alpha(x)\"\" beta"));

    // Tab inserts spaces, Enter keeps the current indentation.
    t.click_text(1, 12);
    t.key(egui::Key::Tab);
    assert!(t.app().left.text.as_str().contains("    indented    \n"));
    t.key(egui::Key::Enter);
    assert!(
        t.app()
            .left
            .text
            .as_str()
            .contains("    indented    \n    \n")
    );

    // After an opening brace, Enter indents one level deeper.
    t.type_text("{");
    t.key(egui::Key::Enter);
    assert!(t.app().left.text.as_str().contains("{\n        }"));

    // Multi-character text input is passed through untouched.
    t.type_text("ab");
    assert!(t.app().left.text.as_str().contains("        ab}"));
}

#[test]
fn ctrl_s_and_save_button_strip_trailing_whitespace_and_write_the_file() {
    let mut t = editor_app();
    t.click_text(3, 10);
    t.type_text(" ");
    t.ctrl(egui::Key::S);
    t.assert_status_containing("Saved file:");
    assert_eq!(
        t.read("notes.txt"),
        "alpha beta\n    indented\nalpha again\nlast alpha\n"
    );

    t.click_text(0, 0);
    t.type_text("Z");
    t.click("Save");
    t.assert_status_containing("Saved file:");
    assert!(t.read("notes.txt").starts_with("Zalpha"));
}

#[test]
fn save_reports_errors() {
    // The empty editor shown at startup cannot be saved.
    let mut t = TestApp::plain();
    t.ctrl(egui::Key::S);
    t.assert_status("Focused file is read-only.");
    assert!(t.has_label("READ ONLY"));

    let mut t = editor_app();
    // Replace the file with a directory so writing fails.
    std::fs::remove_file(t.path("notes.txt")).unwrap();
    std::fs::create_dir(t.path("notes.txt")).unwrap();
    t.ctrl(egui::Key::S);
    t.assert_status_containing("Failed to save file:");
}

#[test]
fn unreadable_files_open_read_only_with_the_error() {
    let mut t = TestApp::builder()
        .bytes("broken.txt", &[0xff, 0xfe, 0x00, 0x41])
        .build();
    t.click_first("broken.txt");
    assert!(t.has_label("Unable to open file"));
    assert!(t.has_label("READ ONLY"));
    assert!(
        t.app()
            .left
            .text
            .as_str()
            .starts_with("Failed to read file:")
    );
}

#[test]
fn ctrl_f_finds_matches_and_navigates_them() {
    let mut t = editor_app();
    t.ctrl(egui::Key::F);
    assert!(t.app().left.source_search.open);
    assert!(t.has_label("Find"));
    assert!(t.has_label("0 results"));

    t.type_text("alpha");
    assert!(t.has_label("1/3"));
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 0)));

    t.key(egui::Key::Enter);
    assert!(t.has_label("2/3"));
    assert_eq!(t.app().left.cursor_position, Some(Position::new(2, 0)));
    t.shift(egui::Key::Enter);
    assert!(t.has_label("1/3"));
    // Wait out the Shift double-tap window first (see D03).
    t.steps(40);
    t.shift(egui::Key::Enter);
    assert!(t.has_label("3/3"), "wraps around backwards");

    t.click("Next");
    assert!(t.has_label("1/3"));
    t.click("Previous");
    assert!(t.has_label("3/3"));

    // F3 / Shift+F3 work while the editor itself has focus.
    t.click_text(1, 2);
    t.key(egui::Key::F3);
    assert!(t.has_label("1/3"));
    t.shift(egui::Key::F3);
    assert!(t.has_label("3/3"));

    t.click("Close");
    assert!(!t.app().left.source_search.open);
}

#[test]
fn quick_shift_enter_presses_step_back_without_opening_search_everywhere() {
    let mut t = editor_app();
    t.click_text(0, 0);
    t.type_text("unsaved ");
    t.ctrl(egui::Key::F);
    t.type_text("alpha");
    t.shift(egui::Key::Enter);
    t.shift(egui::Key::Enter);
    // Shift+Enter is not a lone Shift tap, so "Shift x2" does not fire.
    assert!(!t.app().search_overlay.open);
    assert!(t.app().left.source_search.open);
    assert!(t.app().left.text.as_str().contains("unsaved"));
}

#[test]
fn find_without_matches_and_escape() {
    let mut t = editor_app();
    t.ctrl(egui::Key::F);
    t.type_text("zzz");
    assert!(t.has_label("No results"));
    t.key(egui::Key::F3);
    t.key(egui::Key::Escape);
    assert!(!t.app().left.source_search.open);
}

#[test]
fn ctrl_f_on_an_open_find_bar_keeps_the_current_match() {
    let mut t = editor_app();
    t.ctrl(egui::Key::F);
    t.type_text("alpha");
    t.key(egui::Key::Enter);
    assert!(t.has_label("2/3"));
    t.ctrl(egui::Key::F);
    assert!(t.has_label("2/3"));
    // Closing clears the search; reopening starts from the first match again.
    t.key(egui::Key::Escape);
    t.ctrl(egui::Key::F);
    t.type_text("alpha");
    assert!(t.has_label("1/3"));
}

#[test]
fn find_shrinks_the_current_match_when_text_changes() {
    let mut t = editor_app();
    t.ctrl(egui::Key::F);
    t.type_text("alpha");
    t.key(egui::Key::Enter);
    t.key(egui::Key::Enter);
    assert!(t.has_label("3/3"));
    // Replace the whole text so only one match is left.
    t.click_text(1, 2);
    t.ctrl(egui::Key::A);
    t.type_text("alpha only");
    t.steps(2);
    assert!(t.has_label("1/1"));
}

#[test]
fn clicking_the_gutter_toggles_a_bookmark() {
    let mut t = editor_app();
    let text = t.editor_text_rect();
    let (row_height, _) = t.editor_metrics();
    // The star column is the first 18px of the 56px line-number gutter left of the text.
    let gutter_star = egui::pos2(text.left() - 52.0, text.top() + 2.0 + row_height * 1.5);
    t.click_at(gutter_star);
    t.assert_status("Added bookmark: notes.txt:2");
    assert!(t.saved_state().contains("notes.txt:2"));
    t.steps(25);
    t.click_at(gutter_star);
    t.assert_status("Removed bookmark: notes.txt:2");
}

#[test]
fn page_keys_and_wheel_scroll_long_files() {
    let content = (1..=400).map(|n| format!("line {n}\n")).collect::<String>();
    let mut t = TestApp::builder().file("long.txt", &content).build();
    t.click_first("long.txt");
    t.click_text(0, 0);
    t.key(egui::Key::PageDown);
    assert!(t.app().left.scroll.y > 100.0);
    let after_page = t.app().left.scroll.y;
    t.key(egui::Key::PageUp);
    assert!(t.app().left.scroll.y < after_page);

    let pos = t.editor_text_pos(3, 3);
    t.scroll(pos, egui::vec2(0.0, -300.0));
    assert!(t.app().left.scroll.y > 0.0);
    assert!(t.app().app_perf_debug.frame_index > 0);
}

#[test]
fn external_definitions_open_read_only() {
    let mut t = TestApp::builder()
        .file("src/main.rs", "fn main() { helper(); }\n")
        .build();
    let external = t.tmp.join(".cargo").join("registry").join("dep.rs");
    std::fs::create_dir_all(external.parent().unwrap()).unwrap();
    std::fs::write(&external, "pub fn helper() {}\n").unwrap();
    t.click("src");
    t.click_first("main.rs");
    t.click_text(0, 14);
    t.key(egui::Key::F12);
    let request = t
        .platform
        .lsp_requests("rust-analyzer")
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Definition)
        .unwrap();
    t.platform.lsp_respond(
        "rust-analyzer",
        &request,
        Ok(vec![location(&external, 0, 7)]),
    );
    t.steps(3);
    assert_eq!(t.app().left.path.as_deref(), Some(external.as_path()));
    assert!(t.app().left.read_only);
    assert!(t.has_label("READ ONLY"));
    t.ctrl(egui::Key::S);
    t.assert_status("Focused file is read-only.");
}

#[test]
fn opening_another_file_asks_about_unsaved_edits() {
    let mut t = editor_app();
    t.click_text(0, 0);
    t.type_text("unsaved ");
    assert!(t.has_label("● notes.txt"));

    // Cancel keeps the edits and the file.
    t.click_first("bin.dat.txt");
    assert!(t.has_label("Unsaved Changes"));
    t.assert_status("notes.txt has unsaved changes: save or discard them first.");
    t.click("Cancel");
    t.assert_status("Kept unsaved changes.");
    assert!(t.app().pending_unsaved_action.is_none());
    assert!(t.app().left.text.as_str().starts_with("unsaved alpha"));

    // Discard opens the other file and leaves the disk untouched.
    t.click_first("bin.dat.txt");
    t.click("Discard");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("bin.dat.txt").as_path())
    );
    assert!(!t.read("notes.txt").contains("unsaved"));

    // Save (Enter) writes the edits before switching.
    t.click_first("notes.txt");
    t.click_text(0, 0);
    t.type_text("kept ");
    t.click_first("bin.dat.txt");
    t.key(egui::Key::Enter);
    assert!(t.read("notes.txt").starts_with("kept alpha"));
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("bin.dat.txt").as_path())
    );
    assert!(!t.has_label("Unsaved Changes"));
}

#[test]
fn reopening_the_same_file_keeps_unsaved_edits() {
    let mut t = editor_app();
    t.click_text(0, 0);
    t.type_text("unsaved ");
    t.click_first("notes.txt");
    assert!(t.app().pending_unsaved_action.is_none());
    assert!(t.app().left.text.as_str().starts_with("unsaved alpha"));
    assert!(t.has_label("● notes.txt"));
    t.ctrl(egui::Key::S);
    assert!(t.has_label("notes.txt"));
    assert!(!t.has_label("● notes.txt"));
}

#[test]
fn new_workspace_asks_about_unsaved_edits() {
    let mut t = editor_app();
    t.click_text(0, 0);
    t.type_text("unsaved ");
    t.click("New Workspace");
    assert!(t.has_label("Unsaved Changes"));
    t.key(egui::Key::Escape);
    assert!(t.app().pending_unsaved_action.is_none());
    assert!(t.app().left.text.as_str().starts_with("unsaved alpha"));
    t.click("New Workspace");
    t.click("Discard");
    t.assert_status("New session canceled.");
}

#[test]
fn assistance_skips_selections_and_pairs_square_brackets() {
    let mut t = TestApp::builder()
        .file("code.txt", "call(arg]\nopen [ here\n")
        .build();
    t.click_first("code.txt");
    // With a selection, typing replaces it without auto-pairing.
    t.click_text(0, 0);
    t.shift_held(&[egui::Key::ArrowRight, egui::Key::ArrowRight]);
    t.type_text("(");
    assert!(t.app().left.text.as_str().starts_with("(ll("));
    t.click_text(1, 0);
    t.type_text("[");
    assert!(t.app().left.text.as_str().contains("[]open"));
    // Cursor next to unbalanced brackets: no matching pair is highlighted.
    t.click_text(0, 8);
    t.click_text(1, 7);
    t.steps(2);
}

#[test]
fn right_pane_actions_apply_to_the_right_editor() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.click_first("docs");
    let from = t.label_rect("guide.txt").center();
    t.drag(from, egui::pos2(1250.0, 400.0));
    t.steps(2);
    assert_eq!(t.app().focused_pane, super::super::Pane::Right);
    // Edit and save through the right pane's own Save button.
    t.click_at(egui::pos2(1000.0, 120.0));
    t.click_at(egui::pos2(1000.0, 120.0));
    t.type_text("R");
    assert!(t.app().right.text.as_str().contains('R'));
    t.click_nth("Save", 1);
    t.assert_status_containing("Saved file:");
    assert!(t.read("docs/guide.txt").contains('R'));

    // Clicking the left column moves focus back; its Save works too.
    t.click_at(egui::pos2(500.0, 400.0));
    assert_eq!(t.app().focused_pane, super::super::Pane::Left);
    t.click_nth("Save", 0);
    t.assert_status_containing("Saved file:");
    t.ctrl(egui::Key::F);
    assert!(t.app().left.source_search.open);
}

#[test]
fn find_box_handles_enter_in_the_frame_it_gets_focus() {
    let mut t = editor_app();
    t.ctrl(egui::Key::F);
    t.type_text("alpha");
    t.click_text(1, 2);
    // Ctrl+F re-focuses the box and Enter / Shift+Enter arrive in that same frame.
    t.keys_in_one_frame(&[
        (egui::Key::F, CTRL),
        (egui::Key::Enter, egui::Modifiers::NONE),
    ]);
    assert!(t.has_label("2/3"));
    t.click_text(1, 2);
    t.steps(40);
    t.keys_in_one_frame(&[
        (egui::Key::F, CTRL),
        (egui::Key::Enter, egui::Modifiers::SHIFT),
    ]);
    // Ctrl+F keeps the current match, so Shift+Enter steps back from it.
    assert!(t.has_label("1/3"));
}

#[cfg(windows)]
#[test]
fn renaming_a_locked_file_reports_the_os_error() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    // Another program holds the file open without sharing.
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(t.path("notes.txt"))
        .unwrap();
    t.key(egui::Key::F2);
    t.type_text("locked");
    t.key(egui::Key::Enter);
    assert!(t.has_label_containing("Failed to rename file:"));
}
