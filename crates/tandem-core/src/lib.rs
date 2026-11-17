//! Núcleo de Tandem, independiente de la plataforma.
//!
//! - [`model`]: los dos lados (`A`/`B`, por clave pública), qué máquina es cada uno y el
//!   puesto compartido ([`Desk`]).
//! - [`desk`]: cómo se edita (parches), se sincroniza (gana el último) y se deshace el
//!   puesto.
//! - [`state`]: el estado observado (qué máquinas están encendidas).
//! - [`plan`]: encender y apagar como una lista ordenada de pasos.
//! - [`txn`]: ejecución transaccional de un plan, con vuelta atrás.
//! - [`monitor`]: las entradas de un monitor compatible.

pub mod desk;
pub mod model;
pub mod monitor;
pub mod plan;
pub mod state;
pub mod txn;

pub use desk::{
    DeskHistory, DeskPatch, DeskStamp, DeskVersion, HISTORY_LEN, HistoryEntry, InputPrefs, Modules,
    PatchError, Scene, Shortcuts, UndoError, valid_color,
};
pub use model::{
    Align, Arrangement, Config, DEFAULT_COLORS, Desk, Machine, MachineInfo, Machines, Os, PALETTE,
    Scale, ScreenSpec, Side, default_color,
};
pub use monitor::MonitorInput;
pub use plan::{Abilities, Plan, Step};
pub use state::{Powered, State};
pub use txn::{Executor, TxnError, TxnReport, run};
