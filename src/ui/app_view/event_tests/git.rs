//! Git sidebar: repository status, stage / unstage / discard, commit, push / publish,
//! branches, history, commit details and diff views. Runs real `git` on temp repositories.

use eframe::egui;

use super::super::{DiffDisplayMode, GitDiffContextMode};
use super::support::{TestApp, run_git};

fn repo() -> TestApp {
    TestApp::builder()
        .file("tracked.txt", "one\ntwo\nthree\n")
        .file("other.txt", "other\n")
        .git()
        .build()
}

fn open_git(t: &mut TestApp) {
    t.click("Git");
    t.steps(2);
}

/// Adds a bare repository as `origin`, pushes `main` and sets it as upstream.
fn add_origin(t: &TestApp, push: bool) -> std::path::PathBuf {
    let remote = t.tmp.join("remote.git");
    run_git(
        &t.tmp,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    t.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    if push {
        t.git(&["push", "-q", "-u", "origin", "main"]);
    }
    remote
}

/// Center of the painted path cell in the `index`-th change row that has a "+" button.
fn changed_row_path(t: &TestApp, index: usize) -> egui::Pos2 {
    let plus = t.nth_label_rect("+", index);
    egui::pos2(plus.left() - 120.0, plus.center().y)
}

#[test]
fn git_view_lists_repository_and_changes() {
    let mut t = repo();
    open_git(&mut t);
    t.assert_status("Git: 1 repositories.");
    assert!(t.has_label("workspace"));
    assert!(t.has_label("main ▾"));
    assert!(t.has_label("workspace (0)"));
    assert!(t.has_label("No changes"));

    t.write("tracked.txt", "one\nTWO\nthree\n");
    t.write("new.txt", "brand new\n");
    open_git(&mut t);
    assert!(t.has_label("workspace (2)"));
    assert!(t.has_label("Changed Files"));
    assert_eq!(t.count_label("+"), 2);

    // The workspace tree marks changed files once the new file is discovered.
    t.click("Workspace");
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("new.txt discovered", |app| {
        app.file_tree
            .files
            .iter()
            .any(|path| path.ends_with("new.txt"))
    });
    t.steps(2);
    assert!(t.has_label("M"));
    assert!(t.has_label("?"));
}

#[test]
fn stage_unstage_and_stage_all() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    t.write("new.txt", "new\n");
    open_git(&mut t);

    t.click_nth("+", 0);
    t.assert_status_containing("Git stage completed: workspace");
    assert!(t.has_label("Staged Changes"));
    assert_eq!(t.count_label("-"), 1);

    t.click("-");
    t.assert_status_containing("Git unstage completed: workspace");
    assert!(!t.has_label("Staged Changes"));

    t.click("+ Stage All");
    t.assert_status_containing("Git stage all completed: workspace");
    assert!(t.has_label("No unstaged changes"));

    t.click("- Unstage All");
    t.assert_status_containing("Git unstage all completed: workspace");
    assert!(!t.has_label("Staged Changes"));
}

#[test]
fn discard_restores_tracked_files_and_removes_untracked_ones() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);

    t.click("↶");
    assert!(t.has_label("This will discard local changes for this file."));
    t.click("Cancel");
    assert!(t.app().pending_git_discard.is_none());

    t.click("↶");
    t.key(egui::Key::Escape);
    assert!(t.app().pending_git_discard.is_none());

    // Enter confirms, as Help and the status bar say.
    t.click("↶");
    t.key(egui::Key::Enter);
    assert!(t.app().pending_git_discard.is_none());
    t.assert_status_containing("Git discard completed: workspace");
    assert_eq!(t.read("tracked.txt"), "one\ntwo\nthree\n");

    t.write("scratch.txt", "scratch\n");
    open_git(&mut t);
    t.click("↶");
    t.click("Discard");
    assert!(!t.path("scratch.txt").exists());
}

#[test]
fn discard_reports_git_errors() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    t.click("↶");
    // The file disappears before the user confirms.
    std::fs::remove_file(t.path("tracked.txt")).unwrap();
    t.git(&["rm", "-q", "--cached", "tracked.txt"]);
    t.click("Discard");
    t.assert_status_containing("Git discard failed: workspace | tracked.txt");
}

