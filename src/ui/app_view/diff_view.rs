//! The diff view: painting unified and side-by-side rows, overview markers, selection
//! and scrolling. Rows themselves come from `core::diff`.

use super::*;

/// The rows on screen and where they are.
#[derive(Clone, Copy)]
pub(super) struct DiffVisibleRows<'a> {
    pub(super) rows: &'a [DiffRenderRow<'a>],
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) visible_rect: egui::Rect,
    pub(super) row_height: f32,
    pub(super) gutter_width: f32,
}

/// The screen band row backgrounds are painted in.
#[derive(Clone, Copy)]
pub(super) struct DiffBand {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) row_height: f32,
}

/// What every row of the diff is painted with.
#[derive(Clone, Copy)]
pub(super) struct DiffRowContext<'a> {
    pub(super) gutter_width: f32,
    pub(super) side_selection: Option<&'a DiffSideSelection>,
    pub(super) unified_selection: Option<&'a DiffUnifiedSelection>,
    pub(super) side_scroll: DiffSideBySideScroll,
}

/// The parts of the pane's diff state that painting changes.
pub(super) struct DiffRowsState<'a> {
    pub(super) collapsed_files: &'a mut HashMap<String, bool>,
    pub(super) side_selection: &'a mut Option<DiffSideSelection>,
    pub(super) unified_selection: &'a mut Option<DiffUnifiedSelection>,
}

impl EditorPane {
    pub(super) fn has_diff_text_selection(&self) -> bool {
        match self.diff.display_mode {
            DiffDisplayMode::Unified => self
                .diff
                .unified_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
            DiffDisplayMode::SideBySide => self
                .diff
                .side_selection
                .and_then(|selection| selection.normalized())
                .is_some(),
        }
    }

    pub(super) fn diff_selection_text(&self) -> Option<String> {
        let rows = parse_diff_display_rows(&self.text);
        match self.diff.display_mode {
            DiffDisplayMode::Unified => {
                let selection = self.diff.unified_selection?;
                let render_rows = unified_diff_render_rows(&rows, &self.diff.collapsed_files);
                diff_unified_selection_text_from_render_rows(&render_rows, selection)
            }
            DiffDisplayMode::SideBySide => {
                let selection = self.diff.side_selection?;
                let render_rows = side_by_side_diff_render_rows(&rows, &self.diff.collapsed_files);
                diff_side_selection_text_from_render_rows(&render_rows, selection)
            }
        }
    }

    pub(super) fn reload_current_file_diff_context(&mut self, context_mode: GitDiffContextMode) {
        let Some(diff_view) = self.diff_view().cloned() else {
            return;
        };
        if !diff_view_can_switch_context(&diff_view) || diff_view.context_mode == context_mode {
            return;
        }
        let Some(root) = diff_view.repository_root.clone() else {
            self.pending_action = Some(EditorAction::ShowStatus(
                "Reload diff failed: repository root is unknown.".to_owned(),
            ));
            return;
        };
        let Some(path) = diff_view.file_path.clone() else {
            return;
        };
        match git_file_diff_text_for_path(
            &root,
            path.as_str(),
            diff_view.scope,
            context_mode,
            diff_view.untracked,
        ) {
            Ok(text) => {
                self.replace_text(text);
                self.scroll.y = 0.0;
                self.scroll.x = 0.0;
                self.diff.display_cache = None;
                self.diff.side_selection = None;
                self.diff.unified_selection = None;
                if let Some(diff_view) = self.diff_view_mut() {
                    diff_view.context_mode = context_mode;
                }
                let label = match context_mode {
                    GitDiffContextMode::Changes => "Changes",
                    GitDiffContextMode::FullFile => "Full File",
                };
                self.pending_action = Some(EditorAction::ShowStatus(format!(
                    "Diff context: {label} | {path}"
                )));
            }
            Err(error) => {
                self.pending_action = Some(EditorAction::ShowStatus(format!(
                    "Reload diff failed: {path} | {error}"
                )));
            }
        }
    }

