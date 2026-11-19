//! Connection to the IPC of an agent that is already running (Unix: socket in the data
//! directory; Windows: the user's named pipe).

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

/// Connects as `peer`, waiting up to `wait` for the agent to bring up its socket. If it
/// never arrives, the error gives the last reason.
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
            anyhow::bail!("the agent does not answer on its IPC: {last}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Whoever opens tunnels: a terminal client, not the UI.
pub fn client() -> Peer {
    Peer::Client(ClientKind::Cli)
}
