//! Lo que viaja entre servidor y cliente de la sesión de entrada.
//!
//! Dos canales sobre la misma conexión (docs/research/input-engine.md):
//! - [`StreamMsg`], por un stream confiable y ordenado: entrar, salir, teclas,
//!   botones y el latido;
//! - [`Datagram`], no confiable: movimiento y scroll coalescidos.
//!
//! Los números de secuencia unen los dos: cada datagrama lleva uno creciente, y
//! los mensajes del stream que dependen de la posición (entrar, botones, la
//! posición exacta al detenerse) llevan el último enviado. El cliente descarta cualquier datagrama que no sea más nuevo
//! que lo último que aplicó, así uno atrasado nunca mueve el cursor hacia atrás.
//!
//! Las marcas `t_us` son del reloj monotónico del servidor ([`super::latency::now_us`])
//! al capturar: el cliente mide con ellas la latencia (T-093).

use serde::{Deserialize, Serialize};
use tandem_proto::ProtoError;

use tandem_proto::InputLatency;

use super::latency::Beat;
use crate::event::{Button, HidUsage, KeyState, Modifiers, Scroll};
use crate::geometry::Point;

/// Mensajes del stream confiable de entrada. Las posiciones van en las
/// coordenadas nativas del cursor del cliente.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StreamMsg {
    /// El cursor entra a la pantalla del cliente en `pos`, con estos modificadores
    /// presionados (el usuario puede cruzar con Shift apretado). `seq` es el último
    /// datagrama enviado: los anteriores ya no valen.
    Enter {
        seq: u64,
        pos: Point,
        modifiers: Modifiers,
    },
    /// El cursor vuelve al servidor: el cliente suelta todo.
    Leave,
    Key {
        usage: HidUsage,
        state: KeyState,
        t_us: u64,
    },
    /// Un clic en `pos`: si el datagrama con el último movimiento se perdió o llega
    /// después, el clic igual cae donde el usuario lo vio.
    Button {
        button: Button,
        state: KeyState,
        seq: u64,
        pos: Point,
        t_us: u64,
    },
    /// Posición exacta cuando el mouse se detiene, para que el cursor del cliente
    /// termine donde debe aunque se hayan perdido datagramas.
    Sync { seq: u64, pos: Point },
    /// Latido, en los dos sentidos: el silencio prolongado cuenta como enlace caído. Con
    /// su marca y el eco del último del otro lado estiman el desfase de los relojes.
    Heartbeat(Beat),
    /// Del cliente al servidor: la latencia que midió en la última ventana.
    Latency(InputLatency),
}

/// Datagramas de la conexión entre agentes. Es un enum para que otros usos
/// puedan sumarse sin confundirse con los de entrada.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Datagram {
    Motion(MotionDatagram),
}

/// Movimiento y scroll acumulados desde el datagrama anterior.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionDatagram {
    pub seq: u64,
    /// Delta en las unidades nativas del cliente, ya convertido por el servidor con la
    /// escala de cada pantalla que cruzó: el cliente solo lo suma, así su cursor queda
    /// exactamente donde el servidor cree aunque el tramo pase por pantallas de escalas
    /// distintas.
    pub dx: f64,
    pub dy: f64,
    pub scroll: Option<Scroll>,
    /// Resincronización absoluta: dónde tiene que quedar el cursor después de este
    /// datagrama (coordenadas nativas del cliente). Corrige lo que se haya perdido.
    pub resync: Option<Point>,
    /// Marca del primer evento que acumula (el más viejo).
    pub t_us: u64,
}

impl Datagram {
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        tandem_proto::encode(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        tandem_proto::decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ModifierKey;

    #[test]
    fn mensajes_ida_y_vuelta() {
        let msgs = [
            StreamMsg::Enter {
                seq: 9,
                pos: Point::new(3440.0, 540.5),
                modifiers: Modifiers::from_iter([ModifierKey::ShiftLeft]),
            },
            StreamMsg::Leave,
            StreamMsg::Key {
                usage: HidUsage::key(0x04),
                state: KeyState::Pressed,
                t_us: 17,
            },
            StreamMsg::Button {
                button: Button::Back,
                state: KeyState::Released,
                seq: u64::MAX,
                pos: Point::new(-10.0, 0.25),
                t_us: u64::MAX,
            },
            StreamMsg::Sync {
                seq: 3,
                pos: Point::new(1.0, 2.0),
            },
            StreamMsg::Heartbeat(Beat {
                sent_us: 1,
                echo: None,
            }),
            StreamMsg::Heartbeat(Beat {
                sent_us: 9_000_000,
                echo: Some(super::super::latency::Echo {
                    sent_us: 8_000_000,
                    held_us: 999_000,
                }),
            }),
            StreamMsg::Latency(InputLatency {
                link: None,
                count: 1000,
                p50_us: 400,
                p99_us: 900,
                max_us: 3000,
            }),
        ];
        for m in msgs {
            let bytes = tandem_proto::encode(&m).unwrap();
            assert_eq!(tandem_proto::decode::<StreamMsg>(&bytes).unwrap(), m);
        }
    }

    #[test]
    fn datagrama_ida_y_vuelta_y_compacto() {
        let d = Datagram::Motion(MotionDatagram {
            seq: 1_000_000,
            dx: 1.5,
            dy: -2.0,
            scroll: Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: -240,
            }),
            resync: Some(Point::new(100.0, 200.0)),
            t_us: 3_600_000_000,
        });
        let bytes = d.encode().unwrap();
        // Tiene que caber de sobra en cualquier MTU.
        assert!(bytes.len() < 64, "{} bytes", bytes.len());
        assert_eq!(Datagram::decode(&bytes).unwrap(), d);
        assert!(Datagram::decode(&[0xFF]).is_err());
    }
}
