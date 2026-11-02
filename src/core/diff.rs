//! Unified diff text as rows for the diff view: parsing `git diff` output into
//! display rows (files, hidden-line gaps, numbered lines), laying them out unified or
//! side by side, overview markers, and the text a selection copies. No painting here.

use std::collections::HashMap;

use crate::core::text::char_index_to_byte_index;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffDisplayMode {
    Unified,
    SideBySide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffSideSelectionPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffSideSelection {
    pub(crate) side: DiffSide,
    pub(crate) anchor: DiffSideSelectionPoint,
    pub(crate) head: DiffSideSelectionPoint,
    pub(crate) dragging: bool,
}

impl DiffSideSelection {
    pub(crate) fn normalized(self) -> Option<(DiffSideSelectionPoint, DiffSideSelectionPoint)> {
        if self.anchor == self.head {
            return None;
        }
        if (self.anchor.row, self.anchor.column) <= (self.head.row, self.head.column) {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffUnifiedSelection {
    pub(crate) anchor: DiffSideSelectionPoint,
    pub(crate) head: DiffSideSelectionPoint,
    pub(crate) dragging: bool,
}

impl DiffUnifiedSelection {
    pub(crate) fn normalized(self) -> Option<(DiffSideSelectionPoint, DiffSideSelectionPoint)> {
        if self.anchor == self.head {
            return None;
        }
        if (self.anchor.row, self.anchor.column) <= (self.head.row, self.head.column) {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffDisplayRow {
    Section(String),
    FileHeader {
        path: String,
        status: &'static str,
    },
    HiddenLines(usize),
    Line {
        line_label: String,
        content: String,
        kind: DiffLineKind,
    },
    Message(String),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DiffCell<'a> {
    pub(crate) line_label: &'a str,
    pub(crate) content: &'a str,
    pub(crate) kind: DiffLineKind,
}

pub(crate) enum DiffRenderRow<'a> {
    Section(&'a str),
    FileHeader {
        path: &'a str,
        status: &'static str,
    },
    HiddenLines(usize),
    UnifiedLine {
        line_label: &'a str,
        marker: &'static str,
        content: &'a str,
        kind: DiffLineKind,
    },
    SideBySide {
        old: DiffCell<'a>,
        new: DiffCell<'a>,
    },
    Message(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffMarkerKind {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DiffOverviewMarker {
    pub(crate) row_index: usize,
    pub(crate) kind: DiffMarkerKind,
}

#[derive(Debug, Default)]
pub(crate) struct DiffRenderSummary {
    pub(crate) total_rows: usize,
    pub(crate) markers: Vec<DiffOverviewMarker>,
    pub(crate) left_content_width: f32,
    pub(crate) right_content_width: f32,
}

pub(crate) fn diff_unified_background_kind(row: &DiffRenderRow<'_>) -> Option<DiffLineKind> {
    match row {
        DiffRenderRow::UnifiedLine { kind, .. } => diff_visible_background_kind(*kind),
        _ => None,
    }
}

pub(crate) fn diff_side_background_kind(
    row: &DiffRenderRow<'_>,
    side: DiffSide,
) -> Option<DiffLineKind> {
    match row {
        DiffRenderRow::SideBySide { old, new } => match side {
            DiffSide::Old => diff_visible_background_kind(old.kind),
            DiffSide::New => diff_visible_background_kind(new.kind),
        },
        _ => None,
    }
}

pub(crate) fn diff_visible_background_kind(kind: DiffLineKind) -> Option<DiffLineKind> {
    match kind {
        DiffLineKind::Added | DiffLineKind::Removed => Some(kind),
        DiffLineKind::Context => None,
    }
}

pub(crate) fn unified_diff_visible_text(marker: &str, content: &str) -> String {
    if marker.is_empty() {
        format!("  {content}")
    } else {
        format!("{marker} {content}")
    }
}

pub(crate) fn diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    mode: DiffDisplayMode,
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    match mode {
        DiffDisplayMode::Unified => unified_diff_render_rows(rows, collapsed_files),
        DiffDisplayMode::SideBySide => side_by_side_diff_render_rows(rows, collapsed_files),
    }
}

pub(crate) fn diff_overview_markers(rows: &[DiffRenderRow<'_>]) -> Vec<DiffOverviewMarker> {
    rows.iter()
        .enumerate()
        .filter_map(|(row_index, row)| {
            let kind = match row {
                DiffRenderRow::UnifiedLine {
                    kind: DiffLineKind::Added,
                    ..
                } => DiffMarkerKind::Added,
                DiffRenderRow::UnifiedLine {
                    kind: DiffLineKind::Removed,
                    ..
                } => DiffMarkerKind::Removed,
                DiffRenderRow::SideBySide { old, new } => {
                    match (
                        old.kind == DiffLineKind::Removed,
                        new.kind == DiffLineKind::Added,
                    ) {
                        (true, true) => DiffMarkerKind::Modified,
                        (true, false) => DiffMarkerKind::Removed,
                        (false, true) => DiffMarkerKind::Added,
                        (false, false) => return None,
                    }
                }
                _ => return None,
            };
            Some(DiffOverviewMarker { row_index, kind })
        })
        .collect()
}

pub(crate) fn unified_diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    let mut render_rows = Vec::new();
    let mut current_file_collapsed = false;
    for row in rows {
        match row {
            DiffDisplayRow::Section(title) => {
                current_file_collapsed = false;
                render_rows.push(DiffRenderRow::Section(title));
            }
            DiffDisplayRow::FileHeader { path, status } => {
                current_file_collapsed = collapsed_files.get(path).copied().unwrap_or(false);
                render_rows.push(DiffRenderRow::FileHeader { path, status });
            }
            DiffDisplayRow::HiddenLines(count) => {
                if current_file_collapsed {
                    continue;
                }
                render_rows.push(DiffRenderRow::HiddenLines(*count));
            }
            DiffDisplayRow::Line {
                line_label,
                content,
                kind,
            } => {
                if current_file_collapsed {
                    continue;
                }
                let marker = match kind {
                    DiffLineKind::Added => "+",
                    DiffLineKind::Removed => "-",
                    _ => "",
                };
                render_rows.push(DiffRenderRow::UnifiedLine {
                    line_label,
                    marker,
                    content,
                    kind: *kind,
                });
            }
            DiffDisplayRow::Message(text) => {
                if current_file_collapsed {
                    continue;
                }
                render_rows.push(DiffRenderRow::Message(text));
            }
        }
    }
    render_rows
}

pub(crate) fn side_by_side_diff_render_rows<'a>(
    rows: &'a [DiffDisplayRow],
    collapsed_files: &HashMap<String, bool>,
) -> Vec<DiffRenderRow<'a>> {
    let mut render_rows = Vec::new();
    let mut current_file_collapsed = false;
    let mut index = 0;

    while index < rows.len() {
        match &rows[index] {
            DiffDisplayRow::Section(title) => {
                current_file_collapsed = false;
                render_rows.push(DiffRenderRow::Section(title));
                index += 1;
            }
            DiffDisplayRow::FileHeader { path, status } => {
                current_file_collapsed = collapsed_files.get(path).copied().unwrap_or(false);
                render_rows.push(DiffRenderRow::FileHeader { path, status });
                index += 1;
            }
            DiffDisplayRow::HiddenLines(count) => {
                if !current_file_collapsed {
                    render_rows.push(DiffRenderRow::HiddenLines(*count));
                }
                index += 1;
            }
            DiffDisplayRow::Line {
                line_label,
                content,
                kind,
            } => {
                if current_file_collapsed {
                    index += 1;
                    continue;
                }

                if matches!(kind, DiffLineKind::Added | DiffLineKind::Removed) {
                    index = append_side_by_side_change_block(rows, index, &mut render_rows);
                    continue;
                }

                match kind {
                    DiffLineKind::Context => {
                        let cell = DiffCell {
                            line_label,
                            content,
                            kind: DiffLineKind::Context,
                        };
                        render_rows.push(DiffRenderRow::SideBySide {
                            old: cell,
                            new: cell,
                        });
                    }
                    DiffLineKind::Added | DiffLineKind::Removed => {}
                }
                index += 1;
            }
            DiffDisplayRow::Message(text) => {
                if !current_file_collapsed {
                    render_rows.push(DiffRenderRow::Message(text));
                }
                index += 1;
            }
        }
    }
    render_rows
}

pub(crate) fn append_side_by_side_change_block<'a>(
    rows: &'a [DiffDisplayRow],
    start_index: usize,
    render_rows: &mut Vec<DiffRenderRow<'a>>,
) -> usize {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut index = start_index;

    while let Some(DiffDisplayRow::Line {
        line_label,
        content,
        kind,
    }) = rows.get(index)
    {
        match kind {
            DiffLineKind::Removed => removed.push((line_label.as_str(), content.as_str())),
            DiffLineKind::Added => added.push((line_label.as_str(), content.as_str())),
            _ => break,
        }
        index += 1;
    }

    let row_count = removed.len().max(added.len());
    for row_index in 0..row_count {
        let old = removed.get(row_index).copied().unwrap_or(("", ""));
        let new = added.get(row_index).copied().unwrap_or(("", ""));
        render_rows.push(DiffRenderRow::SideBySide {
            old: DiffCell {
                line_label: old.0,
                content: old.1,
                kind: if old.0.is_empty() {
                    DiffLineKind::Context
                } else {
                    DiffLineKind::Removed
                },
            },
            new: DiffCell {
                line_label: new.0,
                content: new.1,
                kind: if new.0.is_empty() {
                    DiffLineKind::Context
                } else {
                    DiffLineKind::Added
                },
            },
        });
    }

    index
}

pub(crate) fn hidden_lines_label(count: usize) -> String {
    if count == 1 {
        "1 hidden line".to_owned()
    } else {
        format!("{count} hidden lines")
    }
}

pub(crate) fn diff_side_selection_columns_for_cell(
    selection: Option<&DiffSideSelection>,
    side: DiffSide,
    row_index: usize,
    char_count: usize,
) -> Option<(usize, usize)> {
    let selection = selection?;
    if selection.side != side {
        return None;
    }
    let (start, end) = selection.normalized()?;
    if row_index < start.row || row_index > end.row {
        return None;
    }
    let start_column = if row_index == start.row {
        start.column
    } else {
        0
    }
    .min(char_count);
    let end_column = if row_index == end.row {
        end.column
    } else {
        char_count
    }
    .min(char_count);
    (start_column < end_column).then_some((start_column, end_column))
}

pub(crate) fn diff_side_selection_text_from_render_rows(
    rows: &[DiffRenderRow<'_>],
    selection: DiffSideSelection,
) -> Option<String> {
    let (start, end) = selection.normalized()?;
    let mut lines = Vec::new();
    for row_index in start.row..=end.row {
        let Some(DiffRenderRow::SideBySide { old, new }) = rows.get(row_index) else {
            continue;
        };
        let cell = match selection.side {
            DiffSide::Old => old,
            DiffSide::New => new,
        };
        if cell.line_label.is_empty() {
            continue;
        }
        let content = cell.content;
        let Some((start_column, end_column)) = diff_side_selection_columns_for_cell(
            Some(&selection),
            selection.side,
            row_index,
            content.chars().count(),
        ) else {
            if content.is_empty() {
                lines.push(String::new());
            }
            continue;
        };
        let start_byte = char_index_to_byte_index(content, start_column).unwrap_or(content.len());
        let end_byte = char_index_to_byte_index(content, end_column).unwrap_or(content.len());
        lines.push(content[start_byte..end_byte].to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    let text = lines.join("\n");
    (!text.is_empty()).then_some(text)
}

pub(crate) fn diff_unified_selection_text_from_render_rows(
    rows: &[DiffRenderRow<'_>],
    selection: DiffUnifiedSelection,
) -> Option<String> {
    let (start, end) = selection.normalized()?;
    let mut lines = Vec::new();
    for row_index in start.row..=end.row {
        let Some(DiffRenderRow::UnifiedLine {
            marker, content, ..
        }) = rows.get(row_index)
        else {
            continue;
        };
        let line = if marker.is_empty() {
            format!("  {content}")
        } else {
            format!("{marker} {content}")
        };
        let Some((start_column, end_column)) = diff_unified_selection_columns_for_line(
            Some(&selection),
            row_index,
            line.chars().count(),
        ) else {
            if line.is_empty() {
                lines.push(String::new());
            }
            continue;
        };
        let start_byte = char_index_to_byte_index(&line, start_column).unwrap_or(line.len());
        let end_byte = char_index_to_byte_index(&line, end_column).unwrap_or(line.len());
        lines.push(line[start_byte..end_byte].to_owned());
    }

    if lines.is_empty() {
        return None;
    }
    let text = lines.join("\n");
    (!text.is_empty()).then_some(text)
}

pub(crate) fn diff_unified_selection_columns_for_line(
    selection: Option<&DiffUnifiedSelection>,
    row_index: usize,
    char_count: usize,
) -> Option<(usize, usize)> {
    let selection = selection?;
    let (start, end) = selection.normalized()?;
    if row_index < start.row || row_index > end.row {
        return None;
    }
    let start_column = if row_index == start.row {
        start.column
    } else {
        0
    }
    .min(char_count);
    let end_column = if row_index == end.row {
        end.column
    } else {
        char_count
    }
    .min(char_count);
    (start_column < end_column).then_some((start_column, end_column))
}

pub(crate) fn diff_row_char_width(row: &DiffDisplayRow) -> usize {
    match row {
        DiffDisplayRow::Section(title) => title.chars().count(),
        DiffDisplayRow::FileHeader { path, .. } => path.chars().count(),
        DiffDisplayRow::Message(text) => text.chars().count(),
        DiffDisplayRow::HiddenLines(count) => hidden_lines_label(*count).chars().count(),
        DiffDisplayRow::Line { content, .. } => content.chars().count(),
    }
}

pub(crate) fn parse_diff_display_rows(source: &str) -> Vec<DiffDisplayRow> {
    if source.trim().is_empty() {
        return vec![DiffDisplayRow::Message("No diffs.".to_owned())];
    }

    let mut parser = DiffRowParser::default();
    for raw_line in source.lines() {
        parser.push_line(raw_line);
    }

    let mut rows = parser.rows;
    if rows.is_empty() {
        rows.push(DiffDisplayRow::Message("No diffs.".to_owned()));
    }
    rows
}

/// Where parsing a unified diff has got to.
#[derive(Default)]
struct DiffRowParser {
    rows: Vec<DiffDisplayRow>,
    old_line: usize,
    new_line: usize,
    in_hunk: bool,
    has_seen_hunk_in_file: bool,
    current_file_row: Option<usize>,
}

impl DiffRowParser {
    fn push_line(&mut self, raw_line: &str) {
        if let Some(title) = raw_line.strip_prefix("diff --code-reader ") {
            self.rows.push(DiffDisplayRow::Section(title.to_owned()));
            self.in_hunk = false;
            self.has_seen_hunk_in_file = false;
            self.current_file_row = None;
        } else if raw_line.starts_with("diff --git ") {
            let path = diff_git_path(raw_line).unwrap_or_else(|| raw_line.to_owned());
            self.current_file_row = Some(self.rows.len());
            self.rows
                .push(DiffDisplayRow::FileHeader { path, status: "M" });
            self.in_hunk = false;
            self.has_seen_hunk_in_file = false;
        } else if raw_line.starts_with("new file mode ") {
            self.set_file_status("A");
        } else if raw_line.starts_with("deleted file mode ") {
            self.set_file_status("D");
        } else if raw_line.starts_with("index ")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
        {
            // File metadata the rows do not show.
        } else if raw_line.starts_with("@@") {
            self.start_hunk(raw_line);
        } else if !self.in_hunk {
            if !raw_line.trim().is_empty() {
                self.rows.push(DiffDisplayRow::Message(raw_line.to_owned()));
            }
        } else {
            self.push_hunk_line(raw_line);
        }
    }

    fn set_file_status(&mut self, new_status: &'static str) {
        if let Some(index) = self.current_file_row
            && let Some(DiffDisplayRow::FileHeader { status, .. }) = self.rows.get_mut(index)
        {
            *status = new_status;
        }
    }

    fn start_hunk(&mut self, raw_line: &str) {
        let Some((next_old_line, next_new_line)) = parse_unified_hunk_header(raw_line) else {
            return;
        };
        if self.has_seen_hunk_in_file {
            let hidden_count = next_old_line
                .saturating_sub(self.old_line)
                .max(next_new_line.saturating_sub(self.new_line));
            if hidden_count > 0 {
                self.rows.push(DiffDisplayRow::HiddenLines(hidden_count));
            }
        }
        self.old_line = next_old_line;
        self.new_line = next_new_line;
        self.in_hunk = true;
        self.has_seen_hunk_in_file = true;
    }

    fn push_hunk_line(&mut self, raw_line: &str) {
        if let Some(content) = raw_line.strip_prefix('+') {
            self.rows.push(DiffDisplayRow::Line {
                line_label: format!("+{}", self.new_line),
                content: content.to_owned(),
                kind: DiffLineKind::Added,
            });
            self.new_line += 1;
        } else if let Some(content) = raw_line.strip_prefix('-') {
            self.rows.push(DiffDisplayRow::Line {
                line_label: format!("-{}", self.old_line),
                content: content.to_owned(),
                kind: DiffLineKind::Removed,
            });
            self.old_line += 1;
        } else if raw_line.starts_with(' ') || raw_line.is_empty() {
            let content = raw_line.strip_prefix(' ').unwrap_or(raw_line).to_owned();
            self.rows.push(DiffDisplayRow::Line {
                line_label: self.new_line.to_string(),
                content,
                kind: DiffLineKind::Context,
            });
            self.old_line += 1;
            self.new_line += 1;
        }
    }
}

pub(crate) fn diff_git_path(line: &str) -> Option<String> {
    line.split_whitespace()
        .nth(3)
        .map(strip_git_diff_prefix)
        .filter(|path| !path.is_empty())
}

pub(crate) fn strip_git_diff_prefix(path: &str) -> String {
    path.strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path)
        .to_owned()
}

pub(crate) fn parse_unified_hunk_header(line: &str) -> Option<(usize, usize)> {
    Some((
        parse_hunk_line_number(line, '-')?,
        parse_hunk_line_number(line, '+')?,
    ))
}

pub(crate) fn parse_hunk_line_number(line: &str, marker: char) -> Option<usize> {
    let start = line.find(marker)? + marker.len_utf8();
    let digits = line[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    Added,
    Removed,
    Context,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff_display_rows_uses_real_hunk_line_numbers() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "index 111..222 100644\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -10,2 +10,3 @@ fn main() {\n",
            " keep();\n",
            "-old();\n",
            "+new();\n",
            "+more();\n",
        ));

        assert!(matches!(
            rows.first(),
            Some(DiffDisplayRow::FileHeader { path, status })
                if path == "src/main.rs" && *status == "M"
        ));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "10".to_owned(),
            content: "keep();".to_owned(),
            kind: DiffLineKind::Context,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "-11".to_owned(),
            content: "old();".to_owned(),
            kind: DiffLineKind::Removed,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "+11".to_owned(),
            content: "new();".to_owned(),
            kind: DiffLineKind::Added,
        }));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "+12".to_owned(),
            content: "more();".to_owned(),
            kind: DiffLineKind::Added,
        }));
    }

    #[test]
    fn parse_diff_display_rows_splits_files() {
        let rows = parse_diff_display_rows(concat!(
            "diff --code-reader Worktree Changes\n",
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/new.txt b/new.txt\n",
            "new file mode 100644\n",
            "--- /dev/null\n",
            "+++ b/new.txt\n",
            "@@ -0,0 +1 @@\n",
            "+hello\n",
        ));

        assert!(rows.contains(&DiffDisplayRow::Section("Worktree Changes".to_owned())));
        assert!(rows.contains(&DiffDisplayRow::FileHeader {
            path: "a.txt".to_owned(),
            status: "M",
        }));
        assert!(rows.contains(&DiffDisplayRow::FileHeader {
            path: "new.txt".to_owned(),
            status: "A",
        }));
    }

    #[test]
    fn parse_diff_display_rows_hides_hunk_and_no_newline_markers() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/test.md b/test.md\n",
            "--- a/test.md\n",
            "+++ b/test.md\n",
            "@@ -30,4 +30,6 @@ abcde\n",
            " line\n",
            "-old\n",
            "\\ No newline at end of file\n",
            "+new\n",
            "\\ No newline at end of file\n",
        ));

        assert!(!rows.iter().any(|row| matches!(
            row,
            DiffDisplayRow::Line { content, .. } if content.contains("No newline at end of file")
        )));
        assert!(rows.contains(&DiffDisplayRow::Line {
            line_label: "30".to_owned(),
            content: "line".to_owned(),
            kind: DiffLineKind::Context,
        }));
    }

    #[test]
    fn parse_diff_display_rows_adds_hidden_line_markers_between_hunks() {
        let rows = parse_diff_display_rows(concat!(
            "diff --git a/test.md b/test.md\n",
            "--- a/test.md\n",
            "+++ b/test.md\n",
            "@@ -10,1 +10,1 @@\n",
            "-old\n",
            "+new\n",
            "@@ -40,1 +40,1 @@\n",
            "-old2\n",
            "+new2\n",
        ));

        assert!(rows.contains(&DiffDisplayRow::HiddenLines(29)));
        assert_eq!(hidden_lines_label(29), "29 hidden lines");
        assert_eq!(hidden_lines_label(1), "1 hidden line");
    }

    #[test]
    fn diff_side_selection_text_copies_only_selected_side() {
        let rows = vec![
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "-1",
                    content: "old alpha",
                    kind: DiffLineKind::Removed,
                },
                new: DiffCell {
                    line_label: "+1",
                    content: "new alpha",
                    kind: DiffLineKind::Added,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "-2",
                    content: "old beta",
                    kind: DiffLineKind::Removed,
                },
                new: DiffCell {
                    line_label: "+2",
                    content: "new beta",
                    kind: DiffLineKind::Added,
                },
            },
        ];
        let selection = DiffSideSelection {
            side: DiffSide::Old,
            anchor: DiffSideSelectionPoint { row: 0, column: 4 },
            head: DiffSideSelectionPoint { row: 1, column: 3 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("alpha\nold".to_owned())
        );
    }

    #[test]
    fn diff_side_selection_text_uses_utf8_char_columns() {
        let rows = vec![DiffRenderRow::SideBySide {
            old: DiffCell {
                line_label: "-1",
                content: "abc",
                kind: DiffLineKind::Removed,
            },
            new: DiffCell {
                line_label: "+1",
                content: "あいう",
                kind: DiffLineKind::Added,
            },
        }];
        let selection = DiffSideSelection {
            side: DiffSide::New,
            anchor: DiffSideSelectionPoint { row: 0, column: 1 },
            head: DiffSideSelectionPoint { row: 0, column: 3 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("いう".to_owned())
        );
    }

    #[test]
    fn diff_side_selection_text_skips_side_by_side_placeholder_cells() {
        let rows = vec![
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "10",
                    content: "before",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "10",
                    content: "before",
                    kind: DiffLineKind::Context,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "",
                    content: "",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "+11",
                    content: "inserted",
                    kind: DiffLineKind::Added,
                },
            },
            DiffRenderRow::SideBySide {
                old: DiffCell {
                    line_label: "11",
                    content: "after",
                    kind: DiffLineKind::Context,
                },
                new: DiffCell {
                    line_label: "12",
                    content: "after",
                    kind: DiffLineKind::Context,
                },
            },
        ];
        let selection = DiffSideSelection {
            side: DiffSide::Old,
            anchor: DiffSideSelectionPoint { row: 0, column: 0 },
            head: DiffSideSelectionPoint { row: 2, column: 5 },
            dragging: false,
        };

        assert_eq!(
            diff_side_selection_text_from_render_rows(&rows, selection),
            Some("before\nafter".to_owned())
        );
    }

    #[test]
    fn diff_unified_selection_text_copies_visible_lines() {
        let rows = vec![
            DiffRenderRow::UnifiedLine {
                line_label: "10",
                marker: "",
                content: "context",
                kind: DiffLineKind::Context,
            },
            DiffRenderRow::UnifiedLine {
                line_label: "+11",
                marker: "+",
                content: "added",
                kind: DiffLineKind::Added,
            },
        ];
        let selection = DiffUnifiedSelection {
            anchor: DiffSideSelectionPoint { row: 0, column: 2 },
            head: DiffSideSelectionPoint { row: 1, column: 4 },
            dragging: false,
        };

        assert_eq!(
            diff_unified_selection_text_from_render_rows(&rows, selection),
            Some("context\n+ ad".to_owned())
        );
    }
}
