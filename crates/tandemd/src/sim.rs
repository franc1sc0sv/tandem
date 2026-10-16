//! Backends simulados para correr el binario sin hardware (`--sim-backends`).
//!
//! Solo existen con la feature `sim-backends`: las pruebas de punta a punta lanzan dos
//! `tandemd` reales y necesitan que los pasos de cada máquina «funcionen» sin DDC, sin
//! escritorios y sin Wake-on-LAN. Cada proceso es opaco para la prueba, así que los
//! pasos dejan rastro en el directorio de datos y se pueden frenar desde afuera:
//!
//! - [`STEPS_FILE`]: cada paso que empieza, una línea JSON (el [`Step`] serializado).
//! - [`HOLD_FILE`]: mientras exista, los pasos que empiezan quedan esperando (ya
//!   anotados). Sirve para matar un proceso con un paso a medias.
//!
//! Sin motor de entrada: el teclado y el mouse compartidos no se simulan aquí.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Step;
use tandem_display::switcher::ManualSwitcher;

use crate::daemon::Backends;
use crate::orchestrator::{LocalSteps, StepFuture, describe};

/// Los pasos que empezaron, uno por línea en JSON, en el directorio de datos.
pub const STEPS_FILE: &str = "sim-steps.jsonl";
/// Si existe en el directorio de datos, los pasos no terminan hasta que se borre.
pub const HOLD_FILE: &str = "sim-hold";
/// Cada cuánto se mira si se borró [`HOLD_FILE`].
pub const HOLD_POLL: Duration = Duration::from_millis(20);

/// Pasos que siempre salen bien, anotados en `dir`.
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
            .map_err(|e| format!("{}: no se pudo anotar el paso: {e}", describe(step)))?;
        // Una sola escritura por línea: quien lee nunca ve media.
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| format!("{}: no se pudo anotar el paso: {e}", describe(step)))
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
            tracing::info!("simulado: {}", describe(step));
            Ok(())
        })
    }
}

/// Los backends de `--sim-backends`, con el rastro en `data_dir`.
pub fn backends(data_dir: &Path) -> Backends {
    Backends::Custom {
        local: Arc::new(SimSteps::new(data_dir)),
        manual: Arc::new(ManualSwitcher::new()),
        input: None,
    }
}

/// Los pasos anotados en `data_dir`, en orden. Sin archivo, ninguno.
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
                machine: Machine::Pc,
            },
            Step::Attach {
                machine: Machine::Pc,
                monitor: "m34".into(),
            },
            Step::UpdateInputGeometry,
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
                    machine: Machine::Pc,
                })
                .await
            }
        });
        tokio::time::sleep(HOLD_POLL * 10).await;
        assert!(!task.is_finished());
        // Ya está anotado: la prueba sabe que el paso empezó.
        assert_eq!(
            recorded(dir.path()).unwrap(),
            [Step::PowerOff {
                machine: Machine::Pc
            }]
        );
        std::fs::remove_file(&hold).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unwritable_dir_fails_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimSteps::new(&dir.path().join("no-existe"));
        let err = sim
            .execute(&Step::PowerOn {
                machine: Machine::Pc,
            })
            .await
            .unwrap_err();
        assert!(err.contains("encender la PC"), "{err}");
    }
}
