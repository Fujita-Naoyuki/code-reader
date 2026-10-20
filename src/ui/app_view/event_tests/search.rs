//! Search Everywhere overlay, workspace (full-text) search and sidebar bookmarks.

use eframe::egui;

use super::super::SidebarView;
use super::support::TestApp;

fn search_workspace() -> TestApp {
    TestApp::builder()
        .file("alpha.txt", "first needle here\nsecond line\n")
        .file("beta.txt", "no match\nanother needle\nneedle again\n")
        .file("src/lib.rs", "pub fn compute_total() -> u32 {\n    1\n}\n")
        .build()
}

fn overlay_row(t: &TestApp, index: usize) -> egui::Pos2 {
    // Result rows are painted below the "N results" line: separator, then 24px rows.
    let header = t.label_containing_rect(" results");
    egui::pos2(
        header.left() + 200.0,
        header.bottom() + 20.0 + index as f32 * 24.0,
    )
}

#[test]
fn ctrl_p_opens_search_everywhere_and_enter_opens_the_selected_file() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::P);
    assert!(t.app().search_overlay.open);
    assert!(t.has_label("Search Everywhere"));
    t.type_text("beta");
    assert_eq!(t.app().search_overlay.query, "beta");
    t.key(egui::Key::Enter);
    assert!(!t.app().search_overlay.open);
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("beta.txt").as_path())
    );
    t.assert_status("Opened File: beta.txt");
    // Re-opening shows the file as recent history first.
    t.ctrl(egui::Key::P);
    t.type_text("beta");
    t.key(egui::Key::Enter);
    t.assert_status("Opened History: beta.txt");
}

#[test]
fn search_everywhere_finds_symbols_and_bookmarks() {
    let mut t = search_workspace();
    t.click_first("alpha.txt");
    t.app_mut().add_bookmark_from_current_position(true);
    t.step();
    t.assert_status("Added temporary bookmark.");

    t.ctrl_shift(egui::Key::O);
    assert!(t.app().search_overlay.open);
    t.type_text("compute_total");
    t.key(egui::Key::Enter);
    t.assert_status("Opened Symbol: compute_total");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("src/lib.rs").as_path())
    );

    t.ctrl(egui::Key::P);
    t.type_text("alpha.txt:1");
    t.key(egui::Key::Enter);
    t.assert_status_containing("alpha.txt");
}

#[test]
fn search_everywhere_keyboard_selection_and_escape() {
    let mut t = search_workspace();
    t.click("Everywhere");
    assert!(t.app().search_overlay.open);
    t.key(egui::Key::ArrowDown);
    assert_eq!(t.app().search_overlay.selected_index, 1);
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowUp);
    assert_eq!(t.app().search_overlay.selected_index, 1);
    t.key(egui::Key::ArrowUp);
    t.key(egui::Key::ArrowUp);
    assert_eq!(t.app().search_overlay.selected_index, 0);
    // Typing resets the selection.
    t.key(egui::Key::ArrowDown);
    t.type_text("a");
    assert_eq!(t.app().search_overlay.selected_index, 0);
    t.key(egui::Key::Escape);
    assert!(!t.app().search_overlay.open);
    assert!(t.app().search_overlay.query.is_empty());
}

#[test]
fn search_everywhere_with_no_results() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::P);
    t.type_text("zzzzzzzz");
    assert!(t.has_label("No results"));
    assert!(t.has_label("0 results"));
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowUp);
    t.key(egui::Key::Enter);
    assert!(
        t.app().search_overlay.open,
        "nothing to open keeps the overlay"
    );
    // Shrinking result lists clamp the selected index.
    t.app_mut().search_overlay.selected_index = 5;
    t.step();
    assert_eq!(t.app().search_overlay.selected_index, 0);
}

#[test]
fn search_everywhere_rows_can_be_hovered_and_clicked() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::P);
    t.type_text(".txt");
    let second = overlay_row(&t, 1);
    t.move_pointer(second);
    t.step();
    assert_eq!(t.app().search_overlay.selected_index, 1);
    t.click_at(second);
    assert!(!t.app().search_overlay.open);
    assert!(t.app().left.path.is_some());
}

#[test]
fn double_shift_opens_search_everywhere() {
    let mut t = search_workspace();
    t.press_shift_twice();
    assert!(t.app().search_overlay.open);
}

#[test]
fn slow_shift_taps_do_not_open_search_everywhere() {
    let mut t = search_workspace();
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.steps(40);
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.step();
    assert!(!t.app().search_overlay.open);
}

#[test]
fn ctrl_f_without_a_searchable_editor_opens_search_everywhere() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::F);
    assert!(t.app().search_overlay.open);
}

#[test]
fn search_everywhere_reuses_cached_results() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::P);
    t.steps(3);
    assert!(t.app().search_overlay.cache.is_some());
    let revision = t.app().search_index_revision;
    t.steps(2);
    assert_eq!(t.app().search_index_revision, revision);
}

#[test]
fn ctrl_shift_f_runs_workspace_search_and_opens_a_match() {
    let mut t = search_workspace();
    t.ctrl_shift(egui::Key::F);
    assert_eq!(t.app().sidebar_view, SidebarView::Search);
    assert!(t.app().workspace_search.open);
    t.type_text("needle");
    t.key(egui::Key::Enter);
    t.assert_status("Workspace search started: needle");
    t.wait_for_status("Workspace search: 3 results in 2 files.");
    assert!(t.has_label("3 results in 2 files"));
    assert!(t.has_label_containing("alpha.txt (1)"));

    t.click_containing("2: another needle");
    t.assert_status_containing("Opened search result:");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("beta.txt").as_path())
    );
    assert_eq!(
        t.app().left.cursor_position,
        Some(crate::core::location::Position::new(1, 8))
    );
}

