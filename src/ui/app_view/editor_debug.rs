//! Diagnostics kept apart from the editors: per-pane render and performance records, the
//! large-file click log and the lines they add to the Report. Editors only call `capture_*`
//! and `push_event` here; nothing in this file changes what the user sees.

use super::*;

impl EditorPane {
    pub(super) fn debug_report_lines(&self, tab_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!(
            "{} editor: title={} tab_width={}",
            self.pane.label(),
            self.title,
            tab_width
        ));
        if let Some(path) = self.path() {
            lines.push(format!("{} path={}", self.pane.label(), path.display()));
        } else {
            lines.push(format!("{} path=none", self.pane.label()));
        }
        if let Some(position) = self.cursor_position {
            lines.push(format!(
                "{} cursor={}:{}",
                self.pane.label(),
                position.line + 1,
                position.character + 1
            ));
        }
        if let Some(position) = self.target_position {
            lines.push(format!(
                "{} definition_target={}:{}",
                self.pane.label(),
                position.line + 1,
                position.character + 1
            ));
        }
        self.push_view_report_lines(&mut lines);
        self.push_render_report_lines(&mut lines);
        if let Some(accept) = &self.debug.virtual_click.last_accept {
            lines.push(accept.report_line(self.pane.label()));
        } else {
            lines.push(format!("{} virtual_click_accept: none", self.pane.label()));
        }
        for event in self.debug.virtual_click.events.iter().rev().take(4) {
            lines.push(event.report_line(self.pane.label()));
        }
        lines
    }

    /// The CSV, Markdown or find bar state of this pane.
    fn push_view_report_lines(&self, lines: &mut Vec<String>) {
        if self.is_csv() {
            lines.push(format!(
                "{} csv_mode={}",
                self.pane.label(),
                self.csv.mode.label()
            ));
            if self.csv.mode == CsvMode::Grid
                && let Some(cell) = self.csv.selected_cell
            {
                lines.push(format!(
                    "{} csv_cell=R{}C{}",
                    self.pane.label(),
                    cell.row + 1,
                    cell.column + 1
                ));
            }
        }
        if self.is_markdown() {
            lines.push(format!(
                "{} markdown_mode={}",
                self.pane.label(),
                self.markdown.mode.label()
            ));
            if !self.markdown.search.query.is_empty() {
                lines.push(format!(
                    "{} markdown_search={}/{} query={}",
                    self.pane.label(),
                    self.markdown.search.current_match + 1,
                    self.markdown.search.match_count,
                    self.markdown.search.query
                ));
            }
        } else if self.source_search.open && !self.source_search.query.is_empty() {
            lines.push(format!(
                "{} source_search={}/{} query={}",
                self.pane.label(),
                self.source_search.current_match + 1,
                self.source_search.match_count,
                self.source_search.query
            ));
        }
    }

    fn push_render_report_lines(&self, lines: &mut Vec<String>) {
        if let Some(debug) = &self.debug.render {
            lines.push(format!(
                "{} render: lines={} text_lines={} galley_rows={} delta={}",
                self.pane.label(),
                debug.line_count,
                debug.text_line_count,
                debug.galley_rows,
                debug.galley_rows as isize - debug.text_line_count as isize
            ));
            lines.push(format!(
                "{} render_scroll: y={:.1} row_h={:.2} clip={:.1}..{:.1} viewport={:.1}..{:.1} galley_y={:.1}",
                self.pane.label(),
                debug.scroll_y,
                debug.row_height,
                debug.clip_top,
                debug.clip_bottom,
                debug.viewport_top,
                debug.viewport_bottom,
                debug.galley_pos_y
            ));
            lines.push(format!(
                "{} visible_rows: first={:?}@{:?} last={:?} anchor={:?}@{:?} range={:?} current_line={:?}",
                self.pane.label(),
                debug.first_visible_row,
                debug.first_visible_top.map(|value| format!("{value:.1}")),
                debug.last_visible_row,
                debug.anchor_line,
                debug.anchor_top.map(|value| format!("{value:.1}")),
                debug.anchored_range,
                debug.current_line
            ));
        }
        if let Some(perf) = &self.debug.perf {
            lines.push(format!(
                "{} perf: view={} total={:.2}ms body={:.2}ms rows={}/{} scroll_delta_y={:.1}",
                self.pane.label(),
                perf.view,
                perf.total_ms,
                perf.body_ms,
                perf.visible_rows,
                perf.total_rows,
                perf.scroll_delta_y
            ));
            lines.push(format!(
                "{} perf_cache: galley={}/{} line_no={}/{} syntax={} search={}/{}",
                self.pane.label(),
                perf.galley_hits,
                perf.galley_misses,
                perf.line_number_hits,
                perf.line_number_misses,
                perf.syntax_spans,
                perf.source_search_open,
                perf.source_search_matches
            ));
        }
    }
}

