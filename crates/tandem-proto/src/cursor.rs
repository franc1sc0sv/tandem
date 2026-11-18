//! El cursor en vivo (T-128): la posición que la UI pinta como un punto sobre cada
//! escritorio y los cruces de borde que destellan.

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Frecuencia máxima de las muestras del cursor, por emisor.
pub const MAX_HZ: u32 = 30;

/// Una posición del cursor, en las coordenadas nativas de su máquina (las del
/// [`ScreenRect`](crate::ScreenRect)).
///
/// Enteras: la UI pinta un punto, y así el mensaje se puede comparar. Las muestras salen
/// de la máquina que tiene el foco: es quien sabe dónde está el cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorSample {
    /// La máquina donde está el cursor.
    pub machine: Machine,
    /// Id de la pantalla que lo contiene (la más cercana si cae en un hueco); vacío si no
    /// se conocen las pantallas.
    pub screen: String,
    pub x: i32,
    pub y: i32,
}

#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    pub fn samples() -> Vec<CursorSample> {
        vec![
            CursorSample {
                machine: Machine::A,
                screen: "mac-1".into(),
                x: 1919,
                y: 540,
            },
            CursorSample {
                machine: Machine::B,
                screen: String::new(),
                x: -3440,
                y: i32::MAX,
            },
        ]
    }
}