    pub(super) fn diff_view_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let return_history = self
            .diff_view()
            .and_then(|diff_view| diff_view.return_history.clone());
        let mut back_to_history = false;
        let mut next_context_mode = None;
        if let EditorContent::Diff(diff_view) = &self.content {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&diff_view.repository_name).small().weak());
                ui.separator();
                let scope = match diff_view.scope {
                    GitDiffScope::All => "All Diffs",
                    GitDiffScope::Worktree => "Worktree",
                    GitDiffScope::Staged => "Staged",
                    GitDiffScope::Commit => "Commit",
                };
                ui.label(RichText::new(scope).small().weak());
                if let Some(path) = &diff_view.file_path {
                    ui.separator();
                    ui.label(RichText::new(path).small().monospace().weak());
                }
                if return_history.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Back to History").clicked() {
                            back_to_history = true;
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.diff.display_mode,
                    DiffDisplayMode::Unified,
                    "Unified",
                );
                ui.selectable_value(
                    &mut self.diff.display_mode,
                    DiffDisplayMode::SideBySide,
                    "Side by Side",
                );
                if diff_view_can_switch_context(diff_view) {
                    ui.separator();
                    if ui
                        .selectable_label(
                            diff_view.context_mode == GitDiffContextMode::Changes,
                            "Changes",
                        )
                        .clicked()
                    {
                        next_context_mode = Some(GitDiffContextMode::Changes);
                    }
                    if ui
                        .selectable_label(
                            diff_view.context_mode == GitDiffContextMode::FullFile,
                            "Full File",
                        )
                        .clicked()
                    {
                        next_context_mode = Some(GitDiffContextMode::FullFile);
                    }
                }
            });
            ui.separator();
        }
        if let Some(context_mode) = next_context_mode {
            self.reload_current_file_diff_context(context_mode);
        }
        if back_to_history && let Some(return_history) = return_history {
            let pane = self.pane;
            let mut next = EditorPane::from_git_history(pane, return_history.view);
            next.scroll.y = return_history.scroll_y;
            next.scroll.x = return_history.scroll_x;
            *self = next;
            return;
        }

        let (source_len, source_hash) = self.text_fingerprint();
        let rows = self.cached_diff_display_rows(source_len, source_hash);
        let display_mode = self.diff.display_mode;
        let side_by_side = display_mode == DiffDisplayMode::SideBySide;
        if side_by_side {
            self.diff.unified_selection = None;
        } else {
            self.diff.side_selection = None;
        }
        let side_scrollbar_height = if side_by_side { 20.0 } else { 0.0 };
        let viewport_height = (ui.available_height() - side_scrollbar_height).max(0.0);
        if let Some(delta) = diff_page_scroll_delta(ui, focused, viewport_height) {
            self.scroll.y = (self.scroll.y + delta).max(0.0);
        }
        self.apply_immediate_wheel_scroll(ui, self.scroll.max_y);

        let scroll_area = if side_by_side {
            ScrollArea::vertical()
        } else {
            ScrollArea::both()
        }
        .id_salt(format!("git_diff_view_{:?}", self.pane))
        .auto_shrink([false, false])
        .scroll_source(ScrollSource {
            drag: false,
            ..ScrollSource::ALL
        })
        .max_height(viewport_height)
        .vertical_scroll_offset(self.scroll.y);

        let side_scroll = DiffSideBySideScroll {
            left_x: self.diff.side_left_scroll_x,
            right_x: self.diff.side_right_scroll_x,
        };
        let scroll_output = scroll_area.show(ui, |ui| {
            if side_by_side {
                ui.set_min_width(ui.available_width());
            } else {
                ui.set_min_width(diff_view_min_width(rows.as_slice(), display_mode));
            }
            render_diff_rows(
                ui,
                rows.as_slice(),
                display_mode,
                DiffRowsState {
                    collapsed_files: &mut self.diff.collapsed_files,
                    side_selection: &mut self.diff.side_selection,
                    unified_selection: &mut self.diff.unified_selection,
                },
                self.scroll.y,
                side_scroll,
            )
        });
        if let Some(selection) = &mut self.diff.side_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        if let Some(selection) = &mut self.diff.unified_selection
            && !ui.input(|input| input.pointer.primary_down())
        {
            selection.dragging = false;
        }
        self.scroll.max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll.y = scroll_output.state.offset.y.clamp(0.0, self.scroll.max_y);
        if side_by_side {
            let viewport_width = scroll_output
                .inner_rect
                .intersect(ui.clip_rect())
                .width()
                .max(ui.available_width());
            diff_side_by_side_scrollbars_ui(
                ui,
                &mut self.diff.side_left_scroll_x,
                &mut self.diff.side_right_scroll_x,
                &scroll_output.inner,
                viewport_width,
            );
        }
        if self.diff_view().is_some_and(|diff_view| {
            diff_view.file_path.is_some() && diff_view.scope != GitDiffScope::All
        }) {
            paint_diff_overview_markers(
                ui,
                scroll_output.inner_rect,
                &scroll_output.inner,
                editor_scroll_row_height(ui),
            );
        }
    }

    pub(super) fn cached_diff_display_rows(
        &mut self,
        source_len: usize,
        source_hash: u64,
    ) -> Rc<Vec<DiffDisplayRow>> {
        if let Some(cache) = &self.diff.display_cache
            && cache.source_len == source_len
            && cache.source_hash == source_hash
        {
            return cache.rows.clone();
        }

        let rows = Rc::new(parse_diff_display_rows(&self.text));
        self.diff.display_cache = Some(DiffDisplayCache {
            source_len,
            source_hash,
            rows: rows.clone(),
        });
        rows
    }
}

