//! Go to Definition and references through rust-analyzer and gopls: starting clients,
//! syncing open documents, requests, responses and readiness messages.

use super::*;

impl CodeReaderApp {
    pub(super) fn lsp_mut(&mut self, language: LspLanguage) -> &mut LspSession {
        match language {
            LspLanguage::Go => &mut self.lsp.go,
            LspLanguage::Rust => &mut self.lsp.rust,
        }
    }

    pub(super) fn go_to_definition(&mut self) {
        let editor = self.focused_editor();
        let Some(path) = editor.path().map(Path::to_path_buf) else {
            self.status_message = "No file is open in the focused pane.".to_owned();
            return;
        };
        let Some(position) = editor.cursor_position else {
            self.status_message = "Place the cursor on a symbol before F12.".to_owned();
            return;
        };
        let Some(language) = self.language_registry.language_for_path(&path) else {
            self.status_message = "No language support for this file.".to_owned();
            return;
        };

        let document = Document::new(path.clone(), editor.text.as_str().to_owned());
        let identifier = language
            .identifier_at(&document, position)
            .map(|identifier| identifier.name)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.lsp.definition_candidates.clear();
        self.lsp.location_list_kind = LocationListKind::Definitions;
        self.close_references_overlay();
        // Every registered language resolves definitions through its language server.
        match language.id().0 {
            "go" => {
                self.request_lsp_definition(LspLanguage::Go, document, position, identifier);
            }
            "rust" => {
                self.request_lsp_definition(LspLanguage::Rust, document, position, identifier);
            }
            other => {
                self.status_message = format!("Go to Definition is not available for {other}.");
            }
        }
    }

    pub(super) fn ensure_lsp_client(&mut self, language: LspLanguage) {
        if self.lsp_mut(language).client.is_some() {
            return;
        }
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.lsp_mut(language).client = Some(platform::current().start_lsp(language.config(roots)));
        self.lsp_mut(language).debug.mark_started();
    }

