//! The path tunnel messages take between the two agents.
//!
//! On the agent it is a dedicated QUIC stream per request (`Channel::Tunnel`, in `tandemd`); in
//! tests, the in-memory paths of [`crate::mem`], which can be cut.

use std::future::Future;
use std::pin::Pin;

use tandem_proto::{TunnelDown, TunnelUp};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The stream broke or the other side could not be reached.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WireError(pub String);

impl WireError {
    pub fn new(e: impl std::fmt::Display) -> Self {
        WireError(e.to_string())
    }
}

/// The origin side of a tunnel stream.
pub trait OriginWire: Send {
    fn send(&mut self, msg: TunnelUp) -> BoxFuture<'_, Result<(), WireError>>;
    /// `None` if the destination closed the stream.
    fn recv(&mut self) -> BoxFuture<'_, Result<Option<TunnelDown>, WireError>>;
    /// From here this stream carries bulk bytes in the origin's direction (files,
    /// ports): data priority, with the pause and the cap of the path (T-101).
    fn start_data(&mut self) -> Result<(), WireError> {
        Ok(())
    }
}

/// Opens tunnel streams toward the destination over a path (a connection). The origin always
/// uses that of the active link ([`crate::Origin::set_link`]).
pub trait Dialer: Send + Sync {
    fn dial(&self) -> BoxFuture<'_, Result<Box<dyn OriginWire>, WireError>>;
}

/// The destination side of a tunnel stream.
pub trait TargetWire: Send {
    /// `None` if the origin closed the stream.
    fn recv(&mut self) -> impl Future<Output = Result<Option<TunnelUp>, WireError>> + Send;
    fn send(&mut self, msg: &TunnelDown) -> impl Future<Output = Result<(), WireError>> + Send;
    /// From here the stream carries the tunnel's output: data priority, with the pause and
    /// the cap of the path (T-101). Before that, requests go as control.
    fn start_data(&mut self) -> Result<(), WireError> {
        Ok(())
    }
}
