//! Entornos de ejecución detectados y recursos en vivo de esta máquina (T-105, DESIGN §8).
//!
//! - [`Host::scan`]: shells (zsh, bash, fish, PowerShell 5/7, cmd), distros de WSL, Docker,
//!   herramientas (git, cargo/rustc, node/pnpm/npm/bun, uv/python, CUDA) y las GPU. Al
//!   arrancar el agente y a pedido.
//! - [`Host::sample`]: CPU (total y por núcleo), RAM, GPU (uso, VRAM, temperatura) y el disco
//!   del sistema; [`ResourceMonitor`] la llama a 1 Hz solo mientras alguien mira.
//!
//! Lo de plataforma va detrás de dos traits: [`Runner`] (los programas que se leen) y
//! [`Meter`] (los contadores del sistema). La lógica se prueba en cualquier sistema con
//! [`fake::FakeRunner`] y las salidas reales guardadas en `tests/fixtures/`, eligiendo el
//! [`Platform`] a mano. Nada supone un modelo de equipo (DECISIONS D10).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_proto::{Environment, GpuLoad, Hardware, Resources};

pub mod fake;
pub mod meter;
pub mod monitor;
pub mod parse;
pub mod run;
pub mod scan;

pub use meter::{Meter, MeterInfo, Reading, SysinfoMeter};
pub use monitor::{ResourceMonitor, Watch};
pub use run::{Output, RunError, Runner, SystemRunner};
pub use scan::GpuSource;

/// El sistema que se escanea: decide qué se busca y cómo. Es el sistema real donde corre
/// (en Linux, el de las pruebas y la CI), no el que se anuncia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// Lo que encontró un escaneo, listo para anunciar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub environments: Vec<Environment>,
    pub hardware: Hardware,
}

/// Esta máquina: escanea y muestrea. Las llamadas bloquean (corren programas): desde async,
/// en `spawn_blocking`.
pub struct Host {
    platform: Platform,
    runner: Arc<dyn Runner>,
    meter: Mutex<Box<dyn Meter>>,
    /// Por dónde leer la GPU en vivo; lo fija el escaneo.
    gpu: Mutex<Option<GpuSource>>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

/// Cuánto se espera a la lectura de la GPU de cada muestra.
const GPU_SAMPLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Sin programas ni contadores: no encuentra nada (agentes de prueba sin backends).
#[derive(Debug, Default)]
struct Nothing;

impl Runner for Nothing {
    fn find(&self, _program: &str, _extra: Option<&str>) -> Option<PathBuf> {
        None
    }
    fn run(
        &self,
        program: &std::path::Path,
        _args: &[&str],
        _timeout: Duration,
    ) -> Result<Output, RunError> {
        Err(RunError::Spawn {
            program: program.display().to_string(),
            reason: "sin programas".into(),
        })
    }
    fn var(&self, _name: &str) -> Option<String> {
        None
    }
}

impl Meter for Nothing {
    fn info(&mut self) -> MeterInfo {
        MeterInfo::default()
    }
    fn read(&mut self) -> Reading {
        Reading::default()
    }
}

impl Host {
    /// La máquina real.
    pub fn system() -> Self {
        Host::new(
            Platform::current(),
            Arc::new(SystemRunner),
            Box::new(SysinfoMeter::new()),
        )
    }

    /// Una que no encuentra nada ni mide nada.
    pub fn none() -> Self {
        Host::new(Platform::current(), Arc::new(Nothing), Box::new(Nothing))
    }

    pub fn new(platform: Platform, runner: Arc<dyn Runner>, meter: Box<dyn Meter>) -> Self {
        Host {
            platform,
            runner,
            meter: Mutex::new(meter),
            gpu: Mutex::new(None),
        }
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Escanea entornos y equipo. Tarda (puede arrancar una distro de WSL).
    pub fn scan(&self) -> Scan {
        let found = scan::scan(self.runner.as_ref(), self.platform);
        let info = self
            .meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .info();
        *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) = Some(found.gpu_source);
        Scan {
            environments: found.environments,
            hardware: Hardware {
                cpu: info.cpu,
                cores: info.cores,
                memory: info.memory,
                gpus: found.gpus,
            },
        }
    }

    /// Una muestra de los recursos ahora.
    pub fn sample(&self) -> Resources {
        let reading = self
            .meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .read();
        let gpus = self.gpu_loads();
        let drive = self.runner.var("SystemDrive");
        meter::resources(&reading, gpus, self.platform, drive.as_deref())
    }

    fn gpu_loads(&self) -> Vec<GpuLoad> {
        let source = self
            .gpu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| self.detect_gpu_source())
            .clone();
        let run = |program: &std::path::Path, args: &[&str]| {
            self.runner
                .run(program, args, GPU_SAMPLE_TIMEOUT)
                .ok()
                .filter(|o| o.success)
                .map(|o| o.text())
        };
        match source {
            GpuSource::NvidiaSmi(smi) => run(&smi, &scan::nvidia_args())
                .map(|t| parse::nvidia_smi(&t).into_iter().map(|g| g.load).collect())
                .unwrap_or_default(),
            GpuSource::Ioreg(ioreg) => run(&ioreg, &scan::IOREG_ARGS)
                .map(|t| parse::ioreg_gpus(&t))
                .unwrap_or_default(),
            GpuSource::None => Vec::new(),
        }
    }

    /// Sin escaneo previo, por dónde se leería la GPU (sin correr nada).
    fn detect_gpu_source(&self) -> GpuSource {
        match self.platform {
            Platform::MacOs => self
                .runner
                .find("ioreg", Some("/usr/sbin"))
                .map_or(GpuSource::None, GpuSource::Ioreg),
            _ => self
                .runner
                .find("nvidia-smi", None)
                .map_or(GpuSource::None, GpuSource::NvidiaSmi),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_programs_finds_nothing() {
        let h = Host::none();
        assert_eq!(h.scan(), Scan::default());
        assert_eq!(h.sample(), Resources::default());
    }
}
