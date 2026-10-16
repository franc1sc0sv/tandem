//! Estado puro del gestor: qué enlaces están sanos y cuál es el activo.
//! Sin E/S ni tiempo, para probarlo con propiedades.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use super::{LinkCandidate, LinkKind, ManagerError, Plane};

/// Cambio del enlace activo. `seq` crece de uno en uno desde 0, así que un
/// consumidor puede comprobar que no se perdió ni se reordenó ninguno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkEvent {
    pub seq: u64,
    pub from: Option<LinkKind>,
    pub to: Option<LinkKind>,
    /// Si con el enlace nuevo hay plano de datos. Al pasar a `false`, los
    /// servicios de datos se pausan ("requiere cable").
    pub data_plane: bool,
}

/// Salud de un enlace según el último ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkStatus {
    pub kind: LinkKind,
    pub addr: SocketAddr,
    /// Ida y vuelta del último ping; `None` si el enlace está caído.
    pub rtt: Option<Duration>,
}

pub(crate) struct LinkTable<H> {
    links: BTreeMap<LinkKind, Slot<H>>,
    active: Option<LinkKind>,
    next_seq: u64,
}

struct Slot<H> {
    addr: SocketAddr,
    up: Option<(H, Duration)>,
}

impl<H> LinkTable<H> {
    pub(crate) fn new(candidates: &[LinkCandidate]) -> Result<Self, ManagerError> {
        if candidates.is_empty() {
            return Err(ManagerError::NoCandidates);
        }
        let mut links = BTreeMap::new();
        for c in candidates {
            let slot = Slot {
                addr: c.addr,
                up: None,
            };
            if links.insert(c.kind, slot).is_some() {
                return Err(ManagerError::DuplicateLink(c.kind));
            }
        }
        Ok(Self {
            links,
            active: None,
            next_seq: 0,
        })
    }

    /// Aplica el resultado de un ping: `Some((handle, rtt))` si respondió.
    /// Devuelve el evento si cambió el enlace activo; el cambio es inmediato,
    /// sin esperar más pings.
    pub(crate) fn report(
        &mut self,
        kind: LinkKind,
        up: Option<(H, Duration)>,
    ) -> Option<LinkEvent> {
        let slot = self.links.get_mut(&kind)?;
        slot.up = up;
        let best = self
            .links
            .iter()
            .find(|(_, s)| s.up.is_some())
            .map(|(k, _)| *k);
        if best == self.active {
            return None;
        }
        let event = LinkEvent {
            seq: self.next_seq,
            from: self.active,
            to: best,
            data_plane: best.is_some_and(|k| k.allows(Plane::Data)),
        };
        self.next_seq += 1;
        self.active = best;
        Some(event)
    }

    pub(crate) fn active(&self) -> Option<(LinkKind, &H)> {
        let kind = self.active?;
        let (handle, _) = self.links.get(&kind)?.up.as_ref()?;
        Some((kind, handle))
    }

    pub(crate) fn statuses(&self) -> Vec<LinkStatus> {
        self.links
            .iter()
            .map(|(kind, s)| LinkStatus {
                kind: *kind,
                addr: s.addr,
                rtt: s.up.as_ref().map(|(_, rtt)| *rtt),
            })
            .collect()
    }

    pub(crate) fn candidates(&self) -> impl Iterator<Item = LinkCandidate> + '_ {
        self.links.iter().map(|(kind, s)| LinkCandidate {
            kind: *kind,
            addr: s.addr,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const RTT: Duration = Duration::from_millis(1);

    fn cand(kind: LinkKind, port: u16) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
        }
    }

    fn all() -> LinkTable<u32> {
        LinkTable::new(&[
            cand(LinkKind::Wifi, 3),
            cand(LinkKind::Usb4, 1),
            cand(LinkKind::Ethernet, 2),
        ])
        .unwrap()
    }

    #[test]
    fn rechaza_config_vacía_o_repetida() {
        assert!(matches!(
            LinkTable::<u32>::new(&[]),
            Err(ManagerError::NoCandidates)
        ));
        assert!(matches!(
            LinkTable::<u32>::new(&[cand(LinkKind::Wifi, 1), cand(LinkKind::Wifi, 2)]),
            Err(ManagerError::DuplicateLink(LinkKind::Wifi))
        ));
    }

