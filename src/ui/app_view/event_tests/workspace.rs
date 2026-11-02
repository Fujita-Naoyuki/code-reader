//! Workspace sidebar: file tree, file create / rename / delete, root folders, sidebar and
//! pane toggles, drag & drop to the right pane and external change detection.

use std::time::Instant;

use eframe::egui;

use super::super::{Pane, SidebarView, WorkspaceFileDialog};
use super::support::TestApp;

#[test]
fn sidebar_and_right_pane_toggles_show_and_hide_panels() {
    let mut t = TestApp::plain();
    assert!(t.has_label("notes.txt"));

    t.click("Sidebar");
    assert!(!t.app().show_workspace_sidebar);
    assert!(!t.has_label("notes.txt"));
    t.click("Sidebar");
    assert!(t.app().show_workspace_sidebar);

    t.click("Right Pane");
    assert!(t.app().show_right_pane);
    assert_eq!(t.count_label("No file open"), 2);

    // Focus the right pane by clicking into it, then hide it: focus falls back to the left.
    let right_title = t.nth_label_rect("No file open", 1);
    t.click_at(right_title.center() + egui::vec2(0.0, 80.0));
    assert_eq!(t.app().focused_pane, Pane::Right);
    t.click("Right Pane");
    assert!(!t.app().show_right_pane);
    assert_eq!(t.app().focused_pane, Pane::Left);
}

#[test]
fn clicking_folders_toggles_them_and_selects_them() {
    let mut t = TestApp::plain();
    assert!(!t.has_label("guide.txt"));
    t.click("docs");
    assert!(t.has_label("guide.txt"));
    assert_eq!(t.app().selected_workspace_path, Some(t.path("docs")));
    assert!(t.app().workspace_sidebar_active);
    t.click("docs");
    assert!(!t.has_label("guide.txt"));

    t.click("workspace");
    assert!(!t.has_label("notes.txt"));
    assert_eq!(t.app().selected_workspace_path, Some(t.root.clone()));
    t.click("workspace");
    assert!(t.has_label("notes.txt"));
}

#[test]
fn new_file_is_created_in_selected_folder_and_opened() {
    let mut t = TestApp::plain();
    t.click("docs");
    t.click_first("New File");
    assert_eq!(t.count_label("New File"), 2, "dialog title is shown");
    t.type_text("intro.md");
    t.key(egui::Key::Enter);

    let created = t.path("docs/intro.md");
    assert_eq!(t.read("docs/intro.md"), "# intro\n");
    assert_eq!(t.app().left.path(), Some(created.as_path()));
    assert!(t.app().workspace_file_dialog.is_none());
    t.assert_status(&format!("Created file: {}", created.display()));
    assert!(t.app().file_tree.files.contains(&created));
}

#[test]
fn new_file_in_root_with_plain_text_starts_empty() {
    let mut t = TestApp::plain();
    t.click_first("New File");
    t.type_text("todo.txt");
    t.click("Create");
    assert_eq!(t.read("todo.txt"), "");
    t.assert_status_containing("Created file");
}

#[test]
fn new_file_dialog_reports_validation_errors_and_stays_open() {
    let mut t = TestApp::plain();
    t.click_first("New File");

    t.click("Create");
    assert!(t.has_label("File name is required."));

    t.type_text("..");
    t.click("Create");
    assert!(t.has_label("File name cannot be . or .."));

    t.fill_text_field(1, "a?b");
    t.click("Create");
    assert!(t.has_label("File name contains an invalid character."));

    t.fill_text_field(1, "notes.txt");
    t.click("Create");
    assert!(t.has_label_containing("File already exists"));

    t.click("Cancel");
    assert!(t.app().workspace_file_dialog.is_none());
}

#[test]
fn new_file_reports_folders_that_disappear() {
    let mut t = TestApp::plain();
    t.click("docs");
    t.click_first("New File");
    t.type_text("x.txt");
    // The folder is deleted while the dialog is open.
    std::fs::remove_dir_all(t.path("docs")).unwrap();
    t.click("Create");
    assert!(t.has_label_containing("Folder not found"));
    t.key(egui::Key::Escape);
    assert!(t.app().workspace_file_dialog.is_none());

    // With the whole workspace root gone, New File reports it right away.
    std::fs::remove_dir_all(&t.root).unwrap();
    t.click_first("New File");
    t.assert_status_containing("Folder not found");
    assert!(t.app().workspace_file_dialog.is_none());
}

