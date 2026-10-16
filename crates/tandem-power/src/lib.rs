//! Wake-on-LAN, apagado remoto (DESIGN §5 y §6).
//!
//! - [`MacAddr`] y [`magic_packet`]: el paquete mágico, sin E/S, para probarlo en cualquier sistema.
//! - [`Wake`] / [`UdpWake`]: mandar el paquete por UDP desde la interfaz Ethernet configurada.
//! - [`Shutdown`]: apagado limpio de esta máquina; la implementación real solo existe en
//!   Windows ([`WindowsShutdown`]).
//!
//! Esperar a que el agente de la PC responda tras el Wake-on-LAN no es de este crate:
//! lo hace el orquestador de `tandemd`, que sabe si el otro agente está conectado.

mod mac;
mod wol;

#[cfg(windows)]
mod windows;

pub use mac::{MacAddr, ParseMacError};
pub use wol::{DEFAULT_WOL_PORT, MAGIC_PACKET_LEN, UdpWake, Wake, WolTarget, magic_packet};

#[cfg(windows)]
pub use windows::WindowsShutdown;

#[derive(Debug, thiserror::Error)]
pub enum PowerError {
    #[error("no se pudo mandar el Wake-on-LAN a {target}: {source}")]
    Send {
        target: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("no se pudo usar la dirección de origen {source_ip} para el Wake-on-LAN: {error}")]
    Bind {
        source_ip: std::net::IpAddr,
        #[source]
        error: std::io::Error,
    },
    #[error("el sistema rechazó el apagado: {0}")]
    Shutdown(String),
}

/// Apagar esta máquina de forma limpia (como `shutdown /s /t 0`).
pub trait Shutdown: Send + Sync {
    /// Inicia el apagado y vuelve enseguida: el sistema cierra los programas después,
    /// y eso le da tiempo al agente de contestar que el paso salió bien.
    fn power_off(&self) -> Result<(), PowerError>;
}
