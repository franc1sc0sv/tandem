//! Núcleo de Tandem, independiente de la plataforma.
//!
//! - [`model`]: los dos lados (`A`/`B`, por clave pública), qué máquina es cada uno y el
//!   puesto compartido (el acomodo de pantallas declarado).
//! - [`state`]: el estado observado (qué máquinas están encendidas).
//! - [`plan`]: encender y apagar como una lista ordenada de pasos.
//! - [`txn`]: ejecución transaccional de un plan.

pub mod model;
pub mod plan;
pub mod state;
pub mod txn;

pub use model::{
    Arrangement, Config, DEFAULT_COLORS, Machine, MachineInfo, Machines, Os, Scale, ScreenSpec,
    Side, default_color,
};
pub use plan::{Abilities, Plan, Step};
pub use state::{Powered, State};
pub use txn::{Executor, TxnError, TxnReport, run};
