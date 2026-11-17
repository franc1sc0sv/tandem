//! El updater dentro de la app (T-122): buscar, «Actualizar todas», «Actualizar solo» y
//! atender el pedido de la otra máquina. Las decisiones están en [`crate::update`]; aquí
//! solo se conectan con el plugin de Tauri, la sesión con `tandemd` y la ventana.
//!
//! - El paquete de cada sistema trae la UI y `tandemd`. En macOS el updater reemplaza
//!   `Tandem.app` y la app se reinicia; al arrancar pone al día el LaunchAgent, que vuelve a
//!   lanzar `tandemd` con el binario nuevo ([`crate::launch_agent`]). En Windows corre el
//!   instalador NSIS en modo pasivo: sus ganchos detienen `tandemd` y `tandemw` antes de
//!   copiar y los vuelven a arrancar al terminar (`windows/hooks.nsh`).
//! - La versión nueva sale de `latest.json` en el último release publicado de GitHub; cada
//!   paquete va firmado con la llave minisign del updater (la pública está en
//!   `tauri.conf.json`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::UpdateStatus;
use tandemd::paths::Paths;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::watch;

use crate::session::{Action, ActionResult, Handle, PeerView, Snapshot};
use crate::update::{
    self, AUTO_CHECK_EVERY, Fleet, PEER_RETURN_TIMEOUT, Peer, Settings, Step, UpdateView, Version,
};

pub const UPDATE_EVENT: &str = "ui-update";

/// La primera búsqueda automática espera a que el arranque termine y la sesión se conecte.
const FIRST_AUTO_CHECK: Duration = Duration::from_secs(90);

/// La versión de esta app (y de su `tandemd`).
const HERE: &str = env!("CARGO_PKG_VERSION");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Updater {
    state: Mutex<State>,
    /// Lo último que dijo `tandemd` de las versiones; «Actualizar todas» espera aquí a que
    /// la otra vuelva.
    agent: watch::Sender<Option<UpdateStatus>>,
}

struct State {
    view: UpdateView,
    found: Option<Version>,
    settings_file: Option<PathBuf>,
    /// Hay una instalación (de todas o la pedida por la otra) en curso.
    busy: bool,
}

impl Updater {
    pub fn new() -> Self {
        let settings_file = Paths::resolve(None, None)
            .ok()
            .map(|p| update::settings_file_for(&p.config));
        let settings = settings_file
            .as_deref()
            .map(update::load_settings)
            .unwrap_or_default();
        Updater {
            state: Mutex::new(State {
                view: UpdateView {
                    here: HERE.to_owned(),
                    peer: None,
                    paired: false,
                    available: None,
                    checking: false,
                    step: None,
                    error: None,
                    auto: settings.auto,
                },
                found: None,
                settings_file,
                busy: false,
            }),
            agent: watch::Sender::new(None),
        }
    }

    pub fn view(&self) -> UpdateView {
        lock(&self.state).view.clone()
    }

    fn change(&self, app: &AppHandle, f: impl FnOnce(&mut State)) -> UpdateView {
        let view = {
            let mut s = lock(&self.state);
            f(&mut s);
            s.view.clone()
        };
        let _ = app.emit(UPDATE_EVENT, &view);
        view
    }

    /// Lo nuevo que dijo `tandemd`. Si la otra pidió instalar una versión más nueva, esta
    /// la instala (es el paso «la otra» de su «Actualizar todas»).
    pub fn on_agent(self: &Arc<Self>, app: &AppHandle, status: &UpdateStatus) {
        self.agent.send_replace(Some(status.clone()));
        let paired = matches!(peer_view(app), Some(PeerView::Paired { .. }));
        let request = update::requested_install(&parse(HERE), status.install.as_deref());
        let mut start = None;
        self.change(app, |s| {
            s.view.peer = status.peer.clone();
            s.view.paired = paired;
            if let Some(v) = request
                && !s.busy
            {
                s.busy = true;
                s.view.step = Some(Step::InstallHere);
                s.view.error = None;
                start = Some(v);
            }
        });
        if let Some(target) = start {
            let (me, app) = (self.clone(), app.clone());
            tauri::async_runtime::spawn(async move {
                let result = install_here(&app, &target).await;
                me.change(&app, |s| {
                    s.busy = false;
                    s.view.step = None;
                    s.view.error = result.err();
                });
            });
        }
    }

    /// Busca una versión nueva en GitHub Releases.
    pub async fn check(&self, app: &AppHandle) -> UpdateView {
        self.change(app, |s| {
            s.view.checking = true;
            s.view.error = None;
        });
        let found = find(app, self.peer(app)).await;
        self.change(app, |s| {
            s.view.checking = false;
            match found {
                Ok(v) => {
                    s.view.available = v.as_ref().map(ToString::to_string);
                    s.found = v;
                }
                Err(e) => s.view.error = Some(e),
            }
        })
    }

    fn peer(&self, app: &AppHandle) -> Peer {
        let status = self.agent.borrow().clone();
        match peer_view(app) {
            Some(PeerView::Unpaired) => Peer::Alone,
            Some(PeerView::Paired { link, .. }) => {
                update::peer_from(true, link.is_some(), status.as_ref())
            }
            // Sin `tandemd` no se sabe nada de la otra: no se puede actualizar de a dos.
            None => Peer::Unreachable,
        }
    }

