//! A scripted language server that speaks real LSP framing (`Content-Length` headers and
//! JSON-RPC) over OS pipes. Unlike the channel-level fake in `support`, it runs the
//! application's real LSP worker (`core::lsp`), so tests cover the protocol code too.

use std::{
    io::{BufRead, BufReader, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use serde_json::{Value, json};

use crate::core::lsp::register_test_process;

/// How the server answers; tests change it at any time through `ScriptedLsp::script`.
#[derive(Default)]
pub(crate) struct Script {
    /// `result` sent for `textDocument/definition`.
    pub definition: Value,
    /// `result` sent for `textDocument/references`.
    pub references: Value,
    /// Answer location requests with this JSON-RPC `error` object instead.
    pub location_error: Option<Value>,
    /// Answer `initialize` with this `error` object.
    pub initialize_error: Option<Value>,
    /// Messages pushed to the client right after the `initialize` response.
    pub after_initialize: Vec<Value>,
    /// Raw bytes written after the `initialize` response (to send malformed frames).
    pub raw_after_initialize: Vec<u8>,
    /// Hang up (close both streams) when this method arrives.
    pub hang_up_on: Option<String>,
    /// Make the "process" fail to start.
    pub start_error: Option<String>,
}

pub(crate) struct ScriptedLsp {
    pub script: Arc<Mutex<Script>>,
    /// Every message the client sent, in order.
    pub received: Arc<Mutex<Vec<Value>>>,
    writer: Arc<Mutex<Option<std::io::PipeWriter>>>,
}

impl ScriptedLsp {
    /// Registers a server reachable as `LspClient` command `<returned name>`.
    pub fn register(server_name: &str, script: Script) -> (String, Self) {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let command = format!(
            "scripted-{server_name}-{}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let script = Arc::new(Mutex::new(script));
        let received = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(Mutex::new(None));
        let server = Self {
            script: script.clone(),
            received: received.clone(),
            writer: writer.clone(),
        };
        register_test_process(
            &command,
            Box::new(move || {
                if let Some(error) = script.lock().unwrap().start_error.clone() {
                    return Err(error);
                }
                let (client_reader, server_writer) = std::io::pipe().map_err(|e| e.to_string())?;
                let (server_reader, client_writer) = std::io::pipe().map_err(|e| e.to_string())?;
                *writer.lock().unwrap() = Some(server_writer);
                let script = script.clone();
                let received = received.clone();
                let writer = writer.clone();
                thread::spawn(move || serve(server_reader, script, received, writer));
                Ok((
                    Box::new(client_writer) as Box<dyn Write + Send>,
                    Box::new(client_reader) as Box<dyn Read + Send>,
                ))
            }),
        );
        (command, server)
    }

    /// Pushes a message (notification or server request) to the client now.
    pub fn send(&self, message: Value) {
        write_message(&self.writer, &message);
    }

    /// Writes raw bytes to the client (for malformed frames).
    pub fn send_raw(&self, bytes: &[u8]) {
        if let Some(writer) = self.writer.lock().unwrap().as_mut() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    /// Closes the server's output, as if the process exited.
    pub fn hang_up(&self) {
        self.writer.lock().unwrap().take();
    }

    pub fn received_methods(&self) -> Vec<String> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| message.get("method").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }
}

fn serve(
    input: std::io::PipeReader,
    script: Arc<Mutex<Script>>,
    received: Arc<Mutex<Vec<Value>>>,
    writer: Arc<Mutex<Option<std::io::PipeWriter>>>,
) {
    let mut reader = BufReader::new(input);
    while let Some(message) = read_message(&mut reader) {
        received.lock().unwrap().push(message.clone());
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        if script.lock().unwrap().hang_up_on.as_deref() == Some(method) {
            writer.lock().unwrap().take();
            return;
        }
        match (method, id) {
            ("initialize", Some(id)) => {
                let (error, after, raw) = {
                    let script = script.lock().unwrap();
                    (
                        script.initialize_error.clone(),
                        script.after_initialize.clone(),
                        script.raw_after_initialize.clone(),
                    )
                };
                match error {
                    Some(error) => respond_error(&writer, id, error),
                    None => respond(&writer, id, json!({ "capabilities": {} })),
                }
                for message in after {
                    write_message(&writer, &message);
                }
                if !raw.is_empty()
                    && let Some(writer) = writer.lock().unwrap().as_mut()
                {
                    let _ = writer.write_all(&raw);
                    let _ = writer.flush();
                }
            }
            ("textDocument/definition" | "textDocument/references", Some(id)) => {
                let (error, result) = {
                    let script = script.lock().unwrap();
                    let result = if method == "textDocument/definition" {
                        script.definition.clone()
                    } else {
                        script.references.clone()
                    };
                    (script.location_error.clone(), result)
                };
                match error {
                    Some(error) => respond_error(&writer, id, error),
                    None => respond(&writer, id, result),
                }
            }
            ("shutdown", Some(id)) => respond(&writer, id, Value::Null),
            ("exit", None) => return,
            _ => {}
        }
    }
}

fn respond(writer: &Mutex<Option<std::io::PipeWriter>>, id: Value, result: Value) {
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn respond_error(writer: &Mutex<Option<std::io::PipeWriter>>, id: Value, error: Value) {
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    );
}

fn write_message(writer: &Mutex<Option<std::io::PipeWriter>>, message: &Value) {
    let payload = serde_json::to_vec(message).unwrap();
    if let Some(writer) = writer.lock().unwrap().as_mut() {
        let _ = write!(writer, "Content-Length: {}\r\n\r\n", payload.len());
        let _ = writer.write_all(&payload);
        let _ = writer.flush();
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut payload = vec![0; length?];
    reader.read_exact(&mut payload).ok()?;
    serde_json::from_slice(&payload).ok()
}

/// An LSP `Location` JSON object for `path` at a position.
pub(crate) fn lsp_location(path: &std::path::Path, line: u32, character: u32) -> Value {
    json!({
        "uri": file_uri(path),
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 },
        }
    })
}

/// An LSP `LocationLink` JSON object (the `target*` form some servers send).
pub(crate) fn lsp_location_link(path: &std::path::Path, line: u32, character: u32) -> Value {
    json!({
        "targetUri": file_uri(path),
        "targetSelectionRange": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 },
        },
        "targetRange": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line + 1, "character": 0 },
        }
    })
}

pub(crate) fn file_uri(path: &std::path::Path) -> String {
    let path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
        .replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path).to_owned();
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    format!("file://{}", path.replace(' ', "%20"))
}
