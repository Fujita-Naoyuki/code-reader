//! User-event tests: each test drives the real application through clicks, key presses and
//! typed text, using the fakes in [`support`] for the clipboard, dialogs, browser and LSP.

mod support;

mod csv;
mod diff;
mod editor;
mod git;
mod lsp;
mod markdown;
mod markdown_rendering;
mod search;
mod sessions;
mod smoke;
mod virtual_editor;
mod windows;
mod workspace;
