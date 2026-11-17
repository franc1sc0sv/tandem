//! Monitores en Tandem (DESIGN §10, DECISIONS D11–D13, T-110).
//!
//! Todo el DDC pasa por `monitor-ddc` (repo `monitor-firmware`, A5): un solo estándar, el del
//! firmware propio; lo demás es «no compatible» y nunca recibe un Set.
//!
//! - [`election`]: quién es el dueño de cada monitor, igual en las dos máquinas.
//! - [`directory`]: lo que se sabe de cada monitor (canales de las dos, dueño, lo que leyó el
//!   dueño) y las vistas para la UI.
//! - [`local`]: los monitores que ve esta máquina: sondeo, eventos y estado de los que le toca
//!   leer, y el cambio de entrada con veredicto.
//! - [`ring`]: el reloj del monitor (`0xEF`), que da la vuelta, y la hora de cada evento.
//! - [`guard`]: vuelta automática de la entrada para las pruebas de hardware, en el proceso y
//!   con un vigilante aparte.
//!
//! Todo es síncrono (el DDC bloquea): el agente lo llama fuera del runtime de tokio.

pub mod directory;
pub mod election;
pub mod guard;
pub mod local;
pub mod ring;

pub use directory::Directory;
pub use election::elect;
pub use local::{LocalConfig, LocalMonitors, NO_VERDICT, NOT_OWNER};

use monitor_ddc::Input;
use tandem_core::MonitorInput;

/// La entrada de `monitor-ddc` como la nombra Tandem.
pub fn to_core(input: Input) -> MonitorInput {
    match input {
        Input::DisplayPort => MonitorInput::DisplayPort,
        Input::Hdmi => MonitorInput::Hdmi,
    }
}

/// Lo inverso de [`to_core`].
pub fn to_ddc(input: MonitorInput) -> Input {
    match input {
        MonitorInput::DisplayPort => Input::DisplayPort,
        MonitorInput::Hdmi => Input::Hdmi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_convert_both_ways() {
        for i in MonitorInput::ALL {
            assert_eq!(to_core(to_ddc(i)), i);
        }
    }
}
