//! The Git history pane: the commit graph and its rows.

use super::*;

impl EditorPane {
    pub(super) fn git_history_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let total_start = Instant::now();
        let scroll_before_y = self.scroll.y;
        let Some(history_view) = self.git_history_view().cloned() else {
            return;
        };

        let header_start = Instant::now();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(history_view.root.display().to_string())
                    .small()
                    .weak(),
            );
            ui.separator();
            let commit_count = history_view
                .rows
                .iter()
                .filter(|row| row.commit.is_some())
                .count();
            ui.label(RichText::new(format!("{commit_count} commits")).small());
        });
        ui.separator();
        let header_ms = elapsed_ms(header_start);

        let viewport_height = ui.available_height();
        if let Some(delta) = diff_page_scroll_delta(ui, focused, viewport_height) {
            self.scroll.y = (self.scroll.y + delta).max(0.0);
        }
        self.apply_immediate_wheel_scroll(ui, self.scroll.max_y);

        let scroll_area = ScrollArea::both()
            .id_salt(format!("git_history_view_{:?}", self.pane))
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll.y);

        let mut clicked_commit = None;
        let body_start = Instant::now();
        let scroll_output = scroll_area.show(ui, |ui| {
            let min_width = git_history_min_width();
            ui.set_min_width(min_width);
            git_history_header_ui(ui, min_width);
            for row in &history_view.rows {
                let selected = row.commit.as_ref().is_some_and(|commit| {
                    history_view
                        .selected_commit_hash
                        .as_deref()
                        .is_some_and(|hash| hash == commit.hash.as_str())
                });
                let response = git_history_row_ui(ui, row, min_width, selected);
                if response.clicked()
                    && let Some(commit) = row.commit.clone()
                {
                    clicked_commit = Some(commit);
                }
            }
        });
        if let Some(commit) = clicked_commit {
            if let Some(history_view) = self.git_history_view_mut() {
                history_view.selected_commit_hash = Some(commit.hash.clone());
            }
            self.pending_action = Some(EditorAction::OpenGitCommitDetails {
                repository_name: history_view.repository_name.clone(),
                root: history_view.root.clone(),
                commit,
            });
            ui.ctx().request_repaint();
        }
        self.scroll.max_y =
            scroll_area_vertical_max_scroll_y(scroll_output.content_size, scroll_output.inner_rect);
        self.scroll.y = scroll_output.state.offset.y.clamp(0.0, self.scroll.max_y);
        self.debug.perf = Some(EditorPerfDebug {
            view: "git-history",
            total_ms: elapsed_ms(total_start),
            header_ms,
            body_ms: elapsed_ms(body_start),
            visible_rows: history_view.rows.len(),
            total_rows: history_view.rows.len(),
            text_len: 0,
            scroll_y: self.scroll.y,
            scroll_x: self.scroll.x,
            scroll_delta_y: self.scroll.y - scroll_before_y,
            ..Default::default()
        });
    }
}

#[derive(Debug, Clone)]
pub(super) struct GitHistoryView {
    pub(super) repository_name: String,
    pub(super) root: PathBuf,
    pub(super) rows: Vec<GitGraphRow>,
    pub(super) selected_commit_hash: Option<String>,
}

pub(super) fn git_graph_color(color_index: usize) -> Color32 {
    GIT_GRAPH_COLORS[color_index % GIT_GRAPH_COLORS.len()]
}

pub(super) const GIT_GRAPH_COLORS: [Color32; 8] = [
    Color32::from_rgb(0, 150, 220),
    Color32::from_rgb(225, 0, 152),
    Color32::from_rgb(37, 216, 70),
    Color32::from_rgb(242, 158, 0),
    Color32::from_rgb(169, 73, 235),
    Color32::from_rgb(86, 182, 194),
    Color32::from_rgb(224, 108, 117),
    Color32::from_rgb(229, 192, 123),
];

pub(super) fn git_history_min_width() -> f32 {
    1120.0
}