    pub(super) fn warm_up_rust_lsp_for_workspace(&mut self) {
        if !self.workspace_has_rust_source() {
            return;
        }

        self.ensure_lsp_client(LspLanguage::Rust);
        let Some(client) = &mut self.lsp.rust.client else {
            return;
        };

        match client.warm_up() {
            Ok(request_id) => {
                self.record_lsp_request_started(
                    LspLanguage::Rust,
                    request_id,
                    LspRequestKind::WarmUp,
                );
                self.lsp.rust.debug.warmup_count += 1;
                self.lsp.rust.debug.last_status = "warmup_queued".to_owned();
            }
            Err(error) => {
                self.lsp.rust.debug.last_status = "warmup_queue_failed".to_owned();
                self.lsp.rust.debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn workspace_has_rust_source(&self) -> bool {
        self.file_tree
            .files
            .iter()
            .any(|path| is_rust_path(path) && !is_external_rust_path(path))
    }

    pub(super) fn sync_open_documents_to_lsp(&mut self, language: LspLanguage) {
        let documents = [Pane::Left, Pane::Right]
            .into_iter()
            .filter_map(|pane| {
                let editor = self.editor(pane);
                let path = editor.path()?;
                if !language.syncs_path(path) {
                    return None;
                }
                Some(Document::new(
                    path.to_path_buf(),
                    editor.text.as_str().to_owned(),
                ))
            })
            .collect::<Vec<_>>();

        for document in documents {
            self.sync_document_to_lsp(language, document);
        }
    }

    pub(super) fn sync_document_to_lsp(&mut self, language: LspLanguage, document: Document) {
        let text_hash = stable_hash(document.text.as_str());
        if self.lsp_mut(language).synced_hashes.get(&document.path) == Some(&text_hash) {
            return;
        }

        self.ensure_lsp_client(language);
        let result = self
            .lsp_mut(language)
            .client
            .as_mut()
            .expect("LSP client is initialized")
            .sync_document(document.clone());

        match result {
            Ok(request_id) => {
                self.lsp_mut(language)
                    .synced_hashes
                    .insert(document.path.clone(), text_hash);
                self.record_lsp_request_started(language, request_id, LspRequestKind::Sync);
                self.lsp_mut(language).debug.sync_count += 1;
                self.lsp_mut(language).debug.last_status =
                    format!("sync_queued {}", document.path.display());
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_status = "sync_queue_failed".to_owned();
                self.lsp_mut(language).debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn record_lsp_request_started(
        &mut self,
        language: LspLanguage,
        request_id: u64,
        kind: LspRequestKind,
    ) -> Instant {
        let started_at = Instant::now();
        self.lsp_mut(language)
            .request_started_at
            .insert(request_id, started_at);
        self.lsp_mut(language).debug.last_request_id = Some(request_id);
        self.lsp_mut(language).debug.last_kind = Some(kind);
        started_at
    }

    pub(super) fn request_lsp_definition(
        &mut self,
        language: LspLanguage,
        document: Document,
        position: Position,
        identifier: String,
    ) -> bool {
        self.ensure_lsp_client(language);
        let origin = self.focused_editor().navigation_point();

        let Some(client) = &mut self.lsp_mut(language).client else {
            return false;
        };
        match client.request_definition(document.clone(), position) {
            Ok(request_id) => {
                let started_at = self.record_lsp_request_started(
                    language,
                    request_id,
                    LspRequestKind::Definition,
                );
                self.lsp_mut(language).pending = Some(PendingLspRequest {
                    request_id,
                    kind: LspRequestKind::Definition,
                    document,
                    position,
                    identifier: identifier.clone(),
                    origin,
                    started_at,
                });
                self.lsp.definition_candidates.clear();
                self.lsp.location_list_kind = LocationListKind::Definitions;
                self.lsp_mut(language).debug.definition_count += 1;
                self.lsp_mut(language).debug.last_status = "definition_queued".to_owned();
                self.lsp_mut(language).debug.record_definition_status(
                    "definition_queued",
                    None,
                    None,
                    0,
                );
                self.status_message = format!(
                    "Resolving `{identifier}` with {}...",
                    language.server_name()
                );
                true
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_status = "definition_queue_failed".to_owned();
                self.lsp_mut(language).debug.last_error = Some(error.clone());
                self.lsp_mut(language).debug.record_definition_status(
                    "definition_queue_failed",
                    Some(error.clone()),
                    None,
                    0,
                );
                self.status_message = format!("{} LSP unavailable. ({error})", language.label());
                false
            }
        }
    }

    pub(super) fn request_lsp_references(
        &mut self,
        language: LspLanguage,
        pending: PendingLspRequest,
    ) -> bool {
        let Some(client) = &mut self.lsp_mut(language).client else {
            return false;
        };
        match client.request_references(pending.document.clone(), pending.position) {
            Ok(request_id) => {
                let started_at = self.record_lsp_request_started(
                    language,
                    request_id,
                    LspRequestKind::References,
                );
                let identifier = pending.identifier.clone();
                self.lsp_mut(language).pending = Some(PendingLspRequest {
                    request_id,
                    kind: LspRequestKind::References,
                    document: pending.document,
                    position: pending.position,
                    identifier: identifier.clone(),
                    origin: pending.origin,
                    started_at,
                });
                self.lsp_mut(language).debug.reference_count += 1;
                self.lsp_mut(language).debug.last_status = "references_queued".to_owned();
                self.status_message = format!(
                    "Finding references for `{identifier}` with {}...",
                    language.server_name()
                );
                true
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_status = "references_queue_failed".to_owned();
                self.lsp_mut(language).debug.last_error = Some(error.clone());
                self.status_message =
                    format!("{} references request failed. ({error})", language.label());
                false
            }
        }
    }

    pub(super) fn poll_lsp(&mut self, language: LspLanguage, context: &egui::Context) {
        self.sync_open_documents_to_lsp(language);

        let mut responses = Vec::new();
        let session = self.lsp_mut(language);
        if let Some(client) = &session.client {
            while let Some(event) = client.try_recv() {
                match event {
                    LspClientEvent::Response(response) => responses.push(response),
                    LspClientEvent::ServerEvent(event) => {
                        session.debug.record_server_event(&event);
                    }
                }
            }
        }

        for response in responses {
            self.handle_lsp_response(language, response);
        }

        let Some(pending) = self.lsp_mut(language).pending.clone() else {
            return;
        };
        if pending.started_at.elapsed() < LSP_REQUEST_TIMEOUT {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        }

        self.lsp_mut(language).pending = None;
        self.lsp_mut(language)
            .request_started_at
            .remove(&pending.request_id);
        self.lsp_mut(language).debug.timeout_count += 1;
        self.lsp_mut(language).debug.last_duration_ms =
            Some(pending.started_at.elapsed().as_secs_f64() * 1000.0);
        match pending.kind {
            LspRequestKind::WarmUp => {}
            LspRequestKind::Definition => {
                self.lsp_mut(language).debug.last_status = "definition_timeout".to_owned();
                let debug = &mut self.lsp_mut(language).debug;
                debug.record_definition_status(
                    "definition_timeout",
                    None,
                    debug.last_duration_ms,
                    0,
                );
                self.lsp.definition_candidates.clear();
                self.lsp.location_list_kind = LocationListKind::Definitions;
                self.status_message = match language {
                    LspLanguage::Go => format!(
                        "gopls timed out for `{}`. Wait a moment and retry F12.",
                        pending.identifier
                    ),
                    LspLanguage::Rust => self.rust_lsp_definition_pending_message(
                        &pending.identifier,
                        "did not answer before timeout",
                    ),
                };
            }
            LspRequestKind::References => {
                self.lsp.definition_candidates.clear();
                self.close_references_overlay();
                self.lsp.location_list_kind = LocationListKind::References;
                self.lsp_mut(language).debug.last_status = "references_timeout".to_owned();
                self.status_message = format!(
                    "{} references timed out for `{}`.",
                    language.server_name(),
                    pending.identifier
                );
            }
            LspRequestKind::Sync => {}
        }
    }

    pub(super) fn handle_lsp_response(
        &mut self,
        language: LspLanguage,
        response: LspLocationResponse,
    ) {
        if response.kind == LspRequestKind::WarmUp {
            self.handle_lsp_warmup_response(language, response);
            return;
        }
        if response.kind == LspRequestKind::Sync {
            self.handle_lsp_sync_response(language, response);
            return;
        }

        let Some(pending) = self.lsp_mut(language).pending.clone() else {
            return;
        };
        if response.request_id != pending.request_id || response.kind != pending.kind {
            return;
        }
        self.lsp_mut(language).pending = None;
        let duration_ms = self
            .lsp_mut(language)
            .request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.lsp_mut(language).debug.last_duration_ms = duration_ms;

        let locations = match response.result {
            Ok(locations) => {
                self.lsp_mut(language).debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_ok",
                    LspRequestKind::Definition => "definition_ok",
                    LspRequestKind::References => "references_ok",
                    LspRequestKind::Sync => "sync_ok",
                }
                .to_owned();
                self.lsp_mut(language).debug.last_error = None;
                self.lsp_mut(language).debug.last_location_count = locations.len();
                if response.kind == LspRequestKind::Definition {
                    self.lsp_mut(language).debug.record_definition_status(
                        "definition_ok",
                        None,
                        duration_ms,
                        locations.len(),
                    );
                }
                locations
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_error = Some(error.clone());
                self.lsp_mut(language).debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_failed",
                    LspRequestKind::Definition => "definition_failed",
                    LspRequestKind::References => "references_failed",
                    LspRequestKind::Sync => "sync_failed",
                }
                .to_owned();
                if pending.kind == LspRequestKind::Definition {
                    self.lsp_mut(language).debug.record_definition_status(
                        "definition_failed",
                        Some(error.clone()),
                        duration_ms,
                        0,
                    );
                }
                self.lsp.definition_candidates.clear();
                self.close_references_overlay();
                self.lsp.location_list_kind = LocationListKind::Definitions;
                self.status_message = format!(
                    "{} LSP failed for `{}`. ({error})",
                    language.label(),
                    pending.identifier
                );
                return;
            }
        };

        match pending.kind {
            LspRequestKind::Definition => {
                if language == LspLanguage::Rust && locations.is_empty() {
                    self.lsp_mut(language).debug.last_status = "definition_empty".to_owned();
                    self.lsp_mut(language).debug.record_definition_status(
                        "definition_empty",
                        None,
                        duration_ms,
                        0,
                    );
                    self.lsp.definition_candidates.clear();
                    self.lsp.location_list_kind = LocationListKind::Definitions;
                    self.status_message = self.rust_lsp_definition_pending_message(
                        &pending.identifier,
                        "returned no definition",
                    );
                    return;
                }
                if locations.len() == 1
                    && location_contains_position(
                        &locations[0],
                        &pending.document.path,
                        pending.position,
                    )
                    && self.request_lsp_references(language, pending.clone())
                {
                    return;
                }
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::Definitions,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::References => {
                self.handle_locations_with_origin(
                    &pending.identifier,
                    locations,
                    LocationListKind::References,
                    pending.origin.clone(),
                );
            }
            LspRequestKind::WarmUp => {}
            LspRequestKind::Sync => {}
        }
    }

    pub(super) fn handle_lsp_warmup_response(
        &mut self,
        language: LspLanguage,
        response: LspLocationResponse,
    ) {
        self.lsp_mut(language).debug.last_duration_ms = self
            .lsp_mut(language)
            .request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.lsp_mut(language).debug.last_request_id = Some(response.request_id);
        self.lsp_mut(language).debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.lsp_mut(language).debug.last_status = "warmup_ok".to_owned();
                self.lsp_mut(language).debug.last_error = None;
                self.lsp_mut(language).debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_status = "warmup_failed".to_owned();
                self.lsp_mut(language).debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn handle_lsp_sync_response(
        &mut self,
        language: LspLanguage,
        response: LspLocationResponse,
    ) {
        self.lsp_mut(language).debug.last_duration_ms = self
            .lsp_mut(language)
            .request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.lsp_mut(language).debug.last_request_id = Some(response.request_id);
        self.lsp_mut(language).debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.lsp_mut(language).debug.last_status = "sync_ok".to_owned();
                self.lsp_mut(language).debug.last_error = None;
                self.lsp_mut(language).debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.lsp_mut(language).debug.last_status = "sync_failed".to_owned();
                self.lsp_mut(language).debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn rust_lsp_definition_pending_message(
        &self,
        identifier: &str,
        reason: &str,
    ) -> String {
        let mut message = format!(
            "rust-analyzer {reason} for `{identifier}`. It may still be indexing workspace/dependencies; wait a moment and retry F12."
        );
        if !self.lsp.rust.debug.initialized {
            message.push_str(" Current state: starting.");
        } else if let Some(server_message) = &self.lsp.rust.debug.last_server_message
            && !server_message.is_empty()
        {
            message.push_str(&format!(" Last server message: {server_message}"));
        }
        message
    }

    pub(super) fn rust_lsp_readiness_status_message(&self) -> Option<String> {
        self.lsp.rust.client.as_ref()?;
        if !self.has_open_workspace_rust_document() && self.lsp.rust.pending.is_none() {
            return None;
        }

        let status = self.rust_lsp_readiness_status();
        Some(format!("rust-analyzer {}", status.status_bar_label()))
    }

    pub(super) fn rust_lsp_readiness_status(&self) -> RustLspReadinessStatus {
        if self.lsp.rust.client.is_none() {
            return RustLspReadinessStatus::NotStarted;
        }

        if self.lsp.rust.debug.reader_error_count > 0
            && self.lsp.rust.debug.last_status == "reader_error"
        {
            return RustLspReadinessStatus::Error;
        }

        if !self.lsp.rust.debug.initialized {
            return RustLspReadinessStatus::WarmingUp("starting");
        }

        if self.lsp.rust.debug.active_progress_count > 0 {
            return RustLspReadinessStatus::WarmingUp("indexing");
        }

        if self.lsp.rust.debug.last_definition_success_at.is_some() {
            return RustLspReadinessStatus::Ready("definition");
        }

        if self.lsp.rust.debug.last_diagnostics_at.is_some() {
            return RustLspReadinessStatus::Ready("diagnostics");
        }

        if self.lsp.rust.debug.sync_count > 0 || self.lsp.rust.debug.warmup_count > 0 {
            return RustLspReadinessStatus::WarmingUp("workspace/dependencies");
        }

        RustLspReadinessStatus::WarmingUp("initializing")
    }

    pub(super) fn rust_lsp_readiness_report_line(&self) -> String {
        format!(
            "rust_lsp_readiness={} active_progress={} started_age_ms={} initialized_age_ms={} last_progress_age_ms={} last_diagnostics_age_ms={} last_definition_success_age_ms={}",
            self.rust_lsp_readiness_status().report_label(),
            self.lsp.rust.debug.active_progress_count,
            debug_instant_age_ms(self.lsp.rust.debug.started_at),
            debug_instant_age_ms(self.lsp.rust.debug.initialized_at),
            debug_instant_age_ms(self.lsp.rust.debug.last_progress_at),
            debug_instant_age_ms(self.lsp.rust.debug.last_diagnostics_at),
            debug_instant_age_ms(self.lsp.rust.debug.last_definition_success_at)
        )
    }

    pub(super) fn has_open_workspace_rust_document(&self) -> bool {
        [Pane::Left, Pane::Right].into_iter().any(|pane| {
            self.editor(pane)
                .path()
                .is_some_and(|path| is_rust_path(path) && !is_external_rust_path(path))
        })
    }

    pub(super) fn handle_locations_with_origin(
        &mut self,
        identifier: &str,
        locations: Vec<Location>,
        kind: LocationListKind,
        origin: Option<NavigationPoint>,
    ) {
        self.lsp.location_list_kind = kind;
        match locations.len() {
            0 => {
                self.lsp.definition_candidates.clear();
                self.close_references_overlay();
                self.status_message =
                    format!("No {} found for `{identifier}`.", kind.singular_label());
            }
            1 => {
                self.lsp.definition_candidates.clear();
                self.close_references_overlay();
                self.open_definition_location_with_origin(locations[0].clone(), origin);
                self.status_message =
                    format!("Jumped to {} of `{identifier}`.", kind.singular_label());
            }
            _ => {
                self.lsp.definition_candidates = locations
                    .into_iter()
                    .map(|location| {
                        let preview = (kind == LocationListKind::References)
                            .then(|| location_line_preview(&location))
                            .flatten();
                        DefinitionCandidate {
                            label: location_result_label_with_preview(
                                &location,
                                kind,
                                preview.as_deref(),
                            ),
                            location,
                            preview,
                        }
                    })
                    .collect();
                if kind == LocationListKind::References {
                    self.open_references_overlay(identifier);
                    self.status_message = format!(
                        "Found {} references for `{identifier}`.",
                        self.lsp.definition_candidates.len()
                    );
                } else {
                    self.close_references_overlay();
                    self.status_message = format!(
                        "Multiple definitions returned for `{identifier}`. Use Report for details."
                    );
                }
            }
        }
    }

    pub(super) fn open_definition_location(&mut self, location: Location) {
        self.open_definition_location_with_origin(location, None);
    }

    pub(super) fn open_definition_location_with_origin(
        &mut self,
        location: Location,
        origin: Option<NavigationPoint>,
    ) {
        self.open_location_in_focused_pane_with_origin(location, true, origin);
    }

    pub(super) fn open_location_in_focused_pane(&mut self, location: Location, push_history: bool) {
        self.open_location_in_focused_pane_with_origin(location, push_history, None);
    }

    pub(super) fn open_location_in_focused_pane_with_origin(
        &mut self,
        location: Location,
        push_history: bool,
        origin: Option<NavigationPoint>,
    ) {
        let history_point = origin.or_else(|| self.focused_editor().navigation_point());
        if let Some(current) = history_point
            && push_history
        {
            self.push_focused_navigation_history(current);
        }

        self.open_navigation_point_in_focused_pane(NavigationPoint {
            path: location.path,
            position: location.range.start,
        });
    }
}

pub(super) const LSP_REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);

pub(super) fn location_contains_position(
    location: &Location,
    path: &Path,
    position: Position,
) -> bool {
    paths_match(&location.path, path) && range_contains_position(location.range, position)
}

pub(super) fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(super) fn range_contains_position(range: Range, position: Position) -> bool {
    if position.line < range.start.line || position.line > range.end.line {
        return false;
    }
    if position.line == range.start.line && position.character < range.start.character {
        return false;
    }
    if position.line == range.end.line && position.character > range.end.character {
        return false;
    }
    true
}

#[derive(Debug, Clone)]
pub(super) struct DefinitionCandidate {
    pub(super) label: String,
    pub(super) location: Location,
    pub(super) preview: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct LspDebugState {
    pub(super) last_status: String,
    pub(super) last_error: Option<String>,
    pub(super) last_kind: Option<LspRequestKind>,
    pub(super) last_request_id: Option<u64>,
    pub(super) last_duration_ms: Option<f64>,
    pub(super) last_location_count: usize,
    pub(super) last_definition_status: String,
    pub(super) last_definition_error: Option<String>,
    pub(super) last_definition_duration_ms: Option<f64>,
    pub(super) last_definition_location_count: usize,
    pub(super) warmup_count: usize,
    pub(super) sync_count: usize,
    pub(super) definition_count: usize,
    pub(super) reference_count: usize,
    pub(super) fallback_count: usize,
    pub(super) timeout_count: usize,
    pub(super) initialized: bool,
    pub(super) started_at: Option<Instant>,
    pub(super) initialized_at: Option<Instant>,
    pub(super) last_progress_at: Option<Instant>,
    pub(super) last_diagnostics_at: Option<Instant>,
    pub(super) last_definition_success_at: Option<Instant>,
    pub(super) active_progress_count: usize,
    pub(super) server_request_count: usize,
    pub(super) server_notification_count: usize,
    pub(super) reader_error_count: usize,
    pub(super) progress_count: usize,
    pub(super) diagnostics_count: usize,
    pub(super) last_server_kind: String,
    pub(super) last_server_method: String,
    pub(super) last_server_message: Option<String>,
}

impl LspDebugState {
    pub(super) fn mark_started(&mut self) {
        let now = Instant::now();
        self.started_at = Some(now);
        self.last_status = "starting".to_owned();
    }

    pub(super) fn record_definition_status(
        &mut self,
        status: &str,
        error: Option<String>,
        duration_ms: Option<f64>,
        location_count: usize,
    ) {
        self.last_definition_status = status.to_owned();
        self.last_definition_error = error;
        self.last_definition_duration_ms = duration_ms;
        self.last_definition_location_count = location_count;
        if status == "definition_ok" && location_count > 0 {
            self.last_definition_success_at = Some(Instant::now());
        }
    }

    pub(super) fn record_server_event(&mut self, event: &LspServerEvent) {
        let now = Instant::now();
        match event.kind {
            LspServerEventKind::Request => self.server_request_count += 1,
            LspServerEventKind::Notification => self.server_notification_count += 1,
            LspServerEventKind::ReaderError => self.reader_error_count += 1,
            LspServerEventKind::Initialized => {
                self.initialized = true;
                self.initialized_at = Some(now);
            }
        }
        if event.method == "$/progress" {
            self.progress_count += 1;
            self.last_progress_at = Some(now);
            if event
                .message
                .as_deref()
                .is_some_and(|message| message.contains("kind=begin"))
            {
                self.active_progress_count += 1;
            } else if event
                .message
                .as_deref()
                .is_some_and(|message| message.contains("kind=end"))
            {
                self.active_progress_count = self.active_progress_count.saturating_sub(1);
            }
        }
        if let Some(count) = event.diagnostics_count {
            self.diagnostics_count = count;
            self.last_diagnostics_at = Some(now);
        }
        self.last_server_kind = format!("{:?}", event.kind);
        self.last_server_method = event.method.clone();
        self.last_server_message = event.message.clone();
        if event.kind == LspServerEventKind::ReaderError {
            self.last_error = event.message.clone();
            self.last_status = "reader_error".to_owned();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RustLspReadinessStatus {
    NotStarted,
    WarmingUp(&'static str),
    Ready(&'static str),
    Error,
}

impl RustLspReadinessStatus {
    pub(super) fn status_bar_label(self) -> String {
        match self {
            Self::NotStarted => "not started".to_owned(),
            Self::WarmingUp(reason) => format!("warming up... ({reason})"),
            Self::Ready(reason) => format!("ready ({reason})"),
            Self::Error => "error".to_owned(),
        }
    }

    pub(super) fn report_label(self) -> String {
        match self {
            Self::NotStarted => "not_started".to_owned(),
            Self::WarmingUp(reason) => format!("warming_up reason={reason}"),
            Self::Ready(reason) => format!("ready reason={reason}"),
            Self::Error => "error".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocationListKind {
    Definitions,
    References,
}

impl LocationListKind {
    pub(super) fn report_title(self) -> &'static str {
        match self {
            Self::Definitions => "Definition locations",
            Self::References => "References",
        }
    }

    pub(super) fn singular_label(self) -> &'static str {
        match self {
            Self::Definitions => "definition",
            Self::References => "reference",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LspLanguage {
    Go,
    Rust,
}

impl LspLanguage {
    fn server_name(self) -> &'static str {
        match self {
            Self::Go => "gopls",
            Self::Rust => "rust-analyzer",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Go => "Go",
            Self::Rust => "Rust",
        }
    }

    fn config(self, roots: Vec<PathBuf>) -> LspServerConfig {
        match self {
            Self::Go => LspServerConfig::go(roots),
            Self::Rust => LspServerConfig::rust_analyzer(roots),
        }
    }

    /// Open documents this server gets told about. rust-analyzer already knows the
    /// read-only sources outside the workspace.
    fn syncs_path(self, path: &Path) -> bool {
        match self {
            Self::Go => is_go_path(path),
            Self::Rust => is_rust_path(path) && !is_external_rust_path(path),
        }
    }
}

/// One language server: its client and what is in flight.
#[derive(Default)]
pub(super) struct LspSession {
    pub(super) client: Option<LspClient>,
    pub(super) pending: Option<PendingLspRequest>,
    pub(super) synced_hashes: HashMap<PathBuf, u64>,
    pub(super) request_started_at: HashMap<u64, Instant>,
    pub(super) debug: LspDebugState,
}

#[derive(Debug, Clone)]
pub(super) struct PendingLspRequest {
    pub(super) request_id: u64,
    pub(super) kind: LspRequestKind,
    pub(super) document: Document,
    pub(super) position: Position,
    pub(super) identifier: String,
    pub(super) origin: Option<NavigationPoint>,
    pub(super) started_at: Instant,
}