#[test]
fn commit_requires_a_message_and_clears_it_on_success() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    t.click("Commit");
    t.assert_status("Git commit failed: workspace | message is empty");

    t.click("+ Stage All");
    // The sidebar is laid out first, so the commit message is text field #0.
    t.fill_text_field(0, "Change tracked");
    t.click("Commit");
    t.assert_status_containing("Git commit completed: workspace");
    assert_eq!(t.git(&["log", "-1", "--format=%s"]), "Change tracked");
    assert!(t.app().git_commit_messages.values().all(String::is_empty));
}

#[test]
fn commit_with_nothing_staged_reports_git_error() {
    let mut t = repo();
    open_git(&mut t);
    // The sidebar is laid out first, so the commit message is text field #0.
    t.fill_text_field(0, "Nothing");
    t.click("Commit");
    t.assert_status_containing("Git commit failed: workspace | ");
}

#[test]
fn commit_and_push_without_upstream_publishes_the_branch() {
    let mut t = repo();
    let remote = add_origin(&t, false);
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    t.click("+ Stage All");
    // The sidebar is laid out first, so the commit message is text field #0.
    t.fill_text_field(0, "Push me");
    t.click("Commit & Push");
    t.assert_status_containing("Git commit & push completed: workspace");
    assert_eq!(
        run_git(&remote, &["log", "-1", "--format=%s", "main"]),
        "Push me"
    );
    assert_eq!(
        t.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main",
        "the branch now tracks the remote, like Publish Branch"
    );
}

#[test]
fn commit_and_push_with_upstream_pushes() {
    let mut t = repo();
    let remote = add_origin(&t, true);
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    t.click("+ Stage All");
    // The sidebar is laid out first, so the commit message is text field #0.
    t.fill_text_field(0, "Pushed");
    t.click("Commit & Push");
    t.assert_status_containing("Git commit & push completed: workspace");
    assert_eq!(
        run_git(&remote, &["log", "-1", "--format=%s", "main"]),
        "Pushed"
    );
}

#[test]
fn ahead_and_behind_counts_are_shown() {
    let mut t = repo();
    add_origin(&t, true);
    t.write("tracked.txt", "ahead\n");
    t.git(&["commit", "-qam", "ahead"]);
    open_git(&mut t);
    assert!(t.has_label("0↓ 1↑"));
}

#[test]
fn menu_fetch_push_and_publish() {
    let mut t = repo();
    open_git(&mut t);

    // Without a remote: fetch succeeds silently, publish has nowhere to go.
    t.click("...");
    t.click("Fetch");
    t.assert_status("Git fetch completed: workspace | Fetch completed.");
    t.click("...");
    t.click("Publish Branch");
    t.assert_status("Git publish failed: workspace | no Git remote is configured");

    add_origin(&t, false);
    t.click("...");
    t.click("Publish Branch");
    t.assert_status_containing("Git publish completed: workspace");
    assert!(t.has_label("main ▾"));

    t.write("tracked.txt", "more\n");
    t.git(&["commit", "-qam", "more"]);
    open_git(&mut t);
    t.click("...");
    t.click("Push");
    t.assert_status_containing("Git push completed: workspace");

    t.click("...");
    t.click("Fetch");
    t.assert_status_containing("Git fetch completed: workspace");
}

#[test]
fn push_failure_is_reported() {
    let mut t = repo();
    let remote = add_origin(&t, true);
    std::fs::remove_dir_all(&remote).unwrap();
    open_git(&mut t);
    t.click("...");
    t.click("Push");
    t.assert_status_containing("Git push failed: workspace");
    t.click("...");
    t.click("Fetch");
    t.assert_status_containing("Git fetch failed: workspace");
}

#[test]
fn pull_reports_success_and_failure() {
    let mut t = repo();
    open_git(&mut t);
    t.click("Pull");
    t.assert_status_containing("Git pull failed: workspace");

    add_origin(&t, true);
    open_git(&mut t);
    t.click("Pull");
    t.assert_status_containing("Git pull completed: workspace");
}

