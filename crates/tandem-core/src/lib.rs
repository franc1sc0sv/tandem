//! Núcleo de Tandem, independiente de la plataforma.
//!
//! - [`model`]: las máquinas y el puesto compartido (el acomodo de pantallas declarado).
//! - [`state`]: el estado observado (qué máquinas están encendidas).
//! - [`plan`]: encender y apagar como una lista ordenada de pasos.
//! - [`txn`]: ejecución transaccional de un plan.

pub mod model;
pub mod plan;
pub mod state;
pub mod txn;

pub use model::{Arrangement, Config, Machine, Scale, ScreenSpec, Side};
pub use plan::{Abilities, Plan, Step};
pub use state::{Powered, State};
pub use txn::{Executor, TxnError, TxnReport, run};
