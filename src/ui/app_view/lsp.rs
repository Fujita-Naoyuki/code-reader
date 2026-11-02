//! Go to Definition and references through rust-analyzer and gopls: starting clients,
//! syncing open documents, requests, responses and readiness messages.

use super::*;

impl CodeReaderApp {
    pub(super) fn go_to_definition(&mut self) {
        let editor = self.focused_editor();
        let Some(path) = editor.path.clone() else {
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
        self.definition_candidates.clear();
        self.location_list_kind = LocationListKind::Definitions;
        self.close_references_overlay();
        // Every registered language resolves definitions through its language server.
        match language.id().0 {
            "go" => {
                self.request_go_lsp_definition(document, position, identifier);
            }
            "rust" => {
                self.request_rust_lsp_definition(document, position, identifier);
            }
            other => {
                self.status_message = format!("Go to Definition is not available for {other}.");
            }
        }
    }

    pub(super) fn ensure_go_lsp_client(&mut self) {
        if self.go_lsp_client.is_some() {
            return;
        }
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.go_lsp_client = Some(platform::current().start_lsp(LspServerConfig::go(roots)));
        self.go_lsp_debug.last_status = "starting".to_owned();
    }

    pub(super) fn sync_open_go_documents_to_lsp(&mut self) {
        let documents = [Pane::Left, Pane::Right]
            .into_iter()
            .filter_map(|pane| {
                let editor = self.editor(pane);
                let path = editor.path.as_ref()?;
                if !is_go_path(path) {
                    return None;
                }
                Some(Document::new(path.clone(), editor.text.as_str().to_owned()))
            })
            .collect::<Vec<_>>();

        for document in documents {
            self.sync_go_document_to_lsp(document);
        }
    }

    pub(super) fn sync_go_document_to_lsp(&mut self, document: Document) {
        let text_hash = stable_hash(document.text.as_str());
        if self.go_lsp_synced_hashes.get(&document.path) == Some(&text_hash) {
            return;
        }

        self.ensure_go_lsp_client();
        let result = self
            .go_lsp_client
            .as_mut()
            .expect("Go LSP client is initialized")
            .sync_document(document.clone());

        match result {
            Ok(request_id) => {
                self.go_lsp_synced_hashes
                    .insert(document.path.clone(), text_hash);
                self.record_go_lsp_request_started(request_id, LspRequestKind::Sync);
                self.go_lsp_debug.sync_count += 1;
                self.go_lsp_debug.last_status = format!("sync_queued {}", document.path.display());
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "sync_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn record_go_lsp_request_started(
        &mut self,
        request_id: u64,
        kind: LspRequestKind,
    ) -> Instant {
        let started_at = Instant::now();
        self.go_lsp_request_started_at
            .insert(request_id, started_at);
        self.go_lsp_debug.last_request_id = Some(request_id);
        self.go_lsp_debug.last_kind = Some(kind);
        started_at
    }

    pub(super) fn request_go_lsp_definition(
        &mut self,
        document: Document,
        position: Position,
        identifier: String,
    ) -> bool {
        self.ensure_go_lsp_client();
        let origin = self.focused_editor().navigation_point();

        let Some(client) = &mut self.go_lsp_client else {
            return false;
        };
        match client.request_definition(document.clone(), position) {
            Ok(request_id) => {
                let started_at =
                    self.record_go_lsp_request_started(request_id, LspRequestKind::Definition);
                self.pending_go_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::Definition,
                    document,
                    position,
                    identifier: identifier.clone(),
                    origin,
                    started_at,
                });
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.go_lsp_debug.definition_count += 1;
                self.go_lsp_debug.last_status = "definition_queued".to_owned();
                self.go_lsp_debug
                    .record_definition_status("definition_queued", None, None, 0);
                self.status_message = format!("Resolving `{identifier}` with gopls...");
                true
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "definition_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error.clone());
                self.go_lsp_debug.record_definition_status(
                    "definition_queue_failed",
                    Some(error.clone()),
                    None,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = format!("Go LSP unavailable. ({error})");
                false
            }
        }
    }

    pub(super) fn request_go_lsp_references(&mut self, pending: PendingGoLspRequest) -> bool {
        let Some(client) = &mut self.go_lsp_client else {
            return false;
        };
        match client.request_references(pending.document.clone(), pending.position) {
            Ok(request_id) => {
                let started_at =
                    self.record_go_lsp_request_started(request_id, LspRequestKind::References);
                let identifier = pending.identifier.clone();
                self.pending_go_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::References,
                    document: pending.document,
                    position: pending.position,
                    identifier: identifier.clone(),
                    origin: pending.origin,
                    started_at,
                });
                self.go_lsp_debug.reference_count += 1;
                self.go_lsp_debug.last_status = "references_queued".to_owned();
                self.status_message =
                    format!("Finding references for `{identifier}` with gopls...");
                true
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "references_queue_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error.clone());
                self.status_message = format!("Go references request failed. ({error})");
                false
            }
        }
    }

