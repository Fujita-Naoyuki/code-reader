//! Go to Definition (F12) and references through rust-analyzer / gopls, the references
//! overlay and Back / Forward navigation. The language servers are in-memory fakes.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::core::location::Position;
use crate::core::lsp::{LspClientEvent, LspRequestKind, LspServerEvent, LspServerEventKind};

use super::super::LocationListKind;
use super::support::{TestApp, location, server_event};

const RUST: &str = "rust-analyzer";
const GO: &str = "gopls";

const MAIN_RS: &str = "fn helper() -> u32 {\n    1\n}\n\nfn main() {\n    let value = helper();\n    println!(\"{value}\");\n}\n";

fn rust_app() -> TestApp {
    let mut t = TestApp::builder()
        .file("src/main.rs", MAIN_RS)
        .file("src/other.rs", "pub fn other() {}\npub fn more() {}\n")
        .file("notes.txt", "plain\n")
        .build();
    t.click("src");
    t
}

/// Opens main.rs, places the cursor on `helper` in `main()` and presses F12.
fn f12_on_helper(t: &mut TestApp) -> crate::core::lsp::LspTestRequest {
    t.click_first("main.rs");
    t.click_text(5, 17);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(5, 17)));
    t.platform.lsp_requests(RUST);
    t.key(egui::Key::F12);
    let requests = t.platform.lsp_requests(RUST);
    requests
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Definition)
        .expect("definition request")
}

fn respond(
    t: &mut TestApp,
    server: &str,
    request: &crate::core::lsp::LspTestRequest,
    result: Result<Vec<crate::core::location::Location>, String>,
) {
    t.platform.lsp_respond(server, request, result);
    t.steps(3);
}

#[test]
fn rust_workspace_warms_up_rust_analyzer_and_syncs_open_files() {
    let mut t = rust_app();
    assert_eq!(t.platform.lsp_server_names(), vec![RUST.to_owned()]);
    let requests = t.platform.lsp_requests(RUST);
    let warmup = requests
        .iter()
        .find(|request| request.kind == LspRequestKind::WarmUp)
        .expect("warm-up request");
    t.platform.lsp_respond(RUST, warmup, Ok(Vec::new()));
    t.step();
    assert_eq!(t.app().rust_lsp_debug.last_status, "warmup_ok");

    t.click_first("main.rs");
    let sync = t
        .platform
        .lsp_requests(RUST)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Sync)
        .expect("sync request");
    assert!(sync.document.as_ref().unwrap().path.ends_with("main.rs"));
    t.platform
        .lsp_respond(RUST, &sync, Err("sync broke".to_owned()));
    t.step();
    assert_eq!(t.app().rust_lsp_debug.last_status, "sync_failed");
    t.platform.lsp_respond(
        RUST,
        &sync,
        Ok(vec![location(&t.path("src/main.rs"), 0, 0)]),
    );
    t.step();
    assert_eq!(t.app().rust_lsp_debug.last_status, "sync_ok");

    // An unchanged document is not re-sent.
    t.steps(3);
    assert!(
        t.platform
            .lsp_requests(RUST)
            .iter()
            .all(|request| request.kind != LspRequestKind::Sync)
    );

    // A failed warm-up is recorded too.
    t.platform
        .lsp_respond(RUST, warmup, Err("no cargo".to_owned()));
    t.step();
    assert_eq!(t.app().rust_lsp_debug.last_status, "warmup_failed");
}

#[test]
fn f12_jumps_to_a_single_rust_definition_and_back_forward_navigate() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    t.assert_status("Resolving `helper` with rust-analyzer...");
    assert_eq!(request.position, Some(Position::new(5, 17)));

    let target = location(&t.path("src/main.rs"), 0, 3);
    respond(&mut t, RUST, &request, Ok(vec![target]));
    t.assert_status("Jumped to definition of `helper`.");
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 3)));
    assert_eq!(t.app().left_navigation_history.back.len(), 1);
    assert!(t.app().rust_lsp_debug.last_definition_success_at.is_some());

    t.alt(egui::Key::ArrowLeft);
    t.assert_status("Returned to previous location.");
    assert_eq!(t.app().left.cursor_position, Some(Position::new(5, 17)));
    t.alt(egui::Key::ArrowLeft);
    t.assert_status("No previous navigation.");

    t.alt(egui::Key::ArrowRight);
    t.assert_status("Moved to next location.");
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 3)));
    t.alt(egui::Key::ArrowRight);
    t.assert_status("No next navigation.");

    t.key(egui::Key::BrowserBack);
    t.assert_status("Returned to previous location.");
}

