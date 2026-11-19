//! Discovery of the other agent (DESIGN §6).
//!
//! Two sources of addresses for the [`LinkManager`](crate::manager::LinkManager):
//!
//! - The fixed ones of the direct links (USB4 `10.77.1.0/30`, Ethernet
//!   `10.77.2.0/30`), which come from the config: [`validate_static_links`] and
//!   [`link_candidates`].
//! - The home LAN over mDNS: each agent announces `_tandem._udp` with the
//!   [`Fingerprint`] of its key and looks for that of its paired peer
//!   ([`PeerResolver`]); announcements from other agents are ignored. What is
//!   discovered enters as a Wi-Fi candidate.
//! - While pairing, whoever waits announces `_tandem-pair._udp` and whoever searches
//!   accepts any agent ([`MdnsDiscovery::pairing`]).
//!
//! Finding the peer over mDNS does not give it trust: the transport still
//! verifies the key by pinning on connect.

mod fingerprint;
mod mdns;
mod peer;
mod statics;

pub use fingerprint::Fingerprint;
pub use mdns::{
    MdnsDiscovery, PAIRING_SERVICE_TYPE, SERVICE_TYPE, announcement, discovery_event, service_info,
};
pub use peer::{Announcement, DiscoveryEvent, PeerResolver, TXT_FINGERPRINT};
pub use statics::{link_candidates, validate_static_links};

use crate::manager::{LinkCandidate, LinkKind};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("invalid fingerprint: 64 hex digits expected")]
    InvalidFingerprint,
    #[error("the fixed link {} ({}) has no port", .0.kind, .0.addr)]
    StaticWithoutPort(LinkCandidate),
    #[error("the fixed link {} has an invalid address: {}", .0.kind, .0.addr)]
    StaticBadAddress(LinkCandidate),
    #[error("the fixed link {0} is configured twice")]
    DuplicateStatic(LinkKind),
    #[error("mDNS: {0}")]
    Mdns(#[from] mdns_sd::Error),
}
