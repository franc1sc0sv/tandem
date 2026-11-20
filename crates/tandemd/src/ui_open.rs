//! Opening Tandem when the system asks the agent for it (T-350).
//!
//! On macOS LaunchServices can take `tandemd` for the running instance of `Tandem.app`, and
//! then opening the app only reaches the agent ([`tandem_files::app_open`]). [`UiOpener`] does
//! what the decision says: it launches the UI beside the agent ([`UiLauncher`]) or asks the
//! connected UIs to show their window ([`UiPresence`], [`AgentToUi::ShowWindow`]).
//!
//! [`AgentToUi::ShowWindow`]: tandem_proto::AgentToUi::ShowWindow

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use tandem_files::app_open::{self, OpenDecision, OpenRequest};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// The UIs connected to this agent's IPC, and the channel to ask them to show their window.
#[derive(Clone)]
pub struct UiPresence(Arc<Presence>);

struct Presence {
    connected: AtomicUsize,
    show: broadcast::Sender<()>,
}

impl Default for UiPresence {
    fn default() -> Self {
        UiPresence(Arc::new(Presence {
            connected: AtomicUsize::new(0),
            show: broadcast::channel(4).0,
        }))
    }
}

impl std::fmt::Debug for UiPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiPresence")
            .field("connected", &self.connected())
            .finish()
    }
}

impl UiPresence {
    /// A UI connection: it counts while it lives and hears the show requests.
    pub fn connect(&self) -> UiConnection {
        // Subscribing first: a request sent while it registers is not lost.
        let show = self.0.show.subscribe();
        self.0.connected.fetch_add(1, Ordering::SeqCst);
        UiConnection {
            presence: self.clone(),
            show,
        }
    }

    /// How many UIs are connected now.
    pub fn connected(&self) -> usize {
        self.0.connected.load(Ordering::SeqCst)
    }

    /// Asks every connected UI to show its main window. Returns how many will hear it.
    pub fn show(&self) -> usize {
        self.0.show.send(()).unwrap_or(0)
    }
}

/// One UI connection in [`UiPresence`].
pub struct UiConnection {
    presence: UiPresence,
    show: broadcast::Receiver<()>,
}

impl UiConnection {
    /// The next request to show the window. Several in a row count as one.
    pub async fn shown(&mut self) {
        loop {
            match self.show.recv().await {
                Ok(()) | Err(RecvError::Lagged(_)) => return,
                // The sender lives in the presence this connection holds: it never closes.
                Err(RecvError::Closed) => std::future::pending::<()>().await,
            }
        }
    }
}

