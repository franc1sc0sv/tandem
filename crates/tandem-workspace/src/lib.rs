//! Git workspaces per tunnel (DESIGN §8, D8, T-107).
//!
//! `tunnel_open { workspace }`: the origin packs the repo (bundle of `HEAD` plus a
//! patch with the pending work), copies it to the destination with `tandem-files` (resumable) and the
//! destination builds `<data>/workspaces/<tunnel>`, which becomes the `cwd` of the tunnel's
//! processes. `bring_back`: the new work returns as a `tandem/<name>` branch of the
//! origin repo, without touching its current branch or its working tree.
//!
//! Everything that touches the system goes through [`git::Git`]; requests travel as
//! [`tandem_proto::WorkspaceUp`] over `Channel::Tunnel`, like files and ports.

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

/// What opening a workspace left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The workspace folder on the destination.
    pub dir: String,
    /// What did not travel because it looked like a credential (T-130).
    pub skipped: Vec<String>,
    /// Credentials already in the history that travels and that cannot be removed (T-130).
    pub history_secrets: Vec<String>,
}

/// An agent's workspaces, in its two roles.
#[derive(Clone)]
pub struct Workspaces {
    inner: Arc<Inner>,
}

struct Inner {
    git: Arc<dyn Git>,
    /// `<data>/workspaces`.
    root: PathBuf,
    /// Destination: the workspaces of the tunnels the other machine opened.
    target: Mutex<HashMap<TunnelId, Dirs>>,
    /// Origin: the repo of each tunnel this machine opened with a workspace.
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

/// Asks the destination for something over a dedicated stream.
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

/// When the tunnel closes, removes the folders (unless asked to keep them).
struct Cleanup {
    inner: Arc<Inner>,
    tunnel: TunnelId,
    keep: bool,
}

impl Resource for Cleanup {
    fn close(&self) {
        let dirs = lock(&self.inner.target).remove(&self.tunnel);
        if let (false, Some(d)) = (self.keep, dirs) {
            // Deleting a large tree takes a while: kept off whoever closes the tunnel.
            std::thread::spawn(move || {
                for dir in [d.work, d.stage] {
                    if let Err(e) = std::fs::remove_dir_all(&dir) {
                        tracing::debug!("could not remove {}: {e}", dir.display());
                    }
                }
            });
        }
    }
}

impl Workspaces {
    /// `root` is `<data>/workspaces`.
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

    /// The workspace folder of `tunnel` on this machine (as destination).
    pub fn dir_of(&self, tunnel: TunnelId) -> Option<PathBuf> {
        lock(&self.inner.target)
            .get(&tunnel)
            .map(|d| d.work.clone())
    }

    /// Serves a tunnel stream opened by the other machine: workspace requests
    /// here; files and ports are resolved by `files`.
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
        let t = host
            .get(tunnel)
            .filter(|t| t.is_open())
            .ok_or_else(|| WorkspaceError::Refused(format!("no tunnel {tunnel} is open")))?;
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
                            "tunnel {tunnel} was closed"
                        )));
                    }
                }
                let dir = stage.clone();
                blocking(move || {
                    // A retry starts from zero: the origin copies everything again.
                    let _ = std::fs::remove_dir_all(&dir);
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| WorkspaceError::Io(format!("create {}: {e}", dir.display())))
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
            .ok_or_else(|| WorkspaceError::Refused(format!("tunnel {tunnel} has no workspace")))
    }

    /// Takes the repo of `spec` to the tunnel's destination (already open). Returns the
    /// destination folder.
    pub async fn open(
        &self,
        files: &Files,
        origin: &Origin,
        tunnel: &Tunnel,
        spec: &WorkspaceSpec,
    ) -> Result<Opened> {
        let id = tunnel.id;
        let out = self.inner.root.join(format!("{id}.out"));
        let (git, repo_path, dir) = (
            self.inner.git.clone(),
            PathBuf::from(&spec.repo_path),
            out.clone(),
        );
        let filter = tandem_files::SecretFilter::new(&spec.include);
        let packed = blocking(move || {
            let _ = std::fs::remove_dir_all(&dir);
            repo::pack_origin(&*git, &repo_path, &dir, &filter)
        })
        .await;
        let result = self.send(files, origin, id, spec, &out, packed).await;
        let _ = std::fs::remove_dir_all(&out);
        let (dir, root, skipped, history_secrets) = result?;
        lock(&self.inner.origin).insert(
            id,
            OriginEntry {
                root,
                title: tunnel.title.clone(),
            },
        );
        Ok(Opened {
            dir,
            skipped,
            history_secrets,
        })
    }

    async fn send(
        &self,
        files: &Files,
        origin: &Origin,
        id: TunnelId,
        spec: &WorkspaceSpec,
        out: &Path,
        packed: Result<repo::Packed>,
    ) -> Result<(String, PathBuf, Vec<String>, Vec<String>)> {
        let packed = packed?;
        let WorkspaceDown::Prepared { stage } = ask(
            origin,
            WorkspaceUp::Prepare {
                tunnel: id,
                keep: spec.keep,
            },
        )
        .await?
        else {
            return Err(WorkspaceError::Unexpected);
        };
        // Submodules go inside `submodules/`: `include` applies there too.
        let include: Vec<String> = spec
            .include
            .iter()
            .map(|p| format!("{}/{p}", repo::SUBMODULES))
            .collect();
        files.put_with(origin, id, out, &stage, &include).await?;
        let WorkspaceDown::Ready { dir } =
            ask(origin, WorkspaceUp::Checkout { tunnel: id }).await?
        else {
            return Err(WorkspaceError::Unexpected);
        };
        tracing::info!(tunnel = %id, head = %packed.head, branch = ?packed.branch, "workspace ready");
        Ok((dir, packed.root, packed.skipped, packed.history_secrets))
    }

    /// Brings the work of `tunnel`'s workspace back as the `tandem/<name>` branch of the
    /// origin repo. `name` defaults to the tunnel title. With `commit`, the pending work is
    /// committed on the destination first; without it, the pending work does not come back.
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
            .map_err(|e| WorkspaceError::Io(format!("create {}: {e}", incoming.display())))?;
        let fetched = async {
            files.get(origin, tunnel, &remote, &local).await?;
            let name = repo::slug(
                name.as_deref().unwrap_or(&entry.title),
                &format!("tunnel-{tunnel}"),
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

    /// Forgets the repo of a closed tunnel.
    pub fn forget(&self, tunnel: TunnelId) {
        lock(&self.inner.origin).remove(&tunnel);
    }
}
