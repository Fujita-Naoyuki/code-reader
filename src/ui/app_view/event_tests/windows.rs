//! Top-bar windows: Help (F1), AI Cmd and the diagnostics Report.

use eframe::egui;

use super::super::{CsvSelection, MarkdownFocusTarget, MarkdownMode, Pane};
use super::support::{CTRL, TestApp};

#[test]
fn f1_and_help_button_open_help_and_escape_closes_it() {
    let mut t = TestApp::plain();
    t.key(egui::Key::F1);
    assert!(t.app().shortcuts_help_open);
    assert_eq!(t.count_label("Help"), 2, "button and window title");
    assert!(t.has_label("Source"), "context title for an empty editor");
    t.key(egui::Key::Escape);
    assert!(!t.app().shortcuts_help_open);

    t.click_first("Help");
    assert!(t.app().shortcuts_help_open);
}

/// Presses F1 and checks the context title at the top of the Help window.
#[track_caller]
fn assert_help_context(t: &mut TestApp, title: &str) {
    t.key(egui::Key::F1);
    t.settle();
    assert_eq!(t.app().shortcut_context_title(), title);
    assert!(t.has_label(title), "Help shows {title:?}");
    let _ = t.app().context_shortcuts();
}

/// Closes the Help window (Escape goes to the topmost window that handles it).
fn close_help(t: &mut TestApp) {
    while t.app().shortcuts_help_open {
        t.key(egui::Key::Escape);
    }
}

fn status_keys(t: &TestApp) -> Vec<&'static str> {
    t.app()
        .status_bar_shortcuts()
        .iter()
        .map(|shortcut| shortcut.keys)
        .collect()
}

#[test]
fn help_and_status_bar_follow_the_current_context() {
    let mut t = TestApp::builder()
        .file("notes.txt", "alpha\n")
        .file("doc.md", "# Title\n\ntext\n\n## Sub\n")
        .file("table.csv", "a,b\n1,2\n")
        .git()
        .build();
    t.write("notes.txt", "changed\n");

    assert_help_context(&mut t, "Source");
    close_help(&mut t);

    t.ctrl(egui::Key::P);
    assert_help_context(&mut t, "Search Everywhere");
    assert_eq!(status_keys(&t)[0], "Enter");
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.click("Workspaces");
    assert_help_context(&mut t, "Recent Workspaces");
    assert_eq!(status_keys(&t), vec!["Esc"]);
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.click("Git");
    t.steps(2);
    t.click("...");
    t.click("Create Branch...");
    assert_help_context(&mut t, "Create Branch");
    assert_eq!(status_keys(&t)[0], "Enter");
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.click("main ▾");
    assert_help_context(&mut t, "Switch Branch");
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.click("↶");
    assert_help_context(&mut t, "Discard Changes");
    // DISCREPANCY(D15): Help and the status bar offer "Enter: Confirm" for Switch Branch
    // and Discard Changes, but those windows do not handle Enter at all.
    assert_eq!(t.app().context_shortcuts()[0].keys, "Enter");
    t.key(egui::Key::Enter);
    assert!(t.app().pending_git_discard.is_some());
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.click("Workspace");
    t.click_first("Rename");
    assert_help_context(&mut t, "Rename Workspace");
    t.key(egui::Key::Escape);
    close_help(&mut t);

    t.ctrl_shift(egui::Key::F);
    assert_help_context(&mut t, "Workspace Search");
    assert_eq!(status_keys(&t)[1], "Esc");
    close_help(&mut t);
    t.click("Clear");
    t.click("Workspace");

    t.click_first("notes.txt");
    assert_help_context(&mut t, "Workspace Files");
    assert_eq!(status_keys(&t)[0], "F2");
    close_help(&mut t);

    t.click_text(0, 0);
    t.ctrl(egui::Key::F);
    assert_help_context(&mut t, "Source Search");
    close_help(&mut t);
    t.click("Close");

    t.click_first("doc.md");
    t.click_at(egui::pos2(900.0, 500.0));
    assert_help_context(&mut t, "Markdown Preview");
    assert_eq!(status_keys(&t)[0], "Ctrl+PgUp/PgDn");
    close_help(&mut t);
    t.alt(egui::Key::Num2);
    assert_help_context(&mut t, "Markdown Source");
    assert_eq!(status_keys(&t)[0], "Alt+1/2/3");
    close_help(&mut t);
    t.alt(egui::Key::Num3);
    assert_help_context(&mut t, "Markdown Both");
    close_help(&mut t);
    t.alt(egui::Key::Num1);
    t.ctrl(egui::Key::F);
    assert_help_context(&mut t, "Markdown Search");
    close_help(&mut t);
    t.click("Close");

    // "Markdown Pages" / "Markdown Outline" need a focus target users cannot reach (D11);
    // their entries are checked by driving the focus target directly.
    for mode in [MarkdownMode::Preview, MarkdownMode::Both] {
        t.app_mut().left.markdown_mode = mode;
        for (target, title) in [
            (MarkdownFocusTarget::Pages, "Markdown Pages"),
            (MarkdownFocusTarget::Outline, "Markdown Outline"),
        ] {
            t.app_mut().left.markdown_focus_target = target;
            assert_eq!(t.app().shortcut_context_title(), title);
            let _ = t.app().status_bar_shortcuts();
            let _ = t.app().context_shortcuts();
        }
    }
    t.app_mut().left.markdown_focus_target = MarkdownFocusTarget::Body;
    t.app_mut().left.markdown_mode = MarkdownMode::Preview;

    t.click_first("table.csv");
    t.click_at(egui::pos2(1200.0, 600.0));
    assert_help_context(&mut t, "CSV Grid");
    assert_eq!(status_keys(&t)[0], "Tab");
    close_help(&mut t);
    // Selecting a row switches the hints to row/column actions.
    let field = t.text_field_rects()[0];
    t.click_at(egui::pos2(field.left() - 40.0, field.center().y));
    assert_eq!(status_keys(&t)[0], "Ctrl+C");
    assert!(matches!(
        t.app().left.csv_selection,
        Some(CsvSelection::Rows { .. })
    ));
    assert_eq!(t.app().focused_pane, Pane::Left);
}

