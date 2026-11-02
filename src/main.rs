//! Code Reader: a local app for reading code. See CLAUDE.md for the layout.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod features;
mod languages;
mod platform;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    match app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("code-reader failed: {error}");
            ExitCode::FAILURE
        }
    }
}
