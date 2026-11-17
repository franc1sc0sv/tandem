//! Los monitores en el agente (T-110): el bucle que lee el bus, la elección del dueño con lo
//! que anuncia la otra y los pedidos de la UI.

use std::sync::{Arc, Weak};

use tandem_core::{MonitorInput, Plan};
use tandem_monitor::LocalMonitors;
use tandem_proto::{ControlMsg, ModuleId, MonitorReport, MonitorView, Reply};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::Agent;
use crate::monitors::{MonitorHub, MonitorTimes, NO_DDC};
use crate::orchestrator::NO_OWNER;

impl Agent {
    /// Los monitores, para el orquestador y las pruebas.
    pub fn monitor_hub(&self) -> &Arc<MonitorHub> {
        &self.monitors
    }

    /// Los monitores como los ve la UI.
    pub fn monitors(&self) -> Vec<MonitorView> {
        self.monitors.views()
    }

    /// Avisa cada vez que cambia algún monitor.
    pub fn subscribe_monitors(&self) -> watch::Receiver<Vec<MonitorView>> {
        self.monitors.subscribe()
    }

    /// Arranca los monitores con el bus de esta máquina: sondea, elige dueños con lo que
    /// anuncie la otra y lee los propios. `inline` solo con un bus que no bloquea (simulación).
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

    /// Lo que leyó la otra de sus monitores.
    pub fn receive_monitor_reports(&self, reports: Vec<MonitorReport>) {
        self.monitors.receive(self.machine().other(), reports);
    }

    /// Los canales de la otra cuentan mientras está prendida: si se apaga o se pierde, sus
    /// monitores pasan a esta (si los ve).
    pub(super) fn sync_monitor_peer(&self) {
        let other = self.machine().other();
        let peer = if self.state().is_on(other) {
            self.capabilities().peer.map(|c| c.monitors)
        } else {
            None
        };
        self.monitors.set_peer(peer);
    }

    /// [`tandem_proto::UiCommand::SwitchMonitor`]: una transacción que hace el dueño.
    pub(super) fn start_switch_monitor(
        self: &Arc<Self>,
        monitor: String,
        input: MonitorInput,
    ) -> Reply {
        let Some(view) = self.monitors().into_iter().find(|v| v.id == monitor) else {
            return Reply::Rejected(format!("no se ve el monitor {monitor}"));
        };
        if !view.compat.is_compatible() {
            return Reply::Rejected(format!(
                "el {} no es compatible: se muestra, pero no se controla",
                view.name
            ));
        }
        if view.owner.is_none() {
            return Reply::Rejected(format!("{}: {NO_OWNER}", view.name));
        }
        let what = format!("poner {input} en el {}", view.name);
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
            tracing::warn!("no se pudieron enumerar los monitores: {e}");
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
        let hub = a.monitors.clone();
        drop(a);
        tokio::select! {
            () = tokio::time::sleep(times.poll_every) => {}
            () = hub.woken() => {}
        }
    }
}
