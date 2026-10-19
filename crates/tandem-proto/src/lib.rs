//! Mensajes entre agentes y UI, versionado.
//!
//! - [`msg`]: los mensajes: [`Hello`] de apertura, control entre agentes ([`ControlMsg`]),
//!   emparejamiento ([`PairingMsg`]) y la API local de la UI ([`UiRequest`], [`AgentToUi`]).
//! - [`frame`]: codificación `postcard` y framing por longitud (u32 LE).
//!
//! Toda conexión empieza con un [`Hello`] en cada sentido, que se lee con
//! [`decode_hello`]: si la versión no coincide se corta ahí con
//! [`ProtoError::IncompatibleVersion`], antes de interpretar nada más.
//!
//! Los mensajes de entrada (teclado y mouse) los define la sesión de entrada, en
//! `tandem_input::session::msg`, con la codificación de este crate.

pub mod frame;
pub mod msg;
mod wire;

pub use frame::{FrameDecoder, MAX_FRAME_LEN, decode, decode_hello, encode, encode_frame};
pub use msg::{
    AgentToUi, CalibrationOutcome, CalibrationProbe, CalibrationStep, CalibrationUpdate,
    Capabilities, Channel, ControlMsg, DeskCapabilities, DeskSync, DisplayInfo, DisplayProfile,
    EdidId, Hello, HubReport, InputCaps, InputPermission, InputStatus, KeyProbeMsg, KeyProbeStatus,
    Link, LinkBasis, ManualSwitchEvent, MonitorCandidate, MonitorCaps, MonitorScan, NewMonitor,
    PairingMsg, PairingStatus, Peer, PeerStatus, PowerCaps, PowerEvent, ProbeAnswer, ProbeAsk,
    ProbeEvent, ProbeMonitor, ProbeOutcome, ProbePlace, ProbeScreen, Reply, SavedProfile,
    ScreenRect, SetupInfo, StepOutcome, TxnEvent, UiCommand, UiRequest, UndoFailure,
};

/// Versión del protocolo. Se sube con cualquier cambio incompatible en los mensajes.
///
/// Solo se habla con la misma versión: los dos agentes y la UI se distribuyen juntos.
pub const PROTOCOL_VERSION: u16 = 13;

/// Desde esta versión cada agente anuncia sus capacidades al conectarse (T-084): uno
/// anterior no las manda y no se le puede hablar como a un par.
pub const CAPABILITIES_SINCE: u16 = 10;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    #[error("no se pudo codificar el mensaje: {0}")]
    Encode(postcard::Error),
    #[error("mensaje mal formado: {0}")]
    Decode(postcard::Error),
    #[error("sobran {0} bytes después del mensaje")]
    TrailingBytes(usize),
    /// La conexión queda inservible: el flujo ya no se puede resincronizar.
    #[error("trama de {len} bytes, el máximo es {max}")]
    FrameTooLarge { len: usize, max: usize },
    #[error(
        "versión de protocolo incompatible: la nuestra es {ours}, la del otro lado es {theirs}{}",
        if *theirs < CAPABILITIES_SINCE {
            " (una versión anterior, sin anuncio de capacidades): instala la misma versión de Tandem en las dos máquinas"
        } else {
            ": instala la misma versión de Tandem en las dos máquinas"
        }
    )]
    IncompatibleVersion { ours: u16, theirs: u16 },
}

#[cfg(test)]
mod tests {
    use super::PROTOCOL_VERSION;

    /// T-081 publicó la v8, T-082 la v9 y T-084 (el anuncio de capacidades) la v10. T-085
    /// agrega al estado lo que no está confirmado y los pedidos «Aplicar modo» y «qué
    /// muestra»: otra forma de los mensajes, otra versión, para que dos binarios distintos
    /// nunca se acepten como compatibles. T-087 (asistente: monitores físicos, teclado por
    /// pulsación) sube a la v12. T-089 (`ProbeInput` y `MarkManual` con `discard_codes`, que cambian
    /// la forma de los pedidos de la UI) sube a la v13.
    #[test]
    fn la_version_queda_por_encima_de_la_de_t084() {
        assert_eq!(PROTOCOL_VERSION, 13);
        assert_eq!(super::CAPABILITIES_SINCE, 10);
    }
}