#[test]
fn detached_head_cannot_be_published() {
    let mut t = repo();
    let head = t.git(&["rev-parse", "HEAD"]);
    t.git(&["checkout", "-q", "--detach", &head]);
    open_git(&mut t);
    let short = t.git(&["rev-parse", "--short", "HEAD"]);
    assert!(t.has_label(&format!("{short} ▾")));
    // The menu disables "Publish Branch" for a detached HEAD.
    t.click("...");
    t.app_mut().status_message = "marker".to_owned();
    t.click("Publish Branch");
    assert_eq!(t.status(), "marker");
}

#[test]
fn create_branch_window() {
    let mut t = repo();
    open_git(&mut t);
    t.click("...");
    t.click("Create Branch...");
    assert!(t.has_label("Branch name"));
    assert!(t.has_label("from: main"));
    t.key(egui::Key::Enter);
    assert!(
        t.app().git_create_branch_root.is_some(),
        "empty name is ignored"
    );
    t.type_text("feature/one");
    t.key(egui::Key::Enter);
    t.assert_status_containing("Git branch created: workspace");
    assert!(t.has_label("feature/one ▾"));

    t.click("...");
    t.click("Create Branch...");
    t.type_text("feature/two");
    t.click("Create");
    assert!(t.has_label("feature/two ▾"));

    t.click("...");
    t.click("Create Branch...");
    t.type_text("feature/two");
    t.click("Create");
    t.assert_status_containing("Git branch create failed: workspace");

    t.click("...");
    t.click("Create Branch...");
    t.key(egui::Key::Escape);
    assert!(t.app().git_create_branch_root.is_none());
}

#[test]
fn switch_branch_window() {
    let mut t = repo();
    t.git(&["branch", "develop"]);
    open_git(&mut t);
    t.click("main ▾");
    assert!(t.has_label("Switch Branch"));
    assert!(t.has_label("2 branches"));
    assert!(t.has_label("current: main"));
    t.key(egui::Key::Escape);
    assert!(t.app().git_switch_branch_root.is_none());

    t.click("...");
    t.click("Switch Branch...");
    // Rows are painted: "develop" is first (sorted), "main" second.
    let header = t.label_rect("current: main");
    let develop_row = egui::pos2(header.left() - 100.0, header.bottom() + 24.0);
    t.click_at(develop_row);
    t.assert_status_containing("Git branch switched: workspace");
    assert!(t.has_label("develop ▾"));

    // Keyboard: the highlight starts on the current branch; Enter on it just closes.
    t.click("develop ▾");
    t.key(egui::Key::Enter);
    assert!(t.app().git_switch_branch_root.is_none());
    assert!(t.has_label("develop ▾"));
    t.click("develop ▾");
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowDown);
    assert_eq!(t.app().git_switch_branch_cursor.as_deref(), Some("main"));
    t.key(egui::Key::ArrowUp);
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::Enter);
    assert!(t.has_label("main ▾"));
    assert!(t.app().git_switch_branch_cursor.is_none());
    t.git(&["switch", "-q", "develop"]);

    // Switching with uncommitted conflicting changes fails.
    t.git(&["switch", "-q", "main"]);
    t.write("tracked.txt", "main side\n");
    t.git(&["commit", "-qam", "main side"]);
    t.git(&["switch", "-q", "develop"]);
    t.write("tracked.txt", "dirty\n");
    open_git(&mut t);
    t.click("develop ▾");
    // "develop" is current (first row); "main" is the second row.
    let header = t.label_rect("current: develop");
    t.click_at(egui::pos2(
        header.left() - 100.0,
        header.bottom() + 24.0 + 24.0,
    ));
    t.assert_status_containing("Git branch switch failed: workspace");
}

#[test]
fn branch_windows_close_when_repository_disappears() {
    let mut t = repo();
    open_git(&mut t);
    t.click("main ▾");
    assert!(t.app().git_switch_branch_root.is_some());
    // The repository goes away; the next workspace scan reloads Git and closes the window.
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.write("trigger.txt", "x");
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("repository gone", |app| app.git_repositories.is_empty());
    assert!(t.app().git_switch_branch_root.is_none());
    t.steps(2);
    assert!(!t.has_label("Switch Branch"));
}

