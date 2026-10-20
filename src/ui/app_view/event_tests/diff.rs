//! Git diff view: unified and side-by-side layouts, file collapsing, hidden-line markers,
//! text selection and copy, scrolling, and the history graph with branches.

use eframe::egui;

use super::super::DiffDisplayMode;
use super::support::TestApp;

/// Diff rows are painted; row 0 starts just below the "Unified" / "Side by Side" toggles.
fn row_pos(t: &TestApp, row: usize, x: f32) -> egui::Pos2 {
    let unified = t.label_rect("Unified");
    let row_height = t.editor_metrics().0.ceil();
    egui::pos2(
        unified.left() + x,
        unified.bottom() + 4.0 + row_height * (row as f32 + 0.5),
    )
}

fn numbered(count: usize, changed: &[usize]) -> String {
    (1..=count)
        .map(|n| {
            if changed.contains(&n) {
                format!("CHANGED line {n} {}\n", "with a very long tail ".repeat(12))
            } else {
                format!("line {n}\n")
            }
        })
        .collect()
}

/// Like `numbered` but lines 5 and 50 are already long, so both diff columns overflow.
fn numbered_with_long_lines() -> String {
    numbered(60, &[5, 50]).replace("CHANGED line", "original line")
}

fn diff_app() -> TestApp {
    let mut t = TestApp::builder()
        .file("long.txt", &numbered_with_long_lines())
        .file("gone.txt", "bye\n")
        .file("keep.txt", "keep\n")
        .git()
        .build();
    // Two distant edits give two hunks with hidden lines between them.
    t.write("long.txt", &numbered(60, &[5, 50]));
    std::fs::remove_file(t.path("gone.txt")).unwrap();
    t.write("added.txt", "fresh\n");
    t.git(&["add", "added.txt", "gone.txt"]);
    t.click("Git");
    t.steps(2);
    t
}

#[test]
fn all_diffs_collapse_and_expand_files() {
    let mut t = diff_app();
    t.click("All Diffs");
    let text = t.app().left.text.as_str().to_owned();
    assert!(text.contains("new file mode"));
    assert!(text.contains("deleted file mode"));

    // Row 0 is the "Staged Changes" section, row 1 the first file header.
    let header = row_pos(&t, 1, 200.0);
    t.click_at(header);
    assert_eq!(
        t.app()
            .left
            .diff_collapsed_files
            .values()
            .filter(|c| **c)
            .count(),
        1
    );
    t.steps(20);
    t.click_at(header);
    assert_eq!(
        t.app()
            .left
            .diff_collapsed_files
            .values()
            .filter(|c| **c)
            .count(),
        0
    );

    // Collapse in side-by-side mode too.
    t.click("Side by Side");
    t.steps(2);
    t.steps(20);
    t.click_at(row_pos(&t, 1, 200.0));
    assert_eq!(
        t.app()
            .left
            .diff_collapsed_files
            .values()
            .filter(|c| **c)
            .count(),
        1
    );
    t.steps(3);
}

#[test]
fn unified_selection_copies_text() {
    let mut t = diff_app();
    // Open the worktree diff of long.txt from the Changed Files list.
    let plus = t.nth_label_rect("+", 0);
    t.click_at(egui::pos2(plus.left() - 120.0, plus.center().y));
    assert!(t.has_label("long.txt: Diff"));

    // Row 0: file header, then context and changed lines of the first hunk.
    let from = row_pos(&t, 2, 120.0);
    let to = row_pos(&t, 4, 400.0);
    t.drag(from, to);
    assert!(t.app().left.has_diff_text_selection());
    t.copy_event();
    t.assert_status_containing("Copied diff selection:");
    let copied = t.platform.clipboard_text().unwrap();
    assert!(copied.contains('\n'));

    t.platform.clipboard_fails.set(true);
    t.copy_event();
    t.assert_status(
        "Diff selection copy requested, but system clipboard failed: clipboard unavailable",
    );

    // Clicking outside the text column (the line-number gutter) does not start a selection.
    t.click_at(row_pos(&t, 3, 10.0));
}

