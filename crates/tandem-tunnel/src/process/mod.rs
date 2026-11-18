//! Procesos dentro de un túnel (T-104, DESIGN §8).
//!
//! `exec` lanza un proceso en la destino con el `Spawner` del agente; su stdout y su stderr
//! son dos flujos del túnel (con el búfer circular y la posición por byte de siempre, así que
//! sobreviven a las caídas del enlace) y el proceso se anota en la tabla del túnel con su
//! estado, código de salida y duración. Cerrar el túnel mata todos los árboles.
//!
//! - [`launch`]: de un `exec` a la orden concreta (shells, WSL, Docker). Portable.
//! - [`Spawner`] / [`Child`]: lo que depende del sistema. [`system::SystemSpawner`] es el
//!   real (grupo de procesos en Unix, objeto de trabajo en Windows); [`fake::FakeSpawner`],
//!   el doble de prueba.
//! - [`output`]: `output { desde }` armado desde los flujos.
//!
//! **Privilegios (A7):** el proceso hereda los del agente; nada aquí los cambia.

pub mod fake;
pub mod launch;
pub mod output;
pub mod system;
pub(crate) mod table;

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::host::{FlowWriter, HostTunnel};
use crate::wire::BoxFuture;

pub use launch::{Launch, LaunchError, Os};

/// El proceso lanzado, para esperarlo y matarlo.
pub trait Child: Send + Sync {
    /// Resuelve cuando el proceso principal terminó, con su código (`None` si lo terminó una
    /// señal). Se llama una vez.
    fn wait(&self) -> BoxFuture<'_, Option<i32>>;
    /// Mata el proceso y todos sus descendientes. Tiene que volver enseguida y se puede
    /// llamar varias veces.
    fn kill_tree(&self);
    /// Ya no queda nada del árbol (el proceso terminó y sus salidas se cerraron): matarlo
    /// después no hace nada, para no tocar un identificador que el sistema ya reutilizó.
    fn release(&self) {}
}

/// Un proceso recién lanzado con sus salidas. Su stdin va cerrado: no hay entrada
/// interactiva.
pub struct Spawned {
    /// El identificador del sistema, si lo dio.
    pub pid: Option<u32>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
    pub child: Arc<dyn Child>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SpawnError(pub String);

/// Lanza procesos en esta máquina. El real es [`system::SystemSpawner`].
pub trait Spawner: Send + Sync {
    fn spawn(&self, launch: &Launch) -> Result<Spawned, SpawnError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("no hay ningún túnel {0} en esta máquina")]
    UnknownTunnel(tandem_proto::TunnelId),
    #[error("el túnel {0} no tiene el proceso {1}")]
    UnknownProcess(tandem_proto::TunnelId, tandem_proto::ProcessId),
    #[error("el túnel {0} está cerrado")]
    Closed(tandem_proto::TunnelId),
    #[error("{0}")]
    Launch(#[from] LaunchError),
    #[error("no se pudo lanzar: {0}")]
    Spawn(#[from] SpawnError),
}

/// Copia lo que sale por `from` al flujo hasta que se cierre. Al terminar suelta el flujo,
/// que queda terminado.
async fn pump(mut from: Box<dyn AsyncRead + Send + Unpin>, to: FlowWriter) {
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => to.write(&buf[..n]),
        }
    }
}

/// Vigila el proceso: copia sus salidas a los flujos y anota cuando termina. Lo que
/// quede vivo del árbol sigue escribiendo mientras tenga la salida abierta.
pub(crate) fn supervise(
    tunnel: HostTunnel,
    id: tandem_proto::ProcessId,
    spawned: Spawned,
    out: FlowWriter,
    err: FlowWriter,
) {
    let Spawned {
        stdout,
        stderr,
        child,
        ..
    } = spawned;
    let pumps = (
        tokio::spawn(pump(stdout, out)),
        tokio::spawn(pump(stderr, err)),
    );
    tokio::spawn(async move {
        let code = child.wait().await;
        tunnel.process_exited(id, code);
        let _ = pumps.0.await;
        let _ = pumps.1.await;
        child.release();
    });
}
