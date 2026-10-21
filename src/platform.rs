//! OS-facing side effects used by the UI.
//!
//! Everything that touches the user's machine outside the workspace files goes through
//! [`Platform`]: the system clipboard, the native folder picker, the web browser, the
//! persisted app-state location, the process working directory and LSP server processes.
//!
//! Production code always runs with [`SystemPlatform`]. Tests replace it per thread with
//! [`install`], so a test can script clipboard contents, folder-picker answers and LSP
//! responses without touching the real desktop or spawning `rust-analyzer` / `gopls`.
//! The platform is thread-local because egui runs the whole UI on one thread and each
//! test runs on its own thread, which keeps tests isolated from each other.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use crate::core::lsp::{LspClient, LspServerConfig};

pub trait Platform {
    /// Location of the persisted `state.json`.
    fn state_file_path(&self) -> PathBuf;
    /// Fallback workspace root when no session is stored yet.
    fn current_dir(&self) -> PathBuf;
    fn copy_text(&self, text: &str) -> Result<(), String>;
    fn read_text(&self) -> Result<String, String>;
    /// Shows a folder picker. `None` means the user canceled.
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;
    fn open_url(&self, url: &str) -> Result<(), String>;
    fn start_lsp(&self, config: LspServerConfig) -> LspClient;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPlatform;

impl Platform for SystemPlatform {
    fn state_file_path(&self) -> PathBuf {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            return PathBuf::from(app_data)
                .join("naoyuki-editor")
                .join("state.json");
        }

        if let Some(home) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(home)
                .join(".config")
                .join("naoyuki-editor")
                .join("state.json");
        }

        PathBuf::from("state.json")
    }

    fn current_dir(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn copy_text(&self, text: &str) -> Result<(), String> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("open clipboard: {error}"))?;
        clipboard
            .set_text(text.to_owned())
            .map_err(|error| format!("set clipboard text: {error}"))
    }

    fn read_text(&self) -> Result<String, String> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("open clipboard: {error}"))?;
        clipboard
            .get_text()
            .map_err(|error| format!("get clipboard text: {error}"))
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).pick_folder()
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        webbrowser::open(url).map_err(|error| error.to_string())
    }

    fn start_lsp(&self, config: LspServerConfig) -> LspClient {
        LspClient::start(config)
    }
}

thread_local! {
    static CURRENT: RefCell<Rc<dyn Platform>> = RefCell::new(Rc::new(SystemPlatform));
}

/// Returns the platform active on this thread.
pub fn current() -> Rc<dyn Platform> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Replaces the platform on this thread until the returned guard is dropped.
#[must_use = "the platform is restored when the guard is dropped"]
#[cfg(test)]
pub fn install(platform: Rc<dyn Platform>) -> PlatformGuard {
    let previous = CURRENT.with(|current| current.replace(platform));
    PlatformGuard {
        previous: Some(previous),
    }
}

#[cfg(test)]
pub struct PlatformGuard {
    previous: Option<Rc<dyn Platform>>,
}

#[cfg(test)]
impl Drop for PlatformGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            CURRENT.with(|current| {
                current.replace(previous);
            });
        }
    }
}
