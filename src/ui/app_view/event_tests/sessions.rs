//! Workspace sessions: New Workspace, the Recent Workspaces window and session rename.

use eframe::egui;

use super::super::Pane;
use super::support::TestApp;

const TWO_SESSIONS: &str = r#"{
    "current_session_id": "s1",
    "sessions": [
        {
            "id": "s1",
            "display_name": "First",
            "root_folders": ["{root}"],
            "bookmarks": [],
            "open_files": {"left": "{root}\\notes.txt", "right": null},
            "pane_positions": {"left": {"line": 0, "character": 0}, "right": {"line": 0, "character": 0}},
            "last_updated_unix": 2
        },
        {
            "id": "s2",
            "display_name": "Second",
            "root_folders": ["{tmp}\\other", "{tmp}\\r2", "{tmp}\\r3", "{tmp}\\r4", "{tmp}\\r5"],
            "bookmarks": [],
            "open_files": {"left": "{tmp}\\other\\other.txt", "right": "{tmp}\\other\\other.txt"},
            "pane_positions": {"left": {"line": 0, "character": 0}, "right": {"line": 0, "character": 0}},
            "last_updated_unix": 1
        }
    ]
}"#;

fn two_sessions() -> TestApp {
    let t = TestApp::builder()
        .file("notes.txt", "alpha\n")
        .state(TWO_SESSIONS)
        .build();
    std::fs::create_dir_all(t.tmp.join("other")).unwrap();
    std::fs::write(t.tmp.join("other").join("other.txt"), "other\n").unwrap();
    t
}

#[test]
fn new_workspace_creates_and_switches_to_a_session() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.click("Right Pane");
    let new_root = t.tmp.join("new-root");
    std::fs::create_dir_all(&new_root).unwrap();
    std::fs::write(new_root.join("fresh.txt"), "fresh").unwrap();

    t.platform.queue_folder_pick(&new_root);
    t.click("New Workspace");
    assert_eq!(
        t.platform.picker_titles.borrow().last().map(String::as_str),
        Some("New workspace session")
    );
    t.assert_status("Created a new session.");
    assert_eq!(t.app().app_state.sessions.len(), 2);
    assert!(t.has_label("Workspace 2"));
    assert!(t.has_label("fresh.txt"));
    assert!(t.app().left.path.is_none());
    assert!(!t.app().show_right_pane);
    assert!(t.app().recent_files.is_empty());
    assert!(t.saved_state().contains("Workspace 2"));
}

#[test]
fn new_workspace_canceled_keeps_current_session() {
    let mut t = TestApp::plain();
    t.click("New Workspace");
    t.assert_status("New session canceled.");
    assert_eq!(t.app().app_state.sessions.len(), 1);
}

#[test]
fn recent_workspaces_switches_session() {
    let mut t = two_sessions();
    t.click("Workspaces");
    assert!(t.app().recent_workspaces_open);
    assert!(t.has_label("2 workspaces"));
    // The window draws session names with the painter; click the name column of the
    // second row (sessions are sorted by last update, newest first).
    let header = t.label_rect("2 workspaces");
    let second_row = egui::pos2(header.left() + 60.0, header.bottom() + 52.0);
    t.click_at(second_row);
    t.assert_status("Switched session.");
    assert_eq!(t.app().app_state.current_session_id, "s2");
    assert!(!t.app().recent_workspaces_open);
    assert!(t.has_label("Second"));
    assert!(t.app().left.path.as_ref().unwrap().ends_with("other.txt"));
    assert_eq!(t.app().focused_pane, Pane::Left);
}

#[test]
fn recent_workspaces_ignores_clicks_on_the_current_session() {
    let mut t = two_sessions();
    t.click("Workspaces");
    let header = t.label_rect("2 workspaces");
    let first_row = egui::pos2(header.left() + 60.0, header.bottom() + 24.0);
    t.click_at(first_row);
    assert_eq!(t.app().app_state.current_session_id, "s1");
    assert!(t.app().recent_workspaces_open);
    t.key(egui::Key::Escape);
    assert!(!t.app().recent_workspaces_open);
}

