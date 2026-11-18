//! Conexión a la IPC de un agente que ya corre (Unix: socket en el directorio de datos;
//! Windows: tubería con nombre del usuario).

use std::path::Path;
use std::time::{Duration, Instant};

use tandem_proto::{ClientKind, Peer};
use tandemd::ipc::IpcClient;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub type Conn = IpcClient<Box<dyn Io>>;

async fn open(data_dir: &Path) -> anyhow::Result<Box<dyn Io>> {
    let endpoint = tandemd::ipc::default_endpoint(data_dir)?;
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(&endpoint).await?))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        Ok(Box::new(ClientOptions::new().open(&endpoint)?))
    }
}

/// Conecta como `peer`, esperando hasta `wait` a que el agente levante su socket. Si no
/// llega, el error dice el último motivo.
pub async fn connect(data_dir: &Path, peer: Peer, wait: Duration) -> anyhow::Result<Conn> {
    let start = Instant::now();
    loop {
        let last = match open(data_dir).await {
            Ok(io) => match IpcClient::handshake_as(io, peer).await {
                Ok(c) => return Ok(c),
                Err(e) => anyhow::Error::from(e),
            },
            Err(e) => e,
        };
        if start.elapsed() >= wait {
            anyhow::bail!("el agente no contesta en su IPC: {last}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Quien abre túneles: un cliente de terminal, no la UI.
pub fn client() -> Peer {
    Peer::Client(ClientKind::Cli)
}
