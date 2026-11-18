//! Workspaces de git por túnel (DESIGN §8, D8, T-107).
//!
//! `tunnel_open { workspace }`: el origen empaqueta el repo (bundle del `HEAD` más un
//! parche con lo pendiente), lo copia a la destino con `tandem-files` (reanudable) y la
//! destino arma `<datos>/workspaces/<túnel>`, que pasa a ser el `cwd` de los procesos del
//! túnel. `bring_back`: lo nuevo vuelve como una rama `tandem/<nombre>` del repo de
//! origen, sin tocar su rama actual ni su árbol de trabajo.
//!
//! Todo lo que toca el sistema va por [`git::Git`]; los pedidos viajan como
//! [`tandem_proto::WorkspaceUp`] por `Channel::Tunnel`, igual que archivos y puertos.

pub mod error;
pub mod git;
pub mod repo;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tandem_files::Files;
use tandem_proto::{
    BringBackReport, Tunnel, TunnelDown, TunnelId, TunnelUp, WorkspaceDown, WorkspaceSpec,
    WorkspaceUp,
};
use tandem_tunnel::{Host, Origin, Resource, TargetWire, WireError};

pub use error::WorkspaceError;
pub use git::{Git, SystemGit};

type Result<T> = std::result::Result<T, WorkspaceError>;

/// Los workspaces de un agente, en sus dos papeles.
#[derive(Clone)]
pub struct Workspaces {
    inner: Arc<Inner>,
}

struct Inner {
    git: Arc<dyn Git>,
    /// `<datos>/workspaces`.
    root: PathBuf,
    /// Destino: los workspaces de los túneles que abrió la otra máquina.
    target: Mutex<HashMap<TunnelId, Dirs>>,
    /// Origen: el repo de cada túnel que abrió esta máquina con workspace.
    origin: Mutex<HashMap<TunnelId, OriginEntry>>,
}

#[derive(Clone)]
struct Dirs {
    work: PathBuf,
    stage: PathBuf,
}

#[derive(Clone)]
struct OriginEntry {
    root: PathBuf,
    title: String,
}

impl std::fmt::Debug for Workspaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspaces").finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| WorkspaceError::Join)?
}

/// Pide algo a la destino por un stream propio.
async fn ask(origin: &Origin, req: WorkspaceUp) -> Result<WorkspaceDown> {
    let mut wire = origin.dial().await?;
    wire.send(TunnelUp::Workspace(req)).await?;
    match wire.recv().await? {
        Some(TunnelDown::Workspace(WorkspaceDown::Failed(why)))
        | Some(TunnelDown::Refused(why)) => Err(WorkspaceError::Refused(why)),
        Some(TunnelDown::Workspace(down)) => Ok(down),
        _ => Err(WorkspaceError::Unexpected),
    }
}

/// Al cerrarse el túnel, quita las carpetas (salvo que se pidiera conservarlas).
struct Cleanup {
    inner: Arc<Inner>,
    tunnel: TunnelId,
    keep: bool,
}

impl Resource for Cleanup {
    fn close(&self) {
        let dirs = lock(&self.inner.target).remove(&self.tunnel);
        if let (false, Some(d)) = (self.keep, dirs) {
            // Borrar un árbol grande tarda: fuera de quien cierra el túnel.
            std::thread::spawn(move || {
                for dir in [d.work, d.stage] {
                    if let Err(e) = std::fs::remove_dir_all(&dir) {
                        tracing::debug!("no se pudo quitar {}: {e}", dir.display());
                    }
                }
            });
        }
    }
}

