//! `tandemw`: arranca `tandemd` sin ventana de consola.
//!
//! Es lo que se registra para el arranque con la sesión de Windows: `tandemd` es una
//! aplicación de consola (su CLI tiene que poder escribir), y lanzada desde la clave `Run`
//! abriría una ventana negra. Este lanzador es de subsistema gráfico y crea el agente con
//! `CREATE_NO_WINDOW`; los argumentos pasan tal cual y el código de salida es el del agente.
//! Los errores van al log de `tandemd`: aquí no hay dónde mostrarlos.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    let mut cmd = Command::new(tandemd::install::daemon_for(&exe));
    cmd.args(std::env::args_os().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW` de `winbase.h`.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.status() {
        Ok(status) => match status.code() {
            Some(0) => ExitCode::SUCCESS,
            Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            None => ExitCode::FAILURE,
        },
        Err(_) => ExitCode::FAILURE,
    }
}