#[test]
fn create_branch_window_closes_when_repository_disappears() {
    let mut t = repo();
    open_git(&mut t);
    t.click("...");
    t.click("Create Branch...");
    assert!(t.app().git_create_branch_root.is_some());
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.write("trigger.txt", "x");
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("repository gone", |app| app.git_repositories.is_empty());
    assert!(t.app().git_create_branch_root.is_none());
}

#[test]
fn file_diff_views_switch_layout_and_context() {
    let mut t = TestApp::builder()
        .file(
            "tracked.txt",
            &(1..=30).map(|n| format!("line {n}\n")).collect::<String>(),
        )
        .git()
        .build();
    let changed = (1..=30)
        .map(|n| {
            if n == 15 {
                "line fifteen\n".to_owned()
            } else {
                format!("line {n}\n")
            }
        })
        .collect::<String>();
    t.write("tracked.txt", &changed);
    open_git(&mut t);

    let path_cell = changed_row_path(&t, 0);
    t.click_at(path_cell);
    t.assert_status("Opened diff: workspace | tracked.txt");
    assert!(t.app().left.diff_view.is_some());
    assert!(t.has_label("tracked.txt: Diff"));
    assert!(t.has_label("Worktree"));

    // "Changes" is also the sidebar section header, so the toggle is the second one.
    t.click("Full File");
    t.assert_status("Diff context: Full File | tracked.txt");
    assert_eq!(
        t.app().left.diff_view.as_ref().unwrap().context_mode,
        GitDiffContextMode::FullFile
    );
    t.click_nth("Changes", 1);
    t.assert_status("Diff context: Changes | tracked.txt");

    t.click("Side by Side");
    assert_eq!(t.app().left.diff.display_mode, DiffDisplayMode::SideBySide);
    t.steps(2);
    t.click("Unified");
    assert_eq!(t.app().left.diff.display_mode, DiffDisplayMode::Unified);

    // Staged diffs open from the staged section.
    t.click("+ Stage All");
    let minus = t.label_rect("-");
    t.click_at(egui::pos2(minus.left() - 120.0, minus.center().y));
    t.assert_status("Opened diff: workspace | tracked.txt");
    assert!(t.has_label("tracked.txt: Staged Diff"));
    assert!(t.has_label("Staged"));
    t.click("Full File");
    t.assert_status("Diff context: Full File | tracked.txt");
}

#[test]
fn untracked_file_diff_and_all_diffs() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    t.write("new.txt", "brand\nnew\n");
    t.write("empty.txt", "");
    t.git(&["add", "tracked.txt"]);
    t.write("other.txt", "other changed\n");
    open_git(&mut t);

    t.click("All Diffs");
    t.assert_status("Opened all diffs: workspace");
    assert!(t.has_label("workspace: All Diffs"));
    let text = t.app().left.text.as_str().to_owned();
    assert!(text.contains("diff --code-reader Staged Changes"));
    assert!(text.contains("diff --code-reader Worktree Changes"));
    assert!(text.contains("diff --code-reader Untracked"));
    assert!(text.contains("+brand"));

    // Untracked file diff is synthesized from the file content.
    let rows = t.count_label("+");
    assert_eq!(rows, 3);
    for index in 0..rows {
        let cell = changed_row_path(&t, index);
        t.click_at(cell);
    }
    assert!(
        t.app().left.diff_view.as_ref().unwrap().untracked
            || t.saw_status_containing("Opened diff")
    );
}

#[test]
fn all_diffs_and_history_report_a_vanished_repository() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    // Let the periodic workspace scan pick up the edit now, then keep it from reloading
    // Git while the test runs, so the stale repository entry stays on screen.
    let before = t.app().workspace_snapshot.clone();
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    // Wait on the snapshot: a status message can lose to "Indexed ..." in the same frame.
    t.wait_until("the scan to pick up the edit", |app| {
        app.workspace_snapshot != before
    });
    t.app_mut().next_workspace_change_poll =
        std::time::Instant::now() + std::time::Duration::from_secs(3600);
    // The sidebar still shows the repository when it disappears.
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.click("All Diffs");
    t.assert_status_containing("Open all diffs failed: workspace");
    t.click("History");
    t.assert_status_containing("Open Git graph failed: workspace");
}