pub(super) fn git_history_header_ui(ui: &mut egui::Ui, width: f32) {
    let row_height = 24.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    for (label, x) in [
        ("Graph", 8.0),
        ("Description", 220.0),
        ("Date", 760.0),
        ("Author", 910.0),
        ("Commit", 1040.0),
    ] {
        painter.text(
            egui::pos2(rect.left() + x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::monospace(13.0),
            ui.visuals().strong_text_color(),
        );
    }
}

pub(super) fn git_history_row_ui(
    ui: &mut egui::Ui,
    row: &GitGraphRow,
    width: f32,
    selected: bool,
) -> egui::Response {
    let row_height = if row.commit.is_some() { 28.0 } else { 6.0 };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    }
    if response.hovered() {
        painter.rect_filled(
            rect,
            0.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().faint_bg_color
            },
        );
    }

    let graph_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 204.0, rect.bottom()));
    paint_git_history_graph(ui, &painter, graph_rect, &row.graph);

    let Some(commit) = row.commit.as_ref() else {
        return response;
    };

    let text_color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color()
    };
    let weak_text_color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let description_left = rect.left() + 220.0;
    let date_left = rect.left() + 760.0;
    let author_left = rect.left() + 910.0;
    let commit_left = rect.left() + 1040.0;
    let description_right = date_left - 12.0;
    let description_clip_rect = egui::Rect::from_min_max(
        egui::pos2(description_left, rect.top()),
        egui::pos2(description_right, rect.bottom()),
    );
    let description_painter = painter.with_clip_rect(description_clip_rect);
    let mut description_x = description_left;
    for reference in commit.refs.iter().take(4) {
        let width = (reference.chars().count() as f32 * 7.0 + 14.0).clamp(28.0, 128.0);
        if description_x + width > description_right {
            let ellipsis_font = FontId::monospace(11.0);
            let ellipsis = elide_text_to_width(
                ui,
                "...",
                ellipsis_font.clone(),
                weak_text_color,
                description_right - description_x,
            );
            if !ellipsis.is_empty() {
                description_painter.text(
                    egui::pos2(description_x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    ellipsis,
                    ellipsis_font,
                    weak_text_color,
                );
            }
            description_x = description_right;
            break;
        }
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(description_x, rect.center().y - 8.0),
            egui::vec2(width, 16.0),
        );
        description_painter.rect_filled(badge_rect, 3.0, git_ref_badge_color(reference));
        let ref_font = FontId::monospace(11.0);
        let reference_label =
            elide_text_to_width(ui, reference, ref_font.clone(), Color32::WHITE, width - 8.0);
        description_painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            reference_label,
            ref_font,
            Color32::WHITE,
        );
        description_x += width + 4.0;
    }

    let subject_font = editor_font_id();
    let subject = elide_text_to_width(
        ui,
        &commit.subject,
        subject_font.clone(),
        text_color,
        description_right - description_x,
    );
    description_painter.text(
        egui::pos2(description_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        subject,
        subject_font,
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        date_left,
        author_left - 10.0,
        &commit.date,
        FontId::monospace(13.0),
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        author_left,
        commit_left - 10.0,
        &commit.author,
        FontId::monospace(13.0),
        text_color,
    );
    paint_git_history_text_cell(
        ui,
        &painter,
        rect,
        commit_left,
        rect.right(),
        &commit.short_hash,
        FontId::monospace(13.0),
        weak_text_color,
    );
    response
}

pub(super) fn paint_git_history_text_cell(
    ui: &egui::Ui,
    painter: &egui::Painter,
    row_rect: egui::Rect,
    left: f32,
    right: f32,
    text: &str,
    font_id: FontId,
    color: Color32,
) {
    let cell_rect = egui::Rect::from_min_max(
        egui::pos2(left, row_rect.top()),
        egui::pos2(right.max(left), row_rect.bottom()),
    );
    let label = elide_text_to_width(ui, text, font_id.clone(), color, cell_rect.width());
    painter.with_clip_rect(cell_rect).text(
        cell_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        font_id,
        color,
    );
}

pub(super) fn paint_git_history_graph(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    graph: &str,
) {
    let cell_width = 10.0;
    let origin_x = rect.left() + 18.0;
    for (column, character) in graph.chars().enumerate() {
        let x = origin_x + column as f32 * cell_width;
        let color = git_graph_color(column / 2);
        let stroke = egui::Stroke::new(2.0, color);
        let top = rect.top();
        let middle = rect.center().y;
        let bottom = rect.bottom();
        match character {
            '|' => {
                painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
            }
            '*' => {
                painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
                painter.circle_filled(egui::pos2(x, middle), 5.0, color);
                painter.circle_stroke(
                    egui::pos2(x, middle),
                    5.0,
                    egui::Stroke::new(1.5, ui.visuals().extreme_bg_color),
                );
            }
            '/' => {
                painter.line_segment(
                    [
                        egui::pos2(x + cell_width * 0.5, top),
                        egui::pos2(x - cell_width * 0.5, bottom),
                    ],
                    stroke,
                );
            }
            '\\' => {
                painter.line_segment(
                    [
                        egui::pos2(x - cell_width * 0.5, top),
                        egui::pos2(x + cell_width * 0.5, bottom),
                    ],
                    stroke,
                );
            }
            '_' | '-' => {
                painter.line_segment(
                    [
                        egui::pos2(x - cell_width * 0.5, middle),
                        egui::pos2(x + cell_width * 0.5, middle),
                    ],
                    stroke,
                );
            }
            _ => {}
        }
    }
}

pub(super) fn git_ref_badge_color(reference: &str) -> Color32 {
    if reference.starts_with("origin/") {
        Color32::from_rgb(48, 119, 164)
    } else if reference.starts_with("tag:") {
        Color32::from_rgb(138, 118, 34)
    } else {
        Color32::from_rgb(20, 128, 180)
    }
}

pub(super) fn elide_text_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), color)
        .size()
        .x
        <= max_width
    {
        return text.to_owned();
    }

    const ELLIPSIS: &str = "...";
    let ellipsis_width = ui
        .painter()
        .layout_no_wrap(ELLIPSIS.to_owned(), font_id.clone(), color)
        .size()
        .x;
    if ellipsis_width > max_width {
        return String::new();
    }

    let characters = text.chars().collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = characters.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let mut candidate = characters[..middle].iter().collect::<String>();
        candidate.push_str(ELLIPSIS);
        let width = ui
            .painter()
            .layout_no_wrap(candidate, font_id.clone(), color)
            .size()
            .x;
        if width <= max_width {
            low = middle;
        } else {
            high = middle.saturating_sub(1);
        }
    }

    let mut result = characters[..low].iter().collect::<String>();
    result.push_str(ELLIPSIS);
    result
}
