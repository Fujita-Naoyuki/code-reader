//! A language server client: a worker thread speaking JSON-RPC to the server
//! process, with requests and answers passed over channels.

mod protocol;

use std::{
    collections::HashMap,
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::core::{
    document::Document,
    location::{Location, Position},
    process::hide_windows_console,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspRequestKind {
    WarmUp,
    Sync,
    Definition,
    References,
}

use protocol::{byte_position_to_lsp_position, parse_lsp_locations, path_to_file_uri};

#[derive(Debug, Clone)]
pub struct LspLocationResponse {
    pub request_id: u64,
    pub kind: LspRequestKind,
    pub result: Result<Vec<Location>, String>,
}

#[derive(Debug, Clone)]
pub enum LspClientEvent {
    Response(LspLocationResponse),
    ServerEvent(LspServerEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspServerEventKind {
    Request,
    Notification,
    ReaderError,
    Initialized,
}

#[derive(Debug, Clone)]
pub struct LspServerEvent {
    pub kind: LspServerEventKind,
    pub method: String,
    pub message: Option<String>,
    pub diagnostics_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub language_id: String,
    pub roots: Vec<PathBuf>,
}

impl LspServerConfig {
    pub fn go(roots: Vec<PathBuf>) -> Self {
        Self {
            server_name: "gopls".to_owned(),
            command: "gopls".to_owned(),
            args: vec!["serve".to_owned()],
            language_id: "go".to_owned(),
            roots,
        }
    }

    pub fn rust_analyzer(roots: Vec<PathBuf>) -> Self {
        Self {
            server_name: "rust-analyzer".to_owned(),
            command: rust_analyzer_command(),
            args: Vec::new(),
            language_id: "rust".to_owned(),
            roots,
        }
    }
}

pub struct LspClient {
    server_name: String,
    command_sender: Sender<LspCommand>,
    event_receiver: Receiver<LspClientEvent>,
    next_request_id: u64,
}

impl LspClient {
    pub fn start(config: LspServerConfig) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let server_name = config.server_name.clone();
        thread::spawn(move || run_lsp_worker(config, command_receiver, event_sender));

        Self {
            server_name,
            command_sender,
            event_receiver,
            next_request_id: 0,
        }
    }

    pub fn request_definition(
        &mut self,
        document: Document,
        position: Position,
    ) -> Result<u64, String> {
        self.request_locations(LspRequestKind::Definition, document, position)
    }

    pub fn request_references(
        &mut self,
        document: Document,
        position: Position,
    ) -> Result<u64, String> {
        self.request_locations(LspRequestKind::References, document, position)
    }

    pub fn warm_up(&mut self) -> Result<u64, String> {
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        self.command_sender
            .send(LspCommand {
                request_id,
                kind: LspRequestKind::WarmUp,
                document: None,
                position: None,
            })
            .map_err(|_| format!("{} LSP worker is not running.", self.server_name))?;
        Ok(request_id)
    }

    pub fn sync_document(&mut self, document: Document) -> Result<u64, String> {
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        self.command_sender
            .send(LspCommand {
                request_id,
                kind: LspRequestKind::Sync,
                document: Some(document),
                position: None,
            })
            .map_err(|_| format!("{} LSP worker is not running.", self.server_name))?;
        Ok(request_id)
    }

    pub fn try_recv(&self) -> Option<LspClientEvent> {
        self.event_receiver.try_recv().ok()
    }

    fn request_locations(
        &mut self,
        kind: LspRequestKind,
        document: Document,
        position: Position,
    ) -> Result<u64, String> {
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        self.command_sender
            .send(LspCommand {
                request_id,
                kind,
                document: Some(document),
                position: Some(position),
            })
            .map_err(|_| format!("{} LSP worker is not running.", self.server_name))?;
        Ok(request_id)
    }
}

/// In-memory counterpart of an [`LspClient`] for tests.
///
/// It receives the commands the client queues and lets the test send back responses and
/// server events, as a real `rust-analyzer` / `gopls` worker would. Dropping it makes the
/// client's requests fail with "LSP worker is not running".
#[cfg(test)]
pub struct LspTestServer {
    commands: Receiver<LspCommand>,
    events: Sender<LspClientEvent>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct LspTestRequest {
    pub request_id: u64,
    pub kind: LspRequestKind,
    pub document: Option<Document>,
    pub position: Option<Position>,
}

#[cfg(test)]
impl LspClient {
    pub fn in_memory(server_name: &str) -> (Self, LspTestServer) {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let client = Self {
            server_name: server_name.to_owned(),
            command_sender,
            event_receiver,
            next_request_id: 0,
        };
        let server = LspTestServer {
            commands: command_receiver,
            events: event_sender,
        };
        (client, server)
    }
}

#[cfg(test)]
impl LspTestServer {
    /// Drains every request the client queued so far.
    pub fn take_requests(&self) -> Vec<LspTestRequest> {
        self.commands
            .try_iter()
            .map(|command| LspTestRequest {
                request_id: command.request_id,
                kind: command.kind,
                document: command.document,
                position: command.position,
            })
            .collect()
    }

    pub fn respond(&self, request: &LspTestRequest, result: Result<Vec<Location>, String>) {
        self.send(LspClientEvent::Response(LspLocationResponse {
            request_id: request.request_id,
            kind: request.kind,
            result,
        }));
    }

    pub fn send(&self, event: LspClientEvent) {
        let _ = self.events.send(event);
    }
}

#[derive(Debug)]
struct LspCommand {
    request_id: u64,
    kind: LspRequestKind,
    document: Option<Document>,
    position: Option<Position>,
}

#[derive(Debug, Clone)]
struct OpenDocument {
    version: i32,
    text: String,
}

/// The byte streams of a running language server. `child` is the server process; it is
/// `None` for the in-memory servers tests attach through `register_test_process`.
struct LspProcess {
    stdin: Box<dyn Write + Send>,
    stdout: Box<dyn Read + Send>,
    child: Option<Child>,
}

struct LspConnection {
    config: LspServerConfig,
    child: Option<Child>,
    stdin: Box<dyn Write + Send>,
    message_receiver: Receiver<Result<Value, String>>,
    event_sender: Sender<LspClientEvent>,
    next_lsp_request_id: i64,
    open_documents: HashMap<String, OpenDocument>,
}

impl Drop for LspConnection {
    fn drop(&mut self) {
        let _ = self.shutdown();
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }
}

fn run_lsp_worker(
    config: LspServerConfig,
    command_receiver: Receiver<LspCommand>,
    event_sender: Sender<LspClientEvent>,
) {
    let mut connection: Option<LspConnection> = None;

    loop {
        let command = match command_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => {
                let mut clear_connection = false;
                if let Some(active_connection) = connection.as_mut()
                    && let Err(error) = active_connection.drain_incoming()
                {
                    active_connection.emit_reader_error(error);
                    clear_connection = true;
                }
                if clear_connection {
                    connection = None;
                }
                None
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        let Some(command) = command else {
            continue;
        };

        if connection.is_none() {
            match LspConnection::start(&config, event_sender.clone()) {
                Ok(next) => connection = Some(next),
                Err(error) => {
                    let _ = event_sender.send(LspClientEvent::Response(LspLocationResponse {
                        request_id: command.request_id,
                        kind: command.kind,
                        result: Err(error),
                    }));
                    continue;
                }
            }
        }

        let result = match command.kind {
            LspRequestKind::WarmUp => Ok(Vec::new()),
            LspRequestKind::Sync => connection
                .as_mut()
                .expect("connection is initialized")
                .sync_document(
                    command
                        .document
                        .as_ref()
                        .expect("sync document is provided"),
                )
                .map(|()| Vec::new()),
            LspRequestKind::Definition | LspRequestKind::References => {
                let Some(document) = command.document.as_ref() else {
                    let _ = event_sender.send(LspClientEvent::Response(LspLocationResponse {
                        request_id: command.request_id,
                        kind: command.kind,
                        result: Err("Missing LSP request document.".to_owned()),
                    }));
                    continue;
                };
                let Some(position) = command.position else {
                    let _ = event_sender.send(LspClientEvent::Response(LspLocationResponse {
                        request_id: command.request_id,
                        kind: command.kind,
                        result: Err("Missing LSP request position.".to_owned()),
                    }));
                    continue;
                };
                connection
                    .as_mut()
                    .expect("connection is initialized")
                    .request_locations(command.kind, document, position)
            }
        };

        if result.is_err() {
            connection = None;
        }

        let _ = event_sender.send(LspClientEvent::Response(LspLocationResponse {
            request_id: command.request_id,
            kind: command.kind,
            result,
        }));
    }
}

impl LspConnection {
    fn start(
        config: &LspServerConfig,
        event_sender: Sender<LspClientEvent>,
    ) -> Result<Self, String> {
        let LspProcess {
            stdin,
            stdout,
            child,
        } = spawn_lsp_process(config)?;
        let (message_sender, message_receiver) = mpsc::channel();
        spawn_lsp_reader(stdout, message_sender);

        let mut connection = Self {
            config: config.clone(),
            child,
            stdin,
            message_receiver,
            event_sender,
            next_lsp_request_id: 1,
            open_documents: HashMap::new(),
        };
        connection.initialize()?;
        Ok(connection)
    }

    fn initialize(&mut self) -> Result<(), String> {
        let root_uri = self.config.roots.first().map(|root| path_to_file_uri(root));
        let workspace_folders = self
            .config
            .roots
            .iter()
            .map(|root| {
                json!({
                    "uri": path_to_file_uri(root),
                    "name": workspace_folder_name(root),
                })
            })
            .collect::<Vec<_>>();

        let request_id = self.next_request_id();
        self.send_request(
            request_id,
            "initialize",
            json!({
                "processId": Value::Null,
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "definition": { "linkSupport": false },
                        "references": {},
                        "synchronization": {
                            "dynamicRegistration": false,
                            "didSave": false
                        }
                    },
                    "workspace": {
                        "workspaceFolders": true,
                        "configuration": true
                    },
                    "window": {
                        "workDoneProgress": true
                    }
                },
                "workspaceFolders": workspace_folders,
            }),
        )?;
        let response = self.read_response(request_id)?;
        response_error(&response)?;
        self.send_notification("initialized", json!({}))?;
        let _ = self
            .event_sender
            .send(LspClientEvent::ServerEvent(LspServerEvent {
                kind: LspServerEventKind::Initialized,
                method: "initialized".to_owned(),
                message: Some(format!("{} initialized", self.config.server_name)),
                diagnostics_count: None,
            }));
        Ok(())
    }

    fn request_locations(
        &mut self,
        kind: LspRequestKind,
        document: &Document,
        position: Position,
    ) -> Result<Vec<Location>, String> {
        self.sync_document(document)?;
        let uri = path_to_file_uri(&document.path);
        let lsp_position = byte_position_to_lsp_position(&document.text, position);
        let request_id = self.next_request_id();
        let (method, params) = match kind {
            LspRequestKind::WarmUp => {
                return Err("Warm-up is not a location request.".to_owned());
            }
            LspRequestKind::Sync => {
                return Err("Sync is not a location request.".to_owned());
            }
            LspRequestKind::Definition => (
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": lsp_position,
                }),
            ),
            LspRequestKind::References => (
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": lsp_position,
                    "context": { "includeDeclaration": false },
                }),
            ),
        };

        self.send_request(request_id, method, params)?;
        let response = self.read_response(request_id)?;
        response_error(&response)?;
        Ok(parse_lsp_locations(
            response.get("result").unwrap_or(&Value::Null),
            &self.open_documents,
        ))
    }

    fn sync_document(&mut self, document: &Document) -> Result<(), String> {
        let uri = path_to_file_uri(&document.path);
        if let Some(open_document) = self.open_documents.get_mut(&uri) {
            if open_document.text == document.text {
                return Ok(());
            }
            open_document.version += 1;
            open_document.text = document.text.clone();
            let version = open_document.version;
            self.send_notification(
                "textDocument/didChange",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "version": version,
                    },
                    "contentChanges": [
                        { "text": document.text }
                    ],
                }),
            )?;
        } else {
            self.send_notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": self.config.language_id.clone(),
                        "version": 1,
                        "text": document.text,
                    },
                }),
            )?;
            self.open_documents.insert(
                uri,
                OpenDocument {
                    version: 1,
                    text: document.text.clone(),
                },
            );
        }

        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let request_id = self.next_request_id();
        self.send_request(request_id, "shutdown", json!(Value::Null))?;
        let _ = self.read_response(request_id);
        self.send_notification("exit", json!(Value::Null))
    }

    fn next_request_id(&mut self) -> i64 {
        let request_id = self.next_lsp_request_id;
        self.next_lsp_request_id += 1;
        request_id
    }

    fn send_request(&mut self, id: i64, method: &str, params: Value) -> Result<(), String> {
        self.send_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
    }

    fn send_notification(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send_json(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn send_json(&mut self, message: &Value) -> Result<(), String> {
        let payload = serde_json::to_vec(message)
            .map_err(|error| format!("Failed to encode LSP message: {error}"))?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", payload.len())
            .map_err(|error| format!("Failed to write LSP header: {error}"))?;
        self.stdin
            .write_all(&payload)
            .map_err(|error| format!("Failed to write LSP payload: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("Failed to flush LSP payload: {error}"))
    }

    fn read_response(&mut self, request_id: i64) -> Result<Value, String> {
        loop {
            let message = self.recv_incoming_message()?;
            if self.process_incoming_message(&message, Some(request_id))? {
                return Ok(message);
            }
        }
    }

    fn drain_incoming(&mut self) -> Result<(), String> {
        loop {
            let message = match self.try_recv_incoming_message()? {
                Some(message) => message,
                None => return Ok(()),
            };
            self.process_incoming_message(&message, None)?;
        }
    }

    fn recv_incoming_message(&mut self) -> Result<Value, String> {
        self.message_receiver
            .recv()
            .map_err(|_| format!("{} LSP reader is not running.", self.config.server_name))?
    }

    fn try_recv_incoming_message(&mut self) -> Result<Option<Value>, String> {
        match self.message_receiver.try_recv() {
            Ok(message) => message.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(format!("{} LSP reader stopped.", self.config.server_name))
            }
        }
    }

    fn process_incoming_message(
        &mut self,
        message: &Value,
        waiting_request_id: Option<i64>,
    ) -> Result<bool, String> {
        if message.get("method").is_none()
            && message.get("id").and_then(Value::as_i64) == waiting_request_id
        {
            return Ok(true);
        }

        if message.get("id").is_some() && message.get("method").is_some() {
            self.reply_to_server_request(message)?;
            self.emit_server_event(message, LspServerEventKind::Request);
        } else if message.get("id").is_none() && message.get("method").is_some() {
            self.emit_server_event(message, LspServerEventKind::Notification);
        }
        Ok(false)
    }

    fn reply_to_server_request(&mut self, request: &Value) -> Result<(), String> {
        let Some(id) = request.get("id").cloned() else {
            return Ok(());
        };
        let result = match request.get("method").and_then(Value::as_str) {
            Some("workspace/configuration") => workspace_configuration_response(request),
            Some("workspace/workspaceFolders") => self.workspace_folders_json(),
            _ => Value::Null,
        };
        self.send_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    fn emit_server_event(&self, message: &Value, kind: LspServerEventKind) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
            .to_owned();
        let params = message.get("params").unwrap_or(&Value::Null);
        let event = LspServerEvent {
            kind,
            method: method.clone(),
            message: server_message_summary(&method, params),
            diagnostics_count: diagnostics_count(&method, params),
        };
        let _ = self.event_sender.send(LspClientEvent::ServerEvent(event));
    }

    fn emit_reader_error(&self, error: String) {
        let _ = self
            .event_sender
            .send(LspClientEvent::ServerEvent(LspServerEvent {
                kind: LspServerEventKind::ReaderError,
                method: "reader".to_owned(),
                message: Some(error),
                diagnostics_count: None,
            }));
    }

    fn workspace_folders_json(&self) -> Value {
        Value::Array(
            self.config
                .roots
                .iter()
                .map(|root| {
                    json!({
                        "uri": path_to_file_uri(root),
                        "name": workspace_folder_name(root),
                    })
                })
                .collect(),
        )
    }
}

fn spawn_lsp_process(config: &LspServerConfig) -> Result<LspProcess, String> {
    #[cfg(test)]
    if let Some(process) = test_process(&config.command) {
        return process;
    }

    let mut command = Command::new(&config.command);
    hide_windows_console(&mut command);
    command
        .args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(root) = config.roots.first() {
        command.current_dir(root);
    }

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            format!("{} is not installed or not on PATH.", config.server_name)
        } else {
            format!("Failed to start {}: {error}", config.server_name)
        }
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("Failed to open {} stdin.", config.server_name))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Failed to open {} stdout.", config.server_name))?;
    Ok(LspProcess {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        child: Some(child),
    })
}