#[test]
fn ai_cmd_window_previews_edits_and_copies_the_command() {
    let mut t = TestApp::plain();
    t.click("AI Cmd");
    assert!(t.app().ai_command_open);
    assert!(t.has_label("Preview"));
    assert!(t.has_label_containing("claude"));

    // Text field #0 is the (read-only) editor; the window's fields follow.
    t.fill_text_field(1, "codex");
    assert_eq!(t.app().app_state.cli_agent.executable, "codex");
    assert!(
        t.saved_state().contains("codex"),
        "edits are saved immediately"
    );
    assert!(t.has_label_containing("codex"));

    t.click("Copy");
    let copied = t.platform.clipboard_text().unwrap();
    assert!(copied.starts_with("cd \""));
    assert!(copied.ends_with("codex"));
    t.assert_status_containing("Copied Claude Code command: base");

    // An empty executable shows the error instead of a preview and disables Copy.
    t.fill_text_field(1, "");
    assert!(t.has_label("AI command executable is empty."));
    t.click("Copy");
    assert!(t.platform.clipboard_text().unwrap().ends_with("codex"));

    t.key(egui::Key::Escape);
    assert!(!t.app().ai_command_open);
}

#[test]
fn ai_cmd_add_dir_option_is_required_with_several_roots() {
    let mut t = TestApp::plain();
    let extra = t.tmp.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    t.click("AI Cmd");
    assert!(t.has_label_containing("--add-dir"));
    t.fill_text_field(2, "");
    assert!(t.has_label("AI command add-dir option is empty."));
}

#[test]
fn ctrl_shift_a_copies_the_ai_command() {
    let mut t = TestApp::plain();
    t.ctrl_shift(egui::Key::A);
    t.assert_status_containing("Copied Claude Code command");
    assert!(t.platform.clipboard_text().unwrap().contains("claude"));

    t.platform.clipboard_fails.set(true);
    // Wait out the Shift double-tap window (see D03) before pressing Ctrl+Shift+A again.
    t.steps(40);
    t.ctrl_shift(egui::Key::A);
    t.assert_status("AI command copy failed: clipboard unavailable");

    t.platform.clipboard_fails.set(false);
    t.click("AI Cmd");
    t.fill_text_field(1, "");
    t.key(egui::Key::Escape);
    t.steps(40);
    t.ctrl_shift(egui::Key::A);
    t.assert_status("AI command unavailable: AI command executable is empty.");
}

