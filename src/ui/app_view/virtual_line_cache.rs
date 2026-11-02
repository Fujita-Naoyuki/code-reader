//! Laid-out lines and line numbers of the large-file editor, kept between frames so a
//! scroll only lays out the lines that came into view.

use super::*;

impl EditorPane {
    pub(super) fn cached_virtual_line_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_info: LineInfo,
        style: &VirtualLineStyle<'_>,
        highlight_line_span_range: std::ops::Range<usize>,
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let VirtualLineStyle {
            source_len,
            source_hash,
            highlight_spans,
            highlights,
        } = *style;
        let LineHighlights {
            bracket_pair,
            search_matches: source_search_matches,
            ..
        } = highlights;

        let mut frame_counters = frame_counters;
        let can_cache = source_search_matches.is_empty()
            && !line_contains_bracket_pair(line_info, bracket_pair);
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();

        if can_cache {
            let cache = self
                .virtual_editor
                .line_layout_cache
                .get_or_insert_with(|| VirtualLineLayoutCache {
                    source_len,
                    source_hash,
                    dark_mode,
                    entries: HashMap::new(),
                });
            if cache.source_len != source_len
                || cache.source_hash != source_hash
                || cache.dark_mode != dark_mode
            {
                *cache = VirtualLineLayoutCache {
                    source_len,
                    source_hash,
                    dark_mode,
                    entries: HashMap::new(),
                };
            }

            if let Some(entry) = cache.entries.get_mut(&line_number)
                && entry.line_start == line_info.start
                && entry.line_content_end == line_info.content_end
            {
                entry.last_used_frame = frame;
                if let Some(counters) = frame_counters.as_deref_mut() {
                    counters.galley_hits += 1;
                }
                return entry.galley.clone();
            }
        }
        if let Some(counters) = frame_counters {
            counters.galley_misses += 1;
        }

        let mut job = highlighted_line_layout_job(
            ui,
            &self.text,
            line_info,
            highlight_spans,
            highlight_line_span_range,
            highlights,
        );
        job.wrap.max_width = f32::INFINITY;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));

        if can_cache && let Some(cache) = &mut self.virtual_editor.line_layout_cache {
            cache.entries.insert(
                line_number,
                CachedVirtualLineLayout {
                    line_start: line_info.start,
                    line_content_end: line_info.content_end,
                    galley: galley.clone(),
                    last_used_frame: frame,
                },
            );
            prune_virtual_line_layout_cache(cache, frame);
        }

        galley
    }

    pub(super) fn cached_virtual_line_number_galley(
        &mut self,
        ui: &egui::Ui,
        line_number: usize,
        line_count: usize,
        active: bool,
        (inactive_color, active_color): (Color32, Color32),
        frame_counters: Option<&mut VirtualFrameCounters>,
    ) -> Arc<egui::Galley> {
        let mut frame_counters = frame_counters;
        let dark_mode = ui.visuals().dark_mode;
        let frame = ui.ctx().cumulative_frame_nr();
        let cache = self
            .virtual_editor
            .line_number_layout_cache
            .get_or_insert_with(|| VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            });
        if cache.line_count != line_count
            || cache.dark_mode != dark_mode
            || cache.inactive_color != inactive_color
            || cache.active_color != active_color
        {
            *cache = VirtualLineNumberLayoutCache {
                line_count,
                dark_mode,
                inactive_color,
                active_color,
                entries: HashMap::new(),
            };
        }

        let key = (line_number, active);
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used_frame = frame;
            if let Some(counters) = frame_counters.as_deref_mut() {
                counters.line_number_hits += 1;
            }
            return entry.galley.clone();
        }
        if let Some(counters) = frame_counters {
            counters.line_number_misses += 1;
        }

        let color = if active { active_color } else { inactive_color };
        let galley =
            ui.painter()
                .layout_no_wrap((line_number + 1).to_string(), editor_font_id(), color);
        cache.entries.insert(
            key,
            CachedVirtualLineNumberLayout {
                galley: galley.clone(),
                last_used_frame: frame,
            },
        );
        prune_virtual_line_number_layout_cache(cache, frame);

        galley
    }
}

pub(super) const VIRTUAL_LINE_LAYOUT_CACHE_LIMIT: usize = 4096;

pub(super) const VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES: u64 = 180;

pub(super) struct CachedVirtualLineLayout {
    pub(super) line_start: usize,
    pub(super) line_content_end: usize,
    pub(super) galley: Arc<egui::Galley>,
    pub(super) last_used_frame: u64,
}

pub(super) struct VirtualLineLayoutCache {
    pub(super) source_len: usize,
    pub(super) source_hash: u64,
    pub(super) dark_mode: bool,
    pub(super) entries: HashMap<usize, CachedVirtualLineLayout>,
}

pub(super) struct CachedVirtualLineNumberLayout {
    pub(super) galley: Arc<egui::Galley>,
    pub(super) last_used_frame: u64,
}

pub(super) struct VirtualLineNumberLayoutCache {
    pub(super) line_count: usize,
    pub(super) dark_mode: bool,
    pub(super) inactive_color: Color32,
    pub(super) active_color: Color32,
    pub(super) entries: HashMap<(usize, bool), CachedVirtualLineNumberLayout>,
}

pub(super) fn line_contains_bracket_pair(
    line: LineInfo,
    bracket_pair: Option<(usize, usize)>,
) -> bool {
    let Some((left, right)) = bracket_pair else {
        return false;
    };
    [left, right]
        .into_iter()
        .any(|index| (line.start..line.content_end).contains(&index))
}

pub(super) fn prune_virtual_line_layout_cache(cache: &mut VirtualLineLayoutCache, frame: u64) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(line_number, entry)| (*line_number, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (line_number, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&line_number);
    }
}

pub(super) fn prune_virtual_line_number_layout_cache(
    cache: &mut VirtualLineNumberLayoutCache,
    frame: u64,
) {
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let cutoff = frame.saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_KEEP_FRAMES);
    cache
        .entries
        .retain(|_, entry| entry.last_used_frame >= cutoff);
    if cache.entries.len() <= VIRTUAL_LINE_LAYOUT_CACHE_LIMIT {
        return;
    }

    let mut entries_by_age = cache
        .entries
        .iter()
        .map(|(key, entry)| (*key, entry.last_used_frame))
        .collect::<Vec<_>>();
    entries_by_age.sort_by_key(|(_, last_used_frame)| *last_used_frame);
    let remove_count = cache
        .entries
        .len()
        .saturating_sub(VIRTUAL_LINE_LAYOUT_CACHE_LIMIT);
    for (key, _) in entries_by_age.into_iter().take(remove_count) {
        cache.entries.remove(&key);
    }
}