/// Factory for an in-memory language server: returns the server's stdin / stdout streams.
#[cfg(test)]
pub type LspTestProcessFactory =
    Box<dyn Fn() -> Result<(Box<dyn Write + Send>, Box<dyn Read + Send>), String> + Send + Sync>;

#[cfg(test)]
static TEST_PROCESSES: std::sync::Mutex<Vec<(String, std::sync::Arc<LspTestProcessFactory>)>> =
    std::sync::Mutex::new(Vec::new());

/// Makes `LspClient::start` connect to `factory` instead of spawning `command`. Tests use a
/// unique command name per server so parallel tests do not see each other's servers.
#[cfg(test)]
pub fn register_test_process(command: &str, factory: LspTestProcessFactory) {
    TEST_PROCESSES
        .lock()
        .unwrap()
        .push((command.to_owned(), std::sync::Arc::new(factory)));
}

#[cfg(test)]
fn test_process(command: &str) -> Option<Result<LspProcess, String>> {
    let factory = TEST_PROCESSES
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(name, _)| name == command)
        .map(|(_, factory)| factory.clone())?;
    Some(factory().map(|(stdin, stdout)| LspProcess {
        stdin,
        stdout,
        child: None,
    }))
}

fn spawn_lsp_reader(
    stdout: Box<dyn Read + Send>,
    message_sender: Sender<Result<Value, String>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let message = read_lsp_message(&mut reader)
                .map_err(|error| format!("Failed to read LSP message: {error}"));
            let should_stop = message.is_err();
            if message_sender.send(message).is_err() || should_stop {
                break;
            }
        }
    })
}

