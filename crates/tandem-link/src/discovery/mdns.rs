use std::net::{SocketAddr, SocketAddrV6};

use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{
    Announcement, DiscoveryError, DiscoveryEvent, Fingerprint, PeerResolver, TXT_FINGERPRINT,
};
use crate::identity::PublicKey;

/// Tipo de servicio DNS-SD que anuncia cada agente.
pub const SERVICE_TYPE: &str = "_tandem._udp.local.";

/// Lo que anuncia un agente solo mientras empareja, con el puerto de su endpoint de
/// emparejamiento. Es otro tipo para que nadie lo confunda con el de un par.
pub const PAIRING_SERVICE_TYPE: &str = "_tandem-pair._udp.local.";

/// Anuncio propio: instancia y host `tandem-<huella corta>`, el puerto QUIC
/// del agente y la huella completa en la clave TXT `fp`. Las direcciones las
/// pone mdns-sd según las interfaces que haya en cada momento.
pub fn service_info(own: &Fingerprint, port: u16) -> Result<ServiceInfo, DiscoveryError> {
    service_info_of(SERVICE_TYPE, own, port)
}

fn service_info_of(
    service_type: &str,
    own: &Fingerprint,
    port: u16,
) -> Result<ServiceInfo, DiscoveryError> {
    let name = format!("tandem-{}", own.short());
    let host = format!("{name}.local.");
    let txt = [(TXT_FINGERPRINT, own.to_hex())];
    let info = ServiceInfo::new(service_type, &name, &host, "", port, &txt[..])?;
    Ok(info.enable_addr_auto())
}

/// Pasa un servicio resuelto por mdns-sd a [`Announcement`].
pub fn announcement(service: &ResolvedService) -> Announcement {
    let port = service.get_port();
    let mut addrs: Vec<SocketAddr> = service
        .get_addresses()
        .iter()
        .map(|ip| match ip {
            // Una IPv6 de enlace local no sirve sin la interfaz por la que llegó.
            ScopedIp::V6(v6) if v6.addr().is_unicast_link_local() => {
                SocketAddrV6::new(*v6.addr(), port, 0, v6.scope_id().index).into()
            }
            other => SocketAddr::new(other.to_ip_addr(), port),
        })
        .collect();
    addrs.sort();
    Announcement {
        instance: service.get_fullname().to_owned(),
        fingerprint: service
            .get_property_val_str(TXT_FINGERPRINT)
            .map(str::to_owned),
        addrs,
    }
}

/// Lo que importa de un evento del navegador; el resto se ignora.
pub fn discovery_event(event: ServiceEvent) -> Option<DiscoveryEvent> {
    match event {
        ServiceEvent::ServiceResolved(s) => Some(DiscoveryEvent::Resolved(announcement(&s))),
        ServiceEvent::ServiceRemoved(_, instance) => Some(DiscoveryEvent::Removed { instance }),
        _ => None,
    }
}

/// Anuncio propio y búsqueda del par en la LAN por mDNS.
///
/// Las direcciones del par se publican en [`MdnsDiscovery::peer_addrs`],
/// de la más a la menos preferida. Al soltarlo se retira el anuncio.
pub struct MdnsDiscovery {
    daemon: ServiceDaemon,
    peers: watch::Receiver<Vec<SocketAddr>>,
    task: JoinHandle<()>,
}

impl MdnsDiscovery {
    /// Anuncia `own` en `port` y busca a los pares de `trusted` (los del
    /// trust store). Tiene que llamarse dentro de un runtime de tokio.
    pub fn start(
        own: &PublicKey,
        port: u16,
        trusted: impl IntoIterator<Item = PublicKey>,
    ) -> Result<Self, DiscoveryError> {
        let own = Fingerprint::of(own);
        let resolver = PeerResolver::new(own, trusted.into_iter().map(|k| Fingerprint::of(&k)));
        Self::run(SERVICE_TYPE, own, Some(port), resolver)
    }

    /// Para emparejarse: anuncia el endpoint de emparejamiento en `port` (si hay) y
    /// busca a cualquier otro agente que esté emparejando ([`PAIRING_SERVICE_TYPE`]).
    /// Tiene que llamarse dentro de un runtime de tokio.
    pub fn pairing(own: &PublicKey, port: Option<u16>) -> Result<Self, DiscoveryError> {
        let own = Fingerprint::of(own);
        Self::run(PAIRING_SERVICE_TYPE, own, port, PeerResolver::any(own))
    }

