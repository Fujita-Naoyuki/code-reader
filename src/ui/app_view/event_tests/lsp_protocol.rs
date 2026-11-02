//! Go to Definition through the application's real LSP worker: process start-up,
//! `initialize`, document sync, location results in their different shapes, server
//! requests and notifications, protocol errors and broken streams. The server on the other
//! end is the scripted JSON-RPC server from `scripted_lsp`.

use eframe::egui;
use serde_json::{Value, json};

use crate::core::location::Position;

use super::scripted_lsp::{Script, lsp_location, lsp_location_link};
use super::support::TestApp;

const RUST: &str = "rust-analyzer";
const GO: &str = "gopls";

const MAIN_RS: &str = "fn helper() -> u32 {\n    1\n}\n\nfn main() {\n    let value = helper();\n    println!(\"{value}\");\n}\n";

fn rust_app(script: Script) -> TestApp {
    let mut t = TestApp::builder()
        .file("src/main.rs", MAIN_RS)
        .file("src/other.rs", "pub fn other() {}\n")
        .scripted_lsp(RUST, script)
        .build();
    t.click("src");
    t.click_first("main.rs");
    t
}

fn press_f12_on_helper(t: &mut TestApp) {
    t.click_text(5, 17);
    t.key(egui::Key::F12);
}

/// Waits until a status message containing `text` shows up (the worker is asynchronous).
fn wait(t: &mut TestApp, text: &str) {
    t.wait_for_status(text);
}

#[test]
fn definition_round_trip_over_json_rpc() {
    let mut t = rust_app(Script::default());
    let main = t.path("src/main.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!([lsp_location(&main, 0, 3)]);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 3)));
    let methods = t.platform.scripted(RUST).received_methods();
    for expected in [
        "initialize",
        "initialized",
        "textDocument/didOpen",
        "textDocument/definition",
    ] {
        assert!(
            methods.iter().any(|method| method == expected),
            "{methods:?}"
        );
    }
    assert!(t.app().rust_lsp.debug.initialized);

    // Editing sends didChange before the next request.
    t.alt(egui::Key::ArrowLeft);
    t.click_text(4, 0);
    t.type_text("// edited\n");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!(lsp_location_link(&main, 1, 3));
    t.click_text(6, 17);
    t.key(egui::Key::F12);
    wait(&mut t, "Jumped to definition");
    assert!(
        t.platform
            .scripted(RUST)
            .received_methods()
            .iter()
            .any(|method| method == "textDocument/didChange")
    );
}

#[test]
fn single_object_and_empty_results() {
    let mut t = rust_app(Script::default());
    let other = t.path("src/other.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition = lsp_location(&other, 0, 7);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    assert_eq!(t.app().left.path(), Some(other.as_path()));

    t.alt(egui::Key::ArrowLeft);
    t.platform.scripted(RUST).script.lock().unwrap().definition = json!("not a location");
    press_f12_on_helper(&mut t);
    wait(&mut t, "rust-analyzer returned no definition for `helper`.");
}

#[test]
fn definition_at_the_cursor_asks_for_references() {
    let mut t = rust_app(Script::default());
    let main = t.path("src/main.rs");
    {
        let server = t.platform.scripted(RUST);
        let mut script = server.script.lock().unwrap();
        script.definition = json!([lsp_location(&main, 5, 17)]);
        script.references = json!([lsp_location(&main, 0, 3), lsp_location(&main, 5, 16)]);
    }
    press_f12_on_helper(&mut t);
    wait(&mut t, "Found 2 references for `helper`.");
    assert!(
        t.platform
            .scripted(RUST)
            .received_methods()
            .iter()
            .any(|method| method == "textDocument/references")
    );
}

#[test]
fn server_requests_and_notifications_are_answered_and_reported() {
    let script = Script {
        after_initialize: vec![
            json!({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration",
               "params": {"items": [{"section": "rust-analyzer"}, {"section": "files"}]}}),
            json!({"jsonrpc": "2.0", "id": "folders", "method": "workspace/workspaceFolders"}),
            json!({"jsonrpc": "2.0", "id": 901, "method": "window/workDoneProgress/create",
               "params": {"token": 7}}),
            json!({"jsonrpc": "2.0", "method": "$/progress",
               "params": {"token": 7, "value": {"kind": "begin", "title": "Indexing",
                          "message": "crates", "percentage": 10}}}),
            json!({"jsonrpc": "2.0", "method": "$/progress",
               "params": {"token": "t", "value": {"kind": "end"}}}),
            json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "no-value"}}),
            json!({"jsonrpc": "2.0", "method": "window/logMessage",
               "params": {"type": 3, "message": "loading"}}),
            json!({"jsonrpc": "2.0", "method": "window/showMessage",
               "params": {"type": 3, "message": "hello"}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
               "params": {"uri": "file:///x.rs", "diagnostics": [{}, {}]}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "rust-analyzer/status",
               "params": {"health": "ok", "detail": "x".repeat(400)}}),
            json!({"jsonrpc": "2.0", "method": "custom/unknown"}),
        ],
        ..Script::default()
    };
    let mut t = rust_app(script);
    let main = t.path("src/main.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!([lsp_location(&main, 0, 3)]);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    t.wait_until("server events", |app| {
        app.rust_lsp.debug.diagnostics_count == 0
            && app.rust_lsp.debug.server_notification_count >= 8
    });

    let received = t.platform.scripted(RUST).received.lock().unwrap().clone();
    let reply = |id: Value| {
        received
            .iter()
            .find(|message| message.get("id") == Some(&id) && message.get("method").is_none())
            .cloned()
            .unwrap_or_else(|| panic!("no reply to {id}"))
    };
    assert_eq!(reply(json!(900))["result"], json!([{}, {}]));
    assert!(
        reply(json!("folders"))["result"].as_array().unwrap()[0]["uri"]
            .as_str()
            .unwrap()
            .starts_with("file://")
    );
    assert_eq!(reply(json!(901))["result"], Value::Null);
    assert!(t.app().rust_lsp.debug.server_request_count >= 3);
    assert!(t.app().rust_lsp.debug.progress_count >= 3);

    t.click("Report");
    let report = t.platform.clipboard_text().unwrap();
    assert!(
        report.contains("rust_lsp=true initialized=true"),
        "{report}"
    );
}