#[test]
fn history_commit_details_and_commit_diff() {
    let mut t = repo();
    t.write("tracked.txt", "second\n");
    t.git(&["mv", "other.txt", "moved.txt"]);
    t.git(&["commit", "-qam", "Second commit"]);
    t.git(&["tag", "v1"]);
    open_git(&mut t);

    t.click("History");
    t.assert_status("Opened Git graph: workspace");
    assert!(t.has_label("workspace: Git Graph"));
    assert!(t.has_label("2 commits"));
    assert!(t.app().left.git_history_view.is_some());

    // Click the newest commit row (first row below the header).
    let header = t.label_rect("2 commits");
    let first_row = egui::pos2(header.left() + 300.0, header.bottom() + 52.0);
    t.click_at(first_row);
    t.assert_status_containing("Opened commit details:");
    assert!(t.has_label("Commit Details"));
    assert!(t.has_label("Second commit"));
    assert!(t.has_label("Test User"));
    assert!(
        t.has_label_containing("R100 other.txt -> moved.txt")
            || t.has_label_containing("moved.txt")
    );

    t.click("Open Diff");
    t.assert_status_containing("Opened commit diff:");
    assert!(t.app().git_commit_details.is_none());
    assert!(t.has_label("Commit"));
    assert!(t.has_label("Back to History"));

    t.click("Back to History");
    assert!(t.app().left.git_history_view.is_some());

    // Root commit details: no parents.
    let header = t.label_rect("2 commits");
    let second_row = egui::pos2(header.left() + 300.0, header.bottom() + 52.0 + 24.0);
    t.click_at(second_row);
    assert!(t.has_label("none"));
    t.click("Close");
    assert!(t.app().git_commit_details.is_none());

    t.click_at(second_row);
    t.key(egui::Key::Escape);
    assert!(t.app().git_commit_details.is_none());

    // The repository disappears while the history is on screen: the details window shows
    // the error instead of the changed files.
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.click_at(second_row);
    assert!(t.has_label("Commit Details"));
    assert!(t.app().git_commit_details.as_ref().unwrap().error.is_some());
}

#[test]
fn commit_details_show_refs_and_report_diff_errors() {
    let mut t = repo();
    add_origin(&t, true);
    t.git(&["tag", "v1"]);
    open_git(&mut t);
    t.click("History");
    let header = t.label_rect("1 commits");
    t.click_at(egui::pos2(header.left() + 300.0, header.bottom() + 52.0));
    assert!(t.has_label("Refs"));
    assert!(t.has_label("origin/main"));
    assert!(t.has_label("tag: v1"));
    std::fs::remove_dir_all(t.path(".git")).unwrap();
    t.click("Open Diff");
    t.assert_status_containing("Open commit diff failed:");
}

#[test]
fn repository_without_commits_has_an_empty_history() {
    let mut t = TestApp::builder().file("a.txt", "a").build();
    run_git(&t.root, &["init", "-q", "-b", "main"]);
    open_git(&mut t);
    assert!(t.has_label("main ▾"));
    t.click("History");
    t.assert_status("Opened Git graph: workspace");
    assert!(t.has_label("0 commits"));
}

#[test]
fn files_with_spaces_or_japanese_names_can_be_staged_and_diffed() {
    let mut t = repo();
    t.write("my notes.txt", "space\n");
    t.write("日本語.txt", "japanese\n");
    open_git(&mut t);
    let paths = t.app().git_repositories[0]
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["my notes.txt", "日本語.txt"]);
    t.click_nth("+", 0);
    t.assert_status_containing("Staged my notes.txt.");
    let cell = changed_row_path(&t, 0);
    t.click_at(cell);
    t.assert_status("Opened diff: workspace | 日本語.txt");
    // The tree marks both files once they are discovered.
    t.click("Workspace");
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("new files discovered", |app| {
        app.file_tree
            .files
            .iter()
            .any(|path| path.ends_with("日本語.txt"))
    });
    t.steps(2);
    assert!(t.has_label("?"));
    assert!(t.has_label("A"));
}

