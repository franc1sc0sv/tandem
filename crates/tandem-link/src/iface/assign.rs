use std::net::Ipv4Addr;
use std::sync::{Mutex, PoisonError};

use tandem_core::Os;
use thiserror::Error;

use super::NetInterface;

/// Por qué no se pudo asignar la IP de respaldo.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssignError {
    /// Faltan permisos: hay que dar la orden al usuario.
    #[error("sin permisos para cambiar la dirección de la interfaz")]
    PermissionDenied,
    #[error("no se pudo asignar la dirección: {0}")]
    Failed(String),
}

/// Pone la IP de respaldo (`10.77.1.0/30`) en una interfaz. Es plataforma (`netsh`,
/// `ifconfig`), así que va detrás de un trait con doble.
pub trait AddrAssigner: Send + Sync + 'static {
    fn assign(&self, iface: &NetInterface, ip: Ipv4Addr) -> Result<(), AssignError>;
}

/// Programa y argumentos que asignan `ip` con máscara `/30` a la interfaz `name`.
pub fn assign_command(os: Os, name: &str, ip: Ipv4Addr) -> (&'static str, Vec<String>) {
    let ip = ip.to_string();
    match os {
        Os::MacOs => (
            "ifconfig",
            vec![
                name.into(),
                "inet".into(),
                ip,
                "netmask".into(),
                "255.255.255.252".into(),
            ],
        ),
        Os::Windows => (
            "netsh",
            vec![
                "interface".into(),
                "ip".into(),
                "set".into(),
                "address".into(),
                format!("name={name}"),
                "static".into(),
                ip,
                "255.255.255.252".into(),
            ],
        ),
    }
}

/// Si lo que dijo la orden al fallar es una falta de permisos (en inglés y en español: los
/// mensajes de `netsh` salen en el idioma del sistema).
pub fn is_permission_failure(output: &str) -> bool {
    let o = output.to_lowercase();
    [
        "elevation",
        "elevación",
        "administrator",
        "administrador",
        "not permitted",
        "permission denied",
        "access is denied",
        "acceso denegado",
        "requires",
        "requiere",
    ]
    .iter()
    .any(|p| o.contains(p))
}

/// El asignador real: ejecuta la orden del sistema. En Linux (solo desarrollo) no hay
/// nada que asignar y siempre pide la orden al usuario.
pub struct SystemAssigner;

impl AddrAssigner for SystemAssigner {
    #[cfg(any(windows, target_os = "macos"))]
    fn assign(&self, iface: &NetInterface, ip: Ipv4Addr) -> Result<(), AssignError> {
        let os = if cfg!(windows) {
            Os::Windows
        } else {
            Os::MacOs
        };
        let (program, args) = assign_command(os, &iface.name, ip);
        let out = std::process::Command::new(program)
            .args(&args)
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::PermissionDenied => AssignError::PermissionDenied,
                _ => AssignError::Failed(e.to_string()),
            })?;
        if out.status.success() {
            return Ok(());
        }
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if is_permission_failure(&text) {
            Err(AssignError::PermissionDenied)
        } else {
            Err(AssignError::Failed(text.trim().to_owned()))
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn assign(&self, _iface: &NetInterface, _ip: Ipv4Addr) -> Result<(), AssignError> {
        Err(AssignError::PermissionDenied)
    }
}

/// Asignador de mentira: apunta lo que se le pidió y falla como se le diga.
#[derive(Default)]
pub struct FakeAssigner {
    fail: Mutex<Option<AssignError>>,
    calls: Mutex<Vec<(String, Ipv4Addr)>>,
}

impl FakeAssigner {
    pub fn failing(error: AssignError) -> Self {
        Self {
            fail: Mutex::new(Some(error)),
            calls: Mutex::default(),
        }
    }

    pub fn calls(&self) -> Vec<(String, Ipv4Addr)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl AddrAssigner for FakeAssigner {
    fn assign(&self, iface: &NetInterface, ip: Ipv4Addr) -> Result<(), AssignError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((iface.name.clone(), ip));
        let fail = self
            .fail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        fail.map_or(Ok(()), Err)
    }
}