    #[test]
    fn elige_el_mejor_sano_y_vuelve_al_preferido() {
        let mut t = all();
        assert_eq!(t.active().map(|(k, _)| k), None);

        let e = t.report(LinkKind::Wifi, Some((3, RTT))).unwrap();
        assert_eq!(
            (e.seq, e.from, e.to, e.data_plane),
            (0, None, Some(LinkKind::Wifi), false)
        );
        let e = t.report(LinkKind::Usb4, Some((1, RTT))).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));
        assert!(e.data_plane);
        // Un enlace peor que sube no cambia nada.
        assert_eq!(t.report(LinkKind::Ethernet, Some((2, RTT))), None);
        // Pings repetidos tampoco.
        assert_eq!(t.report(LinkKind::Usb4, Some((1, RTT))), None);

        let e = t.report(LinkKind::Usb4, None).unwrap();
        assert_eq!(
            (e.from, e.to),
            (Some(LinkKind::Usb4), Some(LinkKind::Ethernet))
        );
        assert_eq!(t.active(), Some((LinkKind::Ethernet, &2)));
        let e = t.report(LinkKind::Usb4, Some((10, RTT))).unwrap();
        assert_eq!((e.seq, e.to), (3, Some(LinkKind::Usb4)));
        // El handle es el de la conexión nueva.
        assert_eq!(t.active(), Some((LinkKind::Usb4, &10)));
    }

    #[test]
    fn sin_enlaces_sanos_no_hay_activo() {
        let mut t = all();
        t.report(LinkKind::Ethernet, Some((2, RTT)));
        let e = t.report(LinkKind::Ethernet, None).unwrap();
        assert_eq!(
            (e.from, e.to, e.data_plane),
            (Some(LinkKind::Ethernet), None, false)
        );
        assert_eq!(t.active(), None);
    }

    #[test]
    fn un_enlace_no_configurado_se_ignora() {
        let mut t = LinkTable::new(&[cand(LinkKind::Wifi, 3)]).unwrap();
        assert_eq!(t.report(LinkKind::Usb4, Some((1, RTT))), None);
        assert_eq!(t.active(), None);
    }

    #[test]
    fn el_estado_lista_cada_enlace_en_orden_de_prioridad() {
        let mut t = all();
        t.report(LinkKind::Ethernet, Some((2, Duration::from_millis(3))));
        let s = t.statuses();
        assert_eq!(
            s.iter().map(|s| s.kind).collect::<Vec<_>>(),
            LinkKind::ALL.to_vec()
        );
        assert_eq!(s[1].rtt, Some(Duration::from_millis(3)));
        assert_eq!(s[0].rtt, None);
        assert_eq!(s[2].addr.port(), 3);
    }

    fn kind() -> impl Strategy<Value = LinkKind> {
        prop::sample::select(LinkKind::ALL.to_vec())
    }

    proptest! {
        /// Tras cualquier secuencia de pings: el activo es el mejor sano, el
        /// plano de datos existe si y solo si hay un cable sano, y los eventos
        /// salen numerados y encadenados (cada `from` es el `to` anterior).
        #[test]
        fn invariantes_de_la_tabla(
            configured in prop::sample::subsequence(LinkKind::ALL.to_vec(), 1..=3),
            reports in prop::collection::vec((kind(), any::<bool>()), 0..60),
        ) {
            let cands: Vec<_> = configured.iter().enumerate()
                .map(|(i, k)| cand(*k, i as u16 + 1)).collect();
            let mut t = LinkTable::<u8>::new(&cands).unwrap();
            let mut up = std::collections::BTreeSet::new();
            let mut last: Option<LinkKind> = None;
            let mut seq = 0;
            for (k, ok) in reports {
                let event = t.report(k, ok.then_some((0, RTT)));
                if configured.contains(&k) {
                    if ok { up.insert(k); } else { up.remove(&k); }
                }
                let best = up.iter().next().copied();
                prop_assert_eq!(t.active().map(|(k, _)| k), best);
                match event {
                    Some(e) => {
                        prop_assert_eq!(e.seq, seq);
                        prop_assert_eq!(e.from, last);
                        prop_assert_ne!(e.from, e.to);
                        prop_assert_eq!(e.to, best);
                        prop_assert_eq!(e.data_plane, up.iter().any(|k| k.is_wired()));
                        prop_assert!(!(e.data_plane && e.to == Some(LinkKind::Wifi)));
                        seq += 1;
                        last = e.to;
                    }
                    None => prop_assert_eq!(last, best),
                }
            }
        }
    }
}
