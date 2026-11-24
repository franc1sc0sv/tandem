//! Wake-on-LAN and remote shutdown (DESIGN §5 and §6).
//!
//! - [`MacAddr`] and [`magic_packet`]: the magic packet, with no I/O, so it can be tested on any system.
//! - [`Wake`] / [`UdpWake`]: send the packet over UDP from the configured Ethernet interface.
//! - [`Shutdown`]: clean shutdown of this machine; the real implementation only exists on
//!   Windows ([`WindowsShutdown`]).
//!
//! Waiting for the PC agent to answer after Wake-on-LAN is not this crate's job:
//! the `tandemd` orchestrator does it, since it knows whether the other agent is connected.

mod mac;
pub mod nic;
mod wol;

#[cfg(windows)]
mod windows;

pub use mac::{MacAddr, ParseMacError};
pub use wol::{DEFAULT_WOL_PORT, MAGIC_PACKET_LEN, UdpWake, Wake, WolTarget, magic_packet};

#[cfg(windows)]
pub use windows::WindowsShutdown;

#[derive(Debug, thiserror::Error)]
pub enum PowerError {
    #[error("could not send Wake-on-LAN to {target}: {source}")]
    Send {
        target: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("could not use source address {source_ip} for Wake-on-LAN: {error}")]
    Bind {
        source_ip: std::net::IpAddr,
        #[source]
        error: std::io::Error,
    },
    #[error("the system rejected the shutdown: {0}")]
    Shutdown(String),
}

/// Shut this machine down cleanly (like `shutdown /s /t 0`).
pub trait Shutdown: Send + Sync {
    /// Starts the shutdown and returns right away: the system closes programs afterwards,
    /// which gives the agent time to answer that the step went fine.
    fn power_off(&self) -> Result<(), PowerError>;
}
