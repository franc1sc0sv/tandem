//! Simulated backends to run the binary without hardware (`--sim-backends`).
//!
//! They only exist with the `sim-backends` feature: the end-to-end tests launch two real
//! `tandemd`s and need each machine's steps to "work" without Wake-on-LAN
//! and without turning anything off. Each process is opaque to the test, so the
//! steps leave a trace in the data directory and can be held back from outside:
//!
//! - [`STEPS_FILE`]: each step that starts, a JSON line (the serialized [`Step`]).
//! - [`HOLD_FILE`]: while it exists, the steps that start are left waiting (already
//!   recorded). Useful to kill a process with a step midway.
//!
//! Without an input engine: the shared keyboard and mouse are not simulated here.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Step;

use crate::daemon::Backends;
use crate::orchestrator::{LocalSteps, StepFuture, describe};

/// The steps that started, one per line in JSON, in the data directory.
pub const STEPS_FILE: &str = "sim-steps.jsonl";
/// If it exists in the data directory, steps do not finish until it is deleted.
pub const HOLD_FILE: &str = "sim-hold";
/// How often it checks whether [`HOLD_FILE`] was deleted.
pub const HOLD_POLL: Duration = Duration::from_millis(20);

/// Steps that always succeed, recorded in `dir`.
#[derive(Debug, Clone)]
pub struct SimSteps {
    dir: PathBuf,
}

impl SimSteps {
    pub fn new(dir: &Path) -> Self {
        SimSteps {
            dir: dir.to_owned(),
        }
    }

    fn record(&self, step: &Step) -> Result<(), String> {
        let line = serde_json::to_string(step).map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(STEPS_FILE))
            .map_err(|e| format!("{}: could not record the step: {e}", describe(step)))?;
        // A single write per line: whoever reads never sees half.
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| format!("{}: could not record the step: {e}", describe(step)))
    }
}

impl LocalSteps for SimSteps {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            self.record(step)?;
            let hold = self.dir.join(HOLD_FILE);
            while hold.exists() {
                tokio::time::sleep(HOLD_POLL).await;
            }
            tracing::info!("simulated: {}", describe(step));
            Ok(())
        })
    }
}

/// The `--sim-backends` backends, with the trace in `data_dir`.
pub fn backends(data_dir: &Path) -> Backends {
    Backends::Custom {
        local: Arc::new(SimSteps::new(data_dir)),
        input: None,
        env: None,
    }
}

/// The steps recorded in `data_dir`, in order. Without a file, none.
pub fn recorded(data_dir: &Path) -> std::io::Result<Vec<Step>> {
    let text = match std::fs::read_to_string(data_dir.join(STEPS_FILE)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).map_err(std::io::Error::other))
        .collect()
}

#[cfg(test)]
mod tests {
    use tandem_core::Machine;

    use super::*;

    #[tokio::test]
    async fn records_each_step_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimSteps::new(dir.path());
        assert!(recorded(dir.path()).unwrap().is_empty());
        let steps = [
            Step::PowerOn {
                machine: Machine::B,
            },
            Step::PowerOff {
                machine: Machine::A,
            },
        ];
        for s in &steps {
            sim.execute(s).await.unwrap();
        }
        assert_eq!(recorded(dir.path()).unwrap(), steps);
    }

    #[tokio::test(start_paused = true)]
    async fn hold_file_keeps_the_step_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let sim = Arc::new(SimSteps::new(dir.path()));
        let hold = dir.path().join(HOLD_FILE);
        std::fs::write(&hold, "").unwrap();
        let task = tokio::spawn({
            let sim = sim.clone();
            async move {
                sim.execute(&Step::PowerOff {
                    machine: Machine::B,
                })
                .await
            }
        });
        tokio::time::sleep(HOLD_POLL * 10).await;
        assert!(!task.is_finished());
        // Already recorded: the test knows the step started.
        assert_eq!(
            recorded(dir.path()).unwrap(),
            [Step::PowerOff {
                machine: Machine::B
            }]
        );
        std::fs::remove_file(&hold).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unwritable_dir_fails_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimSteps::new(&dir.path().join("missing"));
        let err = sim
            .execute(&Step::PowerOn {
                machine: Machine::B,
            })
            .await
            .unwrap_err();
        assert!(err.contains("power on machine B"), "{err}");
    }
}