#[derive(Debug, Clone)]
pub(super) struct DiffDisplayCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) rows: Rc<Vec<DiffDisplayRow>>,
}

/// State only the diff view uses.
pub(super) struct DiffPaneState {
    pub(super) side_left_scroll_x: f32,
    pub(super) side_right_scroll_x: f32,
    pub(super) display_mode: DiffDisplayMode,
    pub(super) collapsed_files: HashMap<String, bool>,
    pub(super) side_selection: Option<DiffSideSelection>,
    pub(super) unified_selection: Option<DiffUnifiedSelection>,
    pub(super) display_cache: Option<DiffDisplayCache>,
}

impl Default for DiffPaneState {
    fn default() -> Self {
        Self {
            side_left_scroll_x: 0.0,
            side_right_scroll_x: 0.0,
            display_mode: DiffDisplayMode::Unified,
            collapsed_files: HashMap::new(),
            side_selection: None,
            unified_selection: None,
            display_cache: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct GitDiffView {
    pub(super) title: String,
    pub(super) repository_name: String,
    pub(super) repository_root: Option<PathBuf>,
    pub(super) file_path: Option<String>,
    pub(super) scope: GitDiffScope,
    pub(super) context_mode: GitDiffContextMode,
    pub(super) untracked: bool,
    pub(super) return_history: Option<GitHistoryReturnState>,
}

#[derive(Debug, Clone)]
pub(super) struct GitHistoryReturnState {
    pub(super) view: GitHistoryView,
    pub(super) scroll_y: f32,
    pub(super) scroll_x: f32,
}

pub(super) fn diff_view_can_switch_context(diff_view: &GitDiffView) -> bool {
    diff_view.file_path.is_some()
        && diff_view.repository_root.is_some()
        && !diff_view.untracked
        && matches!(
            diff_view.scope,
            GitDiffScope::Worktree | GitDiffScope::Staged
        )
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DiffSideBySideScroll {
    pub(super) left_x: f32,
    pub(super) right_x: f32,
}

pub(super) fn render_diff_rows(
    ui: &mut egui::Ui,
    rows: &[DiffDisplayRow],
    mode: DiffDisplayMode,
    state: DiffRowsState<'_>,
    scroll_y: f32,
    side_scroll: DiffSideBySideScroll,
) -> DiffRenderSummary {
    let DiffRowsState {
        collapsed_files,
        side_selection,
        unified_selection,
    } = state;

    let render_rows = diff_render_rows(rows, mode, collapsed_files);
    let markers = diff_overview_markers(&render_rows);
    let row_height = editor_scroll_row_height(ui);
    let gutter_width = diff_gutter_width(ui, rows, mode);
    let (left_content_width, right_content_width) =
        diff_side_by_side_content_widths(ui, &render_rows, gutter_width);
    let start = ((scroll_y / row_height).floor() as usize).saturating_sub(8);
    let visible_count = ((ui.available_height() / row_height).ceil() as usize).saturating_add(18);
    let end = (start + visible_count).min(render_rows.len());

    ui.add_space(start as f32 * row_height);
    let visible_height = (end - start) as f32 * row_height;
    let (visible_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), visible_height),
        egui::Sense::empty(),
    );
    handle_diff_selection_input(
        ui,
        &DiffVisibleRows {
            rows: &render_rows,
            start,
            end,
            visible_rect,
            row_height,
            gutter_width,
        },
        mode,
        side_selection,
        unified_selection,
        side_scroll,
    );
    paint_diff_visible_background_runs(
        ui,
        &render_rows,
        mode,
        start,
        end,
        visible_rect,
        row_height,
    );
    for (row_index, row) in render_rows[start..end].iter().enumerate() {
        let row_rect = diff_visible_row_rect(visible_rect, row_index, row_height);
        render_diff_render_row(
            ui,
            row_rect,
            start + row_index,
            row,
            collapsed_files,
            &DiffRowContext {
                gutter_width,
                side_selection: side_selection.as_ref(),
                unified_selection: unified_selection.as_ref(),
                side_scroll,
            },
        );
    }
    ui.add_space(render_rows.len().saturating_sub(end) as f32 * row_height);

    DiffRenderSummary {
        total_rows: render_rows.len(),
        markers,
        left_content_width,
        right_content_width,
    }
}

pub(super) fn diff_visible_row_rect(
    visible_rect: egui::Rect,
    row_index: usize,
    row_height: f32,
) -> egui::Rect {
    let top = visible_rect.top() + row_index as f32 * row_height;
    egui::Rect::from_min_size(
        egui::pos2(visible_rect.left(), top),
        egui::vec2(visible_rect.width(), row_height),
    )
}

pub(super) fn paint_diff_visible_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    mode: DiffDisplayMode,
    start: usize,
    end: usize,
    visible_rect: egui::Rect,
    row_height: f32,
) {
    if start >= end {
        return;
    }
    let left = visible_rect.left();
    let top = visible_rect.top();
    let width = visible_rect.width().max(1.0);
    match mode {
        DiffDisplayMode::Unified => paint_unified_diff_background_runs(
            ui,
            rows,
            start,
            end,
            DiffBand {
                left,
                top,
                width,
                row_height,
            },
        ),
        DiffDisplayMode::SideBySide => paint_side_by_side_diff_background_runs(
            ui,
            rows,
            start,
            end,
            DiffBand {
                left,
                top,
                width,
                row_height,
            },
        ),
    }
}

pub(super) fn paint_unified_diff_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    band: DiffBand,
) {
    let mut run_kind = None;
    let mut run_start = start;
    for (row_index, row) in rows.iter().enumerate().take(end).skip(start) {
        let kind = diff_unified_background_kind(row);
        if kind != run_kind {
            paint_diff_background_run(ui, run_kind, run_start, row_index, start, band);
            run_kind = kind;
            run_start = row_index;
        }
    }
    paint_diff_background_run(ui, run_kind, run_start, end, start, band);
}

