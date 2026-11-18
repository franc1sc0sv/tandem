//! Métricas de red por camino, para la pantalla Red y el resumen del Puesto (T-102,
//! DESIGN §6, mockups `V3-Red` y `V3-Puesto`).
//!
//! Los tiempos van en microsegundos enteros (las latencias por cable son de décimas de
//! milisegundo) y las fechas en milisegundos Unix, para que los mensajes se comparen con
//! `Eq` y la UI diga «hace 3 min».

use serde::{Deserialize, Serialize};

use crate::msg::Link;

/// Muestras del historial de cada camino (una por segundo).
pub const HISTORY_SECS: usize = 60;
/// Cambios de camino que se guardan.
pub const MAX_SWITCHES: usize = 50;
/// Lo que dura la prueba de ancho de banda.
pub const MEASURE_SECS: u64 = 3;

/// El papel de un camino ahora.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathState {
    /// Lleva la entrada y los pedidos.
    Primary,
    /// Responde, listo para tomar el tráfico si el principal cae.
    Backup,
    /// Nunca respondió desde que arrancó el agente (o no está configurado): «Sin cable».
    Unplugged,
    /// Respondió y dejó de hacerlo.
    Down,
}

/// El último ancho de banda medido de un camino.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bandwidth {
    pub bytes_per_sec: u64,
    /// Cuándo se midió.
    pub at_ms: u64,
}

/// Un segundo del historial de un camino. `None`: no respondió o no hubo entrada.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencySample {
    pub rtt_us: Option<u32>,
    pub input_p99_us: Option<u32>,
}

/// Un camino con sus números.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMetrics {
    pub link: Link,
    pub state: PathState,
    /// Ida y vuelta del último ping; `None` si no responde.
    pub rtt_us: Option<u32>,
    /// Latencia de la entrada en el último segundo; `None` sin entrada por este camino.
    pub input_p50_us: Option<u32>,
    pub input_p99_us: Option<u32>,
    /// El p99 que se le tolera a la entrada por este camino (DESIGN §7.1): la línea de
    /// «objetivo» de la gráfica.
    pub input_target_us: u32,
    pub bandwidth: Option<Bandwidth>,
    /// Hasta [`HISTORY_SECS`] muestras, de la más vieja a la más nueva.
    pub history: Vec<LatencySample>,
}

/// Un cambio del camino principal («pasó a Wi-Fi en 0.6 s»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSwitch {
    pub at_ms: u64,
    pub from: Option<Link>,
    pub to: Option<Link>,
    /// Si `from` se cayó: desde su última respuesta hasta que `to` tomó el tráfico (lo
    /// que la entrada estuvo sin camino, como mucho). `None` si fue una subida a un camino
    /// mejor, sin corte.
    pub took_ms: Option<u32>,
}

/// Un aviso de la red para la pantalla Red: algo que el agente no pudo hacer solo y el
/// usuario sí (por ejemplo, asignar la IP de respaldo de un cable sin permisos).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkNotice {
    pub interface: String,
    pub message: String,
    /// La orden exacta que resuelve el aviso.
    pub command: String,
}

/// La red con la otra máquina vista desde este agente.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// Los tres caminos, en orden de prioridad (USB-C, Ethernet, Wi-Fi).
    pub paths: Vec<PathMetrics>,
    /// Hasta [`MAX_SWITCHES`] cambios, del más nuevo al más viejo.
    pub switches: Vec<PathSwitch>,
    /// El camino que se está midiendo ahora, si alguno.
    pub measuring: Option<Link>,
    /// Lo que el usuario tiene que hacer a mano (T-094).
    pub notices: Vec<NetworkNotice>,
}

impl NetworkStatus {
    /// El camino principal, si hay.
    pub fn primary(&self) -> Option<&PathMetrics> {
        self.paths.iter().find(|p| p.state == PathState::Primary)
    }

    pub fn path(&self, link: Link) -> Option<&PathMetrics> {
        self.paths.iter().find(|p| p.link == link)
    }
}

/// Lo que contesta quien recibe la prueba de ancho de banda (por el mismo stream, al
/// llegarle el fin): cuánto recibió y en cuánto tiempo, del primer byte al último.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasureReport {
    pub bytes: u64,
    pub micros: u64,
}

