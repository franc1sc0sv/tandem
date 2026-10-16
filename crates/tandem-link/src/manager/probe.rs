use std::net::SocketAddr;

/// Cómo se habla con el otro agente por un enlace: abrir una sesión y medir
/// su salud con un ping. El gestor solo conoce este trait; la implementación
/// real es [`QuicProber`](super::QuicProber) y las pruebas usan un doble.
pub trait Prober: Send + Sync + 'static {
    /// Conexión viva por un enlace.
    type Session: Send + 'static;
    /// Lo que el gestor entrega de la conexión activa (por ejemplo, la
    /// [`Connection`](crate::transport::Connection) de QUIC).
    type Handle: Clone + Send + Sync + 'static;
    type Error: std::error::Error + Send + 'static;

    fn connect(
        &self,
        addr: SocketAddr,
    ) -> impl Future<Output = Result<Self::Session, Self::Error>> + Send;

    /// Un ping de ida y vuelta. El gestor le pone el tiempo máximo y, si
    /// falla o vence, descarta la sesión y vuelve a conectar.
    fn ping(
        &self,
        session: &mut Self::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn handle(session: &Self::Session) -> Self::Handle;
}
