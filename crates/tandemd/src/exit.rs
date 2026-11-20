//! The last line the agent writes to the log before its process ends (T-334).
//!
//! On macOS the agent's thread ends the process with `std::process::exit`, and AppKit can end
//! it with its own `exit(0)`; neither returns through `main`, so the line is written right
//! before. The file layer of [`crate::logging`] writes each event straight to the file (no
//! buffer, no background thread), so the line is on disk when `exit` runs.

use std::fmt;

use crate::daemon::ShutdownReason;

/// How the agent's process ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    /// The orderly shutdown finished.
    Stopped(ShutdownReason),
    /// The agent failed (the error's text).
    Failed(String),
    /// The agent's thread panicked (the panic itself is logged by the panic hook).
    Panicked,
    /// AppKit ends the process because the session is ending (T-333); its text says how.
    SessionEnd(String),
}

impl Exit {
    /// The process's exit code: 0 only when nothing went wrong.
    pub fn code(&self) -> i32 {
        match self {
            Exit::Stopped(_) | Exit::SessionEnd(_) => 0,
            Exit::Failed(_) | Exit::Panicked => 1,
        }
    }

    /// Writes the exit line (an error for a non-zero code) and returns the code.
    pub fn log(&self) -> i32 {
        let code = self.code();
        if code == 0 {
            tracing::info!("{self}");
        } else {
            tracing::error!("{self}");
        }
        code
    }
}

/// `tandemd exiting with code N: <why>`, on one line.
impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tandemd exiting with code {}: ", self.code())?;
        match self {
            Exit::Stopped(reason) => write!(f, "stopped by {reason}"),
            Exit::Failed(error) => {
                // A multi-line error (a TOML parse error) would hide the exit in the middle.
                let one_line: Vec<&str> = error
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                write!(f, "error: {}", one_line.join(" "))
            }
            Exit::Panicked => f.write_str("the agent thread panicked"),
            Exit::SessionEnd(how) => write!(f, "AppKit terminates it for {how}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_line_says_the_code_and_why() {
        let cases = [
            (
                Exit::Stopped(ShutdownReason::Sigterm),
                "tandemd exiting with code 0: stopped by SIGTERM",
            ),
            (
                Exit::Stopped(ShutdownReason::CtrlC),
                "tandemd exiting with code 0: stopped by Ctrl+C",
            ),
            (
                Exit::Failed("could not initialize the log".into()),
                "tandemd exiting with code 1: error: could not initialize the log",
            ),
            (
                Exit::Panicked,
                "tandemd exiting with code 1: the agent thread panicked",
            ),
            (
                Exit::SessionEnd("log out".into()),
                "tandemd exiting with code 0: AppKit terminates it for log out",
            ),
        ];
        for (exit, line) in cases {
            assert_eq!(exit.to_string(), line);
        }
    }

    #[test]
    fn a_multi_line_error_stays_on_one_line() {
        let exit =
            Exit::Failed("invalid configuration: TOML parse error\n  |\n1 | machine =\n".into());
        let line = exit.to_string();
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(
            line,
            "tandemd exiting with code 1: error: invalid configuration: TOML parse error | 1 | machine ="
        );
    }

    #[test]
    fn only_failures_exit_with_a_non_zero_code() {
        assert_eq!(Exit::Stopped(ShutdownReason::Requested).code(), 0);
        assert_eq!(Exit::SessionEnd("restart".into()).code(), 0);
        assert_eq!(Exit::Failed(String::new()).code(), 1);
        assert_eq!(Exit::Panicked.code(), 1);
    }
}