#[test]
fn definition_in_another_file_opens_it_and_reveals_it_in_the_tree() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    let target = location(&t.path("src/other.rs"), 1, 7);
    respond(&mut t, RUST, &request, Ok(vec![target]));
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("src/other.rs").as_path())
    );
    assert_eq!(
        t.app().selected_workspace_path,
        Some(t.path("src/other.rs"))
    );
    // The breadcrumb shows the previous location and can be clicked to return.
    assert!(t.has_label_containing("main.rs:6"));
    t.click_containing("main.rs:6");
    t.assert_status_containing("Opened navigation history: main.rs:6");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("src/main.rs").as_path())
    );
}

#[test]
fn several_definitions_are_listed_in_the_report() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    let other = t.path("src/other.rs");
    respond(
        &mut t,
        RUST,
        &request,
        Ok(vec![location(&other, 0, 7), location(&other, 1, 7)]),
    );
    t.assert_status("Multiple definitions returned for `helper`. Use Report for details.");
    assert_eq!(t.app().definition_candidates.len(), 2);
    assert!(!t.app().references_overlay.open);
}

#[test]
fn definition_at_the_cursor_falls_back_to_references_overlay() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    let main = t.path("src/main.rs");
    // rust-analyzer answers with the symbol under the cursor itself: look for references.
    respond(&mut t, RUST, &request, Ok(vec![location(&main, 5, 17)]));
    t.assert_status("Finding references for `helper` with rust-analyzer...");
    let references = t
        .platform
        .lsp_requests(RUST)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::References)
        .expect("references request");
    let other = t.path("src/other.rs");
    respond(
        &mut t,
        RUST,
        &references,
        Ok(vec![
            location(&main, 0, 3),
            location(&main, 5, 16),
            location(&other, 1, 7),
        ]),
    );
    t.assert_status("Found 3 references for `helper`.");
    assert!(t.app().references_overlay.open);
    assert!(t.has_label("References"));
    assert!(t.has_label("3 references in 2 files"));
    assert_eq!(t.app().location_list_kind, LocationListKind::References);

    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::ArrowDown);
    assert_eq!(t.app().references_overlay.selected_index, 2);
    t.key(egui::Key::ArrowUp);
    assert_eq!(t.app().references_overlay.selected_index, 1);

    t.type_text("other");
    assert_eq!(t.app().references_overlay.selected_index, 0);
    assert!(t.has_label("1 references in 1 files"));
    t.key(egui::Key::Enter);
    assert!(!t.app().references_overlay.open);
    assert_eq!(t.app().left.path.as_deref(), Some(other.as_path()));
    assert_eq!(t.app().left.cursor_position, Some(Position::new(1, 7)));
}

#[test]
fn references_overlay_click_filter_without_matches_and_escape() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    let main = t.path("src/main.rs");
    respond(&mut t, RUST, &request, Ok(vec![location(&main, 5, 17)]));
    let references = t
        .platform
        .lsp_requests(RUST)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::References)
        .unwrap();
    respond(
        &mut t,
        RUST,
        &references,
        Ok(vec![location(&main, 0, 3), location(&main, 5, 16)]),
    );
    assert!(t.app().references_overlay.open);
    t.click_containing("1  fn helper() -> u32 {");
    assert!(!t.app().references_overlay.open);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 3)));

    // Re-open the same list and filter it down to nothing.
    t.app_mut().open_references_overlay("helper");
    t.app_mut().location_list_kind = LocationListKind::References;
    t.settle();
    t.type_text("zzz");
    assert!(t.has_label("No references"));
    t.key(egui::Key::ArrowDown);
    t.key(egui::Key::Enter);
    assert!(t.app().references_overlay.open);
    t.key(egui::Key::Escape);
    assert!(!t.app().references_overlay.open);

    // The overlay closes itself when its list is no longer references.
    t.app_mut().open_references_overlay("helper");
    t.app_mut().location_list_kind = LocationListKind::Definitions;
    t.step();
    assert!(!t.app().references_overlay.open);
}

