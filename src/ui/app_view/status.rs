//! The status bar message. `status_message` holds the result of what the user just did;
//! background work reports through `set_background_status` so it cannot overwrite that
//! result (D01). A background notice shows once the result is old enough.

use super::*;

/// How long the result of a user action stays before a newer background notice replaces it.
pub(super) const OPERATION_STATUS_HOLD: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct BackgroundStatus {
    pub(super) message: String,
    pub(super) changed_at: Option<Instant>,
    /// The last user-action message seen and when it appeared.
    pub(super) operation_seen: String,
    pub(super) operation_changed_at: Option<Instant>,
}

impl CodeReaderApp {
    pub(super) fn set_background_status(&mut self, message: String) {
        self.background_status.message = message;
        self.background_status.changed_at = Some(Instant::now());
    }

    /// Notes when `status_message` changed (it is written directly from many places) and
    /// retires it once a newer notice may show, so repeating the same action later still
    /// counts as a change.
    pub(super) fn track_operation_status(&mut self) {
        if self.background_status.operation_seen != self.status_message {
            self.background_status
                .operation_seen
                .clone_from(&self.status_message);
            self.background_status.operation_changed_at = Some(Instant::now());
        }
        let background = &self.background_status;
        let background_is_newer = match (background.changed_at, background.operation_changed_at) {
            (Some(notice), Some(operation)) => notice > operation,
            _ => false,
        };
        let operation_held = background
            .operation_changed_at
            .is_some_and(|changed_at| changed_at.elapsed() >= OPERATION_STATUS_HOLD);
        if background_is_newer && operation_held {
            self.status_message.clear();
            self.background_status.operation_seen.clear();
        }
    }

    /// The user action's result while there is one, otherwise the background notice.
    pub(super) fn visible_status(&self) -> &str {
        if self.status_message.is_empty() {
            &self.background_status.message
        } else {
            &self.status_message
        }
    }

    pub(super) fn status_bar_message(&self) -> String {
        let status = self.visible_status();
        if status.contains("rust-analyzer") {
            return status.to_owned();
        }

        let Some(rust_lsp_status) = self.rust_lsp_readiness_status_message() else {
            return status.to_owned();
        };

        if status.is_empty() {
            rust_lsp_status
        } else {
            format!("{status} | {rust_lsp_status}")
        }
    }
}