/// A pane's diagnostics, read only by the Report.
#[derive(Default)]
pub(super) struct EditorDebugState {
    pub(super) render: Option<EditorRenderDebug>,
    pub(super) perf: Option<EditorPerfDebug>,
    pub(super) virtual_click: VirtualClickDebug,
}

#[derive(Debug, Clone, Default)]
pub(super) struct EditorPerfDebug {
    pub(super) view: &'static str,
    pub(super) total_ms: f32,
    pub(super) header_ms: f32,
    pub(super) body_ms: f32,
    pub(super) visible_rows: usize,
    pub(super) total_rows: usize,
    pub(super) galley_hits: usize,
    pub(super) galley_misses: usize,
    pub(super) line_number_hits: usize,
    pub(super) line_number_misses: usize,
    pub(super) text_len: usize,
    pub(super) scroll_y: f32,
    pub(super) scroll_x: f32,
    pub(super) scroll_delta_y: f32,
    pub(super) source_search_open: bool,
    pub(super) source_search_matches: usize,
    pub(super) syntax_spans: usize,
    pub(super) virtual_cache_entries: usize,
    pub(super) line_number_cache_entries: usize,
}

#[derive(Debug, Clone, Default)]
pub(super) struct VirtualClickDebug {
    pub(super) next_event_index: u64,
    pub(super) events: Vec<VirtualClickEventDebug>,
    pub(super) last_accept: Option<VirtualClickAcceptDebug>,
}

#[derive(Debug, Clone)]
pub(super) struct VirtualClickEventDebug {
    pub(super) index: u64,
    pub(super) source: &'static str,
    pub(super) accepted: bool,
    pub(super) reason: &'static str,
    pub(super) pointer_pos: Option<egui::Pos2>,
    pub(super) response_rect: egui::Rect,
    pub(super) viewport_rect: egui::Rect,
    pub(super) text_clip_rect: egui::Rect,
    pub(super) response_hovered: bool,
    pub(super) response_contains_pointer: bool,
    pub(super) response_clicked: bool,
    pub(super) response_primary_clicked: bool,
    pub(super) primary_pressed: bool,
    pub(super) primary_released: bool,
    pub(super) primary_clicked: bool,
    pub(super) any_pressed: bool,
    pub(super) any_released: bool,
    pub(super) any_click: bool,
    pub(super) dragging: bool,
    pub(super) response_hit: bool,
    pub(super) viewport_hit: bool,
    pub(super) text_hit: bool,
    pub(super) scrollbar_hit: bool,
    pub(super) pane_focused: bool,
    pub(super) text_edit_focused: bool,
    pub(super) scroll_x: f32,
    pub(super) scroll_y: f32,
    pub(super) row_height: f32,
    pub(super) line_count: usize,
    pub(super) visible_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct VirtualClickAcceptDebug {
    pub(super) event_index: u64,
    pub(super) pointer_pos: egui::Pos2,
    pub(super) line: usize,
    pub(super) character: u32,
    pub(super) cursor_position: Position,
    pub(super) text_left: f32,
    pub(super) text_clip_left: f32,
    pub(super) x_offset: f32,
}

impl VirtualClickDebug {
    pub(super) fn push_event(&mut self, mut event: VirtualClickEventDebug) -> u64 {
        self.next_event_index = self.next_event_index.wrapping_add(1);
        event.index = self.next_event_index;
        self.events.push(event);
        if self.events.len() > 8 {
            let remove_count = self.events.len().saturating_sub(8);
            self.events.drain(0..remove_count);
        }
        self.next_event_index
    }