#[test]
fn new_file_next_to_selected_file_uses_its_folder() {
    let mut t = TestApp::plain();
    t.click("docs");
    t.click("guide.txt");
    t.click_first("New File");
    match &t.app().workspace_file_dialog {
        Some(WorkspaceFileDialog::NewFile { folder, .. }) => assert_eq!(folder, &t.path("docs")),
        other => panic!("unexpected dialog: {other:?}"),
    }
}

#[test]
fn f2_renames_the_selected_file_and_updates_open_editor() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.steps(25);
    let star = t.gutter_star_pos(0, 4);
    t.click_at(star);
    // Re-select the file in the tree so F2 applies to it.
    t.click_first("notes.txt");
    t.key(egui::Key::F2);
    assert!(t.has_label("Rename File"));
    // The file stem is pre-selected, so typing replaces "notes" and keeps ".txt".
    t.type_text("renamed");
    t.key(egui::Key::Enter);

    let renamed = t.path("renamed.txt");
    assert!(renamed.exists());
    assert!(!t.path("notes.txt").exists());
    assert_eq!(t.app().left.path(), Some(renamed.as_path()));
    assert_eq!(t.app().left.title, "renamed.txt");
    assert_eq!(t.app().recent_files.first(), Some(&renamed));
    let bookmarks = &t.app().current_session().unwrap().bookmarks;
    assert_eq!(bookmarks[0].path, renamed);
    assert_eq!(bookmarks[0].label, "renamed.txt:1");
    t.assert_status(&format!("Renamed file: {}", renamed.display()));
}

#[test]
fn rename_dialog_reports_errors() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.click_nth("Rename", 1);
    t.click_nth("Rename", 2);
    assert!(t.has_label("The new name is the same as the current name."));

    t.fill_text_field(1, "docs");
    t.click_nth("Rename", 2);
    assert!(t.has_label_containing("File already exists"));

    t.fill_text_field(1, "");
    t.click_nth("Rename", 2);
    assert!(t.has_label("File name is required."));

    // The file is replaced by a folder while the dialog is open.
    std::fs::remove_file(t.path("notes.txt")).unwrap();
    std::fs::create_dir(t.path("notes.txt")).unwrap();
    t.fill_text_field(1, "other.txt");
    t.click_nth("Rename", 2);
    assert!(t.has_label("Select a file to rename."));

    t.click("Cancel");
    assert!(t.app().workspace_file_dialog.is_none());
}

#[test]
fn file_dialogs_refuse_files_whose_root_was_removed() {
    let mut t = TestApp::plain();
    let extra = t.tmp.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("extra.txt"), "extra").unwrap();
    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);

    t.click_first("extra.txt");
    t.key(egui::Key::F2);
    // Remove the root that holds the file while the dialog is still open.
    t.click_nth("×", 1);
    t.fill_text_field(1, "moved.txt");
    t.click_nth("Rename", 2);
    assert!(t.has_label("File is outside the current workspace."));
    t.click("Cancel");

    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    t.click_first("extra.txt");
    t.key(egui::Key::Delete);
    t.click_nth("×", 1);
    t.click_nth("Delete", 1);
    assert!(t.has_label("File is outside the current workspace."));
    assert!(extra.join("extra.txt").exists());
}

#[test]
fn rename_dialog_closes_with_escape() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.key(egui::Key::F2);
    t.key(egui::Key::Escape);
    assert!(t.app().workspace_file_dialog.is_none());
}

#[test]
fn delete_key_deletes_selected_file_after_confirmation() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.steps(25);
    let star = t.gutter_star_pos(0, 4);
    t.click_at(star);
    t.click_first("notes.txt");
    t.key(egui::Key::Delete);
    assert!(t.has_label("This cannot be undone."));
    t.key(egui::Key::Enter);

    assert!(!t.path("notes.txt").exists());
    assert!(t.app().left.path().is_none());
    assert!(t.app().recent_files.is_empty());
    assert!(t.app().current_session().unwrap().bookmarks.is_empty());
    assert!(t.app().selected_workspace_path.is_none());
}

#[test]
fn delete_button_removes_file_shown_in_right_pane() {
    let mut t = TestApp::plain();
    t.click("docs");
    let from = t.label_rect("guide.txt").center();
    t.drag(from, egui::pos2(1250.0, 400.0));
    t.steps(2);
    assert_eq!(
        t.app().right.path(),
        Some(t.path("docs/guide.txt").as_path())
    );
    t.click_first("guide.txt");
    t.click_nth("Delete", 0);
    t.click_nth("Delete", 1);
    assert!(!t.path("docs/guide.txt").exists());
    assert!(t.app().right.path().is_none());
}

