use std::{
    path::PathBuf,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
