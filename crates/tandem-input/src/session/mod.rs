//! Sesión de entrada sobre el enlace (DESIGN §7, docs/research/input-engine.md).
//!
//! El **servidor** es la máquina con el teclado y el mouse: captura, decide con la
//! geometría cuándo el cursor cruza y manda la entrada al **cliente**, que la
//! inyecta. El movimiento y el scroll van en datagramas coalescidos con número de
//! secuencia y resincronización absoluta; teclas, botones, entrar y salir van por
//! un stream confiable y ordenado ([`msg`]).
//!
//! - [`ServerSession`] y [`ClientSession`]: máquinas de estados sin E/S, que se
//!   prueban evento por evento.
//! - [`Capture`] e [`Injector`]: lo que cada plataforma implementa (T-022 a T-025);
//!   [`fake`] tiene los dobles de prueba.
//! - [`ServerDriver`] y [`ClientDriver`]: las conectan con una conexión QUIC de
//!   `tandem-link`, con latido de 1 s. Cuando el enlace cae o cambia, el driver
//!   termina: el servidor devuelve el foco a su máquina y el cliente suelta todo
//!   lo que tenía presionado.
//!
//! Las posiciones son coordenadas nativas del cursor de cada máquina (las de sus
//! [`Screen`](crate::geometry::Screen)); los deltas que viajan, puntos lógicos.

mod client;
pub mod fake;
pub mod msg;
#[cfg(test)]
mod props;
mod quic;
mod server;
#[cfg(test)]
mod testutil;

use std::fmt::Display;
use std::time::Duration;

pub use client::ClientSession;
pub use msg::{Datagram, MotionDatagram, StreamMsg};
pub use quic::{ClientDriver, ServerDriver, SessionEnd};
pub use server::{ServerOutput, ServerSession};

use crate::event::InputEvent;
use crate::geometry::Point;

/// Control de la captura del servidor. Los eventos capturados no pasan por aquí:
/// la captura corre en su propio hilo (CGEventTap, hooks de Windows) y los manda
/// por un canal al driver.
///
/// Los movimientos que captura son `MotionRelative` en unidades nativas locales,
/// sin aceleración; puede mandar `MotionAbsolute` con la posición real del cursor
/// local para corregir el seguimiento mientras el foco es local.
pub trait Capture: Send {
    type Error: Display;

    /// `true`: tragar la entrada local y congelar y ocultar el cursor, porque el
    /// foco está en la otra máquina. `false`: devolverla.
    fn set_grab(&mut self, grab: bool) -> Result<(), Self::Error>;

    /// Poner el cursor local en `pos` (al volver de la otra máquina).
    fn warp(&mut self, pos: Point) -> Result<(), Self::Error>;
}

/// Inyección en el cliente. Recibe teclas y botones, `MotionAbsolute` en
/// coordenadas nativas locales y scroll.
pub trait Injector: Send {
    type Error: Display;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error>;
}

/// Tiempos de la sesión.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    /// Cada cuánto manda cada lado un latido (DESIGN §6 y research: ~1 s).
    pub heartbeat_interval: Duration,
    /// Silencio del otro lado que cuenta como enlace caído. Es el último recurso:
    /// normalmente el gestor de enlaces avisa antes y el driver se detiene.
    pub heartbeat_timeout: Duration,
    /// Cuánto tiene que estar quieto el mouse para mandar la posición absoluta.
    pub resync_idle: Duration,
    /// Cada cuántos datagramas va uno con la posición absoluta aunque el mouse
    /// no pare.
    pub resync_every: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_secs(3),
            resync_idle: Duration::from_millis(50),
            resync_every: 16,
        }
    }
}
