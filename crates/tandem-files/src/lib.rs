//! Archivos y puertos por túnel (DESIGN §8, T-106), y base de los arrastres (§9, T-121).
//!
//! - [`path`]: rutas normalizadas entre Windows y Unix.
//! - [`source`] / [`sink`]: leer un árbol con hashes BLAKE3 por fragmento (1 MiB) y del total;
//!   escribirlo verificando, con reanudación desde disco, permisos y fechas conservados.
//! - [`transfer`]: `put` y `get` sobre un stream de túnel, con progreso y reintento.
//! - [`ports`]: reenvío de puertos TCP en las dos direcciones.
//! - [`Files`]: lo que cuelga un agente (el lado destino y los reenvíos del origen).
//! - [`drag`]: arrastrar archivos al cruzar el borde (D14), con el sistema detrás de traits.
//! - [`mem`]: caminos en memoria con cortes, para probar con dos agentes en un proceso.
//!
//! Los fragmentos y los pedidos van por `Channel::Tunnel` (T-103), así que heredan la
//! prioridad baja y el tope de ancho de banda de los datos (T-101).

pub mod drag;
#[cfg(target_os = "macos")]
pub mod drag_macos;
pub mod drag_os;
#[cfg(windows)]
pub mod drag_windows;
pub mod error;
pub mod mem;
pub mod path;
pub mod ports;
pub mod sink;
pub mod source;
pub mod transfer;

use std::path::Path;
use std::sync::Arc;

use tandem_proto::{FilesReport, ForwardInfo, TransferProgress, TunnelId, TunnelUp};
use tandem_tunnel::{Host, Origin, TargetWire, WireError};
use tokio::sync::broadcast;

pub use error::FilesError;
pub use ports::{ForwardSpec, Forwards, PortHost};
pub use sink::Sink;
pub use source::Source;
pub use transfer::Options;

/// Archivos y puertos de un agente: atiende los pedidos de la otra máquina (destino) y
/// lleva las transferencias y reenvíos que abrieron los clientes locales (origen).
#[derive(Clone)]
pub struct Files {
    ports: PortHost,
    forwards: Forwards,
    progress: broadcast::Sender<TransferProgress>,
    opts: Arc<Options>,
}

impl Default for Files {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files").finish_non_exhaustive()
    }
}

impl Files {
    pub fn new(opts: Options) -> Self {
        Files {
            ports: PortHost::default(),
            forwards: Forwards::default(),
            progress: broadcast::channel(64).0,
            opts: Arc::new(opts),
        }
    }

    /// El avance de las transferencias que arrancó esta máquina.
    pub fn subscribe_progress(&self) -> broadcast::Receiver<TransferProgress> {
        self.progress.subscribe()
    }

    pub fn forwards(&self) -> &Forwards {
        &self.forwards
    }

    /// Atiende un stream de túnel que abrió la otra máquina: archivos y puertos aquí, lo
    /// demás (abrir, cerrar, canal de datos) lo resuelve `tandem-tunnel`.
    pub async fn serve<W: TargetWire>(&self, host: &Host, mut wire: W) -> Result<(), WireError> {
        let Some(req) = wire.recv().await? else {
            return Ok(());
        };
        self.serve_request(host, wire, req).await
    }

    /// Como [`Files::serve`] con el primer pedido ya leído: para quien reparte pedidos de
    /// otros módulos (workspaces) y deja los de archivos y puertos aquí.
    pub async fn serve_request<W: TargetWire>(
        &self,
        host: &Host,
        mut wire: W,
        req: TunnelUp,
    ) -> Result<(), WireError> {
        match req {
            TunnelUp::Files(f) => transfer::serve(host, &mut wire, f, self.opts.chunk_size).await,
            TunnelUp::Port(p) => ports::serve(host, &self.ports, &mut wire, p).await,
            other => tandem_tunnel::serve_request(host, wire, other).await,
        }
    }

    /// Copia `local` a `remote` en la destino del túnel.
    pub async fn put(
        &self,
        origin: &Origin,
        tunnel: TunnelId,
        local: &Path,
        remote: &str,
    ) -> Result<FilesReport, FilesError> {
        let sink = |p| {
            let _ = self.progress.send(p);
        };
        transfer::put(origin, tunnel, local, remote, &self.opts, &sink).await
    }

    /// Trae `remote` de la destino del túnel a `local`.
    pub async fn get(
        &self,
        origin: &Origin,
        tunnel: TunnelId,
        remote: &str,
        local: &Path,
    ) -> Result<FilesReport, FilesError> {
        let sink = |p| {
            let _ = self.progress.send(p);
        };
        transfer::get(origin, tunnel, remote, local, &self.opts, &sink).await
    }

    /// Reenvía un puerto por el túnel.
    pub async fn forward(
        &self,
        origin: &Origin,
        spec: ForwardSpec,
    ) -> Result<ForwardInfo, FilesError> {
        self.forwards.start(origin, spec).await
    }
}
