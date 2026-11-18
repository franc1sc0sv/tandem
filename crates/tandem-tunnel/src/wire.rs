//! Por dónde van los mensajes de túnel entre los dos agentes.
//!
//! En el agente es un stream QUIC propio por pedido (`Channel::Tunnel`, en `tandemd`); en
//! las pruebas, los caminos en memoria de [`crate::mem`], que se pueden cortar.

use std::future::Future;
use std::pin::Pin;

use tandem_proto::{TunnelDown, TunnelUp};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// El stream se rompió o el otro lado no se pudo alcanzar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WireError(pub String);

impl WireError {
    pub fn new(e: impl std::fmt::Display) -> Self {
        WireError(e.to_string())
    }
}

/// El lado del origen de un stream de túnel.
pub trait OriginWire: Send {
    fn send(&mut self, msg: TunnelUp) -> BoxFuture<'_, Result<(), WireError>>;
    /// `None` si la destino cerró el stream.
    fn recv(&mut self) -> BoxFuture<'_, Result<Option<TunnelDown>, WireError>>;
    /// Desde aquí este stream lleva bytes en volumen en el sentido del origen (archivos,
    /// puertos): prioridad de datos, con la pausa y el tope del camino (T-101).
    fn start_data(&mut self) -> Result<(), WireError> {
        Ok(())
    }
}

/// Abre streams de túnel hacia la destino por un camino (una conexión). El origen usa
/// siempre el del enlace activo ([`crate::Origin::set_link`]).
pub trait Dialer: Send + Sync {
    fn dial(&self) -> BoxFuture<'_, Result<Box<dyn OriginWire>, WireError>>;
}

/// El lado de la destino de un stream de túnel.
pub trait TargetWire: Send {
    /// `None` si el origen cerró el stream.
    fn recv(&mut self) -> impl Future<Output = Result<Option<TunnelUp>, WireError>> + Send;
    fn send(&mut self, msg: &TunnelDown) -> impl Future<Output = Result<(), WireError>> + Send;
    /// Desde aquí el stream lleva la salida del túnel: prioridad de datos, con la pausa y
    /// el tope del camino (T-101). Antes, los pedidos van como control.
    fn start_data(&mut self) -> Result<(), WireError> {
        Ok(())
    }
}