#[test]
fn delete_dialog_reports_errors_and_can_be_canceled() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    t.key(egui::Key::Delete);
    assert!(t.has_label("Delete File"));
    // The file is replaced by a folder before the user confirms.
    std::fs::remove_file(t.path("notes.txt")).unwrap();
    std::fs::create_dir(t.path("notes.txt")).unwrap();
    t.click_nth("Delete", 1);
    assert!(t.has_label("Select a file to delete."));
    t.click("Cancel");
    assert!(t.app().workspace_file_dialog.is_none());

    t.click("docs");
    t.click_first("guide.txt");
    t.key(egui::Key::Delete);
    t.key(egui::Key::Escape);
    assert!(t.app().workspace_file_dialog.is_none());
    assert!(t.path("docs/guide.txt").exists());
}

#[test]
fn file_shortcuts_require_a_selected_file_and_an_active_sidebar() {
    let mut t = TestApp::plain();
    // Not active yet: F2 / Delete do nothing.
    t.key(egui::Key::F2);
    assert!(t.app().workspace_file_dialog.is_none());

    t.click("docs");
    t.key(egui::Key::F2);
    assert_eq!(t.status(), "Select a file to rename.");
    t.key(egui::Key::Delete);
    assert_eq!(t.status(), "Select a file to delete.");
    assert!(t.app().workspace_file_dialog.is_none());

    // Clicking into the editor deactivates the sidebar shortcuts.
    t.click_first("notes.txt");
    let editor_point = egui::pos2(900.0, 400.0);
    t.click_at(editor_point);
    assert!(!t.app().workspace_sidebar_active);
}

#[test]
fn selections_of_deleted_files_are_ignored() {
    let mut t = TestApp::plain();
    t.click_first("notes.txt");
    std::fs::remove_file(t.path("notes.txt")).unwrap();
    t.key(egui::Key::F2);
    assert_eq!(t.status(), "Select a file to rename.");
}

#[test]
fn add_folder_adds_a_root_and_remove_button_drops_it() {
    let mut t = TestApp::plain();
    let extra = t.tmp.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("extra.txt"), "extra").unwrap();

    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    assert_eq!(
        t.platform.picker_titles.borrow().last().map(String::as_str),
        Some("Add workspace folder")
    );
    t.assert_status(&format!("Added folder: {}", extra.display()));
    assert!(t.has_label("extra.txt"));
    assert!(t.saved_state().contains("extra"));

    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    t.assert_status_containing("Folder is already in workspace");

    t.click_nth("Add", 0);
    t.assert_status("Add folder canceled.");

    // Two roots: each shows a remove button.
    assert_eq!(t.count_label("×"), 2);
    t.click_nth("×", 1);
    t.assert_status(&format!("Removed folder: {}", extra.display()));
    assert!(!t.has_label("extra.txt"));
    assert_eq!(t.count_label("×"), 0);
}

#[test]
fn dragging_a_file_onto_the_right_side_opens_it_in_the_right_pane() {
    let mut t = TestApp::plain();
    let from = t.label_rect("notes.txt").center();
    let to = egui::pos2(1250.0, 400.0);
    t.drag(from, to);
    t.steps(2);
    assert!(t.app().show_right_pane);
    assert_eq!(t.app().focused_pane, Pane::Right);
    assert_eq!(t.app().right.path(), Some(t.path("notes.txt").as_path()));
    assert!(t.app().dragging_workspace_file.is_none());
}

#[test]
fn dragging_onto_a_visible_right_pane_replaces_its_file() {
    let mut t = TestApp::plain();
    t.click("Right Pane");
    t.click("docs");
    let from = t.label_rect("guide.txt").center();
    t.drag(from, egui::pos2(1250.0, 400.0));
    t.steps(2);
    assert_eq!(
        t.app().right.path(),
        Some(t.path("docs/guide.txt").as_path())
    );
}

#[test]
fn dropping_a_dragged_file_elsewhere_cancels_the_drag() {
    let mut t = TestApp::plain();
    let from = t.label_rect("notes.txt").center();
    t.drag(from, egui::pos2(600.0, 400.0));
    t.steps(2);
    assert!(t.app().dragging_workspace_file.is_none());
    assert!(t.app().right.path().is_none());
}

#[test]
fn external_file_changes_refresh_the_tree() {
    let mut t = TestApp::plain();
    t.wait_for_indexing();
    t.write("added.txt", "new file");
    t.app_mut().next_workspace_change_poll = Instant::now();
    t.wait_for_status("Detected 1 external file changes");
    t.assert_status_containing("updated changed symbols");
    assert!(t.has_label("added.txt"));

    // A dependency manifest change forces a full symbol reindex.
    t.write("Cargo.toml", "[package]\nname = \"x\"\n");
    t.app_mut().next_workspace_change_poll = Instant::now();
    t.wait_until("Cargo.toml discovered", |app| {
        app.file_tree
            .files
            .iter()
            .any(|path| path.ends_with("Cargo.toml"))
    });
    t.wait_for_indexing();
}

