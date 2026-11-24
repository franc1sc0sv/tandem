//! Detected execution environments and live resources of this machine (T-105, DESIGN §8).
//!
//! - [`Host::scan`]: shells (zsh, bash, fish, PowerShell 5/7, cmd), WSL distros, Docker,
//!   tools (git, cargo/rustc, node/pnpm/npm/bun, uv/python, CUDA) and the GPUs. When the
//!   agent starts and on demand.
//! - [`Host::sample`]: CPU (total and per core), RAM, GPU (usage, VRAM, temperature) and the
//!   system disk; [`ResourceMonitor`] calls it at 1 Hz only while someone is watching.
//!
//! Anything platform-specific goes behind two traits: [`Runner`] (the programs that are read)
//! and [`Meter`] (the system counters). The logic is tested on any system with
//! [`fake::FakeRunner`] and the saved real outputs in `tests/fixtures/`, choosing the
//! [`Platform`] by hand. Nothing assumes a machine model (DECISIONS D10).

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

/// The system being scanned: it decides what is looked for and how. It is the real system it runs on
/// (on Linux, the one of the tests and CI), not the one being advertised.
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

/// What a scan found, ready to announce.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub environments: Vec<Environment>,
    pub hardware: Hardware,
}

/// This machine: scans and samples. Calls block (they run programs): from async,
/// in `spawn_blocking`.
pub struct Host {
    platform: Platform,
    runner: Arc<dyn Runner>,
    meter: Mutex<Box<dyn Meter>>,
    /// Where to read the live GPU from; the scan sets it.
    gpu: Mutex<Option<GpuSource>>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

/// How long to wait for the GPU reading of each sample.
const GPU_SAMPLE_TIMEOUT: Duration = Duration::from_secs(3);

/// No programs or counters: it finds nothing (test agents without backends).
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
            reason: "no programs".into(),
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
    /// The real machine.
    pub fn system() -> Self {
        Host::new(
            Platform::current(),
            Arc::new(SystemRunner),
            Box::new(SysinfoMeter::new()),
        )
    }

    /// One that finds nothing and measures nothing.
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

    /// Scans environments and hardware. It takes a while (it may start a WSL distro).
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

    /// A sample of the resources now.
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

    /// Without a previous scan, where the GPU would be read from (without running anything).
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