pub(super) fn paint_side_by_side_diff_background_runs(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    start: usize,
    end: usize,
    band: DiffBand,
) {
    let DiffBand {
        left,
        top,
        width,
        row_height,
    } = band;

    let center_x = left + width * 0.5;
    let left_width = (center_x - 1.0 - left).max(0.0);
    let right_left = center_x + 1.0;
    let right_width = (left + width - right_left).max(0.0);
    paint_side_diff_background_runs_for_side(
        ui,
        rows,
        DiffSide::Old,
        start,
        end,
        DiffBand {
            left,
            top,
            width: left_width,
            row_height,
        },
    );
    paint_side_diff_background_runs_for_side(
        ui,
        rows,
        DiffSide::New,
        start,
        end,
        DiffBand {
            left: right_left,
            top,
            width: right_width,
            row_height,
        },
    );
}

pub(super) fn paint_side_diff_background_runs_for_side(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    side: DiffSide,
    start: usize,
    end: usize,
    band: DiffBand,
) {
    let width = band.width;
    if width <= 0.0 {
        return;
    }
    let mut run_kind = None;
    let mut run_start = start;
    for (row_index, row) in rows.iter().enumerate().take(end).skip(start) {
        let kind = diff_side_background_kind(row, side);
        if kind != run_kind {
            paint_diff_background_run(ui, run_kind, run_start, row_index, start, band);
            run_kind = kind;
            run_start = row_index;
        }
    }
    paint_diff_background_run(ui, run_kind, run_start, end, start, band);
}

