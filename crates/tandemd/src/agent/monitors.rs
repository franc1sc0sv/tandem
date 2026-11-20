//! The monitors in the agent (T-110): the loop that reads the bus, the owner choice with what
//! the other announces and the UI requests.

use std::sync::{Arc, Weak};

use tandem_core::{DisplayKey, MonitorInput, Plan};
use tandem_monitor::LocalMonitors;
use tandem_proto::{ControlMsg, ModuleId, MonitorReport, MonitorView, Reply};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::Agent;
use crate::monitors::{MonitorHub, MonitorTimes, NO_DDC};
use crate::orchestrator::NO_OWNER;

impl Agent {
    /// The monitors, for the orchestrator and the tests.
    pub fn monitor_hub(&self) -> &Arc<MonitorHub> {
        &self.monitors
    }

    /// The monitors as the UI sees them.
    pub fn monitors(&self) -> Vec<MonitorView> {
        self.monitors.views()
    }

    /// Notifies every time some monitor changes.
    pub fn subscribe_monitors(&self) -> watch::Receiver<Vec<MonitorView>> {
        self.monitors.subscribe()
    }

    /// Starts the monitors with this machine's bus: probes, chooses owners with what
    /// the other announces and reads its own. `inline` only with a bus that does not block (simulation).
    pub fn start_monitors(
        self: &Arc<Self>,
        core: LocalMonitors,
        times: MonitorTimes,
        inline: bool,
    ) -> JoinHandle<()> {
        self.monitors.attach(core, inline);
        self.set_module_problem(ModuleId::Monitors, None);
        tokio::spawn(monitor_loop(Arc::downgrade(self), times))
    }

    /// This machine's screens with the EDID identity the system gives for each one (the input
    /// engine reads them, T-340): with them each monitor it sees is linked to its screen.
    pub fn set_screen_keys(&self, screens: Vec<(String, DisplayKey)>) {
        if self.monitors.set_screens(screens) {
            self.reconcile_monitors();
        }
    }

    /// What the other read from its monitors.
    pub fn receive_monitor_reports(&self, reports: Vec<MonitorReport>) {
        self.monitors.receive(self.machine().other(), reports);
        self.reconcile_monitors();
    }

    /// The other's channels count while it is on: if it turns off or is lost, its
    /// monitors pass to this one (if it sees them).
    pub(super) fn sync_monitor_peer(&self) {
        let other = self.machine().other();
        let peer = if self.state().is_on(other) {
            self.capabilities().peer.map(|c| c.monitors)
        } else {
            None
        };
        self.monitors.set_peer(peer);
    }

    /// [`tandem_proto::UiCommand::SwitchMonitor`]: a transaction done by the owner.
    pub(super) fn start_switch_monitor(
        self: &Arc<Self>,
        monitor: String,
        input: MonitorInput,
    ) -> Reply {
        let Some(view) = self.monitors().into_iter().find(|v| v.id == monitor) else {
            return Reply::Rejected(format!("monitor {monitor} is not seen"));
        };
        if !view.compat.is_compatible() {
            return Reply::Rejected(format!(
                "{} is not compatible: it is shown, but not controlled",
                view.name
            ));
        }
        if view.owner.is_none() {
            return Reply::Rejected(format!("{}: {NO_OWNER}", view.name));
        }
        let what = format!("set {input} on {}", view.name);
        self.start_plan(&what, move |_| Ok(Plan::switch_monitor(monitor, input)))
    }
}

async fn monitor_loop(agent: Weak<Agent>, times: MonitorTimes) {
    let mut last_scan: Option<tokio::time::Instant> = None;
    loop {
        let Some(a) = agent.upgrade() else { return };
        let scan = last_scan.is_none_or(|t| t.elapsed() >= times.scan_every);
        if scan {
            last_scan = Some(tokio::time::Instant::now());
        }
        let tick = a.monitors.tick(scan).await;
        if let Some(e) = &tick.scan_error {
            tracing::warn!("could not enumerate the monitors: {e}");
            a.set_module_problem(ModuleId::Monitors, Some(NO_DDC.into()));
        } else if scan {
            a.set_module_problem(ModuleId::Monitors, None);
        }
        if tick.channels_changed {
            a.refresh_capabilities();
        }
        if let Some(reports) = tick.reports {
            a.orch.peer().send(ControlMsg::Monitors(reports));
        }
        a.reconcile_monitors();
        let hub = a.monitors.clone();
        drop(a);
        tokio::select! {
            () = tokio::time::sleep(times.poll_every) => {}
            () = hub.woken() => {}
        }
    }
}
