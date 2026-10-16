//! Descubrimiento del otro agente (DESIGN §6).
//!
//! Dos fuentes de direcciones para el [`LinkManager`](crate::manager::LinkManager):
//!
//! - Las fijas de los enlaces directos (USB4 `10.77.1.0/30`, Ethernet
//!   `10.77.2.0/30`), que vienen de la config: [`validate_static_links`] y
//!   [`link_candidates`].
//! - La LAN de casa por mDNS: cada agente anuncia `_tandem._udp` con la
//!   [`Fingerprint`] de su llave y busca la de su par emparejado
//!   ([`PeerResolver`]); los anuncios de otros agentes se ignoran. Lo que se
//!   descubre entra como candidato Wi-Fi.
//!
//! Encontrar al par por mDNS no le da confianza: el transporte sigue
//! verificando la llave por pinning al conectar.

mod fingerprint;
mod mdns;
mod peer;
mod statics;

pub use fingerprint::Fingerprint;
pub use mdns::{MdnsDiscovery, SERVICE_TYPE, announcement, discovery_event, service_info};
pub use peer::{Announcement, DiscoveryEvent, PeerResolver, TXT_FINGERPRINT};
pub use statics::{link_candidates, validate_static_links};

use crate::manager::{LinkCandidate, LinkKind};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("huella inválida: se esperan 64 dígitos hex")]
    InvalidFingerprint,
    #[error("el enlace fijo {} ({}) no tiene puerto", .0.kind, .0.addr)]
    StaticWithoutPort(LinkCandidate),
    #[error("el enlace fijo {} tiene una dirección inválida: {}", .0.kind, .0.addr)]
    StaticBadAddress(LinkCandidate),
    #[error("el enlace fijo {0} está configurado dos veces")]
    DuplicateStatic(LinkKind),
    #[error("mDNS: {0}")]
    Mdns(#[from] mdns_sd::Error),
}