#[test]
fn rust_definition_errors_empty_results_and_stale_responses() {
    let mut t = rust_app();
    let request = f12_on_helper(&mut t);
    // A response for another request id is ignored.
    let mut stale = request.clone();
    stale.request_id += 1000;
    respond(&mut t, RUST, &stale, Ok(Vec::new()));
    assert!(t.app().pending_rust_lsp_request.is_some());

    respond(&mut t, RUST, &request, Ok(Vec::new()));
    t.assert_status_containing("rust-analyzer returned no definition for `helper`.");
    t.assert_status_containing("Current state: starting.");

    // After initialization the last server message is included.
    t.platform.lsp_send(
        RUST,
        server_event(LspServerEventKind::Initialized, "initialized"),
    );
    t.platform.lsp_send(
        RUST,
        LspClientEvent::ServerEvent(LspServerEvent {
            kind: LspServerEventKind::Notification,
            method: "window/logMessage".to_owned(),
            message: Some("loading crates".to_owned()),
            diagnostics_count: None,
        }),
    );
    t.step();
    t.click_text(5, 17);
    t.key(egui::Key::F12);
    let request = t
        .platform
        .lsp_requests(RUST)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Definition)
        .unwrap();
    respond(&mut t, RUST, &request, Ok(Vec::new()));
    t.assert_status_containing("Last server message: loading crates");

    t.key(egui::Key::F12);
    let request = t
        .platform
        .lsp_requests(RUST)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Definition)
        .unwrap();
    respond(&mut t, RUST, &request, Err("boom".to_owned()));
    t.assert_status("Rust LSP failed for `helper`. (boom)");
    assert_eq!(
        t.app().rust_lsp_debug.last_definition_status,
        "definition_failed"
    );
}

#[test]
fn rust_definition_and_references_time_out() {
    let mut t = rust_app();
    f12_on_helper(&mut t);
    t.app_mut()
        .pending_rust_lsp_request
        .as_mut()
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(10);
    t.step();
    t.assert_status_containing("rust-analyzer did not answer before timeout for `helper`.");
    assert_eq!(t.app().rust_lsp_debug.timeout_count, 1);

    let request = {
        t.key(egui::Key::F12);
        t.platform
            .lsp_requests(RUST)
            .into_iter()
            .find(|request| request.kind == LspRequestKind::Definition)
            .unwrap()
    };
    let main = t.path("src/main.rs");
    respond(&mut t, RUST, &request, Ok(vec![location(&main, 5, 17)]));
    t.app_mut()
        .pending_rust_lsp_request
        .as_mut()
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(10);
    t.step();
    t.assert_status("rust-analyzer references timed out for `helper`.");
    assert_eq!(t.app().location_list_kind, LocationListKind::References);
}

#[test]
fn rust_requests_fail_when_the_server_is_gone() {
    let mut t = rust_app();
    t.click_first("main.rs");
    t.click_text(5, 17);
    t.platform.stop_lsp_servers();
    t.key(egui::Key::F12);
    t.assert_status("Rust LSP unavailable. (rust-analyzer LSP worker is not running.)");
    assert_eq!(
        t.app().rust_lsp_debug.last_status,
        "definition_queue_failed"
    );

    // Sync and references fail the same way.
    t.app_mut().rust_lsp_synced_hashes.clear();
    t.step();
    assert_eq!(t.app().rust_lsp_debug.last_status, "sync_queue_failed");
    let pending = super::super::PendingGoLspRequest {
        request_id: 1,
        kind: LspRequestKind::Definition,
        document: crate::core::document::Document::new(t.path("src/main.rs"), MAIN_RS.to_owned()),
        position: Position::new(5, 17),
        identifier: "helper".to_owned(),
        origin: None,
        started_at: Instant::now(),
    };
    assert!(!t.app_mut().request_rust_lsp_references(pending));
    t.step();
    t.assert_status("Rust references request failed. (rust-analyzer LSP worker is not running.)");
}

