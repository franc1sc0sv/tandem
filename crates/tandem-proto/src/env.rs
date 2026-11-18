//! Execution environments, hardware and live resources of a machine (T-105).
//!
//! Everything comes from the scan of the machine that announces it (DECISIONS D10): no type assumes
//! a hardware model. Percentages are in tenths (`0..=1000`) so the messages can be
//! be compared with `Eq`.

use serde::{Deserialize, Serialize};

/// What kind of environment it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnvKind {
    /// A system shell (zsh, bash, fish, PowerShell, cmd).
    Shell,
    /// A WSL distro.
    Wsl,
    /// Docker (the CLI and, if it answers, its daemon).
    Docker,
}

/// A detected execution environment: where a tunnel can run processes (DESIGN §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// Stable across scans, to pick it when executing (`zsh`, `pwsh`, `wsl:Ubuntu-24.04`,
    /// `docker`).
    pub id: String,
    pub kind: EnvKind,
    /// For display ("PowerShell 7", "Ubuntu 24.04", "Docker Desktop").
    pub name: String,
    pub version: Option<String>,
    /// The default of its kind: the user's shell, the default WSL distro.
    pub default: bool,
    /// It can be used now (Docker with the daemon alive).
    pub ready: bool,
    /// The line under the name: the tools, or why it is not ready.
    pub detail: String,
    /// Tools found inside this environment.
    pub tools: Vec<Tool>,
    /// The GPU reaches this environment (CUDA in WSL, `--gpus all` in Docker).
    pub gpu: bool,
}

/// A tool found in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// `git`, `cargo`, `node`, `uv`, `python`, `CUDA`...
    pub name: String,
    pub version: String,
}

/// The hardware, according to the scan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hardware {
    /// Processor model, if the system says so.
    pub cpu: Option<String>,
    /// Logical cores.
    pub cores: u32,
    /// RAM total, en bytes.
    pub memory: u64,
    pub gpus: Vec<Gpu>,
}

/// A GPU of the hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// Own memory, in bytes; `None` if it shares RAM (Apple) or is unknown.
    pub vram: Option<u64>,
    pub driver: Option<String>,
}

/// A sample of a machine's resources (1 Hz while someone is watching).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    pub cpu: CpuLoad,
    pub memory: MemoryUse,
    /// In the order of [`Hardware::gpus`] when the system gives its readings.
    pub gpus: Vec<GpuLoad>,
    /// The system volume; `None` if it could not be read.
    pub disk: Option<DiskSpace>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuLoad {
    /// Total usage, in tenths of a percent (0-1000).
    pub load: u16,
    /// Usage of each logical core, in tenths of a percent.
    pub cores: Vec<u16>,
    /// In degrees Celsius, if the system gives it.
    pub temperature: Option<i16>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryUse {
    /// En bytes.
    pub used: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuLoad {
    pub name: String,
    /// Usage, in tenths of a percent; `None` if the system does not give it.
    pub load: Option<u16>,
    /// En bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// In degrees Celsius.
    pub temperature: Option<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSpace {
    /// En bytes.
    pub free: u64,
    pub total: u64,
}

#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    pub fn environments() -> Vec<Environment> {
        vec![
            Environment {
                id: "pwsh".into(),
                kind: EnvKind::Shell,
                name: "PowerShell 7".into(),
                version: Some("7.5.2".into()),
                default: true,
                ready: true,
                detail: "winget · python · git".into(),
                tools: vec![Tool {
                    name: "git".into(),
                    version: "2.51.0".into(),
                }],
                gpu: false,
            },
            Environment {
                id: "wsl:Ubuntu-24.04".into(),
                kind: EnvKind::Wsl,
                name: "Ubuntu 24.04".into(),
                version: Some("2".into()),
                default: true,
                ready: true,
                detail: "uv · cargo · CUDA 12.8".into(),
                tools: vec![],
                gpu: true,
            },
            Environment {
                id: "docker".into(),
                kind: EnvKind::Docker,
                name: "Docker Desktop".into(),
                version: None,
                default: false,
                ready: false,
                detail: "The daemon is not responding".into(),
                tools: vec![],
                gpu: false,
            },
        ]
    }

    pub fn env_kind_arm(k: EnvKind) -> usize {
        match k {
            EnvKind::Shell => 0,
            EnvKind::Wsl => 1,
            EnvKind::Docker => 2,
        }
    }

    pub fn hardware() -> Hardware {
        Hardware {
            cpu: Some("AMD Ryzen 7 9800X3D 8-Core Processor".into()),
            cores: 16,
            memory: 64 << 30,
            gpus: vec![Gpu {
                name: "NVIDIA GeForce RTX 5080".into(),
                vram: Some(16 << 30),
                driver: Some("581.57".into()),
            }],
        }
    }

    pub fn resources() -> Vec<Resources> {
        vec![
            Resources::default(),
            Resources {
                cpu: CpuLoad {
                    load: 640,
                    cores: vec![1000, 280],
                    temperature: Some(62),
                },
                memory: MemoryUse {
                    used: 17 << 30,
                    total: 24 << 30,
                },
                gpus: vec![GpuLoad {
                    name: "NVIDIA GeForce RTX 5080".into(),
                    load: Some(970),
                    vram_used: Some(14 << 30),
                    vram_total: Some(16 << 30),
                    temperature: Some(71),
                }],
                disk: Some(DiskSpace {
                    free: 412 << 30,
                    total: 1800 << 30,
                }),
            },
        ]
    }
}
