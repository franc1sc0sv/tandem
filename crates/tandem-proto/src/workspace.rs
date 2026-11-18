//! Workspaces de git por túnel (DESIGN §8, D8, T-107): los mensajes.
//!
//! La ida y la vuelta de los datos (bundles y parches) viajan como archivos
//! ([`crate::FilesUp`]); estos pedidos solo preparan carpetas y piden a la destino que
//! ejecute git. La lógica está en `tandem-workspace`.

use serde::{Deserialize, Serialize};

use crate::TunnelId;

/// El repo que acompaña a un túnel (`tunnel_open { workspace }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    /// Ruta del repo en la máquina que abre (puede ser un worktree o una subcarpeta).
    pub repo_path: String,
    /// Conservar la carpeta de la destino al cerrar el túnel.
    pub keep: bool,
    /// Credenciales sin seguimiento que sí viajan, una por una (rutas del repo). Por
    /// omisión nada que parezca una se manda (T-130).
    pub include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: String,
    pub summary: String,
    pub author: String,
}

/// Lo que dejó `bring_back`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BringBackReport {
    /// La rama creada en el origen; `None` si no había commits nuevos.
    pub branch: Option<String>,
    pub commits: Vec<CommitInfo>,
    /// Se hizo un commit con lo pendiente de la destino.
    pub committed_pending: bool,
}

/// Del origen a la destino.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceUp {
    /// Crear las carpetas del workspace del túnel. Contesta [`WorkspaceDown::Prepared`].
    Prepare { tunnel: TunnelId, keep: bool },
    /// Con el bundle y el parche ya copiados a `stage`: clonar, aplicar y dejar la carpeta
    /// como `cwd` de los procesos. Contesta [`WorkspaceDown::Ready`].
    Checkout { tunnel: TunnelId },
    /// Empaquetar lo nuevo: `commit` (un mensaje) hace commit de lo pendiente antes.
    /// Contesta [`WorkspaceDown::Packed`].
    Pack {
        tunnel: TunnelId,
        commit: Option<String>,
    },
}

/// De la destino al origen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceDown {
    Prepared {
        /// Carpeta donde el origen deja `repo.bundle`, `changes.patch` y `submodules/`.
        stage: String,
    },
    Ready {
        dir: String,
    },
    Packed {
        /// Archivo (en la destino) con los commits nuevos; `None` si no hay.
        bundle: Option<String>,
        /// El commit del que parte (el `HEAD` del origen al abrir).
        base: String,
        commits: Vec<CommitInfo>,
        committed_pending: bool,
    },
    Failed(String),
}