pub(super) fn paint_diff_background_run(
    ui: &egui::Ui,
    kind: Option<DiffLineKind>,
    run_start: usize,
    run_end: usize,
    visible_start: usize,
    band: DiffBand,
) {
    let DiffBand {
        left,
        top,
        width,
        row_height,
    } = band;

    let Some(kind) = kind else {
        return;
    };
    if run_start >= run_end {
        return;
    }
    let y1 = top + (run_start - visible_start) as f32 * row_height;
    let y2 = top + (run_end - visible_start) as f32 * row_height;
    let rect = egui::Rect::from_min_max(egui::pos2(left, y1), egui::pos2(left + width, y2));
    let rect = diff_row_background_rect(rect);
    ui.painter_at(rect)
        .rect_filled(rect, 0.0, diff_line_background(kind));
}

pub(super) fn diff_side_by_side_content_widths(
    ui: &egui::Ui,
    rows: &[DiffRenderRow<'_>],
    gutter_width: f32,
) -> (f32, f32) {
    let char_width = editor_char_width(ui);
    let base_width = gutter_width + 20.0;
    let mut left_width: f32 = 0.0;
    let mut right_width: f32 = 0.0;
    for row in rows {
        if let DiffRenderRow::SideBySide { old, new } = row {
            left_width =
                left_width.max(base_width + old.content.chars().count() as f32 * char_width);
            right_width =
                right_width.max(base_width + new.content.chars().count() as f32 * char_width);
        }
    }
    (left_width, right_width)
}

pub(super) fn render_diff_render_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    row: &DiffRenderRow<'_>,
    collapsed_files: &mut HashMap<String, bool>,
    context: &DiffRowContext<'_>,
) {
    match row {
        DiffRenderRow::Section(title) => {
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                *title,
                FontId::monospace(11.0),
                ui.visuals().weak_text_color(),
            );
        }
        DiffRenderRow::FileHeader { path, status } => {
            diff_file_header_row_ui(ui, rect, path, status, collapsed_files);
        }
        DiffRenderRow::HiddenLines(count) => {
            hidden_lines_row_ui(ui, rect, *count);
        }
        DiffRenderRow::UnifiedLine {
            line_label,
            marker,
            content,
            kind,
        } => {
            unified_diff_row_ui(
                ui,
                rect,
                row_index,
                (line_label, marker, content, *kind),
                context.gutter_width,
                context.unified_selection,
            );
        }
        DiffRenderRow::SideBySide { old, new } => {
            side_by_side_diff_row_ui(ui, rect, row_index, *old, *new, context);
        }
        DiffRenderRow::Message(text) => {
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                *text,
                editor_font_id(),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

pub(super) fn paint_diff_overview_markers(
    ui: &egui::Ui,
    rect: egui::Rect,
    summary: &DiffRenderSummary,
    row_height: f32,
) {
    let viewport_rect = rect.intersect(ui.clip_rect());
    if summary.total_rows == 0 || summary.markers.is_empty() || !viewport_rect.is_positive() {
        return;
    }

    let lane_width = 4.0;
    let lane_rect = egui::Rect::from_min_max(
        egui::pos2(
            viewport_rect.right() - lane_width - 2.0,
            viewport_rect.top(),
        ),
        egui::pos2(viewport_rect.right() - 2.0, viewport_rect.bottom()),
    );
    let painter = ui.painter_at(lane_rect);
    painter.rect_filled(
        lane_rect,
        0.0,
        ui.visuals()
            .widgets
            .noninteractive
            .bg_stroke
            .color
            .linear_multiply(0.35),
    );

    let total = summary.total_rows.saturating_sub(1).max(1) as f32;
    let marker_height = row_height.clamp(3.0, 8.0);
    for marker in &summary.markers {
        let ratio = marker.row_index as f32 / total;
        let y = (lane_rect.top() + ratio * lane_rect.height())
            .clamp(lane_rect.top(), lane_rect.bottom() - marker_height);
        let marker_rect = egui::Rect::from_min_size(
            egui::pos2(lane_rect.left(), y),
            egui::vec2(lane_rect.width(), marker_height),
        )
        .intersect(lane_rect);
        painter.rect_filled(marker_rect, 0.0, diff_marker_color(marker.kind));
    }
}

pub(super) fn diff_marker_color(kind: DiffMarkerKind) -> Color32 {
    match kind {
        DiffMarkerKind::Added => Color32::from_rgb(72, 160, 92),
        DiffMarkerKind::Removed => Color32::from_rgb(206, 92, 92),
        DiffMarkerKind::Modified => Color32::from_rgb(196, 160, 72),
    }
}

pub(super) fn unified_diff_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    (line_label, marker, content, kind): (&str, &str, &str, DiffLineKind),
    gutter_width: f32,
    unified_selection: Option<&DiffUnifiedSelection>,
) {
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(rect.left() + gutter_width - 8.0, rect.top()),
        egui::Align2::RIGHT_TOP,
        line_label,
        editor_font_id(),
        ui.visuals().weak_text_color(),
    );
    let selectable_text = unified_diff_visible_text(marker, content);
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + gutter_width + 8.0, rect.top()),
        rect.max,
    );
    paint_unified_diff_text(
        ui,
        row_index,
        content_rect,
        &selectable_text,
        diff_line_color(ui, kind),
        unified_selection,
    );
}

