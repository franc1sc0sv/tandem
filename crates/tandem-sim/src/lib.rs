//! Simulación del puesto, solo para pruebas (docs/TESTING.md).
//!
//! Dos `tandemd` reales en el mismo proceso, cada uno con su orquestador, su gestor
//! de enlaces y el protocolo de control real, unidos por una red en memoria con un
//! enlace por cable (USB4, Ethernet, Wi-Fi) que se puede cortar. Los backends de
//! plataforma se reemplazan por un puesto físico simulado:
//!
//! - [`world`]: lo que de verdad muestran los monitores (con el 34" que siempre lee
//!   `0x09`), los escritorios, la pantalla integrada, la energía de la PC, la geometría
//!   de cada motor de entrada y el hub USB.
//! - [`steps`]: los pasos de cada máquina sobre ese puesto.
//! - [`net`]: los enlaces y el `PeerLink` que sigue al enlace activo.
//! - [`fault`]: [`FaultPlan`] para fallar, colgar o tardar un paso y cortar enlaces.
//! - [`desk`]: [`Desk`], que arma todo y comprueba el puesto contra el estado.
//!
//! Todo corre con el reloj de tokio pausado: los tiempos (arranque de la PC, pings,
//! timeouts) son exactos y no hay esperas reales.

pub mod desk;
pub mod fault;
pub mod net;
pub mod steps;
pub mod world;

pub use desk::{Desk, DeskBuilder, Outcome, SimError, TxnRun};
pub use fault::{FaultAction, FaultPlan};
pub use steps::HardwareTimes;
pub use world::{Physical, Power, Screen, World};