    pub(super) fn poll_go_lsp(&mut self, context: &egui::Context) {
        self.sync_open_go_documents_to_lsp();

        let mut responses = Vec::new();
        if let Some(client) = &self.go_lsp_client {
            while let Some(event) = client.try_recv() {
                match event {
                    LspClientEvent::Response(response) => responses.push(response),
                    LspClientEvent::ServerEvent(event) => {
                        self.go_lsp_debug.record_server_event(&event);
                    }
                }
            }
        }

        for response in responses {
            self.handle_go_lsp_response(response);
        }

        let Some(pending) = self.pending_go_lsp_request.clone() else {
            return;
        };
        if pending.started_at.elapsed() < GO_LSP_REQUEST_TIMEOUT {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        }

        self.pending_go_lsp_request = None;
        self.go_lsp_request_started_at.remove(&pending.request_id);
        self.go_lsp_debug.timeout_count += 1;
        self.go_lsp_debug.last_duration_ms =
            Some(pending.started_at.elapsed().as_secs_f64() * 1000.0);
        match pending.kind {
            LspRequestKind::WarmUp => {}
            LspRequestKind::Definition => {
                self.go_lsp_debug.last_status = "definition_timeout".to_owned();
                self.go_lsp_debug.last_location_count = 0;
                self.go_lsp_debug.record_definition_status(
                    "definition_timeout",
                    None,
                    self.go_lsp_debug.last_duration_ms,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = format!(
                    "gopls timed out for `{}`. Wait a moment and retry F12.",
                    pending.identifier
                );
            }
            LspRequestKind::References => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::References;
                self.go_lsp_debug.last_status = "references_timeout".to_owned();
                self.status_message =
                    format!("gopls references timed out for `{}`.", pending.identifier);
            }
            LspRequestKind::Sync => {}
        }
    }