pub(super) fn diff_file_header_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    path: &str,
    status: &str,
    collapsed_files: &mut HashMap<String, bool>,
) {
    let collapsed = collapsed_files.get(path).copied().unwrap_or(false);
    let icon = if collapsed { "▶" } else { "▼" };
    let response = ui.interact(
        rect,
        ui.auto_id_with(("diff_file_header", path)),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    painter.text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        path,
        FontId::monospace(13.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        status,
        FontId::monospace(11.0),
        ui.visuals().weak_text_color(),
    );
    if response.on_hover_text("Toggle file diff").clicked() {
        collapsed_files.insert(path.to_owned(), !collapsed);
        ui.ctx().request_repaint();
    }
}

pub(super) fn hidden_lines_row_ui(ui: &mut egui::Ui, rect: egui::Rect, count: usize) {
    if count == 0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("◇  {}", hidden_lines_label(count)),
        FontId::monospace(11.0),
        ui.visuals().weak_text_color(),
    );
}

pub(super) fn side_by_side_diff_row_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row_index: usize,
    old: DiffCell<'_>,
    new: DiffCell<'_>,
    context: &DiffRowContext<'_>,
) {
    let side_scroll = context.side_scroll;

    let width = rect.width().max(1.0);
    let center_x = rect.left() + width * 0.5;
    let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(center_x - 1.0, rect.max.y));
    let right_rect = egui::Rect::from_min_max(egui::pos2(center_x + 1.0, rect.min.y), rect.max);
    let painter = ui.painter_at(rect);

    painter.line_segment(
        [
            egui::pos2(center_x, rect.top()),
            egui::pos2(center_x, rect.bottom()),
        ],
        egui::Stroke::new(2.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
    );

    paint_side_by_side_cell_text(
        ui,
        row_index,
        DiffSide::Old,
        left_rect,
        old,
        context,
        side_scroll.left_x,
    );
    paint_side_by_side_cell_text(
        ui,
        row_index,
        DiffSide::New,
        right_rect,
        new,
        context,
        side_scroll.right_x,
    );
}