fn read_lsp_message(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "LSP stream closed",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length =
                Some(value.trim().parse::<usize>().map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{error}"))
                })?);
        }
    }

    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{error}")))
}

fn response_error(response: &Value) -> Result<(), String> {
    let Some(error) = response.get("error") else {
        return Ok(());
    };
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        return Err(message.to_owned());
    }
    Err(format!("LSP request failed: {error}"))
}

fn workspace_configuration_response(request: &Value) -> Value {
    let count = request
        .get("params")
        .and_then(|params| params.get("items"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Value::Array((0..count).map(|_| json!({})).collect())
}

fn diagnostics_count(method: &str, params: &Value) -> Option<usize> {
    (method == "textDocument/publishDiagnostics").then(|| {
        params
            .get("diagnostics")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    })
}

fn server_message_summary(method: &str, params: &Value) -> Option<String> {
    let summary = match method {
        "window/logMessage" | "window/showMessage" => params
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "$/progress" => progress_message_summary(params),
        "textDocument/publishDiagnostics" => {
            let count = diagnostics_count(method, params).unwrap_or(0);
            let uri = params
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            Some(format!("{count} diagnostics for {uri}"))
        }
        "rust-analyzer/status" => Some(truncate_debug_json(params)),
        _ => None,
    }?;
    Some(truncate_debug_text(&summary, 240))
}

fn progress_message_summary(params: &Value) -> Option<String> {
    let token = params
        .get("token")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        })
        .unwrap_or_else(|| "<none>".to_owned());
    let value = params.get("value")?;
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let title = value.get("title").and_then(Value::as_str);
    let message = value.get("message").and_then(Value::as_str);
    let percentage = value.get("percentage").and_then(Value::as_u64);
    let mut parts = vec![format!("token={token}"), format!("kind={kind}")];
    if let Some(title) = title {
        parts.push(format!("title={title}"));
    }
    if let Some(message) = message {
        parts.push(format!("message={message}"));
    }
    if let Some(percentage) = percentage {
        parts.push(format!("percentage={percentage}"));
    }
    Some(parts.join(" "))
}

fn truncate_debug_json(value: &Value) -> String {
    truncate_debug_text(&value.to_string(), 240)
}

fn truncate_debug_text(value: &str, limit: usize) -> String {
    let mut output = String::new();
    for ch in value.chars().take(limit) {
        output.push(ch);
    }
    if value.chars().count() > limit {
        output.push_str("...");
    }
    output
}

fn workspace_folder_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn rust_analyzer_command() -> String {
    if let Some(command) = env::var_os("RUST_ANALYZER").filter(|value| !value.is_empty()) {
        return command.to_string_lossy().into_owned();
    }
    find_vscode_rust_analyzer()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rust-analyzer".to_owned())
}

fn find_vscode_rust_analyzer() -> Option<PathBuf> {
    let home = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))?;
    let extensions_dir = PathBuf::from(home).join(".vscode").join("extensions");
    let executable = if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    };

    let mut candidates = fs::read_dir(extensions_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("rust-lang.rust-analyzer") {
                return None;
            }
            let candidate = entry.path().join("server").join(executable);
            candidate.is_file().then_some(candidate)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}
