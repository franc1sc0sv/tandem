//! This machine's display of a monitor that shows the other, off (T-341, [`crate::displays`]):
//! the loop that follows what each monitor shows, the wake before a switch to this machine's input
//! ([`DisplayWake`]) and the restore at exit. The built-in display follows its mode (T-353) in the
//! same turn.

use std::sync::{Arc, PoisonError, Weak};
use std::time::{Duration, Instant};

use tandem_core::{Machine, MonitorInput};
use tandem_input::displays::DisplayControl;
use tokio::task::JoinHandle;

use super::Agent;
use crate::displays::DisplayGuard;
use crate::monitors::{inputs, shows};
use crate::orchestrator::{DisplayWake, StepFuture};

/// How often the displays are looked at without any change: for the grace of an unknown input
/// and the end of a wake.
const DISPLAYS_EVERY: Duration = Duration::from_secs(1);

impl Agent {
    /// Starts display control with `guard`: first it turns back on what a previous run left off,
    /// then it follows the monitors. The orchestrator wakes the displays before each switch.
    pub fn start_displays(self: &Arc<Self>, guard: Arc<DisplayGuard>) -> JoinHandle<()> {
        guard.restore_saved();
        *self
            .displays
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(guard);
        self.orch.set_displays(Arc::new(AgentDisplays {
            agent: Arc::downgrade(self),
        }));
        self.refresh_capabilities();
        tokio::spawn(displays_loop(Arc::downgrade(self)))
    }

    pub fn display_guard(&self) -> Option<Arc<DisplayGuard>> {
        self.displays
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// For the "Monitors" module.
    pub(super) fn display_control(&self) -> DisplayControl {
        self.display_guard()
            .map_or(DisplayControl::NotNeeded, |g| g.control())
    }

    /// Exit: every display this machine turned off goes back on, and none goes off again.
    pub fn restore_displays(&self) {
        if let Some(g) = self.display_guard() {
            g.shut_down();
        }
    }

    /// One turn: this machine's displays follow what each monitor shows.
    fn reconcile_displays(&self, guard: &DisplayGuard) {
        let me = self.machine();
        let mut now = shows(&self.monitors(), &self.desk());
        // A monitor that is off is treated like one showing the other machine (T-349): nobody
        // sees this machine's display of it, so it goes off with the same rules and limits.
        for (id, link) in self.monitors.links() {
            if link.state == tandem_core::LinkState::Off {
                now.insert(id, me.other());
            }
        }
        let t = Instant::now();
        guard.reconcile(me, &now, &self.monitors.keys(), t);
        // The built-in display after the external ones: what it does depends on which are on.
        guard.reconcile_builtin(self.desk().builtin_display.of(me), t);
    }
}

/// The orchestrator's view of the displays.
struct AgentDisplays {
    agent: Weak<Agent>,
}

impl DisplayWake for AgentDisplays {
    fn machine_on(&self, monitor: &str, input: MonitorInput) -> Option<Machine> {
        let agent = self.agent.upgrade()?;
        let views = agent.monitors();
        let view = views.iter().find(|v| v.id == monitor);
        inputs(&agent.desk(), monitor, view).machine_on(input)
    }

    fn display_on<'a>(&'a self, monitor: &'a str) -> StepFuture<'a> {
        let guard = self.agent.upgrade().and_then(|a| a.display_guard());
        let monitor = monitor.to_owned();
        Box::pin(async move {
            let Some(guard) = guard else { return Ok(()) };
            tokio::task::spawn_blocking(move || guard.wake(&monitor, Instant::now()))
                .await
                .map_err(|e| format!("the display change stopped: {e}"))?
        })
    }
}

async fn displays_loop(agent: Weak<Agent>) {
    let Some(a) = agent.upgrade() else { return };
    let mut views = a.subscribe_monitors();
    let mut desk = a.subscribe_desk();
    drop(a);
    loop {
        let Some(a) = agent.upgrade() else { return };
        let Some(guard) = a.display_guard() else {
            return;
        };
        // CoreGraphics can take a moment: off the runtime's threads.
        let turn = {
            let a = a.clone();
            tokio::task::spawn_blocking(move || a.reconcile_displays(&guard))
        };
        drop(a);
        if let Err(e) = turn.await {
            tracing::warn!("the display turn stopped: {e}");
        }
        tokio::select! {
            r = views.changed() => if r.is_err() { return },
            r = desk.changed() => if r.is_err() { return },
            () = tokio::time::sleep(DISPLAYS_EVERY) => {}
        }
    }
}