#[test]
fn rust_analyzer_readiness_is_shown_in_the_status_bar() {
    let mut t = rust_app();
    t.click_first("main.rs");
    t.app_mut().status_message.clear();
    t.step();
    assert!(t.status_bar_contains("rust-analyzer warming up... (starting)"));

    t.platform.lsp_send(
        RUST,
        server_event(LspServerEventKind::Initialized, "initialized"),
    );
    t.step();
    assert!(t.status_bar_contains("rust-analyzer warming up... (workspace/dependencies)"));

    let progress = |kind: &str| {
        LspClientEvent::ServerEvent(LspServerEvent {
            kind: LspServerEventKind::Notification,
            method: "$/progress".to_owned(),
            message: Some(format!("token=x kind={kind}")),
            diagnostics_count: None,
        })
    };
    t.platform.lsp_send(RUST, progress("begin"));
    t.step();
    assert!(t.status_bar_contains("rust-analyzer warming up... (indexing)"));
    t.platform.lsp_send(RUST, progress("report"));
    t.platform.lsp_send(RUST, progress("end"));
    t.platform.lsp_send(
        RUST,
        LspClientEvent::ServerEvent(LspServerEvent {
            kind: LspServerEventKind::Notification,
            method: "textDocument/publishDiagnostics".to_owned(),
            message: None,
            diagnostics_count: Some(2),
        }),
    );
    t.platform.lsp_send(
        RUST,
        server_event(LspServerEventKind::Request, "workspace/configuration"),
    );
    t.step();
    assert!(t.status_bar_contains("rust-analyzer ready (diagnostics)"));

    t.app_mut().status_message = "Busy".to_owned();
    t.step();
    assert!(t.status_bar_contains("Busy | rust-analyzer ready (diagnostics)"));

    t.platform.lsp_send(
        RUST,
        LspClientEvent::ServerEvent(LspServerEvent {
            kind: LspServerEventKind::ReaderError,
            method: "reader".to_owned(),
            message: Some("pipe closed".to_owned()),
            diagnostics_count: None,
        }),
    );
    t.step();
    assert!(t.status_bar_contains("rust-analyzer error"));

    // Messages that already mention rust-analyzer are shown as-is.
    t.app_mut().status_message = "rust-analyzer says hi".to_owned();
    t.step();
    assert_eq!(t.app().status_bar_message(), "rust-analyzer says hi");

    // Without an open Rust file there is no readiness suffix.
    t.click_first("notes.txt");
    t.app_mut().status_message = "Plain".to_owned();
    t.step();
    assert_eq!(t.app().status_bar_message(), "Plain");
    assert_eq!(
        t.app().rust_lsp_readiness_report_line().split(' ').next(),
        Some("rust_lsp_readiness=error")
    );
    t.app_mut().rust_lsp_client = None;
    assert_eq!(
        t.app().rust_lsp_readiness_report_line().split(' ').next(),
        Some("rust_lsp_readiness=not_started")
    );
}

#[test]
fn rust_analyzer_ready_after_a_successful_definition() {
    let mut t = rust_app();
    t.platform.lsp_send(
        RUST,
        server_event(LspServerEventKind::Initialized, "initialized"),
    );
    let request = f12_on_helper(&mut t);
    let target = location(&t.path("src/main.rs"), 0, 3);
    respond(&mut t, RUST, &request, Ok(vec![target]));
    t.app_mut().status_message.clear();
    t.step();
    assert!(t.status_bar_contains("rust-analyzer ready (definition)"));

    t.app_mut().rust_lsp_debug.warmup_count = 0;
    t.app_mut().rust_lsp_debug.sync_count = 0;
    t.app_mut().rust_lsp_debug.last_definition_success_at = None;
    t.step();
    assert!(t.status_bar_contains("rust-analyzer warming up... (initializing)"));
}

#[test]
fn f12_preconditions_are_reported() {
    let mut t = rust_app();
    t.key(egui::Key::F12);
    t.assert_status("No file is open in the focused pane.");

    t.click_first("notes.txt");
    t.app_mut().left.cursor_position = None;
    t.key(egui::Key::F12);
    t.assert_status("Place the cursor on a symbol before F12.");

    t.click_text(0, 1);
    t.key(egui::Key::F12);
    t.assert_status("No language support for this file.");

    // F12 is ignored while Search Everywhere is open.
    t.ctrl(egui::Key::P);
    t.app_mut().status_message = "marker".to_owned();
    t.key(egui::Key::F12);
    assert_eq!(t.status(), "marker");
}

#[test]
fn editor_context_menu_offers_definition_and_back() {
    let mut t = rust_app();
    t.click_first("main.rs");
    t.click_text(5, 17);
    let pos = t.editor_text_pos(5, 17);
    t.secondary_click_at(pos);
    assert!(t.has_label("Go to Definition"));
    t.click("Go to Definition");
    t.assert_status("Resolving `helper` with rust-analyzer...");

    t.secondary_click_at(pos);
    t.click("Back");
    t.assert_status("No previous navigation.");
}

const MAIN_GO: &str =
    "package main\n\nfunc helper() int {\n    return 1\n}\n\nfunc main() {\n    _ = helper()\n}\n";

fn go_app() -> TestApp {
    let mut t = TestApp::builder()
        .file("main.go", MAIN_GO)
        .file("util.go", "package main\n\nfunc util() {}\n")
        .file("go.mod", "module example\n\ngo 1.22\n")
        .build();
    t.click_first("main.go");
    t
}

fn go_f12(t: &mut TestApp) -> crate::core::lsp::LspTestRequest {
    t.click_text(7, 9);
    t.platform.lsp_requests(GO);
    t.key(egui::Key::F12);
    t.platform
        .lsp_requests(GO)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::Definition)
        .expect("go definition request")
}

