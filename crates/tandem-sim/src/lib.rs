//! Simulación del puesto, solo para pruebas (docs/TESTING.md).
//!
//! Dos `tandemd` reales en el mismo proceso, cada uno con su orquestador, su gestor
//! de enlaces y el protocolo de control real, unidos por una red en memoria con un
//! enlace por cable (USB4, Ethernet, Wi-Fi) que se puede cortar. Los backends de
//! plataforma se reemplazan por un puesto físico simulado:
//!
//! - [`world`]: la energía de cada máquina y el hub USB.
//! - [`steps`]: los pasos de cada máquina sobre ese puesto.
//! - [`net`]: los enlaces y el `PeerLink` que sigue al enlace activo.
//! - [`fault`]: [`FaultPlan`] para fallar, colgar o tardar un paso y cortar enlaces.
//! - [`desk`]: [`Desk`], que arma todo y comprueba el puesto contra el estado.
//! - [`monitors`]: los dobles de `monitor-ddc` conectados a las dos máquinas (T-110).
//! - [`wifi`]: [`SimWifi`], un Wi-Fi con ancho de banda y cola, para la prioridad de la
//!   entrada sobre los datos (T-101).
//!
//! - [`input`]: [`InputPair`], dos agentes con el motor de entrada por QUIC real en
//!   loopback, para el banco de latencia de la entrada (T-093). Ahí el reloj es el real.
//!
//! Salvo [`input`], todo corre con el reloj de tokio pausado: los tiempos (arranque de la
//! PC, pings, timeouts) son exactos y no hay esperas reales. La geometría y las sesiones
//! del motor de entrada se prueban además en `tandemd/tests/input.rs`.

pub mod desk;
pub mod fault;
pub mod input;
pub mod monitors;
pub mod net;
pub mod scenes;
pub mod steps;
pub mod wifi;
pub mod world;

pub use desk::{Desk, DeskBuilder, Outcome, SimError, TxnRun};
pub use fault::{FaultAction, FaultPlan};
pub use input::{InputPair, SimAgent};
pub use monitors::{MachineBus, MonitorSpec, SimMonitors};
pub use scenes::SceneScreens;
pub use steps::HardwareTimes;
pub use wifi::{SimWifi, WifiWriter};
pub use world::{Physical, Power, World};
