//! Workspace sessions and the state file they are saved in.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::core::cli_agent::CliAgentSettings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub current_session_id: String,
    pub sessions: Vec<WorkspaceSession>,
    #[serde(default)]
    pub cli_agent: CliAgentSettings,
}

impl AppState {
    pub fn new(initial_root: PathBuf) -> Self {
        let session = WorkspaceSession::new("Default", vec![initial_root]);

        Self {
            current_session_id: session.id.clone(),
            sessions: vec![session],
            cli_agent: CliAgentSettings::default(),
        }
    }

    /// Makes `current_session_id` point at an existing session: the most recently updated
    /// one, or a new "Default" session on `fallback_root` when there is none. Returns
    /// whether anything changed (the caller should then save the state).
    pub fn repair_current_session(&mut self, fallback_root: PathBuf) -> bool {
        if self.current_session().is_some() {
            return false;
        }
        match self
            .sessions
            .iter()
            .max_by_key(|session| session.last_updated_unix)
        {
            Some(session) => self.current_session_id = session.id.clone(),
            None => {
                let session = WorkspaceSession::new("Default", vec![fallback_root]);
                self.current_session_id = session.id.clone();
                self.sessions.push(session);
            }
        }
        true
    }

    pub fn current_session(&self) -> Option<&WorkspaceSession> {
        self.sessions
            .iter()
            .find(|session| session.id == self.current_session_id)
    }

    pub fn current_session_mut(&mut self) -> Option<&mut WorkspaceSession> {
        self.sessions
            .iter_mut()
            .find(|session| session.id == self.current_session_id)
    }

    pub fn switch_to(&mut self, session_id: &str) {
        if self.sessions.iter().any(|session| session.id == session_id) {
            self.current_session_id = session_id.to_owned();
        }
    }

    pub fn create_session(&mut self, root: PathBuf) {
        let name = format!("Workspace {}", self.sessions.len() + 1);
        let mut session = WorkspaceSession::new(name, vec![root]);
        session.id = self.unique_session_id(session.id);
        self.current_session_id = session.id.clone();
        self.sessions.push(session);
    }

    fn unique_session_id(&self, preferred_id: String) -> String {
        if self
            .sessions
            .iter()
            .all(|session| session.id != preferred_id)
        {
            return preferred_id;
        }

        for suffix in 2.. {
            let candidate = format!("{preferred_id}-{suffix}");
            if self.sessions.iter().all(|session| session.id != candidate) {
                return candidate;
            }
        }

        unreachable!()
    }

    pub fn delete_session(&mut self, session_id: &str) -> Result<WorkspaceSession, String> {
        if self.sessions.len() <= 1 {
            return Err("Keep at least one workspace session.".to_owned());
        }
        if session_id == self.current_session_id {
            return Err("Cannot delete the current workspace session.".to_owned());
        }

        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Err("Workspace session was not found.".to_owned());
        };

        Ok(self.sessions.remove(index))
    }

    pub fn rename_current_session(&mut self, display_name: &str) -> Result<(), String> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err("Workspace session name cannot be empty.".to_owned());
        }

        let Some(session) = self.current_session_mut() else {
            return Err("Current workspace session was not found.".to_owned());
        };

        session.display_name = display_name.to_owned();
        session.touch();
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSession {
    pub id: String,
    pub display_name: String,
    pub root_folders: Vec<PathBuf>,
    pub bookmarks: Vec<SessionBookmark>,
    pub open_files: OpenFiles,
    pub pane_positions: PanePositions,
    pub last_updated_unix: u64,
}

impl WorkspaceSession {
    pub fn new(display_name: impl Into<String>, root_folders: Vec<PathBuf>) -> Self {
        Self {
            id: format!("session-{}", now_millis()),
            display_name: display_name.into(),
            root_folders,
            bookmarks: Vec::new(),
            open_files: OpenFiles::default(),
            pane_positions: PanePositions::default(),
            last_updated_unix: now_unix(),
        }
    }

