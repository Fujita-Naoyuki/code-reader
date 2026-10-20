//! Top-bar windows: Help (F1), AI Cmd and the diagnostics Report.

use eframe::egui;

use super::super::{
    CsvSelection, GitDiscardRequest, MarkdownFocusTarget, MarkdownMode, Pane, SidebarView,
};
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

/// Opens Help in a given UI state and returns the context title shown at its top.
fn help_title(t: &mut TestApp) -> String {
    t.app_mut().shortcuts_help_open = true;
    t.settle();
    let title = t.app().shortcut_context_title().to_owned();
    assert!(t.has_label(&title));
    t.app_mut().shortcuts_help_open = false;
    t.step();
    title
}

#[test]
fn help_and_status_bar_follow_the_current_context() {
    let mut t = TestApp::builder()
        .file("notes.txt", "alpha\n")
        .file("doc.md", "# Title\n\ntext\n\n## Sub\n")
        .file("table.csv", "a,b\n1,2\n")
        .build();

    assert_eq!(help_title(&mut t), "Source");

    t.app_mut().search_overlay.open = true;
    assert_eq!(help_title(&mut t), "Search Everywhere");
    assert!(t.has_label_containing("Enter Open | Up/Down Select"));
    t.app_mut().close_search_overlay();

    t.app_mut().recent_workspaces_open = true;
    assert_eq!(help_title(&mut t), "Recent Workspaces");
    t.app_mut().recent_workspaces_open = false;

    let root = t.root.clone();
    t.app_mut().git_create_branch_root = Some(root.clone());
    assert_eq!(t.app().shortcut_context_title(), "Create Branch");
    assert_eq!(t.app().status_bar_shortcuts()[0].keys, "Enter");
    t.app_mut().git_create_branch_root = None;

    t.app_mut().git_switch_branch_root = Some(root.clone());
    assert_eq!(t.app().shortcut_context_title(), "Switch Branch");
    t.app_mut().git_switch_branch_root = None;

    t.app_mut().pending_git_discard = Some(GitDiscardRequest {
        repository_root: root.clone(),
        repository_name: "repo".to_owned(),
        path: "x".to_owned(),
        untracked: false,
    });
    assert_eq!(t.app().shortcut_context_title(), "Discard Changes");
    assert_eq!(t.app().context_shortcuts()[0].label, "Confirm");
    t.app_mut().pending_git_discard = None;

    t.app_mut().start_session_rename();
    assert_eq!(help_title(&mut t), "Rename Workspace");
    t.app_mut().cancel_session_rename();

    t.app_mut().open_workspace_search();
    assert_eq!(help_title(&mut t), "Workspace Search");
    assert_eq!(t.app().status_bar_shortcuts()[1].label, "Clear");
    t.app_mut().close_workspace_search();
    t.app_mut().sidebar_view = SidebarView::Workspace;

    t.step();
    t.click_first("notes.txt");
    assert_eq!(help_title(&mut t), "Workspace Files");
    assert_eq!(t.app().status_bar_shortcuts()[0].keys, "F2");
    t.app_mut().workspace_sidebar_active = false;

    t.app_mut().open_source_search();
    assert_eq!(help_title(&mut t), "Source Search");
    t.app_mut().focused_editor_mut().close_source_search();
    assert_eq!(help_title(&mut t), "Source");

    t.open_file("doc.md");
    t.app_mut().workspace_sidebar_active = false;
    assert_eq!(help_title(&mut t), "Markdown Preview");
    for (target, title) in [
        (MarkdownFocusTarget::Pages, "Markdown Pages"),
        (MarkdownFocusTarget::Outline, "Markdown Outline"),
        (MarkdownFocusTarget::Body, "Markdown Preview"),
    ] {
        t.app_mut().left.markdown_focus_target = target;
        assert_eq!(t.app().shortcut_context_title(), title);
        let _ = t.app().status_bar_shortcuts();
        let _ = t.app().context_shortcuts();
    }
    t.app_mut().left.markdown_mode = MarkdownMode::Both;
    for (target, title) in [
        (MarkdownFocusTarget::Pages, "Markdown Pages"),
        (MarkdownFocusTarget::Outline, "Markdown Outline"),
        (MarkdownFocusTarget::Body, "Markdown Both"),
    ] {
        t.app_mut().left.markdown_focus_target = target;
        assert_eq!(t.app().shortcut_context_title(), title);
        let _ = t.app().status_bar_shortcuts();
        let _ = t.app().context_shortcuts();
    }
    t.app_mut().left.markdown_mode = MarkdownMode::Markdown;
    assert_eq!(help_title(&mut t), "Markdown Source");
    let _ = t.app().status_bar_shortcuts();
    t.app_mut().left.markdown_mode = MarkdownMode::Preview;
    t.app_mut().left.markdown_focus_target = MarkdownFocusTarget::Body;
    assert!(t.app_mut().open_markdown_search());
    assert_eq!(help_title(&mut t), "Markdown Search");
    t.app_mut().left.markdown_search.open = false;

    t.open_file("table.csv");
    t.app_mut().workspace_sidebar_active = false;
    assert_eq!(help_title(&mut t), "CSV Grid");
    assert_eq!(t.app().status_bar_shortcuts()[0].keys, "Tab");
    t.app_mut().left.csv_selection = Some(CsvSelection::Rows { start: 0, end: 0 });
    assert_eq!(t.app().status_bar_shortcuts()[0].keys, "Ctrl+C");
    t.app_mut().left.csv_selection = None;

    let pane = t.app().focused_pane;
    assert_eq!(pane, Pane::Left);
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

    t.app_mut().app_state.cli_agent.executable.clear();
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
    let location = super::support::location(&t.path("src/main.rs"), 0, 3);
    t.app_mut()
        .definition_candidates
        .push(super::super::DefinitionCandidate {
            label: "candidate".to_owned(),
            location,
            preview: None,
        });
    t.click("Report");
    t.assert_status("Copied diagnostics report.");
    let report = t.platform.clipboard_text().unwrap();
    assert!(report.starts_with("CodeReader perf report"));
    assert!(report.contains("App details:"));
    assert!(report.contains("Editor details:"));
    assert!(report.contains("Symbol index: total="));
    assert!(report.contains("candidate ->"));
    assert!(report.contains("rust_lsp=true"));

    t.platform.clipboard_fails.set(true);
    t.click("Report");
    t.assert_status("Diagnostics report copy failed: clipboard unavailable");
}

#[test]
fn report_without_session_or_command() {
    let mut t = TestApp::plain();
    t.app_mut().app_state.sessions.clear();
    t.step();
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
