use std::collections::BTreeSet;
use std::net::SocketAddr;

use super::DiscoveryError;
use crate::manager::{LinkCandidate, LinkKind};

/// Comprueba las direcciones fijas del otro agente que vienen de la config
/// (los enlaces directos, por ejemplo `10.77.1.2` por USB4 y `10.77.2.2` por
/// Ethernet; DESIGN §6).
pub fn validate_static_links(links: &[LinkCandidate]) -> Result<(), DiscoveryError> {
    let mut kinds = BTreeSet::new();
    for l in links {
        if l.addr.port() == 0 {
            return Err(DiscoveryError::StaticWithoutPort(*l));
        }
        if l.addr.ip().is_unspecified() || l.addr.ip().is_multicast() {
            return Err(DiscoveryError::StaticBadAddress(*l));
        }
        if !kinds.insert(l.kind) {
            return Err(DiscoveryError::DuplicateStatic(l.kind));
        }
    }
    Ok(())
}

/// Candidatos para el [`LinkManager`](crate::manager::LinkManager): las
/// direcciones fijas de la config tal cual y, si la config no fija el Wi-Fi,
/// la mejor dirección que el par anunció en la LAN como enlace Wi-Fi.
///
/// Una dirección descubierta que ya es la de un enlace fijo se descarta: el
/// par también anuncia por mDNS en las interfaces de cable y esa dirección no
/// es la de la LAN.
pub fn link_candidates(statics: &[LinkCandidate], discovered: &[SocketAddr]) -> Vec<LinkCandidate> {
    let mut out: Vec<LinkCandidate> = statics.to_vec();
    if !statics.iter().any(|l| l.kind == LinkKind::Wifi) {
        let lan = discovered
            .iter()
            .find(|d| !statics.iter().any(|s| s.addr.ip() == d.ip()));
        if let Some(&addr) = lan {
            out.push(LinkCandidate {
                kind: LinkKind::Wifi,
                addr,
            });
        }
    }
    out.sort_by_key(|l| l.kind);
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn link(kind: LinkKind, addr: &str) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: addr.parse().unwrap(),
        }
    }

    fn statics() -> Vec<LinkCandidate> {
        vec![
            link(LinkKind::Ethernet, "10.77.2.2:47770"),
            link(LinkKind::Usb4, "10.77.1.2:47770"),
        ]
    }

    #[test]
    fn valida_las_fijas() {
        assert!(validate_static_links(&statics()).is_ok());
        assert!(validate_static_links(&[]).is_ok());
        assert!(matches!(
            validate_static_links(&[link(LinkKind::Usb4, "10.77.1.2:0")]),
            Err(DiscoveryError::StaticWithoutPort(_))
        ));
        assert!(matches!(
            validate_static_links(&[link(LinkKind::Usb4, "0.0.0.0:47770")]),
            Err(DiscoveryError::StaticBadAddress(_))
        ));
        assert!(matches!(
            validate_static_links(&[
                link(LinkKind::Usb4, "10.77.1.2:47770"),
                link(LinkKind::Usb4, "10.77.1.6:47770"),
            ]),
            Err(DiscoveryError::DuplicateStatic(LinkKind::Usb4))
        ));
    }

    #[test]
    fn las_fijas_pasan_tal_cual_y_en_orden_de_prioridad() {
        assert_eq!(
            link_candidates(&statics(), &[]),
            vec![
                link(LinkKind::Usb4, "10.77.1.2:47770"),
                link(LinkKind::Ethernet, "10.77.2.2:47770"),
            ]
        );
    }

    #[test]
    fn lo_descubierto_se_agrega_como_wifi() {
        let lan: SocketAddr = "192.168.1.20:47770".parse().unwrap();
        let got = link_candidates(&statics(), &["10.77.1.2:47770".parse().unwrap(), lan]);
        assert_eq!(
            got.last(),
            Some(&link(LinkKind::Wifi, "192.168.1.20:47770"))
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn un_wifi_fijo_manda_sobre_lo_descubierto() {
        let mut s = statics();
        s.push(link(LinkKind::Wifi, "192.168.1.99:47770"));
        let got = link_candidates(&s, &["192.168.1.20:47770".parse().unwrap()]);
        assert_eq!(
            got.last(),
            Some(&link(LinkKind::Wifi, "192.168.1.99:47770"))
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn sin_fijas_queda_solo_lo_descubierto() {
        let got = link_candidates(&[], &["192.168.1.20:47770".parse().unwrap()]);
        assert_eq!(got, vec![link(LinkKind::Wifi, "192.168.1.20:47770")]);
        assert!(link_candidates(&[], &[]).is_empty());
    }

    proptest! {
        /// El resultado siempre es aceptable para el gestor: sin tipos
        /// repetidos, con todas las fijas y en orden de prioridad.
        #[test]
        fn candidatos_validos(
            kinds in prop::sample::subsequence(LinkKind::ALL.to_vec(), 0..=3),
            found in prop::collection::vec((0u8..3, 1u8..4), 0..6),
        ) {
            let statics: Vec<_> = kinds
                .iter()
                .enumerate()
                .map(|(i, &kind)| LinkCandidate {
                    kind,
                    addr: SocketAddr::from(([10, 77, i as u8 + 1, 2], 47770)),
                })
                .collect();
            let found: Vec<SocketAddr> = found
                .into_iter()
                .map(|(net, host)| SocketAddr::from(([10, 77, net + 1, host], 47770)))
                .collect();
            let got = link_candidates(&statics, &found);
            let mut seen = BTreeSet::new();
            prop_assert!(got.iter().all(|l| seen.insert(l.kind)));
            prop_assert!(got.windows(2).all(|w| w[0].kind < w[1].kind));
            prop_assert!(statics.iter().all(|s| got.contains(s)));
            for l in got.iter().filter(|l| !statics.contains(l)) {
                prop_assert_eq!(l.kind, LinkKind::Wifi);
                prop_assert!(!statics.iter().any(|s| s.addr.ip() == l.addr.ip()));
            }
        }
    }
}