    /// «Actualizar todas»: arranca en segundo plano; el avance llega por [`UPDATE_EVENT`].
    pub fn update_all(self: &Arc<Self>, app: &AppHandle) -> Result<UpdateView, String> {
        let target = lock(&self.state)
            .found
            .clone()
            .ok_or("no hay una versión nueva: busca primero")?;
        let steps =
            update::plan(&parse(HERE), &self.peer(app), &target).map_err(|e| e.to_string())?;
        let mut taken = false;
        let view = self.change(app, |s| {
            if !s.busy {
                s.busy = true;
                taken = true;
                s.view.error = None;
            }
        });
        if !taken {
            return Err("ya hay una actualización en curso".into());
        }
        let (me, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            let mut fleet = AppFleet {
                app: app.clone(),
                agent: me.agent.subscribe(),
            };
            let progress = {
                let (me, app) = (me.clone(), app.clone());
                move |step| {
                    me.change(&app, |s| s.view.step = Some(step));
                }
            };
            let result =
                update::run(&steps, &target, &mut fleet, PEER_RETURN_TIMEOUT, progress).await;
            me.change(&app, |s| {
                s.busy = false;
                s.view.step = None;
                s.view.error = result.err().map(|e| e.to_string());
            });
        });
        Ok(view)
    }

    pub fn set_auto(&self, app: &AppHandle, on: bool) -> Result<UpdateView, String> {
        let file = lock(&self.state).settings_file.clone();
        if let Some(file) = file {
            update::save_settings(&file, Settings { auto: on })
                .map_err(|e| format!("no se pudo guardar {}: {e}", file.display()))?;
        }
        Ok(self.change(app, |s| s.view.auto = on))
    }

    /// «Actualizar solo»: busca cada [`AUTO_CHECK_EVERY`] y, si esta máquina es la que guía
    /// ([`update::drives_auto`]), actualiza las dos.
    pub fn spawn_auto(self: &Arc<Self>, app: &AppHandle) {
        let (me, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_AUTO_CHECK).await;
            loop {
                if me.view().auto {
                    me.check(&app).await;
                    let drives =
                        this_machine(&app).is_some_and(|m| update::drives_auto(m, &me.peer(&app)));
                    if drives
                        && me.view().available.is_some()
                        && let Err(e) = me.update_all(&app)
                    {
                        eprintln!("tandem-ui: actualizar solo: {e}");
                    }
                }
                tokio::time::sleep(AUTO_CHECK_EVERY).await;
            }
        });
    }
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

fn parse(s: &str) -> Version {
    // La versión propia sale de Cargo.toml, que `scripts/release/version.sh` comprueba.
    s.parse().unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
        pre: vec![],
    })
}

fn snapshot(app: &AppHandle) -> Option<Snapshot> {
    app.try_state::<Handle>().map(|h| h.snapshot())
}

fn peer_view(app: &AppHandle) -> Option<PeerView> {
    match snapshot(app)? {
        Snapshot::Connected { peer, .. } => Some(peer),
        _ => None,
    }
}

fn this_machine(app: &AppHandle) -> Option<Machine> {
    match snapshot(app)? {
        Snapshot::Connected { agent, .. } => Some(agent),
        _ => None,
    }
}

/// La versión del último release, si alguna de las dos máquinas va atrás (aunque esta ya esté
/// al día, ver [`update::fetch_release`]).
async fn find(app: &AppHandle, peer: Peer) -> Result<Option<Version>, String> {
    let updater = app
        .updater_builder()
        .version_comparator(move |current, release| {
            match (
                current.to_string().parse::<Version>(),
                release.version.to_string().parse::<Version>(),
            ) {
                (Ok(c), Ok(r)) => update::fetch_release(&c, &r, &peer),
                // Lo que no se entiende como versión no se ofrece.
                _ => false,
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("no se pudo buscar: {e}"))?;
    update
        .map(|u| {
            u.version
                .parse()
                .map_err(|e: update::BadVersion| e.to_string())
        })
        .transpose()
}

/// Baja `target` (o una más nueva, si salió otra entre tanto), la instala y reinicia.
async fn install_here(app: &AppHandle, target: &Version) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("no se pudo buscar: {e}"))?
        .ok_or_else(|| format!("el release de la {target} ya no está publicado"))?;
    let found: Version = update
        .version
        .parse()
        .map_err(|e: update::BadVersion| e.to_string())?;
    if found < *target {
        return Err(format!(
            "el release publicado es la {found}, no la {target}"
        ));
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("no se pudo instalar: {e}"))?;
    // En Windows el instalador ya cerró la app; en macOS hay que reiniciarla.
    app.restart();
}

/// La otra máquina por `tandemd` y el updater de esta.
struct AppFleet {
    app: AppHandle,
    agent: watch::Receiver<Option<UpdateStatus>>,
}

impl Fleet for AppFleet {
    async fn ask_peer(&mut self, target: &Version) -> Result<(), String> {
        let handle = self
            .app
            .try_state::<Handle>()
            .ok_or("la sesión con tandemd no arrancó")?
            .inner()
            .clone();
        match handle
            .act(Action::UpdatePeer {
                version: target.to_string(),
            })
            .await
        {
            ActionResult::Done => Ok(()),
            ActionResult::Rejected { reason } | ActionResult::Unavailable { reason } => Err(reason),
            other => Err(format!("respuesta inesperada de tandemd: {other:?}")),
        }
    }

    async fn peer_changed(&mut self) -> Option<Peer> {
        self.agent.changed().await.ok()?;
        let status = self.agent.borrow_and_update().clone();
        let peer = match peer_view(&self.app) {
            Some(PeerView::Paired { link, .. }) => {
                update::peer_from(true, link.is_some(), status.as_ref())
            }
            Some(PeerView::Unpaired) => Peer::Alone,
            None => Peer::Unreachable,
        };
        Some(peer)
    }

    async fn install_here(&mut self, target: &Version) -> Result<(), String> {
        install_here(&self.app, target).await
    }
}
