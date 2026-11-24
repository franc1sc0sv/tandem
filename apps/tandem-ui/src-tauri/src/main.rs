//! Entry point of the desktop app.

// No extra console on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match tandem_ui::app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tandem-ui: {e}");
            ExitCode::FAILURE
        }
    }
}