impl MeasureReport {
    /// Bytes por segundo; `None` si no llegó nada que medir.
    pub fn bytes_per_sec(&self) -> Option<u64> {
        if self.bytes == 0 || self.micros == 0 {
            return None;
        }
        let rate = u128::from(self.bytes) * 1_000_000 / u128::from(self.micros);
        Some(u64::try_from(rate).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    /// El ejemplo del mockup `V3-Red`: USB-C principal, Ethernet sin cable, Wi-Fi de
    /// respaldo, y el corte del cable.
    pub fn status() -> NetworkStatus {
        let sample = LatencySample {
            rtt_us: Some(300),
            input_p99_us: Some(1900),
        };
        NetworkStatus {
            paths: vec![
                PathMetrics {
                    link: Link::Usb4,
                    state: PathState::Primary,
                    rtt_us: Some(300),
                    input_p50_us: Some(400),
                    input_p99_us: Some(1900),
                    input_target_us: 4000,
                    bandwidth: Some(Bandwidth {
                        bytes_per_sec: 1_175_000_000,
                        at_ms: 1_763_400_000_000,
                    }),
                    history: vec![sample, LatencySample::default(), sample],
                },
                PathMetrics {
                    link: Link::Ethernet,
                    state: PathState::Unplugged,
                    rtt_us: None,
                    input_p50_us: None,
                    input_p99_us: None,
                    input_target_us: 4000,
                    bandwidth: None,
                    history: vec![],
                },
                PathMetrics {
                    link: Link::Wifi,
                    state: PathState::Backup,
                    rtt_us: Some(3800),
                    input_p50_us: None,
                    input_p99_us: None,
                    input_target_us: 20_000,
                    bandwidth: None,
                    history: vec![LatencySample {
                        rtt_us: Some(3800),
                        input_p99_us: None,
                    }],
                },
            ],
            notices: vec![],
            switches: vec![
                PathSwitch {
                    at_ms: 1_763_400_060_000,
                    from: Some(Link::Wifi),
                    to: Some(Link::Usb4),
                    took_ms: None,
                },
                PathSwitch {
                    at_ms: 1_763_400_000_000,
                    from: Some(Link::Usb4),
                    to: Some(Link::Wifi),
                    took_ms: Some(600),
                },
                PathSwitch {
                    at_ms: 1_763_399_000_000,
                    from: None,
                    to: Some(Link::Usb4),
                    took_ms: None,
                },
            ],
            measuring: Some(Link::Wifi),
        }
    }

    pub fn path_state_arm(s: PathState) -> usize {
        match s {
            PathState::Primary => 0,
            PathState::Backup => 1,
            PathState::Unplugged => 2,
            PathState::Down => 3,
        }
    }

    /// Un estado de cada clase.
    pub fn path_states() -> Vec<PathState> {
        let mut v: Vec<_> = status().paths.iter().map(|p| p.state).collect();
        v.push(PathState::Down);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;
    use crate::{decode, encode};

    #[test]
    fn el_estado_de_la_red_va_y_vuelve() {
        let s = status();
        assert_eq!(decode::<NetworkStatus>(&encode(&s).unwrap()).unwrap(), s);
        let r = MeasureReport {
            bytes: 3,
            micros: 1,
        };
        assert_eq!(decode::<MeasureReport>(&encode(&r).unwrap()).unwrap(), r);
        let seen: std::collections::BTreeSet<_> =
            path_states().into_iter().map(path_state_arm).collect();
        assert_eq!(seen.len(), 4, "faltan muestras de algún estado");
    }

    #[test]
    fn el_principal_es_el_que_lleva_el_tráfico() {
        let s = status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Usb4));
        assert_eq!(
            s.path(Link::Ethernet).map(|p| p.state),
            Some(PathState::Unplugged)
        );
        assert_eq!(NetworkStatus::default().primary(), None);
    }

    #[test]
    fn el_ancho_de_banda_sale_de_lo_recibido_y_su_tiempo() {
        let r = |bytes, micros| MeasureReport { bytes, micros }.bytes_per_sec();
        assert_eq!(r(30_000_000, 3_000_000), Some(10_000_000));
        assert_eq!(r(1, 1), Some(1_000_000));
        assert_eq!(r(0, 3_000_000), None);
        assert_eq!(r(10, 0), None);
        assert_eq!(r(u64::MAX, 1), Some(u64::MAX));
    }
}
