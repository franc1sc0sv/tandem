//! Estado puro del gestor: qué enlaces están sanos y cuál es el activo.
//! Sin E/S: el tiempo y si hay una transacción en curso llegan como argumentos,
//! para probarlo con propiedades.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::time::Instant;

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
    /// Cuánto tiene que estar sano un enlace que ya se cayó antes de volver a él.
    settle: Duration,
}

struct Slot<H> {
    addr: SocketAddr,
    up: Option<(H, Duration)>,
    /// Primer ping bueno de la racha actual.
    up_since: Option<Instant>,
    /// Se cayó alguna vez después de haber estado sano: desde entonces, para
    /// volver a él hay que esperar `settle`.
    dropped: bool,
}

impl<H> Slot<H> {
    /// Si se puede pasar a este enlace sin miedo a que parpadee.
    fn settled(&self, now: Instant, settle: Duration) -> bool {
        match self.up_since {
            Some(since) => !self.dropped || now.saturating_duration_since(since) >= settle,
            None => false,
        }
    }
}

impl<H> LinkTable<H> {
    pub(crate) fn new(
        candidates: &[LinkCandidate],
        settle: Duration,
    ) -> Result<Self, ManagerError> {
        if candidates.is_empty() {
            return Err(ManagerError::NoCandidates);
        }
        let mut links = BTreeMap::new();
        for c in candidates {
            let slot = Slot {
                addr: c.addr,
                up: None,
                up_since: None,
                dropped: false,
            };
            if links.insert(c.kind, slot).is_some() {
                return Err(ManagerError::DuplicateLink(c.kind));
            }
        }
        Ok(Self {
            links,
            active: None,
            next_seq: 0,
            settle,
        })
    }

    /// Aplica el resultado de un ping: `Some((handle, rtt))` si respondió.
    /// Devuelve el evento si cambió el enlace activo.
    ///
    /// Si el activo cae, el cambio es inmediato, haya o no una transacción
    /// (`held`). Subir a un enlace mejor solo se hace sin transacción en curso y,
    /// si ese enlace ya se cayó alguna vez, tras `settle` de pings buenos seguidos:
    /// así un cable que parpadea no arrastra el tráfico de un lado a otro.
    pub(crate) fn report(
        &mut self,
        kind: LinkKind,
        up: Option<(H, Duration)>,
        now: Instant,
        held: bool,
    ) -> Option<LinkEvent> {
        let Some(slot) = self.links.get_mut(&kind) else {
            return self.reevaluate(now, held);
        };
        match (&slot.up, &up) {
            (None, Some(_)) => slot.up_since = Some(now),
            (Some(_), None) => {
                slot.up_since = None;
                slot.dropped = true;
            }
            _ => {}
        }
        slot.up = up;
        self.reevaluate(now, held)
    }

    /// Vuelve a elegir el activo sin un ping nuevo (por ejemplo, al terminar
    /// una transacción).
    pub(crate) fn reevaluate(&mut self, now: Instant, held: bool) -> Option<LinkEvent> {
        let best = self.choose(now, held);
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

    fn choose(&self, now: Instant, held: bool) -> Option<LinkKind> {
        let active = self
            .active
            .filter(|k| self.links.get(k).is_some_and(|s| s.up.is_some()));
        let settled = self
            .links
            .iter()
            .find(|(_, s)| s.settled(now, self.settle))
            .map(|(k, _)| *k);
        match active {
            Some(a) if held => Some(a),
            Some(a) => Some(settled.filter(|b| *b < a).unwrap_or(a)),
            // Sin activo sano: el mejor estable y, si no hay, cualquiera sano;
            // mejor un enlace que parpadea que ninguno.
            None => settled.or_else(|| {
                self.links
                    .iter()
                    .find(|(_, s)| s.up.is_some())
                    .map(|(k, _)| *k)
            }),
        }
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
    const SETTLE: Duration = Duration::from_secs(3);

    fn cand(kind: LinkKind, port: u16) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
        }
    }

    fn all() -> LinkTable<u32> {
        LinkTable::new(
            &[
                cand(LinkKind::Wifi, 3),
                cand(LinkKind::Usb4, 1),
                cand(LinkKind::Ethernet, 2),
            ],
            SETTLE,
        )
        .unwrap()
    }

    /// Tabla con reloj propio, para escribir las pruebas en milisegundos.
    struct Clock<H> {
        t: LinkTable<H>,
        start: Instant,
        held: bool,
    }