#[test]
fn protocol_errors_are_reported() {
    let mut t = rust_app(Script::default());
    t.platform
        .scripted(RUST)
        .script
        .lock()
        .unwrap()
        .location_error = Some(json!({"code": -32603, "message": "content modified"}));
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`. (content modified)");

    // An error object without a message is shown as JSON. The failed request also drops the
    // connection, so the next one starts the server again.
    t.platform
        .scripted(RUST)
        .script
        .lock()
        .unwrap()
        .location_error = Some(json!({"code": -32000}));
    t.steps(40);
    press_f12_on_helper(&mut t);
    wait(&mut t, "LSP request failed: {");
}

#[test]
fn initialize_failures_and_missing_servers() {
    let script = Script {
        initialize_error: Some(json!({"code": -32002, "message": "not ready"})),
        ..Script::default()
    };
    let mut t = rust_app(script);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`. (not ready)");

    let script = Script {
        start_error: Some("cannot start".to_owned()),
        ..Script::default()
    };
    let mut t = rust_app(script);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`. (cannot start)");

    let mut t = TestApp::builder()
        .file("src/main.rs", MAIN_RS)
        .missing_lsp()
        .build();
    t.click("src");
    t.click_first("main.rs");
    press_f12_on_helper(&mut t);
    wait(&mut t, "rust-analyzer is not installed or not on PATH.");
}

#[test]
fn broken_streams_become_reader_errors() {
    // Missing Content-Length, a bad length and invalid JSON each stop the reader.
    for raw in [
        b"Header: x\r\n\r\n{}".to_vec(),
        b"Content-Length: abc\r\n\r\n".to_vec(),
        b"Content-Length: 5\r\n\r\nnope!".to_vec(),
    ] {
        let script = Script {
            raw_after_initialize: raw,
            ..Script::default()
        };
        let mut t = rust_app(script);
        // Later requests restart the server and clear the last error, so the event count is
        // what records the failure.
        t.wait_until("reader error", |app| {
            app.rust_lsp.debug.reader_error_count > 0
        });
    }

    // The server hangs up in the middle of a definition request.
    let script = Script {
        hang_up_on: Some("textDocument/definition".to_owned()),
        ..Script::default()
    };
    let mut t = rust_app(script);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`.");

    // The server goes away while idle: the worker notices on its next poll.
    let mut t = rust_app(Script::default());
    let main = t.path("src/main.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!([lsp_location(&main, 0, 3)]);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    t.platform.scripted(RUST).hang_up();
    t.wait_until("reader error", |app| {
        app.rust_lsp.debug.reader_error_count > 0
    });
}

#[test]
fn servers_are_shut_down_when_the_workspace_changes() {
    let mut t = rust_app(Script::default());
    let main = t.path("src/main.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!([lsp_location(&main, 0, 3)]);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    // Adding a folder rebuilds the workspace and drops (shuts down) the language servers.
    let extra = t.tmp.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    t.platform.queue_folder_pick(&extra);
    t.click_nth("Add", 0);
    let first = t.platform.scripted_servers.borrow()[0].1.received.clone();
    t.wait_until("shutdown sent", |_| {
        first
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.get("method") == Some(&json!("exit")))
    });
}

#[test]
fn gopls_round_trip_with_multibyte_text() {
    let source = "package main\n\n// 日本語のコメント\nfunc helper() int {\n    return 1\n}\n\nfunc main() {\n    _ = \"あ\" + helper()\n}\n";
    let mut t = TestApp::builder()
        .file("main.go", source)
        .file("go.mod", "module example\n\ngo 1.22\n")
        .scripted_lsp(GO, Script::default())
        .build();
    t.click_first("main.go");
    let main = t.path("main.go");
    t.platform.scripted(GO).script.lock().unwrap().definition = json!([lsp_location(&main, 3, 5)]);
    // Column 18 in bytes is after the multibyte string literal.
    t.click_text(8, 15);
    t.key(egui::Key::F12);
    wait(&mut t, "Jumped to definition of `helper`.");
    assert_eq!(t.app().left.cursor_position, Some(Position::new(3, 5)));
    let received = t.platform.scripted(GO).received.lock().unwrap().clone();
    let definition = received
        .iter()
        .find(|message| message.get("method") == Some(&json!("textDocument/definition")))
        .unwrap();
    let didopen = received
        .iter()
        .find(|message| message.get("method") == Some(&json!("textDocument/didOpen")))
        .unwrap();
    assert_eq!(didopen["params"]["textDocument"]["languageId"], json!("go"));
    // The position is sent in UTF-16 code units.
    assert!(
        definition["params"]["position"]["character"]
            .as_u64()
            .unwrap()
            < 18
    );
}

#[test]
fn misconfigured_server_commands_are_reported() {
    // An executable that is not a language server: it prints something and exits.
    let mut t = TestApp::builder()
        .file("src/main.rs", MAIN_RS)
        .real_lsp_command("git")
        .build();
    t.click("src");
    t.click_first("main.rs");
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`.");

    // A folder instead of an executable cannot be started at all.
    let folder = t.tmp.display().to_string();
    let mut t = TestApp::builder()
        .file("src/main.rs", MAIN_RS)
        .real_lsp_command(&folder)
        .build();
    t.click("src");
    t.click_first("main.rs");
    press_f12_on_helper(&mut t);
    wait(&mut t, "Failed to start rust-analyzer:");
}

#[test]
fn notifications_that_arrive_before_an_answer_are_processed_first() {
    let mut t = rust_app(Script {
        before_location_response: vec![
            json!({"jsonrpc": "2.0", "method": "window/logMessage",
                   "params": {"type": 3, "message": "computing"}}),
            json!({"jsonrpc": "2.0", "id": 77, "method": "workspace/configuration",
                   "params": {"items": [{}]}}),
        ],
        ..Script::default()
    });
    let main = t.path("src/main.rs");
    t.platform.scripted(RUST).script.lock().unwrap().definition =
        json!([lsp_location(&main, 0, 3)]);
    press_f12_on_helper(&mut t);
    wait(&mut t, "Jumped to definition of `helper`.");
    assert_eq!(
        t.app().rust_lsp.debug.last_server_message.as_deref(),
        None,
        "the request after the log message carries no message"
    );
}

#[test]
fn writes_fail_once_the_server_has_exited() {
    // The server exits right after `initialized`, so opening the document cannot be sent.
    let mut t = rust_app(Script {
        hang_up_on: Some("initialized".to_owned()),
        ..Script::default()
    });
    press_f12_on_helper(&mut t);
    wait(&mut t, "Rust LSP failed for `helper`.");
}
