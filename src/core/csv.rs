//! CSV documents for the grid view: parsing, serialization that keeps the file's line
//! endings, and row / column selection edits (copy, paste, delete). No egui here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CsvCellPosition {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsvSelection {
    Rows { start: usize, end: usize },
    Columns { start: usize, end: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsvSelectionKind {
    Rows,
    Columns,
}

impl CsvSelection {
    pub(crate) fn row_range(self) -> Option<(usize, usize)> {
        match self {
            Self::Rows { start, end } => Some(ordered_range(start, end)),
            Self::Columns { .. } => None,
        }
    }

    pub(crate) fn column_range(self) -> Option<(usize, usize)> {
        match self {
            Self::Columns { start, end } => Some(ordered_range(start, end)),
            Self::Rows { .. } => None,
        }
    }

    pub(crate) fn contains(self, row: usize, column: usize) -> bool {
        if let Some((start, end)) = self.row_range() {
            return (start..=end).contains(&row);
        }

        if let Some((start, end)) = self.column_range() {
            return (start..=end).contains(&column);
        }

        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CsvDocument {
    pub(crate) rows: Vec<Vec<String>>,
}

impl CsvDocument {
    pub(crate) fn column_count(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub(crate) fn ensure_cell(&mut self, row: usize, column: usize) {
        let column_count = self.column_count().max(column + 1);

        while self.rows.len() <= row {
            self.rows.push(vec![String::new(); column_count]);
        }

        for fields in &mut self.rows {
            if fields.len() < column_count {
                fields.resize(column_count, String::new());
            }
        }
    }

    pub(crate) fn cell_mut(&mut self, row: usize, column: usize) -> &mut String {
        self.ensure_cell(row, column);

        if self.rows[row].len() <= column {
            self.rows[row].resize(column + 1, String::new());
        }

        &mut self.rows[row][column]
    }
}

pub(crate) fn parse_csv(source: &str) -> Result<CsvDocument, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = source.chars().peekable();
    let mut in_quotes = false;
    let mut field_started_with_quote = false;
    let mut just_finished_row = false;

    while let Some(character) = chars.next() {
        if in_quotes {
            match character {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                _ => field.push(character),
            }
            just_finished_row = false;
            continue;
        }

        match character {
            '"' if field.is_empty() && !field_started_with_quote => {
                in_quotes = true;
                field_started_with_quote = true;
                just_finished_row = false;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                field_started_with_quote = false;
                just_finished_row = false;
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started_with_quote = false;
                just_finished_row = true;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started_with_quote = false;
                just_finished_row = true;
            }
            _ => {
                field.push(character);
                just_finished_row = false;
            }
        }
    }

    if in_quotes {
        return Err("unterminated quoted field".to_owned());
    }

    if !just_finished_row || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    Ok(CsvDocument { rows })
}

/// Row separator and final newline of a CSV file, kept when the grid rewrites it so a
/// one-cell edit stays a one-line diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CsvTextStyle {
    pub(crate) line_ending: &'static str,
    pub(crate) trailing_newline: bool,
}

impl CsvTextStyle {
    pub(crate) fn of(text: &str) -> Self {
        Self {
            line_ending: if text.contains("\r\n") { "\r\n" } else { "\n" },
            trailing_newline: text.ends_with('\n'),
        }
    }

    pub(crate) fn join(self, rows: &[Vec<String>]) -> String {
        let mut text = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|field| serialize_csv_field(field))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(self.line_ending);
        if self.trailing_newline && !text.is_empty() {
            text.push_str(self.line_ending);
        }
        text
    }
}

pub(crate) fn serialize_csv(document: &CsvDocument, style: CsvTextStyle) -> String {
    style.join(&normalized_csv_rows(document, false))
}

pub(crate) fn serialize_csv_for_save(document: &CsvDocument, style: CsvTextStyle) -> String {
    style.join(&normalized_csv_rows(document, true))
}

pub(crate) fn copy_csv_selection_as_csv(document: &CsvDocument, selection: CsvSelection) -> String {
    let rows = normalized_csv_rows(document, false);
    if rows.is_empty() {
        return String::new();
    }

    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return String::new();
    }

    let (row_start, row_end) = selection
        .row_range()
        .unwrap_or((0, rows.len().saturating_sub(1)));
    let (column_start, column_end) = selection
        .column_range()
        .unwrap_or((0, column_count.saturating_sub(1)));
    let row_end = row_end.min(rows.len().saturating_sub(1));
    let column_end = column_end.min(column_count.saturating_sub(1));

    (row_start..=row_end)
        .map(|row| {
            (column_start..=column_end)
                .map(|column| {
                    rows.get(row)
                        .and_then(|fields| fields.get(column))
                        .map(|field| serialize_csv_field(field))
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn paste_csv_selection_from_text(
    document: &mut CsvDocument,
    selection: CsvSelection,
    text: &str,
) -> Result<usize, String> {
    let pasted = parse_csv(text)?;
    let (target_row, target_column) = match selection {
        CsvSelection::Rows { start, end } => (ordered_range(start, end).0, 0),
        CsvSelection::Columns { start, end } => (0, ordered_range(start, end).0),
    };
    let mut cell_count = 0;

    for (row_offset, row) in pasted.rows.iter().enumerate() {
        for (column_offset, field) in row.iter().enumerate() {
            let row = target_row + row_offset;
            let column = target_column + column_offset;
            *document.cell_mut(row, column) = field.clone();
            cell_count += 1;
        }
    }

    Ok(cell_count)
}

pub(crate) fn delete_csv_selection(
    document: &mut CsvDocument,
    selection: CsvSelection,
) -> (CsvSelection, String) {
    match selection {
        CsvSelection::Rows { start, end } => {
            let (start, end) = ordered_range(start, end);
            let row_count = document.rows.len().max(1);
            let start = start.min(row_count.saturating_sub(1));
            let end = end.min(row_count.saturating_sub(1));

            if !document.rows.is_empty() {
                document.rows.drain(start..=end);
            }
            if document.rows.is_empty() {
                document.rows.push(vec![String::new()]);
            }

            let next = start.min(document.rows.len().saturating_sub(1));
            let label = if start == end {
                format!("row {}", start + 1)
            } else {
                format!("rows {}-{}", start + 1, end + 1)
            };

            (
                CsvSelection::Rows {
                    start: next,
                    end: next,
                },
                label,
            )
        }
        CsvSelection::Columns { start, end } => {
            let (start, end) = ordered_range(start, end);
            let column_count = document.column_count().max(1);
            let start = start.min(column_count.saturating_sub(1));
            let end = end.min(column_count.saturating_sub(1));

            if document.rows.is_empty() {
                document.rows.push(vec![String::new()]);
            }
            for row in &mut document.rows {
                row.resize(column_count, String::new());
                row.drain(start..=end);
                if row.is_empty() {
                    row.push(String::new());
                }
            }

            let next_column_count = document.column_count().max(1);
            let next = start.min(next_column_count.saturating_sub(1));
            let label = if start == end {
                format!("column {}", csv_column_label(start))
            } else {
                format!(
                    "columns {}-{}",
                    csv_column_label(start),
                    csv_column_label(end)
                )
            };

            (
                CsvSelection::Columns {
                    start: next,
                    end: next,
                },
                label,
            )
        }
    }
}

pub(crate) fn normalized_csv_rows(
    document: &CsvDocument,
    trim_extra_trailing_empty_rows: bool,
) -> Vec<Vec<String>> {
    let mut rows = document.rows.clone();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);

    for row in &mut rows {
        row.resize(column_count, String::new());
    }

    if trim_extra_trailing_empty_rows {
        let mut trailing_empty_rows = 0;
        for row in rows.iter().rev() {
            if row.iter().all(String::is_empty) {
                trailing_empty_rows += 1;
            } else {
                break;
            }
        }

        if trailing_empty_rows > 0 {
            rows.truncate(rows.len().saturating_sub(trailing_empty_rows));
        }
    }

    rows
}

pub(crate) fn serialize_csv_field(field: &str) -> String {
    let needs_quotes = field.contains([',', '"', '\n', '\r'])
        || field.starts_with([' ', '\t'])
        || field.ends_with([' ', '\t']);

    if !needs_quotes {
        return field.to_owned();
    }

    format!("\"{}\"", field.replace('"', "\"\""))
}

pub(crate) fn ordered_range(start: usize, end: usize) -> (usize, usize) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

pub(crate) fn csv_selection_label(selection: CsvSelection) -> String {
    match selection {
        CsvSelection::Rows { start, end } => {
            let (start, end) = ordered_range(start, end);
            if start == end {
                format!("Selected row {}", start + 1)
            } else {
                format!("Selected rows {}-{}", start + 1, end + 1)
            }
        }
        CsvSelection::Columns { start, end } => {
            let (start, end) = ordered_range(start, end);
            if start == end {
                format!("Selected column {}", csv_column_label(start))
            } else {
                format!(
                    "Selected columns {}-{}",
                    csv_column_label(start),
                    csv_column_label(end)
                )
            }
        }
    }
}

pub(crate) fn csv_cell_display_text(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

pub(crate) fn csv_column_label(mut column: usize) -> String {
    let mut label = String::new();
    column += 1;

    while column > 0 {
        let offset = (column - 1) % 26;
        label.insert(0, (b'A' + offset as u8) as char);
        column = (column - 1) / 26;
    }

    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_supports_quotes_commas_and_newlines() {
        let document =
            parse_csv("name,note\nAlice,\"hello, world\"\nBob,\"line 1\nline 2\"").unwrap();

        assert_eq!(
            document.rows,
            vec![
                vec!["name".to_owned(), "note".to_owned()],
                vec!["Alice".to_owned(), "hello, world".to_owned()],
                vec!["Bob".to_owned(), "line 1\nline 2".to_owned()],
            ]
        );
    }

    #[test]
    fn serialize_csv_quotes_only_when_needed() {
        let document = CsvDocument {
            rows: vec![vec![
                "plain".to_owned(),
                "a,b".to_owned(),
                "with \"quote\"".to_owned(),
                " tail ".to_owned(),
            ]],
        };

        assert_eq!(
            serialize_csv(&document, CsvTextStyle::of("")),
            "plain,\"a,b\",\"with \"\"quote\"\"\",\" tail \""
        );
    }

    #[test]
    fn ensure_cell_expands_rows_and_columns() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned()]],
        };

        document.ensure_cell(1, 2);

        assert_eq!(
            document.rows,
            vec![
                vec!["a".to_owned(), String::new(), String::new()],
                vec![String::new(), String::new(), String::new()],
            ]
        );
    }

    #[test]
    fn serialize_csv_for_save_removes_trailing_empty_rows() {
        let document = CsvDocument {
            rows: vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec![String::new(), String::new()],
                vec![String::new(), String::new()],
            ],
        };

        assert_eq!(serialize_csv(&document, CsvTextStyle::of("")), "a,b\n,\n,");
        assert_eq!(
            serialize_csv_for_save(&document, CsvTextStyle::of("")),
            "a,b"
        );
    }

    #[test]
    fn serialize_csv_keeps_line_endings_and_final_newline() {
        let document = parse_csv("a,b\r\n1,2\r\n").expect("parses");
        let style = CsvTextStyle::of("a,b\r\n1,2\r\n");
        assert_eq!(serialize_csv(&document, style), "a,b\r\n1,2\r\n");
        assert_eq!(serialize_csv_for_save(&document, style), "a,b\r\n1,2\r\n");
        let empty = CsvDocument { rows: Vec::new() };
        assert_eq!(serialize_csv(&empty, style), "");
    }

    #[test]
    fn copy_csv_column_selection_keeps_csv_format() {
        let document = CsvDocument {
            rows: vec![
                vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
                vec!["1".to_owned(), "2,2".to_owned(), "3".to_owned()],
                vec!["4".to_owned(), "5".to_owned(), "6".to_owned()],
            ],
        };

        assert_eq!(
            copy_csv_selection_as_csv(&document, CsvSelection::Columns { start: 1, end: 2 }),
            "B,C\n\"2,2\",3\n5,6"
        );
    }

    #[test]
    fn copy_csv_row_selection_keeps_csv_format() {
        let document = CsvDocument {
            rows: vec![
                vec!["A".to_owned(), "B".to_owned()],
                vec!["hello".to_owned(), "with \"quote\"".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ],
        };

        assert_eq!(
            copy_csv_selection_as_csv(&document, CsvSelection::Rows { start: 1, end: 2 }),
            "hello,\"with \"\"quote\"\"\"\ntail,row"
        );
    }

    #[test]
    fn paste_csv_column_selection_writes_from_selected_column() {
        let mut document = CsvDocument {
            rows: vec![
                vec![
                    "a".to_owned(),
                    "b".to_owned(),
                    "c".to_owned(),
                    "d1".to_owned(),
                ],
                vec![
                    "aaaa".to_owned(),
                    "bbb".to_owned(),
                    "ccc".to_owned(),
                    "d2".to_owned(),
                ],
            ],
        };

        let cell_count = paste_csv_selection_from_text(
            &mut document,
            CsvSelection::Columns { start: 4, end: 4 },
            "d1\nd2",
        )
        .unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(document.rows[0][4], "d1");
        assert_eq!(document.rows[1][4], "d2");
    }

    #[test]
    fn paste_csv_row_selection_writes_from_first_column() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned(), "b".to_owned()]],
        };

        let cell_count = paste_csv_selection_from_text(
            &mut document,
            CsvSelection::Rows { start: 1, end: 1 },
            "x,y",
        )
        .unwrap();

        assert_eq!(cell_count, 2);
        assert_eq!(
            document.rows,
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["x".to_owned(), "y".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_row_selection_removes_selected_rows() {
        let mut document = CsvDocument {
            rows: vec![
                vec!["h1".to_owned(), "h2".to_owned()],
                vec!["a".to_owned(), "b".to_owned()],
                vec!["c".to_owned(), "d".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Rows { start: 1, end: 2 });

        assert_eq!(label, "rows 2-3");
        assert_eq!(selection, CsvSelection::Rows { start: 1, end: 1 });
        assert_eq!(
            document.rows,
            vec![
                vec!["h1".to_owned(), "h2".to_owned()],
                vec!["tail".to_owned(), "row".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_column_selection_removes_selected_columns() {
        let mut document = CsvDocument {
            rows: vec![
                vec![
                    "A".to_owned(),
                    "B".to_owned(),
                    "C".to_owned(),
                    "D".to_owned(),
                ],
                vec![
                    "1".to_owned(),
                    "2".to_owned(),
                    "3".to_owned(),
                    "4".to_owned(),
                ],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Columns { start: 1, end: 2 });

        assert_eq!(label, "columns B-C");
        assert_eq!(selection, CsvSelection::Columns { start: 1, end: 1 });
        assert_eq!(
            document.rows,
            vec![
                vec!["A".to_owned(), "D".to_owned()],
                vec!["1".to_owned(), "4".to_owned()],
            ]
        );
    }

    #[test]
    fn delete_csv_all_rows_keeps_one_empty_cell() {
        let mut document = CsvDocument {
            rows: vec![vec!["a".to_owned()], vec!["b".to_owned()]],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Rows { start: 0, end: 1 });

        assert_eq!(label, "rows 1-2");
        assert_eq!(selection, CsvSelection::Rows { start: 0, end: 0 });
        assert_eq!(document.rows, vec![vec![String::new()]]);
    }

    #[test]
    fn delete_csv_all_columns_keeps_one_empty_column() {
        let mut document = CsvDocument {
            rows: vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["c".to_owned(), "d".to_owned()],
            ],
        };

        let (selection, label) =
            delete_csv_selection(&mut document, CsvSelection::Columns { start: 0, end: 1 });

        assert_eq!(label, "columns A-B");
        assert_eq!(selection, CsvSelection::Columns { start: 0, end: 0 });
        assert_eq!(
            document.rows,
            vec![vec![String::new()], vec![String::new()]]
        );
    }
}
