//! The monitors in the agent (T-110): the loop that reads the bus, the owner choice with what
//! the other announces and the UI requests.

use std::sync::{Arc, Weak};

use tandem_core::{DisplayKey, Machine, MonitorInput, Plan};
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
            self.capabilities().peer
        } else {
            None
        };
        self.monitors
            .set_peer_cables(peer.as_ref().map(|c| c.cables.clone()));
        self.monitors.set_peer(peer.map(|c| c.monitors));
    }

    /// Why `monitor` must not be switched to `machine`'s input: that machine's cable is
    /// disconnected, so it would leave a black screen (T-349). Checked before any DDC write.
    pub(super) fn unplugged(&self, monitor: &str, machine: Machine) -> Option<String> {
        let views = self.monitors();
        let view = views.iter().find(|v| v.id == monitor)?;
        view.link.refuses_switch_to(machine).then(|| {
            format!(
                "{}: the cable of {} is unplugged; switching to it would leave a black screen",
                view.name,
                self.machine_name(machine)
            )
        })
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
        let to = crate::monitors::inputs(&self.desk(), &monitor, Some(&view)).machine_on(input);
        if let Some(why) = to.and_then(|m| self.unplugged(&monitor, m)) {
            return Reply::Rejected(why);
        }
        let what = format!("set {input} on {}", view.name);
        self.start_plan(&what, move |_| Ok(Plan::switch_monitor(monitor, input)))
    }
}

async fn monitor_loop(agent: Weak<Agent>, times: MonitorTimes) {
    let mut last_scan: Option<tokio::time::Instant> = None;
    // Whether the other's agent is reachable decides whether this machine may read itself the
    // monitors the other owns (T-346): it is looked at every turn and a change starts one.
    let Some(mut connected) = agent.upgrade().map(|a| a.orch.peer_connected()) else {
        return;
    };
    loop {
        let Some(a) = agent.upgrade() else { return };
        a.monitors
            .set_peer_reachable(*connected.borrow_and_update());
        // The desk's cables, for the links (T-349). Not a wake-up of its own: an extra turn would
        // spend the periodic scan early (on a machine just powered on, before its bus answers).
        a.monitors.set_desk(&a.desk());
        // A display change (hot-plug, T-349) rescans now instead of at the next period.
        let scan = a.monitors.take_scan_request()
            || last_scan.is_none_or(|t| t.elapsed() >= times.scan_every);
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
        if a.monitors.take_cables_changed() | tick.channels_changed {
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
            // The sender lives in the agent: once it is gone, the next turn ends the loop.
            _ = connected.changed() => {}
        }
    }
}