#[test]
fn workspace_change_scan_without_changes_keeps_status() {
    let mut t = TestApp::plain();
    t.wait_for_indexing();
    t.app_mut().status_message = "unchanged".to_owned();
    t.app_mut().next_workspace_change_poll = Instant::now();
    t.steps(3);
    t.wait_until("scan finished", |app| {
        app.workspace_change_receiver.is_none()
    });
    assert_eq!(t.status(), "unchanged");
}

#[test]
fn sidebar_tabs_switch_views() {
    let mut t = TestApp::plain();
    t.click("Search");
    assert_eq!(t.app().sidebar_view, SidebarView::Search);
    assert!(t.app().workspace_search.open);
    t.click("Git");
    assert_eq!(t.app().sidebar_view, SidebarView::Git);
    assert!(t.has_label("No Git repositories found in this workspace."));
    t.click("Workspace");
    assert_eq!(t.app().sidebar_view, SidebarView::Workspace);
}

#[test]
fn session_restores_open_files_and_positions() {
    let state = r#"{
        "current_session_id": "s1",
        "sessions": [{
            "id": "s1",
            "display_name": "Restored",
            "root_folders": ["{root}"],
            "bookmarks": [{"label": "notes.txt:2", "path": "{root}\\notes.txt", "position": {"line": 1, "character": 0}}],
            "open_files": {"left": "{root}\\notes.txt", "right": "{root}\\docs\\guide.txt"},
            "pane_positions": {"left": {"line": 1, "character": 2}, "right": {"line": 0, "character": 0}},
            "last_updated_unix": 1
        }]
    }"#;
    let mut t = TestApp::builder()
        .file("notes.txt", "alpha\nbeta\ngamma\n")
        .file("docs/guide.txt", "guide line\n")
        .state(state)
        .build();
    assert_eq!(t.app().left.path(), Some(t.path("notes.txt").as_path()));
    assert_eq!(
        t.app().right.path(),
        Some(t.path("docs/guide.txt").as_path())
    );
    assert_eq!(
        t.app().left.cursor_position,
        Some(crate::core::location::Position::new(1, 2))
    );
    assert!(t.has_label("Restored"));
    assert!(t.has_label("notes.txt:2"));
    assert_eq!(t.app().recent_files.len(), 2);
    t.step();
}

#[test]
fn files_under_external_library_folders_open_read_only() {
    let mut t = TestApp::plain();
    let registry = t.tmp.join(".cargo").join("registry").join("dep");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(registry.join("lib.rs"), "pub fn dep() {}\n").unwrap();
    let gomod = t.tmp.join("go").join("pkg").join("mod").join("dep");
    std::fs::create_dir_all(&gomod).unwrap();
    std::fs::write(gomod.join("dep.go"), "package dep\n\nfunc Dep() {}\n").unwrap();
    t.platform.queue_folder_pick(&registry);
    t.click_nth("Add", 0);
    t.platform.queue_folder_pick(&gomod);
    t.click_nth("Add", 0);
    t.click_first("lib.rs");
    assert!(t.app().left.read_only());
    t.click_first("dep.go");
    assert!(t.app().left.read_only());
    t.click("Report");
    assert!(
        t.platform
            .clipboard_text()
            .unwrap()
            .contains("by_source workspace=")
    );
}

#[test]
fn inconsistent_state_falls_back_to_an_existing_session() {
    let state = r#"{
        "current_session_id": "missing",
        "sessions": [{
            "id": "other",
            "display_name": "Other",
            "root_folders": ["{root}"],
            "bookmarks": [],
            "open_files": {"left": null, "right": null},
            "pane_positions": {"left": {"line": 0, "character": 0}, "right": {"line": 0, "character": 0}},
            "last_updated_unix": 1
        }]
    }"#;
    let mut t = TestApp::builder()
        .file("notes.txt", "x\n")
        .state(state)
        .build();
    // The dangling id is repaired on start and saved.
    assert_eq!(t.app().current_session().unwrap().id, "other");
    assert!(t.has_label("notes.txt"));
    assert!(
        t.saved_state()
            .contains("\"current_session_id\": \"other\"")
    );

    let extra = t.tmp.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("extra.txt"), "e\n").unwrap();
    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    t.assert_status(&format!("Added folder: {}", extra.display()));
    assert!(t.has_label("extra.txt"));
    assert!(t.saved_state().contains("extra"));
}