    pub(super) fn handle_go_lsp_response(&mut self, response: LspLocationResponse) {
        if response.kind == LspRequestKind::Sync {
            self.handle_go_lsp_sync_response(response);
            return;
        }

        let Some(pending) = self.pending_go_lsp_request.clone() else {
            return;
        };
        if response.request_id != pending.request_id || response.kind != pending.kind {
            return;
        }
        self.pending_go_lsp_request = None;
        let duration_ms = self
            .go_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.go_lsp_debug.last_duration_ms = duration_ms;

        let locations = match response.result {
            Ok(locations) => {
                self.go_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_ok",
                    LspRequestKind::Definition => "definition_ok",
                    LspRequestKind::References => "references_ok",
                    LspRequestKind::Sync => "sync_ok",
                }
                .to_owned();
                self.go_lsp_debug.last_error = None;
                self.go_lsp_debug.last_location_count = locations.len();
                if response.kind == LspRequestKind::Definition {
                    self.go_lsp_debug.record_definition_status(
                        "definition_ok",
                        None,
                        duration_ms,
                        locations.len(),
                    );
                }
                locations
            }
            Err(error) => {
                self.go_lsp_debug.last_error = Some(error.clone());
                self.go_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_failed",
                    LspRequestKind::Definition => "definition_failed",
                    LspRequestKind::References => "references_failed",
                    LspRequestKind::Sync => "sync_failed",
                }
                .to_owned();
                if pending.kind == LspRequestKind::Definition {
                    self.go_lsp_debug.record_definition_status(
                        "definition_failed",
                        Some(error.clone()),
                        duration_ms,
                        0,
                    );
                }
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message =
                    format!("Go LSP failed for `{}`. ({error})", pending.identifier);
                return;
            }
        };

        match pending.kind {
            LspRequestKind::Definition => {
                if locations.len() == 1
                    && location_contains_position(
                        &locations[0],
                        &pending.document.path,
                        pending.position,
                    )
                    && self.request_go_lsp_references(pending.clone())
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

    pub(super) fn handle_go_lsp_sync_response(&mut self, response: LspLocationResponse) {
        self.go_lsp_debug.last_duration_ms = self
            .go_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.go_lsp_debug.last_request_id = Some(response.request_id);
        self.go_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.go_lsp_debug.last_status = "sync_ok".to_owned();
                self.go_lsp_debug.last_error = None;
                self.go_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.go_lsp_debug.last_status = "sync_failed".to_owned();
                self.go_lsp_debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn ensure_rust_lsp_client(&mut self) {
        if self.rust_lsp_client.is_some() {
            return;
        }
        let roots = self
            .current_session()
            .map(|session| session.root_folders.clone())
            .unwrap_or_default();
        self.rust_lsp_client =
            Some(platform::current().start_lsp(LspServerConfig::rust_analyzer(roots)));
        self.rust_lsp_debug.mark_started();
    }

    pub(super) fn warm_up_rust_lsp_for_workspace(&mut self) {
        if !self.workspace_has_rust_source() {
            return;
        }

        self.ensure_rust_lsp_client();
        let Some(client) = &mut self.rust_lsp_client else {
            return;
        };

        match client.warm_up() {
            Ok(request_id) => {
                self.record_rust_lsp_request_started(request_id, LspRequestKind::WarmUp);
                self.rust_lsp_debug.warmup_count += 1;
                self.rust_lsp_debug.last_status = "warmup_queued".to_owned();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "warmup_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn workspace_has_rust_source(&self) -> bool {
        self.file_tree
            .files
            .iter()
            .any(|path| is_rust_path(path) && !is_external_rust_path(path))
    }

    pub(super) fn sync_open_rust_documents_to_lsp(&mut self) {
        let documents = [Pane::Left, Pane::Right]
            .into_iter()
            .filter_map(|pane| {
                let editor = self.editor(pane);
                let path = editor.path.as_ref()?;
                if !is_rust_path(path) || is_external_rust_path(path) {
                    return None;
                }
                Some(Document::new(path.clone(), editor.text.as_str().to_owned()))
            })
            .collect::<Vec<_>>();

        for document in documents {
            self.sync_rust_document_to_lsp(document);
        }
    }

    pub(super) fn sync_rust_document_to_lsp(&mut self, document: Document) {
        let text_hash = stable_hash(document.text.as_str());
        if self.rust_lsp_synced_hashes.get(&document.path) == Some(&text_hash) {
            return;
        }

        self.ensure_rust_lsp_client();
        let result = self
            .rust_lsp_client
            .as_mut()
            .expect("Rust LSP client is initialized")
            .sync_document(document.clone());

        match result {
            Ok(request_id) => {
                self.rust_lsp_synced_hashes
                    .insert(document.path.clone(), text_hash);
                self.record_rust_lsp_request_started(request_id, LspRequestKind::Sync);
                self.rust_lsp_debug.sync_count += 1;
                self.rust_lsp_debug.last_status =
                    format!("sync_queued {}", document.path.display());
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "sync_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn record_rust_lsp_request_started(
        &mut self,
        request_id: u64,
        kind: LspRequestKind,
    ) -> Instant {
        let started_at = Instant::now();
        self.rust_lsp_request_started_at
            .insert(request_id, started_at);
        self.rust_lsp_debug.last_request_id = Some(request_id);
        self.rust_lsp_debug.last_kind = Some(kind);
        started_at
    }

    pub(super) fn request_rust_lsp_definition(
        &mut self,
        document: Document,
        position: Position,
        identifier: String,
    ) -> bool {
        self.ensure_rust_lsp_client();
        let origin = self.focused_editor().navigation_point();

        let Some(client) = &mut self.rust_lsp_client else {
            return false;
        };
        match client.request_definition(document.clone(), position) {
            Ok(request_id) => {
                let started_at =
                    self.record_rust_lsp_request_started(request_id, LspRequestKind::Definition);
                self.pending_rust_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::Definition,
                    document,
                    position,
                    identifier: identifier.clone(),
                    origin,
                    started_at,
                });
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.rust_lsp_debug.definition_count += 1;
                self.rust_lsp_debug.last_status = "definition_queued".to_owned();
                self.rust_lsp_debug
                    .record_definition_status("definition_queued", None, None, 0);
                self.status_message = format!("Resolving `{identifier}` with rust-analyzer...");
                true
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "definition_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.rust_lsp_debug.record_definition_status(
                    "definition_queue_failed",
                    Some(error.clone()),
                    None,
                    0,
                );
                self.status_message = format!("Rust LSP unavailable. ({error})");
                false
            }
        }
    }

    pub(super) fn request_rust_lsp_references(&mut self, pending: PendingGoLspRequest) -> bool {
        let Some(client) = &mut self.rust_lsp_client else {
            return false;
        };
        match client.request_references(pending.document.clone(), pending.position) {
            Ok(request_id) => {
                let started_at =
                    self.record_rust_lsp_request_started(request_id, LspRequestKind::References);
                let identifier = pending.identifier.clone();
                self.pending_rust_lsp_request = Some(PendingGoLspRequest {
                    request_id,
                    kind: LspRequestKind::References,
                    document: pending.document,
                    position: pending.position,
                    identifier: identifier.clone(),
                    origin: pending.origin,
                    started_at,
                });
                self.rust_lsp_debug.reference_count += 1;
                self.rust_lsp_debug.last_status = "references_queued".to_owned();
                self.status_message =
                    format!("Finding references for `{identifier}` with rust-analyzer...");
                true
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "references_queue_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.status_message = format!("Rust references request failed. ({error})");
                false
            }
        }
    }

    pub(super) fn poll_rust_lsp(&mut self, context: &egui::Context) {
        self.sync_open_rust_documents_to_lsp();

        let mut responses = Vec::new();
        if let Some(client) = &self.rust_lsp_client {
            while let Some(event) = client.try_recv() {
                match event {
                    LspClientEvent::Response(response) => responses.push(response),
                    LspClientEvent::ServerEvent(event) => {
                        self.rust_lsp_debug.record_server_event(&event);
                    }
                }
            }
        }

        for response in responses {
            self.handle_rust_lsp_response(response);
        }

        let Some(pending) = self.pending_rust_lsp_request.clone() else {
            return;
        };
        if pending.started_at.elapsed() < RUST_LSP_REQUEST_TIMEOUT {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        }

        self.pending_rust_lsp_request = None;
        self.rust_lsp_request_started_at.remove(&pending.request_id);
        self.rust_lsp_debug.timeout_count += 1;
        self.rust_lsp_debug.last_duration_ms =
            Some(pending.started_at.elapsed().as_secs_f64() * 1000.0);
        match pending.kind {
            LspRequestKind::WarmUp => {}
            LspRequestKind::Definition => {
                self.rust_lsp_debug.last_status = "definition_timeout".to_owned();
                self.rust_lsp_debug.record_definition_status(
                    "definition_timeout",
                    None,
                    self.rust_lsp_debug.last_duration_ms,
                    0,
                );
                self.definition_candidates.clear();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message = self.rust_lsp_definition_pending_message(
                    &pending.identifier,
                    "did not answer before timeout",
                );
            }
            LspRequestKind::References => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::References;
                self.rust_lsp_debug.last_status = "references_timeout".to_owned();
                self.status_message = format!(
                    "rust-analyzer references timed out for `{}`.",
                    pending.identifier
                );
            }
            LspRequestKind::Sync => {}
        }
    }

    pub(super) fn handle_rust_lsp_response(&mut self, response: LspLocationResponse) {
        if response.kind == LspRequestKind::WarmUp {
            self.handle_rust_lsp_warmup_response(response);
            return;
        }
        if response.kind == LspRequestKind::Sync {
            self.handle_rust_lsp_sync_response(response);
            return;
        }

        let Some(pending) = self.pending_rust_lsp_request.clone() else {
            return;
        };
        if response.request_id != pending.request_id || response.kind != pending.kind {
            return;
        }
        self.pending_rust_lsp_request = None;
        let duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_duration_ms = duration_ms;

        let locations = match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_ok",
                    LspRequestKind::Definition => "definition_ok",
                    LspRequestKind::References => "references_ok",
                    LspRequestKind::Sync => "sync_ok",
                }
                .to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
                if response.kind == LspRequestKind::Definition {
                    self.rust_lsp_debug.record_definition_status(
                        "definition_ok",
                        None,
                        duration_ms,
                        locations.len(),
                    );
                }
                locations
            }
            Err(error) => {
                self.rust_lsp_debug.last_error = Some(error.clone());
                self.rust_lsp_debug.last_status = match response.kind {
                    LspRequestKind::WarmUp => "warmup_failed",
                    LspRequestKind::Definition => "definition_failed",
                    LspRequestKind::References => "references_failed",
                    LspRequestKind::Sync => "sync_failed",
                }
                .to_owned();
                if pending.kind == LspRequestKind::Definition {
                    self.rust_lsp_debug.record_definition_status(
                        "definition_failed",
                        Some(error.clone()),
                        duration_ms,
                        0,
                    );
                }
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.location_list_kind = LocationListKind::Definitions;
                self.status_message =
                    format!("Rust LSP failed for `{}`. ({error})", pending.identifier);
                return;
            }
        };

        match pending.kind {
            LspRequestKind::Definition => {
                if locations.is_empty() {
                    self.rust_lsp_debug.last_status = "definition_empty".to_owned();
                    self.rust_lsp_debug.record_definition_status(
                        "definition_empty",
                        None,
                        duration_ms,
                        0,
                    );
                    self.definition_candidates.clear();
                    self.location_list_kind = LocationListKind::Definitions;
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
                    && self.request_rust_lsp_references(pending.clone())
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

    pub(super) fn handle_rust_lsp_warmup_response(&mut self, response: LspLocationResponse) {
        self.rust_lsp_debug.last_duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_request_id = Some(response.request_id);
        self.rust_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = "warmup_ok".to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "warmup_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
            }
        }
    }

    pub(super) fn handle_rust_lsp_sync_response(&mut self, response: LspLocationResponse) {
        self.rust_lsp_debug.last_duration_ms = self
            .rust_lsp_request_started_at
            .remove(&response.request_id)
            .map(|started_at| started_at.elapsed().as_secs_f64() * 1000.0);
        self.rust_lsp_debug.last_request_id = Some(response.request_id);
        self.rust_lsp_debug.last_kind = Some(response.kind);
        match response.result {
            Ok(locations) => {
                self.rust_lsp_debug.last_status = "sync_ok".to_owned();
                self.rust_lsp_debug.last_error = None;
                self.rust_lsp_debug.last_location_count = locations.len();
            }
            Err(error) => {
                self.rust_lsp_debug.last_status = "sync_failed".to_owned();
                self.rust_lsp_debug.last_error = Some(error);
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
        if !self.rust_lsp_debug.initialized {
            message.push_str(" Current state: starting.");
        } else if let Some(server_message) = &self.rust_lsp_debug.last_server_message
            && !server_message.is_empty()
        {
            message.push_str(&format!(" Last server message: {server_message}"));
        }
        message
    }

    pub(super) fn status_bar_message(&self) -> String {
        if self.status_message.contains("rust-analyzer") {
            return self.status_message.clone();
        }

        let Some(rust_lsp_status) = self.rust_lsp_readiness_status_message() else {
            return self.status_message.clone();
        };

        if self.status_message.is_empty() {
            rust_lsp_status
        } else {
            format!("{} | {}", self.status_message, rust_lsp_status)
        }
    }

    pub(super) fn rust_lsp_readiness_status_message(&self) -> Option<String> {
        self.rust_lsp_client.as_ref()?;
        if !self.has_open_workspace_rust_document() && self.pending_rust_lsp_request.is_none() {
            return None;
        }

        let status = self.rust_lsp_readiness_status();
        Some(format!("rust-analyzer {}", status.status_bar_label()))
    }

    pub(super) fn rust_lsp_readiness_status(&self) -> RustLspReadinessStatus {
        if self.rust_lsp_client.is_none() {
            return RustLspReadinessStatus::NotStarted;
        }

        if self.rust_lsp_debug.reader_error_count > 0
            && self.rust_lsp_debug.last_status == "reader_error"
        {
            return RustLspReadinessStatus::Error;
        }

        if !self.rust_lsp_debug.initialized {
            return RustLspReadinessStatus::WarmingUp("starting");
        }

        if self.rust_lsp_debug.active_progress_count > 0 {
            return RustLspReadinessStatus::WarmingUp("indexing");
        }

        if self.rust_lsp_debug.last_definition_success_at.is_some() {
            return RustLspReadinessStatus::Ready("definition");
        }

        if self.rust_lsp_debug.last_diagnostics_at.is_some() {
            return RustLspReadinessStatus::Ready("diagnostics");
        }

        if self.rust_lsp_debug.sync_count > 0 || self.rust_lsp_debug.warmup_count > 0 {
            return RustLspReadinessStatus::WarmingUp("workspace/dependencies");
        }

        RustLspReadinessStatus::WarmingUp("initializing")
    }

    pub(super) fn rust_lsp_readiness_report_line(&self) -> String {
        format!(
            "rust_lsp_readiness={} active_progress={} started_age_ms={} initialized_age_ms={} last_progress_age_ms={} last_diagnostics_age_ms={} last_definition_success_age_ms={}",
            self.rust_lsp_readiness_status().report_label(),
            self.rust_lsp_debug.active_progress_count,
            debug_instant_age_ms(self.rust_lsp_debug.started_at),
            debug_instant_age_ms(self.rust_lsp_debug.initialized_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_progress_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_diagnostics_at),
            debug_instant_age_ms(self.rust_lsp_debug.last_definition_success_at)
        )
    }

    pub(super) fn has_open_workspace_rust_document(&self) -> bool {
        [Pane::Left, Pane::Right].into_iter().any(|pane| {
            self.editor(pane)
                .path
                .as_deref()
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
        self.location_list_kind = kind;
        match locations.len() {
            0 => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.status_message =
                    format!("No {} found for `{identifier}`.", kind.singular_label());
            }
            1 => {
                self.definition_candidates.clear();
                self.close_references_overlay();
                self.open_definition_location_with_origin(locations[0].clone(), origin);
                self.status_message =
                    format!("Jumped to {} of `{identifier}`.", kind.singular_label());
            }
            _ => {
                self.definition_candidates = locations
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
                        self.definition_candidates.len()
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

pub(super) const GO_LSP_REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);

pub(super) const RUST_LSP_REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);

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

#[derive(Debug, Clone)]
pub(super) struct PendingGoLspRequest {
    pub(super) request_id: u64,
    pub(super) kind: LspRequestKind,
    pub(super) document: Document,
    pub(super) position: Position,
    pub(super) identifier: String,
    pub(super) origin: Option<NavigationPoint>,
    pub(super) started_at: Instant,
}
