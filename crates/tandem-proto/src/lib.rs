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

pub mod cursor;
pub mod env;
pub mod files;
pub mod frame;
pub mod msg;
pub mod net;
pub mod tunnel;
mod wire;
pub mod workspace;

pub use cursor::CursorSample;
pub use env::{
    CpuLoad, DiskSpace, EnvKind, Environment, Gpu, GpuLoad, Hardware, MemoryUse, Resources, Tool,
};
pub use files::{
    CHUNK_SIZE, Direction, EntryKind, FileEntry, FilesDown, FilesReport, FilesUp, ForwardInfo,
    Hash, MAX_ENTRIES, Manifest, Need, PAGE_BUDGET, PIECE, PortDown, PortUp, TransferProgress,
    entry_cost, entry_fits, page_entries, page_needs,
};
pub use frame::{FrameDecoder, MAX_FRAME_LEN, decode, decode_hello, encode, encode_frame};
pub use msg::{
    AgentToUi, Capabilities, Channel, ControlMsg, DeskCapabilities, FocusMsg, Hello, HubReport,
    InputCaps, InputLatency, InputPermission, InputStatus, KeyProbeMsg, KeyProbeStatus, Link,
    ModuleId, ModuleInfo, ModuleState, MonitorChannel, MonitorCompat, MonitorEvent,
    MonitorEventKind, MonitorReport, MonitorView, PairingMsg, PairingStatus, Peer, PeerStatus,
    PowerCaps, PowerEvent, ProcessInfo, Reply, ScreenRect, SetupInfo, StepOutcome, TxnEvent,
    UiCommand, UiRequest, UpdateMsg, UpdateStatus,
};
pub use net::{
    Bandwidth, LatencySample, MeasureReport, NetworkNotice, NetworkStatus, PathMetrics, PathState,
    PathSwitch,
};
pub use tunnel::{
    Client, ClientKind, ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput,
    ProcessState, Project, ProjectTunnels, Tunnel, TunnelDown, TunnelId, TunnelProcess,
    TunnelState, TunnelUp, TunnelView, group_by_project,
};
pub use workspace::{BringBackReport, CommitInfo, WorkspaceDown, WorkspaceSpec, WorkspaceUp};

/// Versión del protocolo. Se sube con cualquier cambio incompatible en los mensajes.
///
/// Solo se habla con la misma versión: los dos agentes y la UI se distribuyen juntos.
pub const PROTOCOL_VERSION: u16 = 34;

/// Desde esta versión cada agente anuncia sus capacidades al conectarse (T-084): uno
/// anterior no las manda y no se le puede hablar como a un par.
pub const CAPABILITIES_SINCE: u16 = 10;

/// Desde esta versión los dos lados son `A`/`B`, ordenados por clave pública (T-097): uno
/// anterior todavía se presenta como «Mac» o «PC» y no se puede saber qué lado es.
pub const SIDES_SINCE: u16 = 16;

fn version_hint(theirs: u16) -> &'static str {
    if theirs < CAPABILITIES_SINCE {
        " (una versión anterior, sin anuncio de capacidades): instala la misma versión de Tandem en las dos máquinas"
    } else if theirs < SIDES_SINCE {
        " (una versión anterior a los lados A/B, con «Mac» y «PC» fijos): instala la misma versión de Tandem en las dos máquinas"
    } else {
        ": instala la misma versión de Tandem en las dos máquinas"
    }
}

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
        version_hint(*theirs)
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
    /// la forma de los pedidos de la UI) sube a la v13. T-090 (proceso del agente en las
    /// capacidades y pedido de permisos) sube a la v14. T-092 (fuera los monitores, las
    /// sondas y los modos; pantallas con id propio y acomodo declarado) sube a la v15.
    /// T-097 (lados A/B por clave pública, `MachineInfo` en las capacidades y en el puesto)
    /// sube a la v16. T-098 (módulos con estado, entornos y monitores en las capacidades;
    /// `SetModule` en la UI y entre agentes) sube a la v17. T-099 (el puesto con escenas,
    /// atajos, preferencias y módulos, su sello con fecha, parches, historial y el aviso de
    /// cambio; `SetModule` ya no viaja entre agentes: va en el puesto) sube a la v18.
    /// T-110 (monitores: canal con compatibilidad y latencia, informes del dueño,
    /// `SwitchMonitor`) sube a la v19. T-105 (entornos con herramientas, equipo,
    /// recursos en vivo y re-escaneo) sube a la v20. T-101 (`[data]`, el tope de los datos
    /// por Wi-Fi, en el puesto y en sus parches) sube a la v21. T-122 (versiones de la app y pedido
    /// de actualizar a la otra) sube a la v22. T-100 (escenas completas en el puesto, la escena
    /// activa en el estado, pasos de escena, `StepUndone` y `ApplyScene`) sube a la v23.
    /// T-103 (túneles: el canal `Tunnel`, el saludo de los clientes, sus pedidos y el aviso a
    /// la UI) sube a la v25. T-093 (marcas de tiempo y latidos con eco en la entrada,
    /// arbitraje del teclado sin dueño y latencia en el estado de la entrada) sube a la v26:
    /// iba a ser la v24, pero entró después de T-103. T-102 (métricas de red, el canal de la
    /// prueba de ancho de banda) sube a la v28.
    /// T-111 (`[[cables]]` en el puesto y su parche, la entrada de cada máquina según el EDID
    /// en el canal y la vista de cada monitor) sube a la v29. T-128 (el cursor en vivo:
    /// `WatchCursor`, `Cursor` y `Crossed` en la UI y entre agentes) sube a la v30. T-106
    /// (archivos y puertos por túnel, sus pedidos y el avance de las transferencias) sube a
    /// la v31. T-104 (procesos por túnel: `Exec`, `Kill`, `Output`, `GetProcesses` y los
    /// procesos en la vista de túneles) sube a la v32: iba a ser la v29, pero T-111, T-128 y
    /// T-106 entraron antes. T-107 sube a la v33. T-094 (`ControlMsg::LinkClass`, la
    /// clasificación de cada conexión por su interfaz) sube a la v34: iba a ser la v33, pero
    /// T-107 entró antes.
    #[test]
    fn la_version_queda_por_encima_de_la_de_t084() {
        assert_eq!(PROTOCOL_VERSION, 34);
        assert_eq!(super::CAPABILITIES_SINCE, 10);
        assert_eq!(super::SIDES_SINCE, 16);
    }
}