pub(super) fn paint_side_by_side_cell_text(
    ui: &mut egui::Ui,
    row_index: usize,
    side: DiffSide,
    rect: egui::Rect,
    cell: DiffCell<'_>,
    context: &DiffRowContext<'_>,
    scroll_x: f32,
) {
    let DiffCell {
        line_label,
        content,
        kind,
    } = cell;
    let gutter_width = context.gutter_width;
    let side_selection = context.side_selection;

    let gutter_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            (rect.left() + gutter_width).min(rect.right()),
            rect.bottom(),
        ),
    );
    let content_rect = egui::Rect::from_min_max(
        egui::pos2((rect.left() + gutter_width).min(rect.right()), rect.top()),
        rect.max,
    );
    let gutter_painter = ui.painter_at(gutter_rect);

    gutter_painter.text(
        egui::pos2(rect.left() + gutter_width - 8.0, rect.top()),
        egui::Align2::RIGHT_TOP,
        line_label,
        editor_font_id(),
        ui.visuals().weak_text_color(),
    );
    let text_left = rect.left() + gutter_width + 8.0 - scroll_x;
    let char_width = editor_char_width(ui);
    paint_diff_side_selection_for_cell(
        ui,
        content_rect,
        content,
        row_index,
        side,
        side_selection,
        (text_left, char_width),
    );
    let content_painter = ui.painter_at(content_rect);
    content_painter.text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        content,
        editor_font_id(),
        diff_line_color(ui, kind),
    );
}

pub(super) fn paint_unified_diff_text(
    ui: &mut egui::Ui,
    row_index: usize,
    rect: egui::Rect,
    text: &str,
    color: Color32,
    unified_selection: Option<&DiffUnifiedSelection>,
) {
    let clip_rect = rect.intersect(ui.clip_rect());
    if !rect.is_positive() || !clip_rect.is_positive() {
        return;
    }
    let text_left = rect.left();
    let char_width = editor_char_width(ui);
    paint_diff_unified_selection_for_line(
        ui,
        rect,
        text,
        row_index,
        unified_selection,
        text_left,
        char_width,
    );
    ui.painter_at(clip_rect).text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        editor_font_id(),
        color,
    );
}

pub(super) fn diff_side_by_side_scrollbars_ui(
    ui: &mut egui::Ui,
    left_scroll_x: &mut f32,
    right_scroll_x: &mut f32,
    summary: &DiffRenderSummary,
    viewport_width: f32,
) {
    let pane_width = (viewport_width * 0.5).max(1.0);
    let left_max = (summary.left_content_width - pane_width).max(0.0);
    let right_max = (summary.right_content_width - pane_width).max(0.0);
    *left_scroll_x = left_scroll_x.clamp(0.0, left_max);
    *right_scroll_x = right_scroll_x.clamp(0.0, right_max);

    if left_max <= 0.0 && right_max <= 0.0 {
        ui.add_space(4.0);
        return;
    }

    ui.add_space(2.0);
    let width = ui.available_width().max(1.0);
    let height = 16.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let center_x = rect.left() + rect.width() * 0.5;
    let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(center_x - 1.0, rect.bottom()));
    let right_rect = egui::Rect::from_min_max(egui::pos2(center_x + 1.0, rect.top()), rect.max);

    let left_changed = diff_side_scrollbar_ui(
        ui,
        left_rect,
        left_scroll_x,
        left_max,
        summary.left_content_width,
        pane_width,
        "left_diff_horizontal_scroll",
    );
    if left_changed {
        *right_scroll_x = left_scroll_x.clamp(0.0, right_max);
    }

    let right_changed = diff_side_scrollbar_ui(
        ui,
        right_rect,
        right_scroll_x,
        right_max,
        summary.right_content_width,
        pane_width,
        "right_diff_horizontal_scroll",
    );
    if right_changed {
        *left_scroll_x = right_scroll_x.clamp(0.0, left_max);
    }

    ui.painter().line_segment(
        [
            egui::pos2(center_x, rect.top()),
            egui::pos2(center_x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
    );
}

pub(super) fn diff_side_scrollbar_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &mut f32,
    max: f32,
    content_width: f32,
    viewport_width: f32,
    id_salt: &'static str,
) -> bool {
    if !rect.is_positive() {
        return false;
    }

    let track_rect = rect.shrink2(egui::vec2(8.0, 3.0));
    let painter = ui.painter_at(rect);
    let track_color = if ui.visuals().dark_mode {
        Color32::from_gray(64)
    } else {
        Color32::from_gray(210)
    };
    painter.rect_filled(track_rect, 2.0, track_color);

    if max <= 0.0 {
        return false;
    }

    let id = ui.make_persistent_id(id_salt);
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());
    let thumb_width = (track_rect.width() * (viewport_width / content_width.max(viewport_width)))
        .clamp(24.0, track_rect.width());
    let travel_width = (track_rect.width() - thumb_width).max(1.0);
    let ratio = (*value / max).clamp(0.0, 1.0);
    let mut thumb_left = track_rect.left() + ratio * travel_width;
    let mut changed = false;

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let pointer_ratio =
            ((pointer.x - track_rect.left() - thumb_width * 0.5) / travel_width).clamp(0.0, 1.0);
        *value = pointer_ratio * max;
        thumb_left = track_rect.left() + pointer_ratio * travel_width;
        ui.ctx().request_repaint();
        changed = true;
    }

    let thumb_rect = egui::Rect::from_min_size(
        egui::pos2(thumb_left, track_rect.top()),
        egui::vec2(thumb_width, track_rect.height()),
    );
    let thumb_color = if response.dragged() {
        if ui.visuals().dark_mode {
            Color32::from_gray(180)
        } else {
            Color32::from_gray(100)
        }
    } else if response.hovered() {
        if ui.visuals().dark_mode {
            Color32::from_gray(155)
        } else {
            Color32::from_gray(125)
        }
    } else if ui.visuals().dark_mode {
        Color32::from_gray(130)
    } else {
        Color32::from_gray(150)
    };
    painter.rect_filled(thumb_rect, 2.0, thumb_color);
    changed
}