impl Workspaces {
    /// `root` es `<datos>/workspaces`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_git(root, Arc::new(SystemGit))
    }

    pub fn with_git(root: impl Into<PathBuf>, git: Arc<dyn Git>) -> Self {
        Workspaces {
            inner: Arc::new(Inner {
                git,
                root: root.into(),
                target: Mutex::new(HashMap::new()),
                origin: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// La carpeta del workspace de `tunnel` en esta máquina (como destino).
    pub fn dir_of(&self, tunnel: TunnelId) -> Option<PathBuf> {
        lock(&self.inner.target)
            .get(&tunnel)
            .map(|d| d.work.clone())
    }

    /// Atiende un stream de túnel que abrió la otra máquina: los pedidos de workspace
    /// aquí; archivos y puertos los resuelve `files`.
    pub async fn serve<W: TargetWire>(
        &self,
        files: &Files,
        host: &Host,
        mut wire: W,
    ) -> std::result::Result<(), WireError> {
        let Some(req) = wire.recv().await? else {
            return Ok(());
        };
        match req {
            TunnelUp::Workspace(w) => {
                let down = match self.handle(host, w).await {
                    Ok(d) => d,
                    Err(e) => WorkspaceDown::Failed(e.to_string()),
                };
                wire.send(&TunnelDown::Workspace(down)).await
            }
            other => files.serve_request(host, wire, other).await,
        }
    }

    async fn handle(&self, host: &Host, req: WorkspaceUp) -> Result<WorkspaceDown> {
        let tunnel = match &req {
            WorkspaceUp::Prepare { tunnel, .. }
            | WorkspaceUp::Checkout { tunnel }
            | WorkspaceUp::Pack { tunnel, .. } => *tunnel,
        };
        let t = host.get(tunnel).filter(|t| t.is_open()).ok_or_else(|| {
            WorkspaceError::Refused(format!("no hay ningún túnel {tunnel} abierto"))
        })?;
        match req {
            WorkspaceUp::Prepare { keep, .. } => {
                let dirs = Dirs {
                    work: self.inner.root.join(tunnel.to_string()),
                    stage: self.inner.root.join(format!("{tunnel}.in")),
                };
                let stage = dirs.stage.clone();
                let first = lock(&self.inner.target).insert(tunnel, dirs).is_none();
                if first {
                    let cleanup = Cleanup {
                        inner: self.inner.clone(),
                        tunnel,
                        keep,
                    };
                    if t.add_resource(Box::new(cleanup)).is_err() {
                        lock(&self.inner.target).remove(&tunnel);
                        return Err(WorkspaceError::Refused(format!(
                            "el túnel {tunnel} se cerró"
                        )));
                    }
                }
                let dir = stage.clone();
                blocking(move || {
                    // Un reintento parte de cero: el origen vuelve a copiar todo.
                    let _ = std::fs::remove_dir_all(&dir);
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| WorkspaceError::Io(format!("crear {}: {e}", dir.display())))
                })
                .await?;
                Ok(WorkspaceDown::Prepared {
                    stage: stage.to_string_lossy().into_owned(),
                })
            }
            WorkspaceUp::Checkout { .. } => {
                let dirs = self.target_dirs(tunnel)?;
                let git = self.inner.git.clone();
                let (stage, work) = (dirs.stage, dirs.work.clone());
                blocking(move || repo::checkout(&*git, &stage, &work)).await?;
                let dir = dirs.work.to_string_lossy().into_owned();
                t.set_default_cwd(dir.clone());
                Ok(WorkspaceDown::Ready { dir })
            }
            WorkspaceUp::Pack { commit, .. } => {
                let dirs = self.target_dirs(tunnel)?;
                let git = self.inner.git.clone();
                let out = dirs.stage.join(repo::BACK_BUNDLE);
                let back =
                    blocking(move || repo::pack_back(&*git, &dirs.work, commit.as_deref(), &out))
                        .await?;
                Ok(WorkspaceDown::Packed {
                    bundle: back.bundle.map(|b| b.to_string_lossy().into_owned()),
                    base: back.base,
                    commits: back.commits,
                    committed_pending: back.committed_pending,
                })
            }
        }
    }

    fn target_dirs(&self, tunnel: TunnelId) -> Result<Dirs> {
        lock(&self.inner.target)
            .get(&tunnel)
            .cloned()
            .ok_or_else(|| WorkspaceError::Refused(format!("el túnel {tunnel} no tiene workspace")))
    }

    /// Lleva el repo de `spec` a la destino del túnel (que ya está abierto). Devuelve la
    /// carpeta de la destino.
    pub async fn open(
        &self,
        files: &Files,
        origin: &Origin,
        tunnel: &Tunnel,
        spec: &WorkspaceSpec,
    ) -> Result<String> {
        let id = tunnel.id;
        let out = self.inner.root.join(format!("{id}.out"));
        let (git, repo_path, dir) = (
            self.inner.git.clone(),
            PathBuf::from(&spec.repo_path),
            out.clone(),
        );
        let packed = blocking(move || {
            let _ = std::fs::remove_dir_all(&dir);
            repo::pack_origin(&*git, &repo_path, &dir)
        })
        .await;
        let result = self.send(files, origin, id, spec.keep, &out, packed).await;
        let _ = std::fs::remove_dir_all(&out);
        let (dir, root) = result?;
        lock(&self.inner.origin).insert(
            id,
            OriginEntry {
                root,
                title: tunnel.title.clone(),
            },
        );
        Ok(dir)
    }

    async fn send(
        &self,
        files: &Files,
        origin: &Origin,
        id: TunnelId,
        keep: bool,
        out: &Path,
        packed: Result<repo::Packed>,
    ) -> Result<(String, PathBuf)> {
        let packed = packed?;
        let WorkspaceDown::Prepared { stage } =
            ask(origin, WorkspaceUp::Prepare { tunnel: id, keep }).await?
        else {
            return Err(WorkspaceError::Unexpected);
        };
        files.put(origin, id, out, &stage).await?;
        let WorkspaceDown::Ready { dir } =
            ask(origin, WorkspaceUp::Checkout { tunnel: id }).await?
        else {
            return Err(WorkspaceError::Unexpected);
        };
        tracing::info!(tunnel = %id, head = %packed.head, branch = ?packed.branch, "workspace listo");
        Ok((dir, packed.root))
    }

    /// Trae el trabajo del workspace de `tunnel` como la rama `tandem/<nombre>` del repo
    /// de origen. `name` por omisión es el título del túnel. Con `commit`, antes se hace
    /// commit de lo pendiente en la destino; sin él, lo pendiente no vuelve.
    pub async fn bring_back(
        &self,
        files: &Files,
        origin: &Origin,
        tunnel: TunnelId,
        commit: Option<String>,
        name: Option<String>,
    ) -> Result<BringBackReport> {
        let entry = lock(&self.inner.origin)
            .get(&tunnel)
            .cloned()
            .ok_or(WorkspaceError::NoWorkspace(tunnel))?;
        let WorkspaceDown::Packed {
            bundle,
            commits,
            committed_pending,
            ..
        } = ask(origin, WorkspaceUp::Pack { tunnel, commit }).await?
        else {
            return Err(WorkspaceError::Unexpected);
        };
        let Some(remote) = bundle else {
            return Ok(BringBackReport {
                branch: None,
                commits,
                committed_pending,
            });
        };
        let incoming = self.inner.root.join(format!("{tunnel}.back"));
        let local = incoming.join(repo::BACK_BUNDLE);
        let _ = std::fs::remove_dir_all(&incoming);
        std::fs::create_dir_all(&incoming)
            .map_err(|e| WorkspaceError::Io(format!("crear {}: {e}", incoming.display())))?;
        let fetched = async {
            files.get(origin, tunnel, &remote, &local).await?;
            let name = repo::slug(
                name.as_deref().unwrap_or(&entry.title),
                &format!("tunel-{tunnel}"),
            );
            let git = self.inner.git.clone();
            let (root, bundle) = (entry.root.clone(), local.clone());
            blocking(move || repo::fetch_back(&*git, &root, &bundle, &name)).await
        }
        .await;
        let _ = std::fs::remove_dir_all(&incoming);
        Ok(BringBackReport {
            branch: Some(fetched?),
            commits,
            committed_pending,
        })
    }

    /// Olvida el repo de un túnel cerrado.
    pub fn forget(&self, tunnel: TunnelId) {
        lock(&self.inner.origin).remove(&tunnel);
    }
}
