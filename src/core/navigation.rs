//! Per-pane Back / Forward history of definition jumps and other navigations.

use std::path::{Path, PathBuf};

use crate::core::location::Position;

/// Back steps kept per pane; the breadcrumb shows only the latest ones and "..." before them.
pub(crate) const NAVIGATION_HISTORY_LIMIT: usize = 50;

pub(crate) fn navigation_point_label(point: &NavigationPoint) -> String {
    let file_name = point
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unnamed>");
    format!("{file_name}:{}", point.position.line + 1)
}

pub(crate) fn navigation_point_hover_text(point: &NavigationPoint) -> String {
    format!(
        "{}:{}:{}",
        point.path.display(),
        point.position.line + 1,
        point.position.character + 1
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigationPoint {
    pub(crate) path: PathBuf,
    pub(crate) position: Position,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NavigationHistory {
    pub(crate) back: Vec<NavigationPoint>,
    pub(crate) forward: Vec<NavigationPoint>,
}

impl NavigationHistory {
    pub(crate) fn push_back(&mut self, point: NavigationPoint) {
        if self.back.last() != Some(&point) {
            self.back.push(point);
        }
        self.forward.clear();
        self.trim_back();
    }

    pub(crate) fn go_back(&mut self, current: Option<NavigationPoint>) -> Option<NavigationPoint> {
        let previous = self.back.pop()?;
        if let Some(current) = current {
            push_unique(&mut self.forward, current);
        }
        Some(previous)
    }

    pub(crate) fn go_forward(
        &mut self,
        current: Option<NavigationPoint>,
    ) -> Option<NavigationPoint> {
        let next = self.forward.pop()?;
        if let Some(current) = current {
            push_unique(&mut self.back, current);
            self.trim_back();
        }
        Some(next)
    }

    pub(crate) fn open_back_entry(
        &mut self,
        index: usize,
        current: Option<NavigationPoint>,
    ) -> Option<NavigationPoint> {
        if index >= self.back.len() {
            return None;
        }

        let point = self.back[index].clone();
        let later_entries = self.back.drain(index + 1..).collect::<Vec<_>>();
        self.back.truncate(index);

        if let Some(current) = current {
            push_unique(&mut self.forward, current);
        }
        for entry in later_entries.into_iter().rev() {
            push_unique(&mut self.forward, entry);
        }

        Some(point)
    }

    pub(crate) fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }

    pub(crate) fn remap_path(&mut self, old_path: &Path, new_path: &Path) {
        for point in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if point.path == old_path {
                point.path = new_path.to_path_buf();
            }
        }
    }

    pub(crate) fn retain_path_not(&mut self, path: &Path) {
        self.back.retain(|point| point.path != path);
        self.forward.retain(|point| point.path != path);
    }

    pub(crate) fn trim_back(&mut self) {
        if self.back.len() > NAVIGATION_HISTORY_LIMIT {
            let drain_count = self.back.len() - NAVIGATION_HISTORY_LIMIT;
            self.back.drain(0..drain_count);
        }
    }
}

pub(crate) fn push_unique(points: &mut Vec<NavigationPoint>, point: NavigationPoint) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_navigation_point(file_name: &str, line: u32) -> NavigationPoint {
        NavigationPoint {
            path: PathBuf::from(file_name),
            position: Position::new(line, 0),
        }
    }

    #[test]
    fn navigation_history_supports_back_and_forward() {
        let mut history = NavigationHistory::default();
        let a = test_navigation_point("a.rs", 17);
        let b = test_navigation_point("b.rs", 41);

        history.push_back(a.clone());
        assert_eq!(history.go_back(Some(b.clone())), Some(a.clone()));
        assert_eq!(history.go_forward(Some(a)), Some(b));
    }

    #[test]
    fn navigation_history_clicking_back_entry_preserves_forward_path() {
        let mut history = NavigationHistory::default();
        let a = test_navigation_point("a.rs", 17);
        let b = test_navigation_point("b.rs", 41);
        let c = test_navigation_point("c.rs", 9);

        history.push_back(a.clone());
        history.push_back(b.clone());

        assert_eq!(history.open_back_entry(0, Some(c.clone())), Some(a.clone()));
        assert_eq!(history.back, Vec::<NavigationPoint>::new());
        assert_eq!(history.go_forward(Some(a.clone())), Some(b.clone()));
        assert_eq!(history.go_forward(Some(b)), Some(c));
    }
}