#[test]
fn recent_workspaces_deletes_another_session_after_confirmation() {
    let mut t = two_sessions();
    t.click("Workspaces");
    // Only the non-current session has a delete button.
    assert_eq!(t.count_label("×"), 1);
    t.click("×");
    assert!(t.has_label("Delete this session?"));
    t.click("Cancel");
    assert!(t.app().pending_delete_session_id.is_none());

    t.click("×");
    // Clicking the pending session's name does not switch to it.
    let header = t.label_rect("2 workspaces");
    t.click_at(egui::pos2(header.left() + 60.0, header.bottom() + 52.0));
    assert_eq!(t.app().app_state.current_session_id, "s1");

    t.click_nth("Delete", 1);
    t.assert_status("Deleted session: Second");
    assert_eq!(t.app().app_state.sessions.len(), 1);
    assert!(!t.saved_state().contains("Second"));
}

#[test]
fn deleting_the_current_session_is_refused() {
    let mut t = two_sessions();
    t.app_mut().delete_session("s1");
    t.step();
    t.assert_status("Cannot delete the current workspace session.");
}

#[test]
fn recent_workspaces_with_no_sessions_shows_empty_message() {
    let mut t = TestApp::plain();
    t.app_mut().app_state.sessions.clear();
    t.app_mut().recent_workspaces_open = true;
    t.settle();
    assert!(t.has_label("No recent workspaces"));
    assert!(t.has_label("No session"));
}

#[test]
fn everywhere_button_closes_recent_workspaces() {
    let mut t = two_sessions();
    t.click("Workspaces");
    t.click("Everywhere");
    assert!(!t.app().recent_workspaces_open);
    assert!(t.app().search_overlay.open);
}

#[test]
fn rename_workspace_with_enter() {
    let mut t = TestApp::plain();
    t.click_first("Rename");
    assert!(t.has_label("Rename Workspace"));
    assert!(t.app().editing_session_name);
    // The current name is pre-selected, so typing replaces it.
    t.type_text("Renamed");
    t.key(egui::Key::Enter);
    t.assert_status("Renamed session: Renamed");
    assert!(!t.app().editing_session_name);
    assert!(t.has_label("Renamed"));
    assert!(t.saved_state().contains("Renamed"));
}

#[test]
fn rename_workspace_rejects_empty_name_and_can_be_canceled() {
    let mut t = TestApp::plain();
    t.click_first("Rename");
    t.key(egui::Key::Backspace);
    t.click_nth("Rename", 2);
    t.assert_status("Workspace session name cannot be empty.");
    assert!(t.app().editing_session_name);
    t.click("Cancel");
    assert!(!t.app().editing_session_name);

    t.click_first("Rename");
    t.key(egui::Key::Escape);
    assert!(!t.app().editing_session_name);
}

#[test]
fn rename_without_a_session_reports_it() {
    let mut t = TestApp::plain();
    t.app_mut().app_state.sessions.clear();
    t.step();
    t.click_first("Rename");
    t.assert_status("No current session to rename.");
}

#[test]
fn renaming_reports_save_failures() {
    let mut t = TestApp::plain();
    // A directory where the state file should be makes every save fail.
    std::fs::create_dir_all(&t.platform.state_path).unwrap();
    t.click_first("Rename");
    t.type_text("X");
    t.key(egui::Key::Enter);
    t.assert_status_containing("Renamed session, but save failed");

    t.click_first("notes.txt");
    t.assert_status_containing("Failed to save state");
}

#[test]
fn corrupt_state_file_falls_back_to_default_session() {
    let t = TestApp::builder()
        .file("notes.txt", "x")
        .state("{ not json")
        .build();
    assert_eq!(t.app().app_state.sessions.len(), 1);
    assert_eq!(
        t.app().current_session().unwrap().root_folders,
        vec![t.root.clone()]
    );
}