#[test]
fn workspace_search_button_clear_and_empty_query() {
    let mut t = search_workspace();
    // The sidebar tab and the button inside the view are both labeled "Search".
    t.click("Search");
    t.click_nth("Search", 1);
    t.assert_status("Workspace search query is empty.");

    t.ctrl_shift(egui::Key::F);
    t.type_text("nothing-matches-this");
    t.click_nth("Search", 1);
    t.wait_until("search finished", |app| !app.workspace_search.searching);
    assert!(t.has_label("No results"));

    t.click("Clear");
    assert!(!t.app().workspace_search.open);
    assert!(t.app().workspace_search.query.is_empty());
}

#[test]
fn escape_in_workspace_search_clears_it() {
    let mut t = search_workspace();
    t.ctrl_shift(egui::Key::F);
    t.type_text("needle");
    t.key(egui::Key::Escape);
    // DISCREPANCY(D02): Help and the status bar say "Esc: Clear", but egui drops the text
    // field's focus on Escape before workspace_search_ui() checks it, so nothing is cleared.
    assert!(t.app().workspace_search.open);
    assert_eq!(t.app().workspace_search.query, "needle");
}

#[test]
fn workspace_search_shows_progress_and_ignores_stale_results() {
    let mut t = search_workspace();
    t.ctrl_shift(egui::Key::F);
    t.type_text("needle");
    t.key(egui::Key::Enter);
    // Start a second search before the first result is applied: the first is ignored.
    t.app_mut().workspace_search.active_search_id += 100;
    t.app_mut().workspace_search.searching = true;
    t.step();
    assert!(t.has_label("Searching..."));
    t.wait_until("stale result consumed", |app| {
        app.workspace_search_receiver.is_none()
    });
    assert!(t.app().workspace_search.searching);
}

#[test]
fn workspace_search_reports_a_failed_worker() {
    let mut t = search_workspace();
    let (sender, receiver) = std::sync::mpsc::channel::<super::super::WorkspaceSearchResult>();
    drop(sender);
    t.app_mut().workspace_search_receiver = Some(receiver);
    t.app_mut().workspace_search.searching = true;
    t.step();
    t.assert_status("Workspace search failed.");
    assert!(!t.app().workspace_search.searching);
}

#[test]
fn sidebar_bookmarks_open_and_remove() {
    let mut t = search_workspace();
    assert!(t.has_label("No bookmarks"));
    t.click_first("beta.txt");
    t.app_mut().left.cursor_position = Some(crate::core::location::Position::new(2, 0));
    t.app_mut().add_bookmark_from_current_position(false);
    t.click_first("alpha.txt");
    t.app_mut().add_bookmark_from_current_position(true);
    t.step();
    assert!(t.has_label("beta.txt:3"));
    assert!(t.has_label("alpha.txt:1"));
    // Temporary bookmarks are not persisted.
    assert!(t.saved_state().contains("beta.txt:3"));
    assert!(!t.saved_state().contains("alpha.txt:1"));

    t.click("beta.txt:3");
    t.assert_status("Opened bookmark: beta.txt:3");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("beta.txt").as_path())
    );

    // Remove the temporary one (first in the list), then the persisted one.
    t.click_nth("×", 0);
    t.assert_status("Removed bookmark: alpha.txt:1");
    t.click_nth("×", 0);
    t.assert_status("Removed bookmark: beta.txt:3");
    assert!(!t.saved_state().contains("beta.txt:3"));
}

#[test]
fn bookmark_without_open_file_is_refused() {
    let mut t = search_workspace();
    t.app_mut().add_bookmark_from_current_position(false);
    t.step();
    t.assert_status("Open a file before adding a bookmark.");
    t.app_mut().remove_bookmark(10);
    t.step();
}

#[test]
fn workspace_search_skips_unreadable_files_and_caps_results() {
    let many = "hit\n".repeat(2100);
    let long_line = format!("hit {}\n", "y".repeat(300));
    let mut t = TestApp::builder()
        .bytes("binary.txt", &[0xff, 0xfe, 0x00, b'h', b'i', b't'])
        .file("many.txt", &many)
        .file("other.txt", "hit once\n")
        .file("wide.txt", &long_line)
        .build();
    t.ctrl_shift(egui::Key::F);
    t.type_text("hit");
    t.key(egui::Key::Enter);
    t.wait_until("search finished", |app| !app.workspace_search.searching);
    let total = t.app().workspace_search.match_count();
    assert_eq!(total, 2000, "results stop at the cap");
    assert!(
        t.app()
            .workspace_search
            .results
            .iter()
            .all(|result| !result.path.ends_with("binary.txt"))
    );
}

#[test]
fn search_box_runs_when_enter_arrives_with_the_focus_request() {
    let mut t = search_workspace();
    t.ctrl_shift(egui::Key::F);
    t.type_text("needle");
    t.click_at(egui::pos2(1000.0, 500.0));
    // Ctrl+Shift+F and Enter in the same frame: the box regains focus and searches.
    let modifiers = super::support::CTRL | egui::Modifiers::SHIFT;
    t.set_modifiers(modifiers);
    t.harness.event(egui::Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    t.harness.event(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.step();
    t.assert_status("Workspace search started: needle");
}