#[test]
fn renamed_files_use_the_new_path() {
    let mut t = repo();
    t.git(&["mv", "tracked.txt", "moved file.txt"]);
    open_git(&mut t);
    let changes = &t.app().git_repositories[0].changes;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].status, "R");
    assert_eq!(changes[0].path, "moved file.txt");
}

#[test]
fn repository_errors_and_locked_index_are_reported() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    t.git(&["add", "tracked.txt"]);
    t.write("other.txt", "other changed\n");
    open_git(&mut t);
    // Another git process holds the index lock: every index update fails.
    std::fs::write(t.path(".git/index.lock"), "").unwrap();
    t.click("+ Stage All");
    t.assert_status_containing("Git stage all failed: workspace");
    t.click("- Unstage All");
    t.assert_status_containing("Git unstage all failed: workspace");
    t.click("-");
    t.assert_status_containing("Git unstage failed: workspace | tracked.txt");
    std::fs::remove_file(t.path(".git/index.lock")).unwrap();

    // A corrupt index makes `git status` fail; the error is shown under the repository.
    std::fs::write(t.path(".git/index"), "garbage").unwrap();
    open_git(&mut t);
    assert!(t.app().git_repositories[0].error.is_some());
    assert!(t.has_label_containing("index"));
}

#[test]
fn git_warnings_are_shown_as_command_output() {
    let mut t = repo();
    t.git(&["config", "core.autocrlf", "true"]);
    t.write("tracked.txt", "lf\nline\n");
    t.write("new.txt", "lf\n");
    open_git(&mut t);
    t.click_nth("+", 0);
    t.assert_status_containing("Git stage completed: workspace | warning:");
    t.click("+ Stage All");
    t.assert_status_containing("Git stage all completed: workspace | warning:");
    t.click_nth("-", 0);
    t.click("- Unstage All");
    t.assert_status_containing("Git unstage all completed: workspace");
}