impl Drop for UiConnection {
    fn drop(&mut self) {
        self.presence.0.connected.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Starts the UI. On macOS, the `tandem-ui` beside `tandemd` in the bundle ([`SiblingUi`]).
pub trait UiLauncher: Send + Sync {
    fn launch(&self) -> io::Result<()>;
}

/// Name of the UI's executable in the bundle's `Contents/MacOS`, next to `tandemd`.
pub const UI_EXECUTABLE: &str = "tandem-ui";

/// The UI's executable beside the agent's.
pub fn ui_beside(agent_exe: &Path) -> Option<PathBuf> {
    Some(agent_exe.parent()?.join(UI_EXECUTABLE))
}

/// Does what the system asked when it opened Tandem through the agent. Created before the agent
/// starts (AppKit's run loop serves requests from the first moment); [`UiOpener::attach`] gives
/// it the agent's [`UiPresence`] once there is one. Until then no UI can be connected.
pub struct UiOpener {
    presence: OnceLock<UiPresence>,
    launcher: Box<dyn UiLauncher>,
}

impl UiOpener {
    pub fn new(launcher: Box<dyn UiLauncher>) -> Self {
        UiOpener {
            presence: OnceLock::new(),
            launcher,
        }
    }

    pub fn attach(&self, presence: UiPresence) {
        let _ = self.presence.set(presence);
    }

    /// Answers one request. It must not block: it runs on AppKit's main thread.
    pub fn handle(&self, request: OpenRequest) -> OpenDecision {
        let connected = self.presence.get().is_some_and(|p| p.connected() > 0);
        let decision = app_open::decide(request, connected);
        match decision {
            OpenDecision::LaunchUi => {
                tracing::info!(%request, "open request from LaunchServices: launching the UI");
                if let Err(e) = self.launcher.launch() {
                    tracing::warn!("could not launch the UI: {e}");
                }
            }
            OpenDecision::ShowUi => {
                tracing::info!(%request, "open request from LaunchServices: showing the UI");
                if let Some(p) = self.presence.get() {
                    p.show();
                }
            }
            OpenDecision::Nothing => {
                tracing::debug!(%request, "reopen callback without an open request: ignored");
            }
        }
        decision
    }
}

/// Launches the UI's executable as a new process of its own: its own process group, so
/// launchd stopping the agent's job does not take it along, and nothing inherited on the
/// standard streams. A thread waits for it so it does not stay a zombie.
#[cfg(unix)]
pub struct SiblingUi {
    pub program: PathBuf,
}

#[cfg(unix)]
impl SiblingUi {
    /// The `tandem-ui` beside this process's executable.
    pub fn beside_current_exe() -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let program = ui_beside(&exe)
            .ok_or_else(|| io::Error::other(format!("{} has no folder", exe.display())))?;
        Ok(SiblingUi { program })
    }
}

#[cfg(unix)]
impl UiLauncher for SiblingUi {
    fn launch(&self) -> io::Result<()> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        if !self.program.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} does not exist", self.program.display()),
            ));
        }
        let mut child = Command::new(&self.program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        std::thread::Builder::new()
            .name("ui-launch".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use proptest::prelude::*;
    use tandem_files::app_quit::{CORE_EVENT_CLASS, QUIT_APPLICATION};

    use super::*;

    const REOPEN: OpenRequest = OpenRequest::AppleEvent {
        class: CORE_EVENT_CLASS,
        id: app_open::REOPEN_APPLICATION,
    };
    const QUIT: OpenRequest = OpenRequest::AppleEvent {
        class: CORE_EVENT_CLASS,
        id: QUIT_APPLICATION,
    };

    #[derive(Clone, Default)]
    struct FakeLauncher {
        launches: Arc<Mutex<usize>>,
        fail: bool,
    }

    impl FakeLauncher {
        fn launches(&self) -> usize {
            *self.launches.lock().unwrap()
        }
    }

    impl UiLauncher for FakeLauncher {
        fn launch(&self) -> io::Result<()> {
            *self.launches.lock().unwrap() += 1;
            if self.fail {
                Err(io::Error::other("no ui"))
            } else {
                Ok(())
            }
        }
    }

    fn opener() -> (UiOpener, FakeLauncher, UiPresence) {
        let launcher = FakeLauncher::default();
        let opener = UiOpener::new(Box::new(launcher.clone()));
        let presence = UiPresence::default();
        opener.attach(presence.clone());
        (opener, launcher, presence)
    }

    #[test]
    fn without_a_ui_the_reopen_launches_it() {
        let (opener, launcher, _presence) = opener();
        assert_eq!(opener.handle(REOPEN), OpenDecision::LaunchUi);
        assert_eq!(launcher.launches(), 1);
    }

    #[test]
    fn before_the_agent_exists_the_reopen_launches_the_ui() {
        let launcher = FakeLauncher::default();
        let opener = UiOpener::new(Box::new(launcher.clone()));
        assert_eq!(opener.handle(REOPEN), OpenDecision::LaunchUi);
        assert_eq!(launcher.launches(), 1);
    }

    #[test]
    fn a_failed_launch_is_only_logged() {
        let launcher = FakeLauncher {
            fail: true,
            ..FakeLauncher::default()
        };
        let opener = UiOpener::new(Box::new(launcher.clone()));
        assert_eq!(opener.handle(REOPEN), OpenDecision::LaunchUi);
        assert_eq!(launcher.launches(), 1);
    }

    #[tokio::test]
    async fn with_a_ui_the_reopen_asks_it_to_show_the_window() {
        let (opener, launcher, presence) = opener();
        let mut ui = presence.connect();
        assert_eq!(opener.handle(REOPEN), OpenDecision::ShowUi);
        assert_eq!(launcher.launches(), 0);
        tokio::time::timeout(std::time::Duration::from_secs(1), ui.shown())
            .await
            .unwrap();
    }

    #[test]
    fn a_closed_ui_no_longer_counts() {
        let (opener, launcher, presence) = opener();
        let ui = presence.connect();
        let other = presence.connect();
        assert_eq!(presence.connected(), 2);
        drop(ui);
        drop(other);
        assert_eq!(presence.connected(), 0);
        assert_eq!(opener.handle(REOPEN), OpenDecision::LaunchUi);
        assert_eq!(launcher.launches(), 1);
    }

    #[tokio::test]
    async fn several_requests_while_busy_count_as_one() {
        let presence = UiPresence::default();
        let mut ui = presence.connect();
        for _ in 0..10 {
            presence.show();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), ui.shown())
            .await
            .unwrap();
    }

    #[test]
    fn a_quit_launches_nothing_and_shows_nothing() {
        let (opener, launcher, presence) = opener();
        let _ui = presence.connect();
        assert_eq!(opener.handle(QUIT), OpenDecision::Nothing);
        drop(_ui);
        assert_eq!(opener.handle(QUIT), OpenDecision::Nothing);
        assert_eq!(launcher.launches(), 0);
    }

    #[test]
    fn the_ui_is_the_sibling_of_the_agent() {
        assert_eq!(
            ui_beside(Path::new("/Applications/Tandem.app/Contents/MacOS/tandemd")),
            Some(PathBuf::from(
                "/Applications/Tandem.app/Contents/MacOS/tandem-ui"
            ))
        );
        assert_eq!(ui_beside(Path::new("/")), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_sibling_launcher_runs_the_program_detached() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("launched");
        let program = dir.path().join(UI_EXECUTABLE);
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho $$ > '{}'\n", mark.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        SiblingUi { program }.launch().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !mark.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(mark.exists(), "the UI was not launched");
    }

    #[cfg(unix)]
    #[test]
    fn the_sibling_launcher_fails_without_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let ui = SiblingUi {
            program: dir.path().join(UI_EXECUTABLE),
        };
        assert_eq!(ui.launch().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    proptest! {
        #[test]
        fn the_opener_never_launches_on_a_quit(connected in 0usize..3, class: u32, id: u32) {
            let (opener, launcher, presence) = opener();
            let _uis: Vec<_> = (0..connected).map(|_| presence.connect()).collect();
            let request = OpenRequest::AppleEvent { class, id };
            let decision = opener.handle(request);
            if request.is_quit() {
                prop_assert_eq!(decision, OpenDecision::Nothing);
            }
            // Only a launch decision launches, and only without a UI.
            prop_assert_eq!(launcher.launches(), usize::from(decision == OpenDecision::LaunchUi));
            if decision == OpenDecision::LaunchUi {
                prop_assert_eq!(connected, 0);
            }
        }
    }
}