    pub(super) fn report_lines(&self, pane: Pane) -> Vec<String> {
        let pane_label = pane.label();
        let mut lines = Vec::new();
        if let Some(accept) = &self.last_accept {
            lines.push(accept.report_line(pane_label));
        } else {
            lines.push(format!("{pane_label} virtual_click_accept: none"));
        }
        if self.events.is_empty() {
            lines.push(format!("{pane_label} virtual_click_events: none"));
        } else {
            for event in &self.events {
                lines.push(event.report_line(pane_label));
            }
        }
        lines
    }
}

impl VirtualClickEventDebug {
    pub(super) fn report_line(&self, pane_label: &str) -> String {
        format!(
            "{pane_label} virtual_click_event#{}: source={} accepted={} reason={} pointer={} hits(response/view/text/scrollbar)={}/{}/{}/{} input(primary p/r/c={}/{}/{}, any p/r/c={}/{}/{}, dragging={}) response(hovered={} contains={} clicked={} primary_clicked={}) focus(pane={} text_edit={}) scroll=({:.1},{:.1}) row_h={:.2} lines={} visible={:?} rects(response={} viewport={} text_clip={})",
            self.index,
            self.source,
            self.accepted,
            self.reason,
            format_virtual_pos(self.pointer_pos),
            self.response_hit,
            self.viewport_hit,
            self.text_hit,
            self.scrollbar_hit,
            self.primary_pressed,
            self.primary_released,
            self.primary_clicked,
            self.any_pressed,
            self.any_released,
            self.any_click,
            self.dragging,
            self.response_hovered,
            self.response_contains_pointer,
            self.response_clicked,
            self.response_primary_clicked,
            self.pane_focused,
            self.text_edit_focused,
            self.scroll_x,
            self.scroll_y,
            self.row_height,
            self.line_count,
            self.visible_range,
            format_virtual_rect(self.response_rect),
            format_virtual_rect(self.viewport_rect),
            format_virtual_rect(self.text_clip_rect),
        )
    }
}

impl VirtualClickAcceptDebug {
    pub(super) fn report_line(&self, pane_label: &str) -> String {
        format!(
            "{pane_label} virtual_click_accept: event#{} line={} char={} cursor={}:{} pointer={} text_left={:.1} text_clip_left={:.1} x_offset={:.1}",
            self.event_index,
            self.line + 1,
            self.character + 1,
            self.cursor_position.line + 1,
            self.cursor_position.character + 1,
            format_virtual_pos(Some(self.pointer_pos)),
            self.text_left,
            self.text_clip_left,
            self.x_offset,
        )
    }
}

pub(super) fn format_virtual_pos(position: Option<egui::Pos2>) -> String {
    position
        .map(|position| format!("({:.1},{:.1})", position.x, position.y))
        .unwrap_or_else(|| "None".to_owned())
}

pub(super) fn format_virtual_rect(rect: egui::Rect) -> String {
    format!(
        "({:.1},{:.1})-({:.1},{:.1})",
        rect.left(),
        rect.top(),
        rect.right(),
        rect.bottom()
    )
}

#[derive(Debug, Clone, Default)]
pub(super) struct VirtualFrameCounters {
    pub(super) galley_hits: usize,
    pub(super) galley_misses: usize,
    pub(super) line_number_hits: usize,
    pub(super) line_number_misses: usize,
}

#[derive(Debug, Clone)]
pub(super) struct AppPerfDebug {
    pub(super) last_total_ms: f32,
    pub(super) last_stable_dt_ms: f32,
    pub(super) last_unstable_dt_ms: f32,
    pub(super) avg_total_ms: f32,
    pub(super) max_total_ms: f32,
    pub(super) frame_samples: Vec<f32>,
    pub(super) frame_index: u64,
    pub(super) focused_pane: Pane,
    pub(super) scroll_before_y: f32,
    pub(super) scroll_after_y: f32,
    pub(super) scroll_delta_y: f32,
    pub(super) raw_wheel_delta: egui::Vec2,
    pub(super) smooth_scroll_delta: egui::Vec2,
    pub(super) page_key: Option<&'static str>,
}

impl Default for AppPerfDebug {
    fn default() -> Self {
        Self {
            last_total_ms: 0.0,
            last_stable_dt_ms: 0.0,
            last_unstable_dt_ms: 0.0,
            avg_total_ms: 0.0,
            max_total_ms: 0.0,
            frame_samples: Vec::new(),
            frame_index: 0,
            focused_pane: Pane::Left,
            scroll_before_y: 0.0,
            scroll_after_y: 0.0,
            scroll_delta_y: 0.0,
            raw_wheel_delta: egui::Vec2::ZERO,
            smooth_scroll_delta: egui::Vec2::ZERO,
            page_key: None,
        }
    }
}

impl EditorPerfDebug {
    pub(super) fn report_line(&self, pane: Pane, title: &str) -> String {
        let pane_label = pane.label();
        format!(
            "{}: title={:?} view={} total_ms={:.2} header_ms={:.2} body_ms={:.2} rows={}/{} text_len={} scroll=({:.1},{:.1}) scroll_delta_y={:.1} galley_hit/miss={}/{} line_no_hit/miss={}/{} search_open={} search_matches={} syntax_spans={} cache_entries={}/{}",
            pane_label,
            title,
            self.view,
            self.total_ms,
            self.header_ms,
            self.body_ms,
            self.visible_rows,
            self.total_rows,
            self.text_len,
            self.scroll_x,
            self.scroll_y,
            self.scroll_delta_y,
            self.galley_hits,
            self.galley_misses,
            self.line_number_hits,
            self.line_number_misses,
            self.source_search_open,
            self.source_search_matches,
            self.syntax_spans,
            self.virtual_cache_entries,
            self.line_number_cache_entries
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct EditorRenderDebug {
    pub(super) line_count: usize,
    pub(super) text_line_count: usize,
    pub(super) galley_rows: usize,
    pub(super) scroll_y: f32,
    pub(super) row_height: f32,
    pub(super) clip_top: f32,
    pub(super) clip_bottom: f32,
    pub(super) viewport_top: f32,
    pub(super) viewport_bottom: f32,
    pub(super) galley_pos_y: f32,
    pub(super) first_visible_row: Option<usize>,
    pub(super) first_visible_top: Option<f32>,
    pub(super) last_visible_row: Option<usize>,
    pub(super) anchor_line: Option<usize>,
    pub(super) anchor_top: Option<f32>,
    pub(super) anchored_range: Option<std::ops::Range<usize>>,
    pub(super) current_line: Option<u32>,
}

pub(super) fn elapsed_ms(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}

pub(super) fn capture_virtual_editor_render_debug(
    line_count: usize,
    row_height: f32,
    scroll_y: f32,
    viewport_rect: egui::Rect,
    visible_rows: std::ops::Range<usize>,
    current_line: Option<u32>,
) -> EditorRenderDebug {
    let first_visible_row = (visible_rows.start < visible_rows.end).then_some(visible_rows.start);
    let first_visible_top =
        first_visible_row.map(|line| viewport_rect.top() + line as f32 * row_height - scroll_y);
    let last_visible_row = visible_rows.end.checked_sub(1);

    EditorRenderDebug {
        line_count,
        text_line_count: line_count,
        galley_rows: line_count,
        scroll_y,
        row_height,
        clip_top: viewport_rect.top(),
        clip_bottom: viewport_rect.bottom(),
        viewport_top: viewport_rect.top(),
        viewport_bottom: viewport_rect.bottom(),
        galley_pos_y: viewport_rect.top() - scroll_y,
        first_visible_row,
        first_visible_top,
        last_visible_row,
        anchor_line: Some(visible_rows.start),
        anchor_top: first_visible_top,
        anchored_range: Some(visible_rows),
        current_line,
    }
}

pub(super) fn capture_editor_render_debug(
    output: &egui::text_edit::TextEditOutput,
    line_count: usize,
    text_line_count: usize,
    row_height: f32,
    scroll_y: f32,
    viewport_rect: egui::Rect,
    current_line: Option<u32>,
) -> EditorRenderDebug {
    let visible_rows = visible_galley_row_range(output, line_count, viewport_rect);
    let first_visible_row = (visible_rows.start < visible_rows.end).then_some(visible_rows.start);
    let first_visible_top = first_visible_row.map(|index| galley_line_rect(output, index).top());
    let last_visible_row = visible_rows.end.checked_sub(1);

    EditorRenderDebug {
        line_count,
        text_line_count,
        galley_rows: output.galley.rows.len(),
        scroll_y,
        row_height,
        clip_top: output.text_clip_rect.top(),
        clip_bottom: output.text_clip_rect.bottom(),
        viewport_top: viewport_rect.top(),
        viewport_bottom: viewport_rect.bottom(),
        galley_pos_y: output.galley_pos.y,
        first_visible_row,
        first_visible_top,
        last_visible_row,
        anchor_line: Some(visible_rows.start),
        anchor_top: Some(galley_line_rect(output, visible_rows.start).top()),
        anchored_range: Some(visible_rows),
        current_line,
    }
}

pub(super) fn debug_or_none(value: &str) -> String {
    if value.is_empty() {
        "none".to_owned()
    } else {
        value.to_owned()
    }
}

pub(super) fn debug_instant_age_ms(value: Option<Instant>) -> String {
    value
        .map(|instant| format!("{:.0}", instant.elapsed().as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "none".to_owned())
}