    pub fn touch(&mut self) {
        self.last_updated_unix = now_unix();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBookmark {
    pub label: String,
    pub path: PathBuf,
    #[serde(default)]
    pub position: TextPosition,
    #[serde(default)]
    pub temporary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenFiles {
    pub left: Option<PathBuf>,
    pub right: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PanePositions {
    pub left: TextPosition,
    pub right: TextPosition,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

impl TextPosition {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub(crate) fn recent_workspace_age_label(last_updated_unix: u64) -> String {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(last_updated_unix);
    recent_workspace_age_label_at(last_updated_unix, now_unix)
}

pub(crate) fn recent_workspace_age_label_at(last_updated_unix: u64, now_unix: u64) -> String {
    let elapsed = now_unix.saturating_sub(last_updated_unix);

    if elapsed < 60 {
        "just now".to_owned()
    } else if elapsed < 60 * 60 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 60 * 60 * 24 {
        format!("{}h ago", elapsed / (60 * 60))
    } else {
        format!("{}d ago", elapsed / (60 * 60 * 24))
    }
}

pub(crate) fn recent_files_from_open_files(open_files: &OpenFiles) -> Vec<PathBuf> {
    let mut recent_files = Vec::new();

    if let Some(path) = &open_files.left {
        recent_files.push(path.clone());
    }
    if let Some(path) = &open_files.right
        && !recent_files.iter().any(|recent| recent == path)
    {
        recent_files.push(path.clone());
    }

    recent_files
}

pub(crate) fn load_state(path: &Path) -> Option<AppState> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn save_state(
    path: &Path,
    app_state: &AppState,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut persisted_state = app_state.clone();
    for session in &mut persisted_state.sessions {
        session.bookmarks.retain(|bookmark| !bookmark.temporary);
    }

    let text = serde_json::to_string_pretty(&persisted_state)?;
    fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_current_session_picks_the_latest_or_creates_one() {
        let mut state = AppState::new(PathBuf::from("a"));
        assert!(!state.repair_current_session(PathBuf::from("x")));

        let mut older = WorkspaceSession::new("Older", vec![PathBuf::from("b")]);
        older.id = "older".to_owned();
        older.last_updated_unix = 0;
        state.sessions.push(older);
        state.sessions[0].last_updated_unix = 10;
        let latest_id = state.sessions[0].id.clone();
        state.current_session_id = "missing".to_owned();
        assert!(state.repair_current_session(PathBuf::from("x")));
        assert_eq!(state.current_session_id, latest_id);

        state.sessions.clear();
        state.current_session_id = "missing".to_owned();
        assert!(state.repair_current_session(PathBuf::from("x")));
        let session = state.current_session().unwrap();
        assert_eq!(session.display_name, "Default");
        assert_eq!(session.root_folders, vec![PathBuf::from("x")]);
    }

    #[test]
    fn new_workspace_session_starts_without_bookmarks() {
        let session = WorkspaceSession::new("Workspace", vec![PathBuf::from(r"C:\git\sample")]);

        assert!(session.bookmarks.is_empty());
    }

    #[test]
    fn delete_session_removes_non_current_session() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));
        let current_id = state.current_session_id.clone();
        state.create_session(PathBuf::from(r"C:\git\two"));
        let target_id = current_id;

        let deleted = state.delete_session(&target_id).unwrap();

        assert_eq!(deleted.id, target_id);
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions.iter().all(|session| session.id != target_id));
    }

    #[test]
    fn delete_session_rejects_current_session() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));
        state.create_session(PathBuf::from(r"C:\git\two"));
        let current_id = state.current_session_id.clone();

        let error = state.delete_session(&current_id).unwrap_err();

        assert_eq!(error, "Cannot delete the current workspace session.");
    }

    #[test]
    fn delete_session_keeps_at_least_one_session() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));
        let current_id = state.current_session_id.clone();

        let error = state.delete_session(&current_id).unwrap_err();

        assert_eq!(error, "Keep at least one workspace session.");
    }

    #[test]
    fn create_session_avoids_duplicate_session_ids() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));
        let duplicate_id = state.current_session_id.clone();
        state.sessions.push(WorkspaceSession {
            id: duplicate_id,
            display_name: "Manual duplicate".to_owned(),
            root_folders: vec![PathBuf::from(r"C:\git\duplicate")],
            bookmarks: Vec::new(),
            open_files: OpenFiles::default(),
            pane_positions: PanePositions::default(),
            last_updated_unix: 0,
        });

        state.create_session(PathBuf::from(r"C:\git\two"));

        let new_id = state.current_session_id.clone();
        let new_id_count = state
            .sessions
            .iter()
            .filter(|session| session.id == new_id)
            .count();
        assert_eq!(new_id_count, 1);
    }

    #[test]
    fn rename_current_session_updates_only_current_session() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));
        let first_id = state.current_session_id.clone();
        state.create_session(PathBuf::from(r"C:\git\two"));
        let second_id = state.current_session_id.clone();

        state.rename_current_session("  Rust Sandbox  ").unwrap();

        let first = state
            .sessions
            .iter()
            .find(|session| session.id == first_id)
            .unwrap();
        let second = state
            .sessions
            .iter()
            .find(|session| session.id == second_id)
            .unwrap();
        assert_eq!(first.display_name, "Default");
        assert_eq!(second.display_name, "Rust Sandbox");
    }

    #[test]
    fn rename_current_session_rejects_empty_name() {
        let mut state = AppState::new(PathBuf::from(r"C:\git\one"));

        let error = state.rename_current_session("   ").unwrap_err();

        assert_eq!(error, "Workspace session name cannot be empty.");
    }

    #[test]
    fn recent_workspace_age_label_uses_compact_units() {
        assert_eq!(recent_workspace_age_label_at(1_000, 1_030), "just now");
        assert_eq!(recent_workspace_age_label_at(1_000, 1_180), "3m ago");
        assert_eq!(recent_workspace_age_label_at(1_000, 8_200), "2h ago");
        assert_eq!(recent_workspace_age_label_at(1_000, 260_200), "3d ago");
    }
}