#[test]
fn side_by_side_selection_on_each_side_and_horizontal_scroll() {
    let mut t = diff_app();
    let plus = t.nth_label_rect("+", 0);
    t.click_at(egui::pos2(plus.left() - 120.0, plus.center().y));
    t.click("Side by Side");
    t.steps(2);
    assert_eq!(t.app().left.diff_display_mode, DiffDisplayMode::SideBySide);

    let unified = t.label_rect("Unified");
    let editor_right = 1480.0;
    let center = (unified.left() + editor_right) * 0.5;

    // Old (left) side.
    t.drag(row_pos(&t, 3, 90.0), row_pos(&t, 5, 200.0));
    let selection = t.app().left.diff_side_selection.unwrap();
    assert_eq!(selection.side, super::super::DiffSide::Old);
    t.copy_event();
    t.assert_status_containing("Copied diff selection:");

    // New (right) side.
    let right_x = center - unified.left() + 90.0;
    t.steps(20);
    t.drag(row_pos(&t, 3, right_x), row_pos(&t, 6, right_x + 120.0));
    let selection = t.app().left.diff_side_selection.unwrap();
    assert_eq!(selection.side, super::super::DiffSide::New);
    t.copy_event();

    // The horizontal scrollbars sit under the two columns at the bottom of the view and
    // keep each other in sync.
    let y = t.harness.ctx.content_rect().bottom() - 48.0;
    let start = egui::pos2(center + 40.0, y);
    t.steps(20);
    t.drag(start, start + egui::vec2(300.0, 0.0));
    assert!(t.app().left.diff_side_right_scroll_x > 0.0);
    assert!(t.app().left.diff_side_left_scroll_x > 0.0);
    t.steps(20);
    t.click_at(egui::pos2(unified.left() + 40.0, y));
    assert_eq!(t.app().left.diff_side_left_scroll_x, 0.0);
    assert_eq!(t.app().left.diff_side_right_scroll_x, 0.0);

    t.click("Unified");
    assert!(t.app().left.diff_side_selection.is_none());
}

#[test]
fn diff_view_scrolls_with_keys_and_wheel() {
    let mut t = TestApp::builder()
        .file("big.txt", &numbered(400, &[]))
        .git()
        .build();
    let changed = (1..=400)
        .map(|n| format!("changed {n}\n"))
        .collect::<String>();
    t.write("big.txt", &changed);
    t.click("Git");
    t.steps(2);
    let plus = t.nth_label_rect("+", 0);
    t.click_at(egui::pos2(plus.left() - 120.0, plus.center().y));
    t.click_at(row_pos(&t, 3, 10.0));
    t.key(egui::Key::PageDown);
    assert!(t.app().left.scroll_y > 0.0);
    t.key(egui::Key::PageUp);
    let pos = row_pos(&t, 5, 300.0);
    t.scroll(pos, egui::vec2(0.0, -500.0));
    assert!(t.app().left.scroll_y > 0.0);
    t.click("Full File");
}

#[test]
fn reload_diff_context_reports_git_errors() {
    let mut t = diff_app();
    let plus = t.nth_label_rect("+", 0);
    t.click_at(egui::pos2(plus.left() - 120.0, plus.center().y));
    // The repository disappears before the context is switched.
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.click("Full File");
    t.assert_status_containing("Reload diff failed: long.txt |");
}

#[test]
fn history_graph_draws_branches_and_merges() {
    let mut t = TestApp::builder().file("a.txt", "a\n").git().build();
    t.git(&["switch", "-q", "-c", "feature"]);
    t.write("b.txt", "b\n");
    t.git(&["add", "b.txt"]);
    t.git(&["commit", "-qm", "feature work"]);
    t.git(&["switch", "-q", "main"]);
    t.write("c.txt", "c\n");
    t.git(&["add", "c.txt"]);
    t.git(&["commit", "-qm", "main work"]);
    t.git(&["merge", "-q", "--no-ff", "feature", "-m", "merge feature"]);
    t.git(&["tag", "v1"]);
    t.click("Git");
    t.steps(2);
    t.click("History");
    assert!(t.has_label("4 commits"));
    t.steps(3);
    // Scroll the graph with the keyboard and wheel.
    let header = t.label_rect("4 commits");
    t.click_at(egui::pos2(header.left() + 300.0, header.bottom() + 52.0));
    t.key(egui::Key::Escape);
    t.key(egui::Key::PageDown);
    t.scroll(
        egui::pos2(header.left() + 300.0, header.bottom() + 80.0),
        egui::vec2(0.0, -50.0),
    );
}

