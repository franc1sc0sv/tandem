//! Entornos de ejecución, equipo y recursos en vivo de una máquina (T-105).
//!
//! Todo sale del escaneo de la máquina que lo anuncia (DECISIONS D10): ningún tipo supone
//! un modelo de equipo. Los porcentajes van en décimas (`0..=1000`) para que los mensajes se
//! puedan comparar con `Eq`.

use serde::{Deserialize, Serialize};

/// Qué clase de entorno es.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnvKind {
    /// Un shell del sistema (zsh, bash, fish, PowerShell, cmd).
    Shell,
    /// Una distro de WSL.
    Wsl,
    /// Docker (el CLI y, si responde, su daemon).
    Docker,
}

/// Un entorno de ejecución detectado: donde un túnel puede correr procesos (DESIGN §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// Estable entre escaneos, para elegirlo al ejecutar (`zsh`, `pwsh`, `wsl:Ubuntu-24.04`,
    /// `docker`).
    pub id: String,
    pub kind: EnvKind,
    /// Para mostrar («PowerShell 7», «Ubuntu 24.04», «Docker Desktop»).
    pub name: String,
    pub version: Option<String>,
    /// El predeterminado de su clase: el shell del usuario, la distro de WSL por omisión.
    pub default: bool,
    /// Se puede usar ahora (Docker con el daemon vivo).
    pub ready: bool,
    /// La línea bajo el nombre: las herramientas, o por qué no está listo.
    pub detail: String,
    /// Herramientas encontradas dentro de este entorno.
    pub tools: Vec<Tool>,
    /// La GPU llega a este entorno (CUDA en WSL, `--gpus all` en Docker).
    pub gpu: bool,
}

/// Una herramienta encontrada en un entorno.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// `git`, `cargo`, `node`, `uv`, `python`, `CUDA`...
    pub name: String,
    pub version: String,
}

/// El equipo, según el escaneo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hardware {
    /// Modelo del procesador, si el sistema lo dice.
    pub cpu: Option<String>,
    /// Núcleos lógicos.
    pub cores: u32,
    /// RAM total, en bytes.
    pub memory: u64,
    pub gpus: Vec<Gpu>,
}

/// Una GPU del equipo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// Memoria propia, en bytes; `None` si comparte la RAM (Apple) o no se sabe.
    pub vram: Option<u64>,
    pub driver: Option<String>,
}

/// Una muestra de los recursos de una máquina (1 Hz mientras alguien mira).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    pub cpu: CpuLoad,
    pub memory: MemoryUse,
    /// En el orden de [`Hardware::gpus`] cuando el sistema da sus lecturas.
    pub gpus: Vec<GpuLoad>,
    /// El volumen del sistema; `None` si no se pudo leer.
    pub disk: Option<DiskSpace>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuLoad {
    /// Uso total, en décimas de por ciento (0–1000).
    pub load: u16,
    /// Uso de cada núcleo lógico, en décimas de por ciento.
    pub cores: Vec<u16>,
    /// En grados Celsius, si el sistema la da.
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
    /// Uso, en décimas de por ciento; `None` si el sistema no lo da.
    pub load: Option<u16>,
    /// En bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// En grados Celsius.
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
                detail: "El daemon no responde".into(),
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