    impl<H> Clock<H> {
        fn new(t: LinkTable<H>) -> Self {
            Self {
                t,
                start: Instant::now(),
                held: false,
            }
        }

        fn at(&mut self, ms: u64, kind: LinkKind, up: Option<H>) -> Option<LinkEvent> {
            let now = self.start + Duration::from_millis(ms);
            self.t.report(kind, up.map(|h| (h, RTT)), now, self.held)
        }

        fn release(&mut self, ms: u64) -> Option<LinkEvent> {
            self.held = false;
            let now = self.start + Duration::from_millis(ms);
            self.t.reevaluate(now, false)
        }

        fn active(&self) -> Option<LinkKind> {
            self.t.active().map(|(k, _)| k)
        }
    }

    #[test]
    fn rechaza_config_vacía_o_repetida() {
        assert!(matches!(
            LinkTable::<u32>::new(&[], SETTLE),
            Err(ManagerError::NoCandidates)
        ));
        assert!(matches!(
            LinkTable::<u32>::new(&[cand(LinkKind::Wifi, 1), cand(LinkKind::Wifi, 2)], SETTLE),
            Err(ManagerError::DuplicateLink(LinkKind::Wifi))
        ));
    }

    #[test]
    fn elige_el_mejor_sano_y_vuelve_al_preferido_tras_asentarse() {
        let mut c = Clock::new(all());
        assert_eq!(c.active(), None);

        let e = c.at(0, LinkKind::Wifi, Some(3)).unwrap();
        assert_eq!(
            (e.seq, e.from, e.to, e.data_plane),
            (0, None, Some(LinkKind::Wifi), false)
        );
        // Un enlace que nunca se cayó entra enseguida (el arranque no espera).
        let e = c.at(1, LinkKind::Usb4, Some(1)).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));
        assert!(e.data_plane);
        // Un enlace peor que sube no cambia nada.
        assert_eq!(c.at(2, LinkKind::Ethernet, Some(2)), None);
        // Pings repetidos tampoco.
        assert_eq!(c.at(1000, LinkKind::Usb4, Some(1)), None);

        // La caída del activo conmuta al instante.
        let e = c.at(2000, LinkKind::Usb4, None).unwrap();
        assert_eq!(
            (e.from, e.to),
            (Some(LinkKind::Usb4), Some(LinkKind::Ethernet))
        );
        assert_eq!(c.t.active(), Some((LinkKind::Ethernet, &2)));

        // Vuelve, pero hasta 3 s de pings buenos seguidos no se regresa.
        for ms in [3000, 4000, 5000, 5999] {
            assert_eq!(c.at(ms, LinkKind::Usb4, Some(10)), None, "a {ms} ms");
        }
        let e = c.at(6000, LinkKind::Usb4, Some(10)).unwrap();
        assert_eq!((e.seq, e.to), (3, Some(LinkKind::Usb4)));
        // El handle es el de la conexión nueva.
        assert_eq!(c.t.active(), Some((LinkKind::Usb4, &10)));
    }

    #[test]
    fn un_usb4_que_parpadea_cada_700_ms_no_mueve_el_enlace() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        assert_eq!(c.active(), Some(LinkKind::Usb4));

        // Cortado de [k·700, k·700 + 350) ms; un ping por segundo.
        let mut events = Vec::new();
        for ms in (1000..30_000).step_by(1000) {
            let up = (ms % 700) >= 350;
            events.extend(c.at(ms, LinkKind::Usb4, up.then_some(1)));
            events.extend(c.at(ms, LinkKind::Ethernet, Some(2)));
        }
        let tos: Vec<_> = events.iter().map(|e| e.to).collect();
        assert_eq!(tos, [Some(LinkKind::Ethernet)], "{events:?}");
    }

    #[test]
    fn durante_una_transacción_solo_se_cambia_si_cae_el_activo() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        c.at(0, LinkKind::Wifi, Some(3));
        c.at(1000, LinkKind::Usb4, None);
        assert_eq!(c.active(), Some(LinkKind::Ethernet));

        c.held = true;
        // USB4 vuelve y se asienta, pero hay una transacción: se queda Ethernet.
        for ms in (2000..=9000).step_by(1000) {
            assert_eq!(c.at(ms, LinkKind::Usb4, Some(1)), None, "a {ms} ms");
        }
        // Al terminar, se sube enseguida.
        let e = c.release(9100).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));

        // Si el activo cae en medio de una transacción, se cambia igual.
        c.held = true;
        let e = c.at(10_000, LinkKind::Usb4, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Ethernet));
        assert_eq!(c.release(10_100), None);
    }

    #[test]
    fn sin_activo_prefiere_un_enlace_estable_a_uno_que_parpadea() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        c.at(0, LinkKind::Wifi, Some(3));
        c.at(1000, LinkKind::Usb4, None);
        c.at(1500, LinkKind::Usb4, Some(1));
        let e = c.at(2000, LinkKind::Ethernet, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Wifi));
        assert!(!e.data_plane);
        // Pero sin otra opción, se usa el que parpadea.
        let e = c.at(2100, LinkKind::Wifi, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));
    }

    #[test]
    fn sin_enlaces_sanos_no_hay_activo() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Ethernet, Some(2));
        let e = c.at(1, LinkKind::Ethernet, None).unwrap();
        assert_eq!(
            (e.from, e.to, e.data_plane),
            (Some(LinkKind::Ethernet), None, false)
        );
        assert_eq!(c.active(), None);
    }

    #[test]
    fn un_enlace_no_configurado_se_ignora() {
        let mut c = Clock::new(LinkTable::new(&[cand(LinkKind::Wifi, 3)], SETTLE).unwrap());
        assert_eq!(c.at(0, LinkKind::Usb4, Some(1)), None);
        assert_eq!(c.active(), None);
    }

    #[test]
    fn el_estado_lista_cada_enlace_en_orden_de_prioridad() {
        let mut t = all();
        t.report(
            LinkKind::Ethernet,
            Some((2, Duration::from_millis(3))),
            Instant::now(),
            false,
        );
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
        /// Tras cualquier secuencia de pings, con o sin transacción en curso:
        /// - hay activo si y solo si hay algún enlace sano, y el activo está sano;
        /// - con una transacción, el activo solo cambia si se cayó;
        /// - sin transacción, ningún enlace mejor que el activo lleva `SETTLE`
        ///   sano (o nunca se cayó);
        /// - mientras ningún enlace se haya caído, el activo es el mejor sano;
        /// - los eventos salen numerados y encadenados, y el plano de datos
        ///   existe si y solo si el nuevo activo es de cable.
        #[test]
        fn invariantes_de_la_tabla(
            configured in prop::sample::subsequence(LinkKind::ALL.to_vec(), 1..=3),
            reports in prop::collection::vec(
                (kind(), any::<bool>(), 0u64..2500, any::<bool>()),
                0..80,
            ),
        ) {
            let cands: Vec<_> = configured.iter().enumerate()
                .map(|(i, k)| cand(*k, i as u16 + 1)).collect();
            let mut t = LinkTable::<u8>::new(&cands, SETTLE).unwrap();
            let mut now = Instant::now();
            let mut up = std::collections::BTreeMap::<LinkKind, Instant>::new();
            let mut ever_dropped = std::collections::BTreeSet::new();
            let mut last: Option<LinkKind> = None;
            let mut seq = 0;
            for (k, ok, step_ms, held) in reports {
                now += Duration::from_millis(step_ms);
                let event = t.report(k, ok.then_some((0, RTT)), now, held);
                if configured.contains(&k) {
                    if ok {
                        up.entry(k).or_insert(now);
                    } else if up.remove(&k).is_some() {
                        ever_dropped.insert(k);
                    }
                }
                let active = t.active().map(|(k, _)| k);
                prop_assert_eq!(active.is_some(), !up.is_empty());
                if let Some(a) = active {
                    prop_assert!(up.contains_key(&a));
                }
                if held && last.is_some_and(|l| up.contains_key(&l)) {
                    prop_assert_eq!(active, last);
                }
                if !held && let Some(a) = active {
                    for (b, since) in up.range(..a) {
                        prop_assert!(
                            ever_dropped.contains(b) && now - *since < SETTLE,
                            "{:?} asentado y mejor que {:?}", b, a
                        );
                    }
                }
                if ever_dropped.is_empty() && !held {
                    prop_assert_eq!(active, up.keys().next().copied());
                }
                match event {
                    Some(e) => {
                        prop_assert_eq!(e.seq, seq);
                        prop_assert_eq!(e.from, last);
                        prop_assert_ne!(e.from, e.to);
                        prop_assert_eq!(e.to, active);
                        prop_assert_eq!(e.data_plane, e.to.is_some_and(|k| k.is_wired()));
                        seq += 1;
                        last = e.to;
                    }
                    None => prop_assert_eq!(last, active),
                }
            }
        }
    }
}
