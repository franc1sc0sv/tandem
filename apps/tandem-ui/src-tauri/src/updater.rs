//! The in-app updater (T-122): check, "Update all", "Update automatically" and serving the
//! other machine's request. The decisions live in [`crate::update`]; here they are only wired
//! to the Tauri plugin, the session with `tandemd` and the window.
//!
//! - Each system's package ships the UI and `tandemd`. On macOS the updater replaces
//!   `Tandem.app` and the app restarts; on startup it refreshes the LaunchAgent, which
//!   relaunches `tandemd` with the new binary ([`crate::launch_agent`]). On Windows it runs
//!   the NSIS installer in passive mode: its hooks stop `tandemd` and `tandemw` before
//!   copying and start them again when done (`windows/hooks.nsh`).
//! - The new version comes from `latest.json` in the latest published GitHub release; each
//!   package is signed with the updater's minisign key (the public one is in
//!   `tauri.conf.json`).
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

/// The first automatic check waits for startup to finish and the session to connect.
const FIRST_AUTO_CHECK: Duration = Duration::from_secs(90);

/// The version of this app (and of its `tandemd`).
const HERE: &str = env!("CARGO_PKG_VERSION");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Updater {
    state: Mutex<State>,
    /// The latest `tandemd` said about versions; "Update all" waits here for the other
    /// machine to come back.
    agent: watch::Sender<Option<UpdateStatus>>,
}

struct State {
    view: UpdateView,
    found: Option<Version>,
    settings_file: Option<PathBuf>,
    /// An install (of all, or the one the other machine asked for) is in progress.
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
        let updated_from = update::just_updated(settings.seen.as_deref(), &parse(HERE));
        // Remember this version right away: the "Updated to" result is shown once per update.
        if settings.seen.as_deref() != Some(HERE)
            && let Some(file) = settings_file.as_deref()
        {
            let seen = Settings {
                auto: settings.auto,
                seen: Some(HERE.to_owned()),
            };
            if let Err(e) = update::save_settings(file, seen) {
                eprintln!("tandem-ui: could not save {}: {e}", file.display());
            }
        }
        let admin = crate::admin_service::update_asks_admin(
            &crate::app::admin_platform(),
            &crate::app::admin_acl(),
        );
        Updater {
            state: Mutex::new(State {
                view: UpdateView {
                    seq: 0,
                    here: HERE.to_owned(),
                    peer: None,
                    paired: false,
                    available: None,
                    checking: false,
                    step: None,
                    error: None,
                    auto: settings.auto,
                    admin,
                    updated_from: updated_from.map(|v| v.to_string()),
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
        let view = self.mutate(f);
        let _ = app.emit(UPDATE_EVENT, &view);
        view
    }

    /// Applies `f` and numbers the result, under the same lock: the order of the numbers is
    /// the order of the changes, whichever channel reaches the window first.
    fn mutate(&self, f: impl FnOnce(&mut State)) -> UpdateView {
        let mut s = lock(&self.state);
        f(&mut s);
        update::renumber(&mut s.view);
        s.view.clone()
    }

    /// Takes the install for "Update all" ([`update::take_install`]). `None` if another
    /// install is in progress.
    fn take(&self, first: Option<Step>) -> Option<UpdateView> {
        let mut taken = false;
        let view = self.mutate(|s| taken = update::take_install(&mut s.view, &mut s.busy, first));
        taken.then_some(view)
    }

    /// What `tandemd` just said. If the other machine asked to install a newer version, this
    /// one installs it (it is the "other machine" step of its "Update all").
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

    /// Looks for a new version on GitHub Releases.
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
            // Without `tandemd` nothing is known about the other machine: it cannot update both.
            None => Peer::Unreachable,
        }
    }

    /// "Update all": starts in the background; progress arrives through [`UPDATE_EVENT`].
    pub fn update_all(self: &Arc<Self>, app: &AppHandle) -> Result<UpdateView, String> {
        let target = lock(&self.state)
            .found
            .clone()
            .ok_or("there is no new version: check first")?;
        let steps =
            update::plan(&parse(HERE), &self.peer(app), &target).map_err(|e| e.to_string())?;
        let view = self
            .take(steps.first().copied())
            .ok_or("an update is already in progress")?;
        let _ = app.emit(UPDATE_EVENT, &view);
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
            let seen = Some(HERE.to_owned());
            update::save_settings(&file, Settings { auto: on, seen })
                .map_err(|e| format!("could not save {}: {e}", file.display()))?;
        }
        Ok(self.change(app, |s| s.view.auto = on))
    }

    /// "Update automatically": checks every [`AUTO_CHECK_EVERY`] and, if this machine is the
    /// one that drives ([`update::drives_auto`]), updates both.
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
                        eprintln!("tandem-ui: update automatically: {e}");
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
    // Our own version comes from Cargo.toml, which `scripts/release/version.sh` checks.
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

/// The version of the latest release, if either machine is behind (even when this one is
/// already up to date, see [`update::fetch_release`]).
async fn find(app: &AppHandle, peer: Peer) -> Result<Option<Version>, String> {
    let updater = app
        .updater_builder()
        .version_comparator(move |current, release| {
            match (
                current.to_string().parse::<Version>(),
                release.version.to_string().parse::<Version>(),
            ) {
                (Ok(c), Ok(r)) => update::fetch_release(&c, &r, &peer),
                // Anything that does not parse as a version is not offered.
                _ => false,
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("could not check: {e}"))?;
    update
        .map(|u| {
            u.version
                .parse()
                .map_err(|e: update::BadVersion| e.to_string())
        })
        .transpose()
}

/// Downloads `target` (or a newer one, if another came out meanwhile), installs it and restarts.
async fn install_here(app: &AppHandle, target: &Version) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("could not check: {e}"))?
        .ok_or_else(|| format!("release {target} is no longer published"))?;
    let found: Version = update
        .version
        .parse()
        .map_err(|e: update::BadVersion| e.to_string())?;
    if found < *target {
        return Err(format!("the published release is {found}, not {target}"));
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("could not install: {e}"))?;
    // On Windows the installer already closed the app; on macOS it must be restarted.
    app.restart();
}

/// The other machine through `tandemd`, and this one's updater.
struct AppFleet {
    app: AppHandle,
    agent: watch::Receiver<Option<UpdateStatus>>,
}

impl Fleet for AppFleet {
    async fn ask_peer(&mut self, target: &Version) -> Result<(), String> {
        let handle = self
            .app
            .try_state::<Handle>()
            .ok_or("the session with tandemd did not start")?
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
            other => Err(format!("unexpected reply from tandemd: {other:?}")),
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
