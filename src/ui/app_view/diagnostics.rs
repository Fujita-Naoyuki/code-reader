//! The diagnostics report (the Report button) and app-level performance samples.

use super::*;

impl CodeReaderApp {
    pub(super) fn copy_diagnostics_report(&mut self) {
        let report = self.diagnostics_report_text();
        match copy_text_to_system_clipboard(&report) {
            Ok(()) => {
                self.status_message = "Copied diagnostics report.".to_owned();
            }
            Err(error) => {
                self.status_message = format!("Diagnostics report copy failed: {error}");
            }
        }
    }

    pub(super) fn perf_report_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "CodeReader perf report: build={} sidebar={} right_pane={} focused={}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            self.show_workspace_sidebar,
            self.show_right_pane,
            self.focused_pane.label()
        ));
        lines.push(format!(
            "workspace_files={} symbols={} git_repositories={}",
            self.file_tree.files.len(),
            self.symbol_index.len(),
            self.git_repositories.len()
        ));
        lines.push(format!(
            "app_frame: frame={} total_ms={:.2} avg120_ms={:.2} max120_ms={:.2} stable_dt_ms={:.2} unstable_dt_ms={:.2} focused={} scroll_y={:.1}->{:.1} delta_y={:.1} raw_wheel=({:.1},{:.1}) smooth_scroll=({:.1},{:.1}) page_key={:?}",
            self.app_perf_debug.frame_index,
            self.app_perf_debug.last_total_ms,
            self.app_perf_debug.avg_total_ms,
            self.app_perf_debug.max_total_ms,
            self.app_perf_debug.last_stable_dt_ms,
            self.app_perf_debug.last_unstable_dt_ms,
            self.app_perf_debug.focused_pane.label(),
            self.app_perf_debug.scroll_before_y,
            self.app_perf_debug.scroll_after_y,
            self.app_perf_debug.scroll_delta_y,
            self.app_perf_debug.raw_wheel_delta.x,
            self.app_perf_debug.raw_wheel_delta.y,
            self.app_perf_debug.smooth_scroll_delta.x,
            self.app_perf_debug.smooth_scroll_delta.y,
            self.app_perf_debug.page_key
        ));
        for pane in [Pane::Left, Pane::Right] {
            let editor = self.editor(pane);
            if let Some(perf) = &editor.debug.perf {
                lines.push(perf.report_line(pane, &editor.title));
            } else {
                lines.push(format!("{}: no perf sample", pane.label()));
            }
            lines.extend(editor.debug.virtual_click.report_lines(pane));
        }
        lines.join("\n")
    }

    pub(super) fn diagnostics_report_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(self.perf_report_text());
        lines.push(String::new());
        lines.push("App details:".to_owned());
        lines.push(format!("status={}", self.status_message));
        lines.push(format!(
            "sidebar_view={} tab_width={} indexing={}",
            self.sidebar_view.label(),
            self.tab_width,
            self.symbol_index_receiver.is_some()
        ));
        lines.push(lsp_report_line("go_lsp", &self.go_lsp));
        lines.push(lsp_report_line("rust_lsp", &self.rust_lsp));
        lines.push(self.rust_lsp_readiness_report_line());
        match self.current_session() {
            Some(session) => {
                lines.push(format!(
                    "session={} roots={}",
                    session.display_name,
                    session.root_folders.len()
                ));
                for root in &session.root_folders {
                    lines.push(format!("  root={}", root.display()));
                }
            }
            None => lines.push("session=none".to_owned()),
        }
        match self.cli_agent_command() {
            Ok(command) => {
                lines.push(format!(
                    "ai_command: agent={} base_dir={}",
                    command.agent_name,
                    command.base_dir.display()
                ));
                lines.push(format!("ai_command_text={}", command.command));
            }
            Err(error) => {
                lines.push(format!("ai_command_error={error}"));
            }
        }

        lines.push(String::new());
        lines.push("Editor details:".to_owned());
        lines.extend(self.left.debug_report_lines(self.tab_width));
        lines.extend(self.right.debug_report_lines(self.tab_width));

        lines.push(String::new());
        lines.push(format!(
            "{}: {}",
            self.location_list_kind.report_title(),
            self.definition_candidates.len()
        ));
        if self.definition_candidates.is_empty() {
            lines.push("  none".to_owned());
        } else {
            for candidate in &self.definition_candidates {
                lines.push(format!(
                    "  {} -> {}:{}:{}",
                    candidate.label,
                    candidate.location.path.display(),
                    candidate.location.range.start.line + 1,
                    candidate.location.range.start.character + 1
                ));
            }
        }

        lines.push(String::new());
        self.append_symbol_report_lines(&mut lines);

        lines.join("\n")
    }

    pub(super) fn append_symbol_report_lines(&self, lines: &mut Vec<String>) {
        const TOP_FILE_LIMIT: usize = 12;

        let mut source_counts: HashMap<&'static str, usize> = HashMap::new();
        let mut kind_counts: HashMap<String, usize> = HashMap::new();
        let mut file_counts: HashMap<PathBuf, usize> = HashMap::new();

        for symbol in self.symbol_index.symbols() {
            let source = match symbol.source {
                SymbolSource::Workspace => "workspace",
                SymbolSource::ExternalLibrary => "external",
            };
            *source_counts.entry(source).or_default() += 1;
            *kind_counts
                .entry(format!("{source}:{:?}", symbol.kind))
                .or_default() += 1;
            *file_counts.entry(symbol.location.path.clone()).or_default() += 1;
        }

        lines.push(format!("Symbol index: total={}", self.symbol_index.len()));
        lines.push(format!(
            "  by_source workspace={} external={}",
            source_counts.get("workspace").copied().unwrap_or(0),
            source_counts.get("external").copied().unwrap_or(0)
        ));

        let mut kind_counts = kind_counts.into_iter().collect::<Vec<_>>();
        kind_counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        lines.push("  by_kind:".to_owned());
        for (kind, count) in kind_counts {
            lines.push(format!("    {kind}={count}"));
        }

        let mut file_counts = file_counts.into_iter().collect::<Vec<_>>();
        file_counts.sort_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| {
                left.0
                    .display()
                    .to_string()
                    .cmp(&right.0.display().to_string())
            })
        });
        lines.push(format!(
            "  top_symbol_files: showing={}",
            file_counts.len().min(TOP_FILE_LIMIT)
        ));
        for (path, count) in file_counts.into_iter().take(TOP_FILE_LIMIT) {
            lines.push(format!("    {count} {}", path.display()));
        }

        for pane in [Pane::Left, Pane::Right] {
            let editor = self.editor(pane);
            let Some(path) = editor.path() else {
                lines.push(format!("  {} current_file_symbols: no file", pane.label()));
                continue;
            };

            let current_symbols = self
                .symbol_index
                .symbols()
                .iter()
                .filter(|symbol| symbol.location.path == *path)
                .collect::<Vec<_>>();
            lines.push(format!(
                "  {} current_file_symbols: total={} showing=all path={}",
                pane.label(),
                current_symbols.len(),
                path.display()
            ));
            for symbol in current_symbols {
                lines.push(format!(
                    "    {:?} {}{} -> {}:{}",
                    symbol.kind,
                    symbol
                        .container_name
                        .as_ref()
                        .map(|container| format!("{container}::"))
                        .unwrap_or_default(),
                    symbol.name,
                    symbol.location.range.start.line + 1,
                    symbol.location.range.start.character + 1
                ));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_app_perf_sample(
        &mut self,
        total_ms: f32,
        stable_dt_ms: f32,
        unstable_dt_ms: f32,
        focused_pane: Pane,
        scroll_before_y: f32,
        scroll_after_y: f32,
        raw_wheel_delta: egui::Vec2,
        smooth_scroll_delta: egui::Vec2,
        page_key: Option<&'static str>,
    ) {
        const APP_PERF_SAMPLE_LIMIT: usize = 120;

        self.app_perf_debug.frame_index = self.app_perf_debug.frame_index.wrapping_add(1);
        self.app_perf_debug.last_total_ms = total_ms;
        self.app_perf_debug.last_stable_dt_ms = stable_dt_ms;
        self.app_perf_debug.last_unstable_dt_ms = unstable_dt_ms;
        self.app_perf_debug.focused_pane = focused_pane;
        self.app_perf_debug.scroll_before_y = scroll_before_y;
        self.app_perf_debug.scroll_after_y = scroll_after_y;
        self.app_perf_debug.scroll_delta_y = scroll_after_y - scroll_before_y;
        self.app_perf_debug.raw_wheel_delta = raw_wheel_delta;
        self.app_perf_debug.smooth_scroll_delta = smooth_scroll_delta;
        self.app_perf_debug.page_key = page_key;

        self.app_perf_debug.frame_samples.push(total_ms);
        if self.app_perf_debug.frame_samples.len() > APP_PERF_SAMPLE_LIMIT {
            let remove_count = self
                .app_perf_debug
                .frame_samples
                .len()
                .saturating_sub(APP_PERF_SAMPLE_LIMIT);
            self.app_perf_debug.frame_samples.drain(0..remove_count);
        }

        if self.app_perf_debug.frame_samples.is_empty() {
            self.app_perf_debug.avg_total_ms = 0.0;
            self.app_perf_debug.max_total_ms = 0.0;
        } else {
            let total = self.app_perf_debug.frame_samples.iter().sum::<f32>();
            self.app_perf_debug.avg_total_ms =
                total / self.app_perf_debug.frame_samples.len() as f32;
            self.app_perf_debug.max_total_ms = self
                .app_perf_debug
                .frame_samples
                .iter()
                .copied()
                .fold(0.0, f32::max);
        }
    }
}

fn lsp_report_line(name: &str, session: &LspSession) -> String {
    format!(
        "{name}={} initialized={} pending={} status={} last_kind={} last_request={} last_ms={} locations={} warmups={} syncs={} definitions={} references={} fallbacks={} timeouts={} error={} definition_status={} definition_ms={} definition_locations={} definition_error={} server_req={} server_notif={} reader_errors={} progress={} diagnostics={} last_server={}:{} server_message={}",
        session.client.is_some(),
        session.debug.initialized,
        session
            .pending
            .as_ref()
            .map(|request| format!("{:?}#{}", request.kind, request.request_id))
            .unwrap_or_else(|| "none".to_owned()),
        debug_or_none(&session.debug.last_status),
        session
            .debug
            .last_kind
            .map(|kind| format!("{kind:?}"))
            .unwrap_or_else(|| "none".to_owned()),
        session
            .debug
            .last_request_id
            .map(|request_id| request_id.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        session
            .debug
            .last_duration_ms
            .map(|duration| format!("{duration:.2}"))
            .unwrap_or_else(|| "none".to_owned()),
        session.debug.last_location_count,
        session.debug.warmup_count,
        session.debug.sync_count,
        session.debug.definition_count,
        session.debug.reference_count,
        session.debug.fallback_count,
        session.debug.timeout_count,
        session
            .debug
            .last_error
            .as_deref()
            .map(debug_or_none)
            .unwrap_or_else(|| "none".to_owned()),
        debug_or_none(&session.debug.last_definition_status),
        session
            .debug
            .last_definition_duration_ms
            .map(|duration| format!("{duration:.2}"))
            .unwrap_or_else(|| "none".to_owned()),
        session.debug.last_definition_location_count,
        session
            .debug
            .last_definition_error
            .as_deref()
            .map(debug_or_none)
            .unwrap_or_else(|| "none".to_owned()),
        session.debug.server_request_count,
        session.debug.server_notification_count,
        session.debug.reader_error_count,
        session.debug.progress_count,
        session.debug.diagnostics_count,
        debug_or_none(&session.debug.last_server_kind),
        debug_or_none(&session.debug.last_server_method),
        session
            .debug
            .last_server_message
            .as_deref()
            .map(debug_or_none)
            .unwrap_or_else(|| "none".to_owned())
    )
}