#[test]
fn fetch_reports_new_remote_commits() {
    let mut t = repo();
    let remote = add_origin(&t, true);
    let other = t.tmp.join("other-clone");
    run_git(
        &t.tmp,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    run_git(&other, &["config", "user.name", "Other"]);
    run_git(&other, &["config", "user.email", "other@example.com"]);
    std::fs::write(other.join("remote.txt"), "remote\n").unwrap();
    run_git(&other, &["add", "remote.txt"]);
    run_git(&other, &["commit", "-qm", "remote work"]);
    run_git(&other, &["push", "-q"]);
    open_git(&mut t);
    t.click("...");
    t.click("Fetch");
    t.assert_status_containing("Git fetch completed: workspace | From ");
    assert!(t.has_label("1↓ 0↑"));
}

#[test]
fn tree_markers_for_added_deleted_and_nested_changes() {
    let mut t = TestApp::builder()
        .file("doomed.txt", "x\n")
        .file("dir/inner/deep.txt", "deep\n")
        .git()
        .build();
    t.write("brand.txt", "new\n");
    t.git(&["add", "brand.txt"]);
    std::fs::remove_file(t.path("doomed.txt")).unwrap();
    t.write("dir/inner/deep.txt", "changed\n");
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("brand.txt discovered", |app| {
        app.file_tree
            .files
            .iter()
            .any(|path| path.ends_with("brand.txt"))
    });
    t.steps(3);
    assert!(t.has_label("A"), "staged new file");
    assert!(t.has_label("•"), "folder containing a change");
}

#[test]
fn history_of_a_detached_head_and_an_octopus_merge() {
    let mut t = TestApp::builder().file("a.txt", "a\n").git().build();
    for branch in ["one", "two"] {
        t.git(&["switch", "-q", "-c", branch, "main"]);
        t.write(&format!("{branch}.txt"), "x\n");
        t.git(&["add", "-A"]);
        t.git(&["commit", "-qm", &format!("{branch} work")]);
    }
    // main moves on too, so the merge has three parents (graph prefix "*-.").
    t.git(&["switch", "-q", "main"]);
    t.write("main.txt", "m\n");
    t.git(&["add", "-A"]);
    t.git(&["commit", "-qm", "main work"]);
    let long_subject = format!("octopus {}", "very long subject ".repeat(20));
    t.git(&["merge", "-q", "one", "two", "-m", &long_subject]);
    assert_eq!(
        t.git(&["rev-list", "--parents", "-n", "1", "HEAD"])
            .split(' ')
            .count(),
        4
    );
    let head = t.git(&["rev-parse", "HEAD"]);
    t.git(&["checkout", "-q", "--detach", &head]);
    open_git(&mut t);
    t.click("History");
    assert!(t.has_label("5 commits"));
    assert!(
        t.app().left.git_history_view.as_ref().unwrap().rows[0]
            .graph
            .starts_with("*-.")
    );
    t.steps(3);
}

#[test]
fn staged_diff_without_changes_and_binary_files() {
    let mut t = repo();
    t.write("tracked.txt", "changed\n");
    t.git(&["add", "tracked.txt"]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0, 0, 0, 0];
    png.extend(std::iter::repeat_n(0u8, 64));
    std::fs::write(t.path("blob.bin.txt"), &png).unwrap();
    t.git(&["add", "blob.bin.txt"]);
    open_git(&mut t);
    let minus = t.nth_label_rect("-", 0);
    t.click_at(egui::pos2(minus.left() - 120.0, minus.center().y));
    // Staged changes vanish behind the view's back: reloading shows "No diff".
    t.git(&["reset", "-q"]);
    t.click("Full File");
    let text = t.app().left.text.as_str().to_owned();
    assert!(text.starts_with("No diff for "), "{text}");
    t.click_nth("Changes", 1);
    t.click_nth("Changes", 1);

    t.git(&["add", "-A"]);
    open_git(&mut t);
    t.click("All Diffs");
    assert!(t.app().left.text.as_str().contains("Binary files"));
    t.click("Side by Side");
    t.steps(2);
}

#[test]
fn switch_branch_window_without_local_branches_and_help_in_git_views() {
    let mut t = TestApp::builder().file("a.txt", "a").build();
    run_git(&t.root, &["init", "-q", "-b", "main"]);
    open_git(&mut t);
    t.click("main ▾");
    assert!(t.has_label("No local branches"));
    t.key(egui::Key::Escape);

    let mut t = repo();
    t.write("tracked.txt", "x\n");
    open_git(&mut t);
    t.click("All Diffs");
    t.key(egui::Key::F1);
    assert!(t.has_label("Diff View"));
    t.key(egui::Key::Escape);
    t.click("History");
    t.key(egui::Key::F1);
    assert!(t.has_label("Git History"));
}

#[test]
fn git_view_reloads_after_switching_sessions() {
    let state = r#"{
        "current_session_id": "s1",
        "sessions": [
            {"id": "s1", "display_name": "One", "root_folders": ["{root}"], "bookmarks": [],
             "open_files": {"left": null, "right": null},
             "pane_positions": {"left": {"line": 0, "character": 0}, "right": {"line": 0, "character": 0}},
             "last_updated_unix": 2},
            {"id": "s2", "display_name": "Two", "root_folders": ["{root}"], "bookmarks": [],
             "open_files": {"left": null, "right": null},
             "pane_positions": {"left": {"line": 0, "character": 0}, "right": {"line": 0, "character": 0}},
             "last_updated_unix": 1}
        ]
    }"#;
    let mut t = TestApp::builder()
        .file("a.txt", "a\n")
        .state(state)
        .git()
        .build();
    open_git(&mut t);
    t.click("Workspaces");
    let header = t.label_rect("2 workspaces");
    t.click_at(egui::pos2(header.left() + 60.0, header.bottom() + 52.0));
    t.assert_status("Switched session.");
    t.steps(2);
    t.assert_status("Git: 1 repositories.");
}

#[test]
fn two_roots_in_one_repository_show_it_once() {
    let mut t = TestApp::builder()
        .file("a.txt", "a\n")
        .file("sub/b.txt", "b\n")
        .git()
        .build();
    t.platform.queue_folder_pick(&t.path("sub"));
    t.click_nth("Add", 0);
    open_git(&mut t);
    assert_eq!(t.app().git_repositories.len(), 1);
}
