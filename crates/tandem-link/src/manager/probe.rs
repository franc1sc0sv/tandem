use std::net::SocketAddr;

/// How to talk to the other agent over a link: open a session and measure
/// its health with a ping. The manager only knows this trait; the real
/// implementation is [`QuicProber`](super::QuicProber) and tests use a double.
pub trait Prober: Send + Sync + 'static {
    /// Live connection over a link.
    type Session: Send + 'static;
    /// What the manager hands over of the active connection (for example, QUIC's
    /// [`Connection`](crate::transport::Connection)).
    type Handle: Clone + Send + Sync + 'static;
    type Error: std::error::Error + Send + 'static;

    fn connect(
        &self,
        addr: SocketAddr,
    ) -> impl Future<Output = Result<Self::Session, Self::Error>> + Send;

    /// A round-trip ping. The manager puts the maximum time on it and, if
    /// it fails or expires, discards the session and connects again.
    fn ping(
        &self,
        session: &mut Self::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn handle(session: &Self::Session) -> Self::Handle;
}
