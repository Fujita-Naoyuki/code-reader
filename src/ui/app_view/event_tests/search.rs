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
    assert_eq!(t.app().left.path(), Some(t.path("beta.txt").as_path()));
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
    t.steps(25);
    let star = t.gutter_star_pos(0, 3);
    t.click_at(star);
    t.assert_status("Added bookmark: alpha.txt:1");

    t.ctrl_shift(egui::Key::O);
    assert!(t.app().search_overlay.open);
    t.type_text("compute_total");
    t.key(egui::Key::Enter);
    t.assert_status("Opened Symbol: compute_total");
    assert_eq!(t.app().left.path(), Some(t.path("src/lib.rs").as_path()));

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
}

#[test]
fn search_everywhere_selection_follows_shrinking_results() {
    let mut t = search_workspace();
    t.ctrl(egui::Key::P);
    for _ in 0..3 {
        t.key(egui::Key::ArrowDown);
    }
    assert_eq!(t.app().search_overlay.selected_index, 3);
    // Files disappear while the overlay is open: the selection is clamped.
    std::fs::remove_file(t.path("alpha.txt")).unwrap();
    std::fs::remove_file(t.path("beta.txt")).unwrap();
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("files removed", |app| app.file_tree.files.len() == 1);
    t.steps(2);
    assert!(t.app().search_overlay.selected_index <= 1);
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
    assert!(t.app().left.path().is_some());
}

#[test]
fn double_shift_opens_search_everywhere() {
    let mut t = search_workspace();
    t.press_shift_twice();
    assert!(t.app().search_overlay.open);
}

#[test]
fn shift_taps_with_typing_in_between_do_not_open_search_everywhere() {
    let mut t = search_workspace();
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.step();
    t.type_text("x");
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.step();
    assert!(!t.app().search_overlay.open);
    // A second lone tap right after still opens it.
    t.set_modifiers(egui::Modifiers::SHIFT);
    t.step();
    t.set_modifiers(egui::Modifiers::NONE);
    t.step();
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
    assert_eq!(t.app().left.path(), Some(t.path("beta.txt").as_path()));
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
    assert!(!t.app().workspace_search.open);
    assert!(t.app().workspace_search.query.is_empty());
}

#[test]
fn workspace_search_shows_progress_while_running() {
    let mut builder = TestApp::builder();
    for index in 0..400 {
        builder = builder.file(
            &format!("bulk/file{index}.txt"),
            &"filler line\n".repeat(200),
        );
    }
    let mut t = builder.build();
    t.ctrl_shift(egui::Key::F);
    t.type_text("needle");
    t.harness.event(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    t.step();
    t.step();
    assert!(t.has_label("Searching..."));
    assert!(t.has_label_containing("Searching 400 files for `needle`"));
    t.wait_until("search finished", |app| !app.workspace_search.searching);
    assert!(t.has_label("No results"));
}

#[test]
fn sidebar_bookmarks_open_and_remove() {
    let mut t = search_workspace();
    assert!(t.has_label("No bookmarks"));
    t.click_first("beta.txt");
    t.steps(25);
    let star = t.gutter_star_pos(2, 4);
    t.click_at(star);
    t.click_first("alpha.txt");
    t.steps(25);
    let star = t.gutter_star_pos(0, 3);
    t.click_at(star);
    assert!(t.has_label("beta.txt:3"));
    assert!(t.has_label("alpha.txt:1"));
    assert!(t.saved_state().contains("beta.txt:3"));
    assert!(t.saved_state().contains("alpha.txt:1"));

    t.click("beta.txt:3");
    t.assert_status("Opened bookmark: beta.txt:3");
    assert_eq!(t.app().left.path(), Some(t.path("beta.txt").as_path()));

    // The newest bookmark is listed first.
    t.click_nth("×", 0);
    t.assert_status("Removed bookmark: alpha.txt:1");
    t.click_nth("×", 0);
    t.assert_status("Removed bookmark: beta.txt:3");
    assert!(!t.saved_state().contains("beta.txt:3"));
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

#[test]
fn workspace_search_box_handles_enter_in_the_frame_it_gets_focus() {
    let mut t = search_workspace();
    t.ctrl_shift(egui::Key::F);
    t.type_text("needle");
    t.click_at(egui::pos2(1000.0, 500.0));
    t.keys_in_one_frame(&[
        (egui::Key::F, super::support::CTRL | egui::Modifiers::SHIFT),
        (egui::Key::Enter, egui::Modifiers::NONE),
    ]);
    t.assert_status("Workspace search started: needle");
}

#[test]
fn search_everywhere_lists_rust_items_of_every_kind() {
    let source = "mod direct;\nmod nested;\nmod missing;\nmod inline { pub fn inner() {} }\n\
        pub enum Color { Red }\npub trait Shape { fn area(&self) -> f32; }\n\
        pub type Alias = u32;\npub union Bits { a: u32, b: f32 }\n\
        struct Point { x: i32 } impl Point { fn norm(&self) -> i32 { self.x } }\n\
        struct A; struct B;\n";
    let mut t = TestApp::builder()
        .file("src/lib.rs", source)
        .file("src/direct.rs", "pub fn d() {}\n")
        .file("src/nested/mod.rs", "pub fn n() {}\n")
        .file("README", "no extension but a known name\n")
        .file("notes", "no extension: not shown\n")
        .build();
    assert!(!t.has_label("notes"));
    t.ctrl(egui::Key::P);
    for (query, label) in [
        ("Color", "Color"),
        ("Shape", "Shape"),
        ("area", "Shape::area"),
        ("Alias", "Alias"),
        ("Bits", "Bits"),
        ("norm", "Point::norm"),
        ("direct", "direct"),
        ("nested", "nested"),
    ] {
        t.ctrl(egui::Key::A);
        t.type_text(query);
        let results = t.app_mut().search_overlay_results(&[]);
        assert!(
            results.iter().any(|result| result.label == label),
            "{query}: {:?}",
            results
                .iter()
                .map(|result| result.label.clone())
                .collect::<Vec<_>>()
        );
    }
    // Jump to a module declared in its own file.
    t.ctrl(egui::Key::A);
    t.type_text("direct");
    t.key(egui::Key::Enter);
    t.assert_status_containing("Opened");
}

#[test]
fn deleted_recent_files_drop_out_of_search_everywhere() {
    let mut t = search_workspace();
    t.click_first("alpha.txt");
    std::fs::remove_file(t.path("alpha.txt")).unwrap();
    t.ctrl(egui::Key::P);
    t.type_text("alpha");
    let results = t.app_mut().search_overlay_results(&[]);
    assert!(
        results
            .iter()
            .all(|result| result.kind != crate::features::search::SearchResultKind::History)
    );
}
