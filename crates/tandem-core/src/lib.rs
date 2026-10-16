//! Núcleo de Tandem, independiente de la plataforma.
//!
//! - [`model`]: máquinas, monitores, entradas y la configuración del escritorio.
//! - [`state`]: el estado observado (quién muestra qué) y el modo de la pantalla integrada.
//! - [`plan`]: cómo pasar de un estado a otro como una lista ordenada de pasos.
//! - [`txn`]: ejecución transaccional de un plan, con vuelta atrás si un paso falla.

pub mod model;
pub mod plan;
pub mod state;
pub mod txn;

pub use model::{Config, DdcProfile, Input, Machine, MonitorConfig, MonitorId};
pub use plan::{Abilities, Mode, Plan, Step};
pub use state::{BuiltinDisplay, Powered, State};
pub use txn::{Executor, TxnError, TxnReport, run};
