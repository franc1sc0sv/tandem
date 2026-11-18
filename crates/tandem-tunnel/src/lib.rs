//! Túneles: Claude (o una persona) usa la otra máquina (DESIGN §8, DECISIONS D8, D9, A7).
//!
//! Un túnel lo abre un cliente local por la IPC en la máquina **origen** y vive en la
//! **destino**, con sus flujos de salida y sus recursos. Portable: aquí no hay código de
//! plataforma salvo el lanzador real de procesos (T-104, [`process::system`], detrás del
//! trait [`process::Spawner`]); los archivos y puertos (T-106) y workspaces de git (T-107)
//! se cuelgan de un [`HostTunnel`] como [`FlowWriter`] y [`Resource`].
//!
//! - [`host`]: la destino. [`Host`] registra los túneles; [`serve`] atiende cada stream que
//!   abre el origen (un pedido, o el canal de datos de un túnel).
//! - [`origin`]: el origen. [`Origin`] abre, cierra y lista, y sigue la salida de cada
//!   túnel por el enlace activo, reanudando «desde el byte N» tras un corte.
//! - [`process`]: procesos dentro de un túnel (`exec`, `output`, `kill`).
//! - [`buffer`]: el búfer circular con posición absoluta por byte que usan los dos.
//! - [`wire`]: los traits del transporte (QUIC en `tandemd`, memoria en [`mem`]).
//!
//! **Sobrevivir al enlace:** los túneles no dependen de ningún stream. Si el enlace se cae,
//! lo de la destino sigue corriendo y su salida se guarda (hasta [`DEFAULT_CAPACITY`] por
//! flujo; si se llena, se descarta lo más viejo y el origen recibe el aviso
//! [`tandem_proto::TunnelDown::Lost`]). Al volver, por el mismo camino o por otro, el
//! origen retoma desde el último byte que tiene.
//!
//! **Privilegios (A7):** lo que corre en un túnel tiene los del agente destino.

pub mod buffer;
pub mod host;
pub mod mem;
pub mod origin;
pub mod process;
pub mod wire;

use std::sync::Arc;

use tandem_proto::{
    OutputCursor, ProcessId, ProcessOutput, ProjectTunnels, TunnelId, TunnelProcess, TunnelState,
    TunnelView, group_by_project,
};
use tokio::sync::watch;

pub use buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
pub use host::{
    FlowWriter, Host, HostOptions, HostTunnel, Resource, TunnelClosed, serve, serve_request,
};
pub use origin::{FlowStatus, Link, OpenSpec, Origin, OriginOptions, TunnelError};
pub use process::{ExecError, Spawner};
pub use wire::{BoxFuture, Dialer, OriginWire, TargetWire, WireError};

/// Avisa que cambió algún túnel (abrir, cerrar). Se clona barato; todos los clones son el
/// mismo aviso.
#[derive(Clone)]
pub struct Changes(Arc<watch::Sender<u64>>);

impl Default for Changes {
    fn default() -> Self {
        Changes(Arc::new(watch::Sender::new(0)))
    }
}

impl Changes {
    pub fn bump(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}

/// Los túneles de un agente: los que hospeda (destino) y los que abrió (origen).
#[derive(Clone)]
pub struct Tunnels {
    pub host: Host,
    pub origin: Origin,
    changes: Changes,
}

impl std::fmt::Debug for Tunnels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnels").finish_non_exhaustive()
    }
}

impl Default for Tunnels {
    fn default() -> Self {
        Self::new(HostOptions::default(), OriginOptions::default())
    }
}

impl Tunnels {
    pub fn new(host: HostOptions, origin: OriginOptions) -> Self {
        let changes = Changes::default();
        Tunnels {
            host: Host::new(host, changes.clone()),
            origin: Origin::new(origin, changes.clone()),
            changes,
        }
    }

    /// Los túneles abiertos de las dos máquinas tal como los ve este agente (los que abrió
    /// y los que hospeda), agrupados por proyecto: lo que muestra la UI.
    pub fn view(&self) -> Vec<ProjectTunnels> {
        let mut all: Vec<TunnelView> = self.origin.views();
        for t in self.host.views() {
            if !all.iter().any(|o| o.tunnel.id == t.tunnel.id) {
                all.push(t);
            }
        }
        all.retain(|t| t.tunnel.state == TunnelState::Open);
        group_by_project(all)
    }

    /// Los procesos de un túnel de cualquiera de las dos máquinas, también los que
    /// terminaron hace poco. `None` si no se conoce el túnel.
    pub fn processes(&self, tunnel: TunnelId) -> Option<Vec<TunnelProcess>> {
        self.origin
            .processes(tunnel)
            .or_else(|| self.host.get(tunnel).map(|t| t.processes()))
    }

    /// La salida de un proceso desde `from`: del espejo si el túnel lo abrió esta máquina, y
    /// si no, de sus propios flujos.
    pub fn output(
        &self,
        tunnel: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, TunnelError> {
        match self.origin.output(tunnel, process, from, max) {
            Err(TunnelError::Unknown(_)) => {}
            other => return other,
        }
        let t = self.host.get(tunnel).ok_or(TunnelError::Unknown(tunnel))?;
        t.output(process, from, max)
            .map_err(|_| TunnelError::UnknownProcess(tunnel, process))
    }

    /// Sube con cada túnel que se abre o se cierra, aquí o en la otra máquina.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
}
