use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Tipo de enlace entre las dos máquinas (DESIGN §6).
///
/// El orden de las variantes es la prioridad: `Usb4 < Ethernet < Wifi`, así
/// que el mejor enlace sano es el menor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// USB4 / Thunderbolt directo.
    Usb4,
    /// Ethernet 2.5G directo.
    Ethernet,
    /// Wi-Fi de la LAN de casa.
    Wifi,
}

impl LinkKind {
    pub const ALL: [LinkKind; 3] = [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi];

    /// Prioridad de la tabla de DESIGN §6 (1 = la mejor).
    pub fn priority(self) -> u8 {
        match self {
            LinkKind::Usb4 => 1,
            LinkKind::Ethernet => 2,
            LinkKind::Wifi => 3,
        }
    }

    pub fn is_wired(self) -> bool {
        !matches!(self, LinkKind::Wifi)
    }

    /// Si el enlace puede llevar el plano pedido. Es una regla fija, sin
    /// configuración: el plano de datos nunca va por Wi-Fi.
    pub fn allows(self, plane: Plane) -> bool {
        match plane {
            Plane::Control => true,
            Plane::Data => self.is_wired(),
        }
    }
}

impl fmt::Display for LinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LinkKind::Usb4 => "USB4",
            LinkKind::Ethernet => "Ethernet",
            LinkKind::Wifi => "Wi-Fi",
        })
    }
}

/// Planos de tráfico (DESIGN §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Plane {
    /// Teclado, mouse, portapapeles, mensajes entre agentes, monitores, modos.
    Control,
    /// SSH, carpetas compartidas, servicios GPU, archivos.
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlaneError {
    #[error("no hay ningún enlace sano con el otro agente")]
    NoLink,
    /// Lo que la UI muestra como "requiere cable".
    #[error("el plano de datos requiere cable; el enlace activo es {active}")]
    RequiresCable { active: LinkKind },
}

/// Comprueba si `plane` puede ir por el enlace activo `active`.
pub fn check_plane(plane: Plane, active: Option<LinkKind>) -> Result<LinkKind, PlaneError> {
    let active = active.ok_or(PlaneError::NoLink)?;
    if active.allows(plane) {
        Ok(active)
    } else {
        Err(PlaneError::RequiresCable { active })
    }
}

/// Dirección del otro agente por un enlace concreto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCandidate {
    pub kind: LinkKind,
    pub addr: SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_prioridad_sigue_el_diseño() {
        let mut kinds = LinkKind::ALL;
        kinds.reverse();
        kinds.sort();
        assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi]);
        assert!(kinds.windows(2).all(|w| w[0].priority() < w[1].priority()));
    }

    #[test]
    fn el_plano_de_datos_solo_va_por_cable() {
        assert!(LinkKind::Usb4.allows(Plane::Data));
        assert!(LinkKind::Ethernet.allows(Plane::Data));
        assert!(!LinkKind::Wifi.allows(Plane::Data));
        assert!(LinkKind::ALL.iter().all(|k| k.allows(Plane::Control)));
    }

    #[test]
    fn check_plane_rechaza_datos_por_wifi() {
        assert_eq!(
            check_plane(Plane::Data, Some(LinkKind::Wifi)),
            Err(PlaneError::RequiresCable {
                active: LinkKind::Wifi
            })
        );
        assert_eq!(
            check_plane(Plane::Control, Some(LinkKind::Wifi)),
            Ok(LinkKind::Wifi)
        );
        assert_eq!(
            check_plane(Plane::Data, Some(LinkKind::Ethernet)),
            Ok(LinkKind::Ethernet)
        );
        assert_eq!(check_plane(Plane::Data, None), Err(PlaneError::NoLink));
        assert_eq!(check_plane(Plane::Control, None), Err(PlaneError::NoLink));
    }

    #[test]
    fn el_tipo_se_lee_de_la_config_en_minúsculas() {
        let c: LinkCandidate =
            toml::from_str("kind = \"ethernet\"\naddr = \"10.77.2.2:47770\"").unwrap();
        assert_eq!(c.kind, LinkKind::Ethernet);
        assert_eq!(c.addr, "10.77.2.2:47770".parse().unwrap());
        assert!(toml::from_str::<LinkCandidate>("kind = \"wifi\"\naddr = \"1.2.3.4:1\"").is_ok());
        assert!(toml::from_str::<LinkCandidate>("kind = \"lte\"\naddr = \"1.2.3.4:1\"").is_err());
    }
}
