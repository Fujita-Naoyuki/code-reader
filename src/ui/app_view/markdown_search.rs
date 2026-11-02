//! The rendered Markdown search box and moving between its matches.

use super::*;

impl EditorPane {
    pub(super) fn markdown_search_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
    ) {
        let search_id = ui.id().with(format!("markdown_search_{:?}", self.pane));
        let mut close = false;
        let mut go_next = false;
        let mut go_previous = false;
        let mut keep_focus = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Find").small().strong());
            let input_had_focus = ui.memory(|memory| memory.has_focus(search_id));
            if input_had_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
            }
            // egui drops focus at the start of the frame that carries a plain Escape, so check
            // who had focus in the previous frame.
            if ui.memory(|memory| memory.had_focus_last_frame(search_id))
                && ui.input(raw_escape_pressed)
            {
                close = true;
            }

            let response = ui.add(
                TextEdit::singleline(&mut self.markdown.search.query)
                    .id(search_id)
                    .hint_text("Search rendered markdown")
                    .desired_width(220.0),
            );
            if response.changed() {
                self.markdown.search.current_match = 0;
                self.markdown.search.scrolled_match = None;
            }
            if self.markdown.search.focus_requested {
                response.request_focus();
                self.markdown.search.focus_requested = false;
            }
            let input_has_focus =
                response.has_focus() || ui.memory(|memory| memory.has_focus(search_id));
            if !input_had_focus && input_has_focus {
                match ui.input(raw_search_enter_direction) {
                    Some(1) => {
                        go_next = true;
                        keep_focus = true;
                    }
                    Some(-1) => {
                        go_previous = true;
                        keep_focus = true;
                    }
                    _ => {}
                }
                if ui.input(raw_escape_pressed) {
                    close = true;
                }
            }

            let count_label = if self.markdown.search.query.trim().is_empty() {
                "0 results".to_owned()
            } else if matches.is_empty() {
                "No results".to_owned()
            } else {
                format!(
                    "{}/{}",
                    self.markdown.search.current_match + 1,
                    matches.len()
                )
            };
            ui.label(RichText::new(count_label).small().weak());

            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Previous"))
                .on_hover_text("Previous search match")
                .clicked()
            {
                go_previous = true;
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("Next"))
                .on_hover_text("Next search match")
                .clicked()
            {
                go_next = true;
            }
            if ui.button("Close").on_hover_text("Close search").clicked() {
                close = true;
            }

            if keep_focus && !close {
                ui.memory_mut(|memory| memory.request_focus(search_id));
            }
        });

        if close {
            self.markdown.search.open = false;
            self.markdown.search.focus_requested = false;
            self.markdown.search.query.clear();
            self.markdown.search.current_match = 0;
            self.markdown.search.match_count = 0;
            self.markdown.search.scrolled_match = None;
            return;
        }

        if go_next {
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        } else {
            self.navigate_to_current_markdown_match(matches);
        }
    }

    pub(super) fn handle_markdown_search_global_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
    ) {
        if matches.is_empty() {
            return;
        }

        let mut go_next = false;
        let mut go_previous = false;
        ui.input_mut(|input| {
            if input.key_pressed(egui::Key::F3) {
                if input.modifiers.shift {
                    go_previous = true;
                } else {
                    go_next = true;
                }
                let _ = input.consume_key(input.modifiers, egui::Key::F3);
            }
        });

        if go_next {
            self.move_markdown_search_match(document, matches, 1);
        } else if go_previous {
            self.move_markdown_search_match(document, matches, -1);
        }
    }

    pub(super) fn move_markdown_search_match(
        &mut self,
        document: &MarkdownDocument,
        matches: &[MarkdownSearchMatch],
        delta: isize,
    ) {
        if matches.is_empty() {
            return;
        }

        let len = matches.len() as isize;
        let current = self.markdown.search.current_match as isize;
        self.markdown.search.current_match = (current + delta).rem_euclid(len) as usize;
        self.markdown.search.scrolled_match = None;
        self.navigate_to_current_markdown_match(matches);

        if let Some(search_match) = self.markdown_active_search_match(matches)
            && let Some(page) = document.page_by_id(&search_match.page_id)
        {
            let position = Position::new(page.source_range.start.line as u32, 0);
            self.cursor_position = Some(position);
        }
    }

    pub(super) fn navigate_to_current_markdown_match(&mut self, matches: &[MarkdownSearchMatch]) {
        let Some(search_match) = self.markdown_active_search_match(matches) else {
            return;
        };
        if self.markdown.active_page_id.as_deref() != Some(search_match.page_id.as_str()) {
            self.markdown.active_heading_id = None;
        }
        self.markdown.active_page_id = Some(search_match.page_id.clone());
    }

    pub(super) fn markdown_active_search_match(
        &self,
        matches: &[MarkdownSearchMatch],
    ) -> Option<MarkdownSearchMatch> {
        if self.markdown.search.query.trim().is_empty() {
            return None;
        }
        matches.get(self.markdown.search.current_match).cloned()
    }

    pub(super) fn should_scroll_to_markdown_search_match(
        &mut self,
        search_query: Option<&str>,
        text: &str,
    ) -> bool {
        let Some(query) = search_query.filter(|query| !query.is_empty()) else {
            return false;
        };
        if self.markdown.search.scrolled_match == Some(self.markdown.search.current_match) {
            return false;
        }
        if text.to_lowercase().contains(&query.to_lowercase()) {
            self.markdown.search.scrolled_match = Some(self.markdown.search.current_match);
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct MarkdownSearchState {
    pub(super) open: bool,
    pub(super) focus_requested: bool,
    pub(super) query: String,
    pub(super) current_match: usize,
    pub(super) match_count: usize,
    pub(super) scrolled_match: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownSearchMatch {
    pub(super) page_id: String,
    pub(super) page_index: usize,
    pub(super) start: usize,
    pub(super) end: usize,
}

pub(super) fn markdown_search_matches(
    document: &MarkdownDocument,
    query: &str,
) -> Vec<MarkdownSearchMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    for (page_index, page) in document.pages.iter().enumerate() {
        let page_text = document.page_rendered_text(page);
        let page_text_lower = page_text.to_lowercase();
        let mut cursor = 0;

        while let Some(relative_start) = page_text_lower[cursor..].find(&query_lower) {
            let start = cursor + relative_start;
            let end = start + query_lower.len();
            matches.push(MarkdownSearchMatch {
                page_id: page.id.clone(),
                page_index,
                start,
                end,
            });
            cursor = end.max(start + 1);
        }
    }

    matches
}

pub(super) fn markdown_search_match_background(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(87, 72, 24)
    } else {
        Color32::from_rgb(255, 238, 160)
    }
}