#[test]
fn selection_ignores_non_text_rows_and_spans_empty_lines() {
    let mut t = TestApp::builder()
        .file("a.txt", "one\n\nthree\nfour\n")
        .file("b.txt", "bee\n")
        .git()
        .build();
    t.write("a.txt", "ONE\n\nthree\nFOUR\n");
    t.write("b.txt", "BEE\n");
    t.click("Git");
    t.steps(2);
    t.click("All Diffs");
    // Row 0 is the section title, row 1 the first file header: neither starts a selection.
    t.click_at(row_pos(&t, 0, 200.0));
    assert!(!t.app().left.has_diff_text_selection());
    // A drag that ends on the next file's header selects up to the rows in between,
    // including the empty context line.
    t.steps(20);
    let from = row_pos(&t, 2, 120.0);
    let to = row_pos(&t, 9, 120.0);
    t.drag(from, to);
    t.copy_event();
    let copied = t.platform.clipboard_text().unwrap_or_default();
    assert!(
        copied.contains("\n\n") || copied.contains("three"),
        "{copied:?}"
    );
    // Pressing below the last row does nothing.
    t.steps(20);
    t.click_at(egui::pos2(row_pos(&t, 0, 200.0).x, 700.0));

    t.click("Side by Side");
    t.steps(2);
    t.click_at(row_pos(&t, 1, 200.0));
    t.click_at(row_pos(&t, 3, 10.0));
    t.steps(20);
    let unified = t.label_rect("Unified");
    let center = (unified.left() + 1480.0) * 0.5;
    t.click_at(egui::pos2(center, row_pos(&t, 3, 0.0).y));
    t.steps(20);
    t.drag(row_pos(&t, 2, 90.0), row_pos(&t, 9, 90.0));
    t.copy_event();
}

#[test]
fn collapsed_files_hide_their_hidden_line_markers_and_messages() {
    let mut t = diff_app();
    let mut png = vec![0x89, b'P', b'N', b'G', 0, 0, 0, 0];
    png.extend(std::iter::repeat_n(0u8, 64));
    std::fs::write(t.path("blob.bin.txt"), &png).unwrap();
    t.git(&["add", "blob.bin.txt"]);
    t.write("no-newline.txt", "x");
    t.git(&["add", "no-newline.txt"]);
    t.git(&["commit", "-qm", "base"]);
    std::fs::write(t.path("blob.bin.txt"), [png.as_slice(), &[1u8]].concat()).unwrap();
    t.write("no-newline.txt", "y");
    t.click("Git");
    t.steps(2);
    t.click("All Diffs");
    assert!(
        t.app()
            .left
            .text
            .as_str()
            .contains("No newline at end of file")
    );
    // Collapse every file header in turn.
    for row in 0..12 {
        t.steps(20);
        t.click_at(row_pos(&t, row, 200.0));
    }
    t.click("Side by Side");
    t.steps(2);
}

#[test]
fn light_theme_diff_and_one_sided_scrolling() {
    let mut t = TestApp::builder()
        .file("short.txt", "a\n")
        .git()
        .light()
        .build();
    t.write("short.txt", &format!("{}\n", "b".repeat(400)));
    t.click("Git");
    t.steps(2);
    let plus = t.nth_label_rect("+", 0);
    t.click_at(egui::pos2(plus.left() - 120.0, plus.center().y));
    t.click("Side by Side");
    t.steps(2);
    let y = t.harness.ctx.content_rect().bottom() - 48.0;
    let unified = t.label_rect("Unified");
    let center = (unified.left() + 1480.0) * 0.5;
    t.steps(20);
    t.drag(egui::pos2(center + 40.0, y), egui::pos2(center + 200.0, y));
    assert!(t.app().left.diff_side_right_scroll_x > 0.0);
    // The old column is short: its scrollbar stays inactive.
    t.steps(20);
    t.click_at(egui::pos2(unified.left() + 40.0, y));
    assert_eq!(t.app().left.diff_side_left_scroll_x, 0.0);
}

#[test]
fn history_rows_with_many_long_refs_are_elided() {
    let mut t = TestApp::builder().file("a.txt", "a\n").git().build();
    for name in [
        "feature/a-really-long-branch-name-one",
        "feature/a-really-long-branch-name-two",
        "feature/a-really-long-branch-name-three",
        "feature/a-really-long-branch-name-four",
    ] {
        t.git(&["branch", name]);
    }
    t.click("Git");
    t.steps(2);
    t.click("History");
    t.steps(3);
    assert!(t.has_label("1 commits"));
}
