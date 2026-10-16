//! Punto de entrada de la app de escritorio.

// Sin consola extra en Windows en las compilaciones de release.
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