#[test]
fn two_quick_shift_combinations_open_search_everywhere() {
    let mut t = TestApp::plain();
    t.ctrl_shift(egui::Key::A);
    t.ctrl_shift(egui::Key::A);
    // DISCREPANCY(D03): "Shift x2" only looks at the Shift modifier going down, so two
    // quick Ctrl+Shift shortcuts (or Shift+letter while typing) open Search Everywhere.
    assert!(t.app().search_overlay.open);
}

#[test]
fn ctrl_shift_a_is_ignored_while_search_everywhere_is_open() {
    let mut t = TestApp::plain();
    t.ctrl(egui::Key::P);
    t.key_with(CTRL | egui::Modifiers::SHIFT, egui::Key::A);
    assert!(t.platform.clipboard_text().is_none());
}

#[test]
fn report_copies_diagnostics() {
    let mut t = TestApp::builder()
        .file("src/main.rs", "fn main() {}\n")
        .file("notes.txt", "x\n")
        .build();
    t.click("src");
    t.click_first("main.rs");
    // Two definition candidates from rust-analyzer end up in the report.
    t.click_text(0, 3);
    t.key(egui::Key::F12);
    let request = t
        .platform
        .lsp_requests("rust-analyzer")
        .into_iter()
        .find(|request| request.kind == crate::core::lsp::LspRequestKind::Definition)
        .unwrap();
    let main = t.path("src/main.rs");
    t.platform.lsp_respond(
        "rust-analyzer",
        &request,
        Ok(vec![
            super::support::location(&main, 0, 3),
            super::support::location(&main, 0, 0),
        ]),
    );
    t.steps(3);
    t.click("Report");
    t.assert_status("Copied diagnostics report.");
    let report = t.platform.clipboard_text().unwrap();
    assert!(report.starts_with("CodeReader perf report"));
    assert!(report.contains("App details:"));
    assert!(report.contains("Editor details:"));
    assert!(report.contains("Symbol index: total="));
    assert!(report.contains("Definition locations: 2"));
    assert!(report.contains(" -> "));
    assert!(report.contains("rust_lsp=true"));

    t.platform.clipboard_fails.set(true);
    t.click("Report");
    t.assert_status("Diagnostics report copy failed: clipboard unavailable");
}

#[test]
fn report_without_session_or_command() {
    let mut t = TestApp::builder()
        .file("notes.txt", "x")
        .state(r#"{"current_session_id": "gone", "sessions": []}"#)
        .build();
    t.click("Report");
    let report = t.platform.clipboard_text().unwrap();
    assert!(report.contains("session=none"));
    assert!(report.contains("ai_command_error=No workspace folder is available."));
    assert!(report.contains("  none"));
}

#[test]
fn report_describes_both_editors_in_detail() {
    let mut t = TestApp::builder()
        .file("doc.md", "# Doc\n\nneedle\n")
        .file("data.csv", "a,b\n1,2\n")
        .file("notes.txt", "needle here\n")
        .build();
    t.click_first("doc.md");
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    let from = t.label_rect("data.csv").center();
    t.drag(from, egui::pos2(1250.0, 400.0));
    t.steps(2);
    t.click("Report");
    let report = t.platform.clipboard_text().unwrap();
    assert!(report.contains("Left markdown_mode=Preview"), "{report}");
    assert!(report.contains("Left markdown_search=1/1 query=needle"));
    assert!(report.contains("Right csv_mode=Grid"));
    assert!(report.contains("Right csv_cell=R1C1"));

    t.click_first("notes.txt");
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    t.click("Report");
    let report = t.platform.clipboard_text().unwrap();
    assert!(
        report.contains("source_search=1/1 query=needle"),
        "{report}"
    );
    assert!(report.contains("definition_target=") || report.contains("cursor="));
}

#[test]
fn ai_cmd_edits_report_save_failures() {
    let mut t = TestApp::plain();
    std::fs::create_dir_all(&t.platform.state_path).unwrap();
    t.click("AI Cmd");
    t.fill_text_field(1, "codex");
    t.assert_status_containing("Failed to save state:");
}
