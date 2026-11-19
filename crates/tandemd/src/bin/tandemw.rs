//! `tandemw`: starts `tandemd` without a console window.
//!
//! It is what is registered for startup with the Windows session: `tandemd` is a
//! console application (its CLI must be able to write), and launched from the `Run` key it
//! would open a black window. This launcher is of the graphical subsystem and creates the agent with
//! `CREATE_NO_WINDOW`; the arguments pass as is and the exit code is the agent's.
//! Errors go to `tandemd`'s log: there is nowhere to show them here.

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
