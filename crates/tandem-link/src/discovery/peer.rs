use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

use super::Fingerprint;

/// Clave TXT con la huella del agente que anuncia.
pub const TXT_FINGERPRINT: &str = "fp";

/// Un servicio `_tandem._udp` resuelto en la LAN, ya sin tipos de mdns-sd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Nombre completo de la instancia (identifica el anuncio al retirarse).
    pub instance: String,
    /// Valor crudo de la clave TXT `fp`, si venía.
    pub fingerprint: Option<String>,
    /// Direcciones con el puerto del servicio.
    pub addrs: Vec<SocketAddr>,
}

/// Lo que llega del navegador mDNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Resolved(Announcement),
    Removed { instance: String },
}

/// Filtra los anuncios de la LAN y se queda solo con los del par emparejado.
///
/// Se ignoran los anuncios sin huella o con una huella mal formada, los de
/// agentes no emparejados y el propio (mDNS devuelve también lo que uno
/// anuncia).
#[derive(Debug, Clone)]
pub struct PeerResolver {
    own: Fingerprint,
    /// `None`: cualquier agente salvo el propio (para emparejarse con uno nuevo).
    wanted: Option<BTreeSet<Fingerprint>>,
    /// Anuncios aceptados, por instancia.
    found: BTreeMap<String, (Fingerprint, Vec<SocketAddr>)>,
}

impl PeerResolver {
    pub fn new(own: Fingerprint, peers: impl IntoIterator<Item = Fingerprint>) -> Self {
        let mut wanted: BTreeSet<_> = peers.into_iter().collect();
        wanted.remove(&own);
        Self {
            own,
            wanted: Some(wanted),
            found: BTreeMap::new(),
        }
    }

    /// Acepta a cualquier agente salvo el propio: lo que anuncia quien está
    /// emparejando, que todavía no es de confianza. Encontrarlo no le da confianza:
    /// la da la comparación del código.
    pub fn any(own: Fingerprint) -> Self {
        Self {
            own,
            wanted: None,
            found: BTreeMap::new(),
        }
    }

    /// Huella del anuncio si es de un par buscado; `None` si hay que ignorarlo.
    pub fn accepts(&self, ann: &Announcement) -> Option<Fingerprint> {
        let fp: Fingerprint = ann.fingerprint.as_deref()?.parse().ok()?;
        let wanted = self.wanted.as_ref().is_none_or(|w| w.contains(&fp));
        (fp != self.own && wanted).then_some(fp)
    }

    /// Aplica un evento; devuelve si cambiaron las direcciones del par.
    pub fn apply(&mut self, event: DiscoveryEvent) -> bool {
        let before = self.addrs();
        match event {
            DiscoveryEvent::Resolved(ann) => match self.accepts(&ann) {
                Some(fp) => {
                    let addrs = usable(&ann.addrs);
                    if addrs.is_empty() {
                        self.found.remove(&ann.instance);
                    } else {
                        self.found.insert(ann.instance, (fp, addrs));
                    }
                }
                // Si una instancia que antes era del par ahora anuncia otra
                // huella, deja de contar.
                None => {
                    self.found.remove(&ann.instance);
                }
            },
            DiscoveryEvent::Removed { instance } => {
                self.found.remove(&instance);
            }
        }
        self.addrs() != before
    }

    /// Direcciones del par en la LAN, de la más a la menos preferida.
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.collect(|_| true)
    }

    /// Direcciones de un par concreto.
    pub fn addrs_of(&self, peer: &Fingerprint) -> Vec<SocketAddr> {
        self.collect(|fp| fp == peer)
    }

    fn collect(&self, keep: impl Fn(&Fingerprint) -> bool) -> Vec<SocketAddr> {
        let unique: BTreeSet<SocketAddr> = self
            .found
            .values()
            .filter(|(fp, _)| keep(fp))
            .flat_map(|(_, a)| a.iter().copied())
            .collect();
        let mut all: Vec<_> = unique.into_iter().collect();
        all.sort_by_key(|a| rank(a.ip()));
        all
    }
}

/// Descarta lo que no sirve para conectar con otra máquina.
fn usable(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    addrs
        .iter()
        .copied()
        .filter(|a| a.port() != 0)
        .filter(|a| {
            let ip = a.ip();
            !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())
        })
        .collect()
}