#[test]
fn go_definition_references_and_errors() {
    let mut t = go_app();
    assert_eq!(t.platform.lsp_server_names(), vec![GO.to_owned()]);
    let main = t.path("main.go");

    let request = go_f12(&mut t);
    t.assert_status("Resolving `helper` with gopls...");
    respond(&mut t, GO, &request, Ok(vec![location(&main, 2, 5)]));
    t.assert_status("Jumped to definition of `helper`.");

    t.alt(egui::Key::ArrowLeft);
    let request = go_f12(&mut t);
    respond(&mut t, GO, &request, Ok(vec![location(&main, 7, 9)]));
    t.assert_status("Finding references for `helper` with gopls...");
    let references = t
        .platform
        .lsp_requests(GO)
        .into_iter()
        .find(|request| request.kind == LspRequestKind::References)
        .unwrap();
    respond(
        &mut t,
        GO,
        &references,
        Ok(vec![location(&main, 2, 5), location(&main, 7, 5)]),
    );
    t.assert_status("Found 2 references for `helper`.");
    t.key(egui::Key::Escape);

    let request = go_f12(&mut t);
    respond(&mut t, GO, &request, Ok(Vec::new()));
    t.assert_status("No definition found for `helper`.");

    let request = go_f12(&mut t);
    respond(&mut t, GO, &request, Err("gopls crashed".to_owned()));
    t.assert_status("Go LSP failed for `helper`. (gopls crashed)");

    // Server events and sync responses are recorded for diagnostics.
    t.platform.lsp_send(
        GO,
        server_event(LspServerEventKind::Initialized, "initialized"),
    );
    t.step();
    assert!(t.app().go_lsp_debug.initialized);
    let sync = crate::core::lsp::LspTestRequest {
        request_id: 999,
        kind: LspRequestKind::Sync,
        document: None,
        position: None,
    };
    t.platform.lsp_respond(GO, &sync, Ok(Vec::new()));
    t.step();
    assert_eq!(t.app().go_lsp_debug.last_status, "sync_ok");
    t.platform.lsp_respond(GO, &sync, Err("bad".to_owned()));
    t.step();
    assert_eq!(t.app().go_lsp_debug.last_status, "sync_failed");
}

#[test]
fn go_requests_time_out_and_fail_without_a_server() {
    let mut t = go_app();
    go_f12(&mut t);
    t.app_mut()
        .pending_go_lsp_request
        .as_mut()
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(10);
    t.step();
    t.assert_status("gopls timed out for `helper`. Wait a moment and retry F12.");

    let request = go_f12(&mut t);
    let target = location(&t.path("main.go"), 7, 9);
    respond(&mut t, GO, &request, Ok(vec![target]));
    t.app_mut()
        .pending_go_lsp_request
        .as_mut()
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(10);
    t.step();
    t.assert_status("gopls references timed out for `helper`.");

    // Edits are re-synced; with the server gone, queuing fails.
    t.platform.stop_lsp_servers();
    t.click_text(0, 0);
    t.type_text("// ");
    assert_eq!(t.app().go_lsp_debug.last_status, "sync_queue_failed");
    t.key(egui::Key::F12);
    t.assert_status("Go LSP unavailable. (gopls LSP worker is not running.)");

    let pending = super::super::PendingGoLspRequest {
        request_id: 1,
        kind: LspRequestKind::Definition,
        document: crate::core::document::Document::new(t.path("main.go"), MAIN_GO.to_owned()),
        position: Position::new(7, 9),
        identifier: "helper".to_owned(),
        origin: None,
        started_at: Instant::now(),
    };
    assert!(!t.app_mut().request_go_lsp_references(pending));
    t.step();
    t.assert_status("Go references request failed. (gopls LSP worker is not running.)");
}

#[test]
fn definition_via_the_symbol_index_for_other_languages() {
    // Languages without an LSP route fall back to the symbol index. None are registered
    // besides Rust and Go today, so drive the fallback directly on a Rust file.
    let mut t = rust_app();
    t.click_first("main.rs");
    t.click_text(5, 17);
    t.app_mut()
        .handle_locations("helper", Vec::new(), LocationListKind::Definitions);
    t.step();
    t.assert_status("No definition found for `helper`.");
    let target = location(&t.path("src/main.rs"), 0, 3);
    t.app_mut()
        .handle_locations("helper", vec![target], LocationListKind::References);
    t.step();
    t.assert_status("Jumped to reference of `helper`.");
}
