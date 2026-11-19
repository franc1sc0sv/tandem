use std::net::Ipv4Addr;
use std::sync::{Mutex, PoisonError};

use tandem_core::Os;
use thiserror::Error;

use super::NetInterface;

/// Why the fallback IP could not be assigned.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssignError {
    /// Permissions are missing: the command has to be given to the user.
    #[error("no permission to change the interface address")]
    PermissionDenied,
    #[error("could not assign the address: {0}")]
    Failed(String),
}

/// Sets the fallback IP (`10.77.1.0/30`) on an interface. It is platform code (`netsh`,
/// `ifconfig`), so it goes behind a trait with a double.
pub trait AddrAssigner: Send + Sync + 'static {
    fn assign(&self, iface: &NetInterface, ip: Ipv4Addr) -> Result<(), AssignError>;
}

/// Program and arguments that assign `ip` with mask `/30` to the interface `name`.
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

/// Whether what the command said on failing is a lack of permissions (in English and Spanish: the
/// `netsh` messages come out in the system's language).
pub fn is_permission_failure(output: &str) -> bool {
    let o = output.to_lowercase();
    [
        "elevation",
        // Spanish Windows prints `netsh` errors in Spanish: these words are data to match.
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

/// The real assigner: runs the system command. On Linux (development only) there is
/// nothing to assign and it always asks the user for the command.
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

/// Fake assigner: records what it was asked and fails as told.
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
