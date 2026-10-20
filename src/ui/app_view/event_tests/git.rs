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

    // The workspace tree marks changed files.
    t.click("Workspace");
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

    t.click("↶");
    t.click("Discard");
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
fn commit_and_push_without_upstream_commits_but_fails_to_push() {
    let mut t = repo();
    add_origin(&t, false);
    t.write("tracked.txt", "changed\n");
    open_git(&mut t);
    t.click("+ Stage All");
    // The sidebar is laid out first, so the commit message is text field #0.
    t.fill_text_field(0, "Push me");
    t.click("Commit & Push");
    // DISCREPANCY(D05): "Commit & Push" runs a plain `git push`, so a branch without an
    // upstream is committed but never published, although the "..." menu offers
    // "Publish Branch" for exactly this case.
    t.assert_status_containing("Git commit completed but push failed: workspace");
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
    // The menu disables "Publish Branch"; the action itself also refuses.
    let repository = t.app().git_repositories[0].clone();
    t.app_mut().push_git_repository(&repository);
    t.step();
    t.assert_status("Git publish failed: workspace | current HEAD is not on a named branch");
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

    // Switching with uncommitted conflicting changes fails.
    t.git(&["switch", "-q", "main"]);
    t.write("tracked.txt", "main side\n");
    t.git(&["commit", "-qam", "main side"]);
    t.git(&["switch", "-q", "develop"]);
    t.write("tracked.txt", "dirty\n");
    let repository = t.app().git_repositories[0].clone();
    t.app_mut().switch_git_branch(&repository, "main");
    t.step();
    t.assert_status_containing("Git branch switch failed: workspace");
}

#[test]
fn branch_windows_close_when_repository_disappears() {
    let mut t = repo();
    open_git(&mut t);
    t.app_mut().git_switch_branch_root = Some(t.tmp.join("gone"));
    t.app_mut().git_create_branch_root = Some(t.tmp.join("gone"));
    t.step();
    assert!(t.app().git_switch_branch_root.is_none());
    assert!(t.app().git_create_branch_root.is_none());

    t.app_mut().git_switch_branch_root = Some(t.tmp.join("gone"));
    t.app_mut().git_create_branch_root = Some(t.tmp.join("gone"));
    t.app_mut().refresh_git_view();
    assert!(t.app().git_switch_branch_root.is_none());
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
    assert_eq!(t.app().left.diff_display_mode, DiffDisplayMode::SideBySide);
    t.steps(2);
    t.click("Unified");
    assert_eq!(t.app().left.diff_display_mode, DiffDisplayMode::Unified);

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
fn all_diffs_is_disabled_without_changes_and_reports_errors() {
    let mut t = repo();
    open_git(&mut t);
    let repository = t.app().git_repositories[0].clone();
    t.app_mut().open_git_all_diff(&repository);
    t.step();
    assert_eq!(t.app().left.text.as_str(), "No diffs.");

    let mut broken = repository.clone();
    broken.root = t.tmp.join("not-a-repo");
    std::fs::create_dir_all(&broken.root).unwrap();
    t.app_mut().open_git_all_diff(&broken);
    t.step();
    t.assert_status_containing("Open all diffs failed: workspace");
    t.app_mut().open_git_history(&broken);
    t.step();
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
}

#[test]
fn commit_details_report_errors() {
    let mut t = repo();
    open_git(&mut t);
    let commit = super::super::GitCommitNode {
        hash: "0000000000000000000000000000000000000000".to_owned(),
        short_hash: "00000000".to_owned(),
        parents: Vec::new(),
        author: "nobody".to_owned(),
        date: "2020-01-01".to_owned(),
        refs: vec![
            "origin/main".to_owned(),
            "tag: v1".to_owned(),
            "main".to_owned(),
        ],
        subject: "Missing".to_owned(),
    };
    let root = t.root.clone();
    t.app_mut()
        .open_git_commit_details("workspace".to_owned(), root, commit);
    t.settle();
    assert!(t.has_label("Refs"));
    assert!(t.has_label("origin/main"));
    t.click("Open Diff");
    t.assert_status_containing("Open commit diff failed: 00000000");
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
fn files_with_spaces_or_japanese_names_cannot_be_staged() {
    let mut t = repo();
    t.write("my notes.txt", "space\n");
    t.write("日本語.txt", "japanese\n");
    open_git(&mut t);
    let paths = t.app().git_repositories[0]
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect::<Vec<_>>();
    // DISCREPANCY(D04): `git status --porcelain=v1` without `-z` quotes and octal-escapes
    // such paths, and the escaped text is used as-is for stage / diff / discard.
    assert!(paths.contains(&"\"my notes.txt\"".to_owned()), "{paths:?}");
    assert!(
        paths.iter().any(|path| path.starts_with(r#""\346"#)),
        "{paths:?}"
    );
    t.click_nth("+", 0);
    t.assert_status_containing("Git stage failed: workspace");
    let cell = changed_row_path(&t, 0);
    t.click_at(cell);
    t.assert_status_containing("Open diff failed: workspace");
    // The tree cannot match the escaped names to the files either.
    t.click("Workspace");
    t.steps(2);
    assert!(!t.has_label("?"));
}