pub(super) fn diff_view_min_width(rows: &[DiffDisplayRow], mode: DiffDisplayMode) -> f32 {
    let max_chars = rows
        .iter()
        .map(diff_row_char_width)
        .max()
        .unwrap_or(80)
        .clamp(80, 260);
    match mode {
        DiffDisplayMode::Unified => 96.0 + max_chars as f32 * 8.0,
        DiffDisplayMode::SideBySide => 980.0 + max_chars.min(120) as f32 * 4.0,
    }
}

pub(super) fn diff_gutter_width(
    ui: &egui::Ui,
    rows: &[DiffDisplayRow],
    mode: DiffDisplayMode,
) -> f32 {
    let digits = rows
        .iter()
        .filter_map(|row| match row {
            DiffDisplayRow::Line { line_label, .. } => Some(line_label.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(1);
    let row_height = editor_scroll_row_height(ui);
    let base_width = (digits as f32 * row_height * 0.6 + 22.0).max(38.0);
    match mode {
        DiffDisplayMode::Unified => base_width,
        DiffDisplayMode::SideBySide => base_width.max(50.0),
    }
}

pub(super) fn diff_line_color(ui: &egui::Ui, kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Added => Color32::from_rgb(72, 160, 92),
        DiffLineKind::Removed => Color32::from_rgb(206, 92, 92),
        DiffLineKind::Context => ui.visuals().text_color(),
    }
}

pub(super) fn diff_line_background(kind: DiffLineKind) -> Color32 {
    match kind {
        DiffLineKind::Added => Color32::from_rgba_unmultiplied(72, 160, 92, 24),
        DiffLineKind::Removed => Color32::from_rgba_unmultiplied(206, 92, 92, 24),
        DiffLineKind::Context => Color32::TRANSPARENT,
    }
}

pub(super) fn diff_row_background_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() - 0.5),
        egui::pos2(rect.right(), rect.bottom() + 0.5),
    )
}

pub(super) fn diff_page_scroll_delta(
    ui: &egui::Ui,
    focused: bool,
    viewport_height: f32,
) -> Option<f32> {
    if !focused {
        return None;
    }

    let amount = (viewport_height * EDITOR_PAGE_SCROLL_RATIO).max(editor_row_height(ui));
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown) {
            Some(amount)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp) {
            Some(-amount)
        } else {
            None
        }
    })
}
