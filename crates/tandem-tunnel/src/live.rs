//! Live output for a viewer (the UI's terminal view): each process's output is pushed as it
//! arrives, as the exact continuation of what was already pushed, so the viewer only appends.
//!
//! Each push answers the moment output arrives; under a flood, pushes are spaced at least
//! [`MIN_GAP`] apart and each one carries everything that accumulated, so a chatty
//! process costs a few hundred pushes per second at most, not one per write.

use std::time::Duration;

use tandem_proto::{OutputCursor, ProcessId, ProcessOutput, TunnelId};
use tokio::time::Instant;

use crate::Tunnels;
use crate::process::output::OUTPUT_MAX;

/// The shortest time between two pushes.
pub const MIN_GAP: Duration = Duration::from_millis(4);

/// How long one wait for output lasts before looking again (it costs nothing: a wake-up).
const IDLE: Duration = Duration::from_secs(30);

/// Follows a process's output from a cursor.
pub struct OutputFollower {
    tunnels: Tunnels,
    tunnel: TunnelId,
    process: ProcessId,
    cursor: OutputCursor,
    last: Option<Instant>,
    finished: bool,
}

impl OutputFollower {
    pub fn new(tunnels: Tunnels, tunnel: TunnelId, process: ProcessId, from: OutputCursor) -> Self {
        OutputFollower {
            tunnels,
            tunnel,
            process,
            cursor: from,
            last: None,
            finished: false,
        }
    }

    pub fn cursor(&self) -> OutputCursor {
        self.cursor
    }

    /// The next piece of output. `None` once the process ended and everything was pushed, or if
    /// the process is not known here. Cancel-safe: dropping the future loses nothing.
    pub async fn next(&mut self) -> Option<ProcessOutput> {
        if self.finished {
            return None;
        }
        if let Some(last) = self.last {
            tokio::time::sleep_until(last + MIN_GAP).await;
        }
        loop {
            let out = self
                .tunnels
                .wait_output(self.tunnel, self.process, self.cursor, OUTPUT_MAX, IDLE)
                .await;
            let out = match out {
                Ok(o) => o,
                Err(e) => {
                    tracing::debug!(tunnel = %self.tunnel, process = %self.process, "output no longer followed: {e}");
                    self.finished = true;
                    return None;
                }
            };
            if out.stdout.is_empty() && out.stderr.is_empty() && !out.done {
                continue;
            }
            // From here on nothing awaits: the state changes only together with the return.
            self.cursor = out.next;
            self.finished = out.done;
            self.last = Some(Instant::now());
            return Some(out);
        }
    }
}