    fn run(
        service_type: &str,
        own: Fingerprint,
        port: Option<u16>,
        mut resolver: PeerResolver,
    ) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        if let Some(port) = port {
            daemon.register(service_info_of(service_type, &own, port)?)?;
        }
        let browse = daemon.browse(service_type)?;
        let (tx, peers) = watch::channel(Vec::new());
        let task = tokio::spawn(async move {
            // Termina cuando el daemon se apaga y cierra el canal.
            while let Ok(event) = browse.recv_async().await {
                let Some(event) = discovery_event(event) else {
                    continue;
                };
                if resolver.apply(event) {
                    let addrs = resolver.addrs();
                    tracing::info!(?addrs, "direcciones del par en la LAN");
                    tx.send_replace(addrs);
                }
            }
        });
        Ok(Self {
            daemon,
            peers,
            task,
        })
    }

    pub fn peer_addrs(&self) -> watch::Receiver<Vec<SocketAddr>> {
        self.peers.clone()
    }
}

impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        // `shutdown` manda el adiós (TTL 0) de lo registrado antes de parar.
        if let Err(e) = self.daemon.shutdown() {
            tracing::debug!("no se pudo apagar mDNS: {e}");
        }
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::identity::Identity;

    fn key(seed: u8) -> PublicKey {
        Identity::from_seed([seed; 32]).public_key()
    }

    #[test]
    fn el_anuncio_lleva_la_huella_y_el_puerto() {
        let fp = Fingerprint::of(&key(1));
        let info = service_info(&fp, 47770).unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 47770);
        assert_eq!(
            info.get_fullname(),
            format!("tandem-{}.{SERVICE_TYPE}", fp.short())
        );
        assert_eq!(info.get_hostname(), format!("tandem-{}.local.", fp.short()));
        let fp_txt = fp.to_hex();
        assert_eq!(
            info.get_property_val_str(TXT_FINGERPRINT),
            Some(fp_txt.as_str())
        );
        assert!(info.is_addr_auto());
    }

    /// Ida y vuelta por los tipos de mdns-sd: lo que anuncia un agente lo
    /// acepta el otro, y no lo acepta un tercero.
    #[test]
    fn el_anuncio_resuelto_lo_reconoce_solo_el_par() {
        let (a, b, c) = (key(1), key(2), key(3));
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "tandem-x",
            "tandem-x.local.",
            "192.168.1.20,10.77.1.2",
            47770,
            &[(TXT_FINGERPRINT, Fingerprint::of(&a).to_hex())][..],
        )
        .unwrap();
        let event = ServiceEvent::ServiceResolved(Box::new(info.as_resolved_service()));
        let Some(DiscoveryEvent::Resolved(ann)) = discovery_event(event) else {
            panic!("se esperaba un anuncio resuelto");
        };
        assert_eq!(ann.instance, format!("tandem-x.{SERVICE_TYPE}"));
        assert_eq!(
            ann.addrs,
            vec![
                "10.77.1.2:47770".parse().unwrap(),
                "192.168.1.20:47770".parse().unwrap()
            ]
        );

        let par = PeerResolver::new(Fingerprint::of(&b), [Fingerprint::of(&a)]);
        assert_eq!(par.accepts(&ann), Some(Fingerprint::of(&a)));
        let tercero = PeerResolver::new(Fingerprint::of(&c), [Fingerprint::of(&b)]);
        assert_eq!(tercero.accepts(&ann), None);
        let el_mismo = PeerResolver::new(Fingerprint::of(&a), [Fingerprint::of(&b)]);
        assert_eq!(el_mismo.accepts(&ann), None);
    }

    #[test]
    fn retiro_y_eventos_ignorados() {
        assert_eq!(
            discovery_event(ServiceEvent::ServiceRemoved(
                SERVICE_TYPE.into(),
                "tandem-x._tandem._udp.local.".into()
            )),
            Some(DiscoveryEvent::Removed {
                instance: "tandem-x._tandem._udp.local.".into()
            })
        );
        assert_eq!(
            discovery_event(ServiceEvent::SearchStarted(SERVICE_TYPE.into())),
            None
        );
        assert_eq!(
            discovery_event(ServiceEvent::ServiceFound(
                SERVICE_TYPE.into(),
                "tandem-x._tandem._udp.local.".into()
            )),
            None
        );
    }

    /// Dos agentes en la misma máquina se encuentran por multicast real.
    /// Necesita una interfaz con multicast (en CI y en sandbox no se garantiza);
    /// la prueba entre la Mac y la PC es la de verdad.
    #[tokio::test]
    #[ignore = "hardware"]
    async fn dos_agentes_se_encuentran_en_la_lan() {
        let (a, b) = (key(1), key(2));
        let da = MdnsDiscovery::start(&a, 47771, [b]).unwrap();
        let db = MdnsDiscovery::start(&b, 47772, [a]).unwrap();
        let mut wa = da.peer_addrs();
        let found = tokio::time::timeout(
            Duration::from_secs(10),
            wa.wait_for(|addrs| !addrs.is_empty()),
        )
        .await
        .expect("no se encontró al par")
        .unwrap()
        .clone();
        assert!(found.iter().all(|a| a.port() == 47772), "{found:?}");
        drop(db);
    }
}