/// IPv4 primero (la LAN de casa), luego IPv6 global y al final IPv6 de enlace
/// local, que depende del índice de interfaz.
fn rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) if v4.is_link_local() => 1,
        IpAddr::V4(_) => 0,
        IpAddr::V6(v6) if v6.is_unicast_link_local() => 3,
        IpAddr::V6(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::identity::Identity;

    fn fp(seed: u8) -> Fingerprint {
        Fingerprint::of(&Identity::from_seed([seed; 32]).public_key())
    }

    fn ann(instance: &str, fp: Option<String>, addrs: &[&str]) -> Announcement {
        Announcement {
            instance: instance.into(),
            fingerprint: fp,
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    const ME: u8 = 1;
    const PEER: u8 = 2;
    const STRANGER: u8 = 3;

    fn resolver() -> PeerResolver {
        PeerResolver::new(fp(ME), [fp(PEER)])
    }

    #[test]
    fn acepta_solo_la_huella_del_par() {
        let r = resolver();
        let peer = ann("pc", Some(fp(PEER).to_hex()), &["192.168.1.20:47770"]);
        assert_eq!(r.accepts(&peer), Some(fp(PEER)));
        // En mayúsculas también es la misma huella.
        let upper = ann("pc", Some(fp(PEER).to_hex().to_uppercase()), &[]);
        assert_eq!(r.accepts(&upper), Some(fp(PEER)));
    }

    #[test]
    fn ignora_otros_anuncios() {
        let mut r = resolver();
        let otros = [
            // Otro agente Tandem no emparejado.
            ann("x", Some(fp(STRANGER).to_hex()), &["192.168.1.30:47770"]),
            // El propio anuncio.
            ann("yo", Some(fp(ME).to_hex()), &["192.168.1.10:47770"]),
            // Sin huella.
            ann("sin", None, &["192.168.1.40:47770"]),
            // Huella mal formada o truncada.
            ann("mal", Some("hola".into()), &["192.168.1.50:47770"]),
            ann("corta", Some(fp(PEER).short()), &["192.168.1.60:47770"]),
        ];
        for a in otros {
            assert_eq!(r.accepts(&a), None, "{a:?}");
            assert!(!r.apply(DiscoveryEvent::Resolved(a)));
        }
        assert!(r.addrs().is_empty());

        assert!(r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &["192.168.1.20:47770"],
        ))));
        assert_eq!(r.addrs(), vec!["192.168.1.20:47770".parse().unwrap()]);
    }

    #[test]
    fn la_propia_huella_nunca_es_el_par() {
        // Aunque por error se pase la propia llave como par.
        let r = PeerResolver::new(fp(ME), [fp(ME), fp(PEER)]);
        assert_eq!(r.accepts(&ann("yo", Some(fp(ME).to_hex()), &[])), None);
    }

    #[test]
    fn sin_lista_acepta_a_cualquiera_menos_a_si_mismo() {
        let mut r = PeerResolver::any(fp(ME));
        let extrano = ann("x", Some(fp(STRANGER).to_hex()), &["192.168.1.30:47771"]);
        assert_eq!(r.accepts(&extrano), Some(fp(STRANGER)));
        assert_eq!(r.accepts(&ann("yo", Some(fp(ME).to_hex()), &[])), None);
        assert_eq!(r.accepts(&ann("sin", None, &[])), None);
        assert!(r.apply(DiscoveryEvent::Resolved(extrano)));
        assert_eq!(r.addrs(), vec!["192.168.1.30:47771".parse().unwrap()]);
    }

    #[test]
    fn se_olvida_al_retirarse_o_cambiar_de_huella() {
        let mut r = resolver();
        let peer = ann("pc", Some(fp(PEER).to_hex()), &["192.168.1.20:47770"]);
        assert!(r.apply(DiscoveryEvent::Resolved(peer.clone())));
        // Repetir el mismo anuncio no cambia nada.
        assert!(!r.apply(DiscoveryEvent::Resolved(peer)));
        assert!(r.apply(DiscoveryEvent::Removed {
            instance: "pc".into()
        }));
        assert!(r.addrs().is_empty());

        r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &["192.168.1.20:47770"],
        )));
        assert!(r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(STRANGER).to_hex()),
            &["192.168.1.20:47770"],
        ))));
        assert!(r.addrs().is_empty());
    }

    #[test]
    fn ordena_y_limpia_direcciones() {
        let mut r = resolver();
        r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &[
                "[fe80::1%2]:47770",
                "[2001:db8::5]:47770",
                "127.0.0.1:47770",
                "0.0.0.0:47770",
                "192.168.1.20:0",
                "169.254.3.3:47770",
                "192.168.1.20:47770",
            ],
        )));
        let got: Vec<String> = r.addrs().iter().map(ToString::to_string).collect();
        assert_eq!(
            got,
            [
                "192.168.1.20:47770",
                "169.254.3.3:47770",
                "[2001:db8::5]:47770",
                "[fe80::1%2]:47770",
            ]
        );
        assert_eq!(r.addrs_of(&fp(PEER)), r.addrs());
        assert!(r.addrs_of(&fp(STRANGER)).is_empty());
    }

    proptest! {
        /// Ningún anuncio con una huella ajena aporta direcciones, en
        /// cualquier orden y mezclado con los del par.
        #[test]
        fn solo_el_par_aporta_direcciones(
            events in prop::collection::vec((0u8..4, 0u8..5, any::<bool>(), 1u8..255), 0..40)
        ) {
            let mut r = resolver();
            let mut peer_ips = BTreeSet::new();
            for (who, inst, remove, last) in events {
                let instance = format!("i{inst}");
                let addr: SocketAddr = format!("10.0.{who}.{last}:47770").parse().unwrap();
                if remove {
                    r.apply(DiscoveryEvent::Removed { instance });
                } else {
                    let seed = [ME, PEER, STRANGER, 9][usize::from(who)];
                    r.apply(DiscoveryEvent::Resolved(ann(&instance, Some(fp(seed).to_hex()), &[])
                        .with(addr)));
                    if seed == PEER {
                        peer_ips.insert(addr);
                    }
                }
                for a in r.addrs() {
                    prop_assert!(peer_ips.contains(&a), "{a} no viene del par");
                }
            }
        }
    }

    impl Announcement {
        fn with(mut self, addr: SocketAddr) -> Self {
            self.addrs.push(addr);
            self
        }
    }
}
