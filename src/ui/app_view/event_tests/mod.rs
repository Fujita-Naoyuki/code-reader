//! User-event tests: each test drives the real application through clicks, key presses and
//! typed text, using the fakes in [`support`] for the clipboard, dialogs, browser and LSP.

mod scripted_lsp;
mod support;

mod csv;
mod diff;
mod editor;
mod git;
mod lsp;
mod lsp_protocol;
mod markdown;
mod markdown_rendering;
mod search;
mod sessions;
mod smoke;
mod virtual_editor;
mod windows;
mod workspace;
