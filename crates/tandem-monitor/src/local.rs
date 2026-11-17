//! Los monitores que ve esta máquina por su bus DDC.
//!
//! - [`LocalMonitors::scan`] sondea cada monitor (solo lecturas: `Get 0xEA`) y arma el anuncio
//!   de canales. Uno que no contesta no se anuncia; uno que contesta sin el estándar se anuncia
//!   como «no compatible» y nunca se abre para escribirle.
//! - [`LocalMonitors::set_owned`] abre los que le tocan a esta máquina y suelta los demás.
//!   Cada vez que una máquina pasa a ser dueña pide un cursor nuevo: el anterior pudo quedar
//!   viejo mientras leía la otra (el cursor del firmware es uno solo por monitor).
//! - [`LocalMonitors::poll`] lee eventos y estado de los propios.
//! - [`LocalMonitors::switch`] cambia la entrada y espera el veredicto del firmware.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use monitor_ddc::{
    Clock, DdcBus, DdcError, Event, EventCursor, EventKind, Incompatibility, Input, Link,
    LinkConfig, MonitorHandle, Probe, StandardMonitor, Verdict,
};
use tandem_core::MonitorInput;
use tandem_proto::{MonitorChannel, MonitorCompat, MonitorEvent, MonitorEventKind, MonitorReport};

use crate::{ring, to_core, to_ddc};

/// Principio del error de un cambio sin veredicto: el monitor pudo cambiar o no, y quien
/// orquesta vuelve a la entrada de antes.
pub const NO_VERDICT: &str = "el monitor no dio veredicto";
/// Esta máquina no es la dueña del monitor.
pub const NOT_OWNER: &str = "esta máquina no es la dueña del monitor";

/// La hora de la máquina, en milisegundos Unix.
pub type WallClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// El reloj de pared del sistema.
pub fn system_wall() -> WallClock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalConfig {
    pub link: LinkConfig,
    /// Cuánto se espera el veredicto de un cambio de entrada.
    pub switch_timeout: Duration,
    /// Cuántos eventos se guardan por monitor para la UI.
    pub events_kept: usize,
}

impl Default for LocalConfig {
    fn default() -> Self {
        LocalConfig {
            link: LinkConfig::default(),
            switch_timeout: Duration::from_secs(12),
            events_kept: 20,
        }
    }
}

/// Un monitor que esta máquina lee porque es la dueña.
struct Reader {
    monitor: StandardMonitor,
    /// `None` hasta la primera lectura: el cursor se pide al empezar a leer.
    cursor: Option<EventCursor>,
    /// Cuándo se leyó por última vez (reloj de `monitor-ddc`).
    read_at: Duration,
    events: VecDeque<MonitorEvent>,
}

struct Entry {
    handle: MonitorHandle,
    channel: MonitorChannel,
    reader: Option<Reader>,
}

pub struct LocalMonitors {
    bus: Arc<dyn DdcBus>,
    clock: Arc<dyn Clock>,
    wall: WallClock,
    cfg: LocalConfig,
    entries: BTreeMap<String, Entry>,
}

impl std::fmt::Debug for LocalMonitors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMonitors")
            .field("monitors", &self.entries.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl LocalMonitors {
    pub fn new(
        bus: Arc<dyn DdcBus>,
        clock: Arc<dyn Clock>,
        wall: WallClock,
        cfg: LocalConfig,
    ) -> Self {
        LocalMonitors {
            bus,
            clock,
            wall,
            cfg,
            entries: BTreeMap::new(),
        }
    }

    fn link(&self, handle: MonitorHandle) -> Link {
        Link::new(self.bus.clone(), self.clock.clone(), handle).with_config(self.cfg.link)
    }

    /// Los canales vivos, ordenados por id.
    pub fn channels(&self) -> Vec<MonitorChannel> {
        self.entries.values().map(|e| e.channel.clone()).collect()
    }

    /// Los monitores que esta máquina está leyendo.
    pub fn reading(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|(_, e)| e.reader.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Vuelve a enumerar y sondear. `Err` si el sistema no deja enumerar (sin canal DDC).
    pub fn scan(&mut self) -> Result<Vec<MonitorChannel>, DdcError> {
        let infos = self.bus.monitors()?;
        let mut next = BTreeMap::new();
        for info in infos {
            // Sin EDID no hay identidad estable: no se puede saber si es el mismo que ve la otra.
            let Some(edid) = &info.edid else { continue };
            let id = edid.monitor_id().to_string();
            if next.contains_key(&id) {
                continue;
            }
            let link = self.link(info.handle);
            let started = self.clock.now();
            let probe = link.probe();
            let took = self.clock.now().saturating_sub(started);
            let compat = match probe {
                Probe::Compatible(ring) => MonitorCompat::Compatible {
                    protocol: ring.protocol,
                },
                Probe::NotCompatible(why) => MonitorCompat::NotCompatible {
                    reason: incompatibility(why),
                },
                Probe::NoChannel(e) => {
                    tracing::debug!(monitor = %id, "sin canal DDC: {e}");
                    continue;
                }
            };
            // La latencia se mide al abrir el canal y se conserva mientras siga vivo: la
            // elección del dueño no tiene que oscilar con cada sondeo.
            let (latency_ms, reader) = match self.entries.remove(&id) {
                Some(prev) if prev.handle == info.handle && prev.channel.compat == compat => {
                    (prev.channel.latency_ms, prev.reader)
                }
                _ => (u32::try_from(took.as_millis()).unwrap_or(u32::MAX), None),
            };
            let name = edid
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| info.description.clone());
            next.insert(
                id.clone(),
                Entry {
                    handle: info.handle,
                    channel: MonitorChannel {
                        id,
                        name,
                        compat,
                        latency_ms,
                    },
                    reader,
                },
            );
        }
        self.entries = next;
        Ok(self.channels())
    }

    /// Lee los monitores de `owned` (los compatibles que ve) y suelta los demás.
    pub fn set_owned(&mut self, owned: &BTreeSet<String>) {
        let ids: Vec<String> = self.entries.keys().cloned().collect();
        for id in ids {
            let want = owned.contains(&id);
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            if !want {
                if entry.reader.is_some() {
                    tracing::info!(monitor = %id, "deja de leer el monitor: el dueño es la otra");
                }
                if let Some(e) = self.entries.get_mut(&id) {
                    e.reader = None;
                }
                continue;
            }
            if entry.reader.is_some() || !entry.channel.compat.is_compatible() {
                continue;
            }
            // Solo se abre tras otro sondeo que lo confirme: un monitor que dejó de hablar
            // el estándar nunca recibe un Set.
            match self.link(entry.handle).open_standard() {
                Ok(monitor) => {
                    tracing::info!(monitor = %id, "esta máquina es la dueña: empieza a leer");
                    let read_at = self.clock.now();
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.reader = Some(Reader {
                            monitor,
                            cursor: None,
                            read_at,
                            events: VecDeque::new(),
                        });
                    }
                }
                Err(e) => tracing::debug!(monitor = %id, "no se pudo abrir: {e}"),
            }
        }
    }

    /// Lee eventos y estado de cada monitor propio.
    pub fn poll(&mut self) -> Vec<MonitorReport> {
        let (wall, clock, kept) = (self.wall.clone(), self.clock.clone(), self.cfg.events_kept);
        self.entries
            .iter_mut()
            .filter_map(|(id, entry)| {
                let reader = entry.reader.as_mut()?;
                read_events(reader, clock.as_ref(), wall(), kept);
                let live = reader.monitor.live().ok();
                Some(MonitorReport {
                    id: id.clone(),
                    input: live.and_then(|l| l.input()).map(to_core),
                    power_on: live.map(|l| l.power_on()),
                    signal: live.map(|l| l.signal),
                    events: reader.events.iter().cloned().collect(),
                })
            })
            .collect()
    }

    /// Cambia la entrada de un monitor propio y espera el veredicto del firmware. Un error que
    /// empieza con [`NO_VERDICT`] deja la duda de si cambió.
    pub fn switch(&mut self, id: &str, input: MonitorInput) -> Result<(), String> {
        let Some(entry) = self.entries.get(id) else {
            return Err(format!("esta máquina no ve el monitor {id}"));
        };
        let Some(reader) = &entry.reader else {
            return Err(format!("{id}: {NOT_OWNER}"));
        };
        let timeout = self.cfg.switch_timeout;
        tracing::info!(monitor = %id, entrada = %input, "cambio de entrada");
        match reader.monitor.switch_input(to_ddc(input), timeout) {
            Ok(Verdict::Confirmed { .. }) => Ok(()),
            Ok(Verdict::Failed { current, .. }) => Err(match current {
                Some(c) => format!(
                    "el monitor no mostró imagen en {input}; sigue en {}",
                    to_core(c)
                ),
                None => format!("el monitor no mostró imagen en {input}"),
            }),
            Ok(Verdict::TimedOut) => Err(format!("{NO_VERDICT} en {} s", timeout.as_secs())),
            Err(e) => Err(format!("no se pudo pedir el cambio por DDC: {e}")),
        }
    }
}

/// Lee lo nuevo del anillo. La primera vez solo toma el cursor: lo anterior a que esta
/// máquina fuera la dueña ya no es suyo.
fn read_events(reader: &mut Reader, clock: &dyn Clock, wall_ms: u64, kept: usize) {
    let now = clock.now();
    let Some(cursor) = reader.cursor.as_mut() else {
        match reader.monitor.cursor() {
            Ok(c) => {
                reader.cursor = Some(c);
                reader.read_at = now;
            }
            Err(e) => tracing::debug!("no se pudo tomar el cursor de eventos: {e}"),
        }
        return;
    };
    let elapsed_10ms =
        u64::try_from(now.saturating_sub(reader.read_at).as_millis() / 10).unwrap_or(u64::MAX);
    if let Some(before) = cursor.clock_10ms
        && ring::may_wrap(before, elapsed_10ms)
        && let Ok(clock_now) = reader.monitor.clock_10ms()
        && ring::wrapped(before, clock_now, elapsed_10ms)
    {
        // Sin reloj anterior, `read_events` no lo toma por un reinicio y sigue desde el cursor.
        tracing::info!("el reloj del monitor dio la vuelta (497 días)");
        cursor.clock_10ms = None;
    }
    let batch = match reader.monitor.read_events(cursor) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("no se pudieron leer los eventos: {e}");
            return;
        }
    };
    reader.read_at = now;
    let clock_10ms = cursor.clock_10ms.unwrap_or(0);
    if batch.lost > 0 {
        reader.events.push_back(MonitorEvent {
            at_ms: wall_ms,
            kind: MonitorEventKind::Lost,
            input: None,
        });
    }
    for e in &batch.events {
        reader.events.push_back(event(e, wall_ms, clock_10ms));
    }
    while reader.events.len() > kept {
        reader.events.pop_front();
    }
}

fn event(e: &Event, wall_ms: u64, clock_10ms: u32) -> MonitorEvent {
    let port = |p: u8| Input::from_port(p).map(to_core);
    let (kind, input) = match e.kind() {
        EventKind::Boot => (MonitorEventKind::Boot, None),
        EventKind::InputPort => (MonitorEventKind::Input, port(e.new)),
        EventKind::DdcSwitchRequest => (
            MonitorEventKind::SwitchRequested,
            Input::from_vcp(u16::from(e.new)).map(to_core),
        ),
        EventKind::SwitchDone => (MonitorEventKind::SwitchDone, port(e.new)),
        // `old` es la entrada pedida; `new`, la que quedó.
        EventKind::SwitchFailed => (MonitorEventKind::SwitchFailed, port(e.old)),
        EventKind::PowerFlags | EventKind::PowersaveFlags | EventKind::DdcPowerRequest => {
            (MonitorEventKind::Power, None)
        }
        EventKind::Signal | EventKind::SyncFlags => (MonitorEventKind::Signal, None),
        EventKind::SettingChanged => (MonitorEventKind::Setting, None),
        EventKind::SourceSelect
        | EventKind::SystemState
        | EventKind::WindowPort
        | EventKind::Unknown(_) => (MonitorEventKind::Other, None),
    };
    MonitorEvent {
        at_ms: ring::event_at_ms(wall_ms, clock_10ms, e.time_10ms),
        kind,
        input,
    }
}

/// Por qué no es compatible, para la UI.
fn incompatibility(why: Incompatibility) -> String {
    match why {
        Incompatibility::NoEventRing => "Firmware sin el estándar de Tandem".into(),
        Incompatibility::UnsupportedProtocol(v) => {
            format!("Protocolo {v} del firmware, que esta versión de Tandem no habla")
        }
        Incompatibility::UnknownRing(_) => "No habla el estándar del firmware propio".into(),
    }
}

#[cfg(test)]
mod tests {
    use monitor_ddc::RING_SIZE;
    use monitor_ddc::fake::{
        BusOp, FakeBus, FakeClock, FakeMonitorConfig, Personality, SwitchBehavior, test_edid,
    };

    use super::*;

    struct Rig {
        bus: FakeBus,
        clock: FakeClock,
        std: MonitorHandle,
        factory: MonitorHandle,
        local: LocalMonitors,
    }

    fn factory_config() -> FakeMonitorConfig {
        let mut f = FakeMonitorConfig::new(Personality::Factory);
        f.edid = test_edid("SAM", 0x7154, "Odyssey G5", "FAKE0000002");
        f.description = "fake Odyssey".into();
        f
    }

    fn rig() -> Rig {
        rig_with(LocalConfig::default())
    }

    fn rig_with(cfg: LocalConfig) -> Rig {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let std = bus.add(FakeMonitorConfig::new(Personality::Standard));
        let factory = bus.add(factory_config());
        let mut gone = FakeMonitorConfig::new(Personality::Disconnected);
        gone.edid = test_edid("SAM", 1, "Apagado", "FAKE0000003");
        bus.add(gone);
        let wall_clock = clock.clone();
        let local = LocalMonitors::new(
            Arc::new(bus.clone()),
            Arc::new(clock.clone()),
            Arc::new(move || 1_700_000_000_000 + wall_clock.now().as_millis() as u64),
            cfg,
        );
        Rig {
            bus,
            clock,
            std,
            factory,
            local,
        }
    }

    fn id_of(r: &Rig, compatible: bool) -> String {
        r.local
            .channels()
            .into_iter()
            .find(|c| c.compat.is_compatible() == compatible)
            .map(|c| c.id)
            .unwrap()
    }

    fn sets(bus: &FakeBus, h: MonitorHandle) -> Vec<BusOp> {
        bus.ops_for(h)
            .into_iter()
            .filter(|op| matches!(op, BusOp::Set { .. }))
            .collect()
    }

    #[test]
    fn the_scan_announces_live_channels_with_their_compatibility() {
        let mut r = rig();
        let chans = r.local.scan().unwrap();
        assert_eq!(chans.len(), 2, "el desconectado no se anuncia: {chans:?}");
        let std = chans.iter().find(|c| c.compat.is_compatible()).unwrap();
        assert_eq!(std.name, "LC34G55T");
        assert!(std.id.starts_with("lc34g55t-"), "{}", std.id);
        let fab = chans.iter().find(|c| !c.compat.is_compatible()).unwrap();
        assert_eq!(fab.name, "Odyssey G5");
        assert_eq!(
            fab.compat,
            MonitorCompat::NotCompatible {
                reason: "Firmware sin el estándar de Tandem".into()
            }
        );
        // El sondeo solo lee.
        assert!(sets(&r.bus, r.std).is_empty() && sets(&r.bus, r.factory).is_empty());
    }

    #[test]
    fn the_latency_is_kept_while_the_channel_lives() {
        let mut r = rig();
        r.local.scan().unwrap();
        let first: Vec<u32> = r.local.channels().iter().map(|c| c.latency_ms).collect();
        r.bus.drop_next(r.std, 1); // un reintento: el sondeo tarda más
        r.local.scan().unwrap();
        let again: Vec<u32> = r.local.channels().iter().map(|c| c.latency_ms).collect();
        assert_eq!(first, again);
    }

    #[test]
    fn the_factory_monitor_never_gets_a_set() {
        let mut r = rig();
        r.local.scan().unwrap();
        let all: BTreeSet<String> = r.local.channels().into_iter().map(|c| c.id).collect();
        r.local.set_owned(&all);
        let fab = id_of(&r, false);
        assert!(!r.local.reading().contains(&fab));
        let e = r.local.switch(&fab, MonitorInput::Hdmi).unwrap_err();
        assert!(e.contains(NOT_OWNER), "{e}");
        for _ in 0..3 {
            r.local.poll();
        }
        assert!(sets(&r.bus, r.factory).is_empty());
    }

    #[test]
    fn the_owner_switches_with_the_firmware_verdict_and_reads_the_events() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        r.local.set_owned(&BTreeSet::from([std.clone()]));
        let first = r.local.poll();
        assert_eq!(first[0].input, Some(MonitorInput::DisplayPort));
        assert!(
            first[0].events.is_empty(),
            "lo de antes no es de este dueño"
        );
        r.local.switch(&std, MonitorInput::Hdmi).unwrap();
        assert_eq!(r.bus.screen_input(r.std), Some(Input::Hdmi));
        let report = r.local.poll().remove(0);
        assert_eq!(report.input, Some(MonitorInput::Hdmi));
        let kinds: Vec<_> = report.events.iter().map(|e| (e.kind, e.input)).collect();
        assert!(
            kinds.contains(&(MonitorEventKind::SwitchRequested, Some(MonitorInput::Hdmi))),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&(MonitorEventKind::SwitchDone, Some(MonitorInput::Hdmi))),
            "{kinds:?}"
        );
        // La hora: el cambio se confirmó 300 ms después del pedido.
        let at = |k| report.events.iter().find(|e| e.kind == k).unwrap().at_ms;
        assert_eq!(
            at(MonitorEventKind::SwitchDone) - at(MonitorEventKind::SwitchRequested),
            300
        );
    }

    #[test]
    fn a_failed_switch_says_where_the_monitor_stayed() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let mut cfg = FakeMonitorConfig::new(Personality::Standard);
        cfg.switch = SwitchBehavior::Fail;
        bus.add(cfg);
        let mut local = LocalMonitors::new(
            Arc::new(bus.clone()),
            Arc::new(clock.clone()),
            system_wall(),
            LocalConfig::default(),
        );
        local.scan().unwrap();
        let id = local.channels()[0].id.clone();
        local.set_owned(&BTreeSet::from([id.clone()]));
        let e = local.switch(&id, MonitorInput::Hdmi).unwrap_err();
        assert_eq!(e, "el monitor no mostró imagen en HDMI; sigue en DP");
    }

    #[test]
    fn a_silent_switch_is_reported_without_verdict() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let mut cfg = FakeMonitorConfig::new(Personality::Standard);
        cfg.switch = SwitchBehavior::Silent;
        bus.add(cfg);
        let mut local = LocalMonitors::new(
            Arc::new(bus.clone()),
            Arc::new(clock.clone()),
            system_wall(),
            LocalConfig::default(),
        );
        local.scan().unwrap();
        let id = local.channels()[0].id.clone();
        local.set_owned(&BTreeSet::from([id.clone()]));
        let e = local.switch(&id, MonitorInput::Hdmi).unwrap_err();
        assert!(e.starts_with(NO_VERDICT), "{e}");
    }

    /// Revisión de T-109: al volver a ser dueña se pide un cursor nuevo, así no se repite lo
    /// que leyó la otra máquina mientras tanto.
    #[test]
    fn a_new_owner_starts_with_a_fresh_cursor() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        let mine = BTreeSet::from([std.clone()]);
        r.local.set_owned(&mine);
        r.local.poll();
        r.local.set_owned(&BTreeSet::new());
        assert!(r.local.poll().is_empty());
        r.bus.osd_switch(r.std, Input::Hdmi);
        r.local.set_owned(&mine);
        let report = r.local.poll().remove(0);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert_eq!(report.input, Some(MonitorInput::Hdmi));
        r.bus.osd_switch(r.std, Input::DisplayPort);
        let report = r.local.poll().remove(0);
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].kind, MonitorEventKind::Input);
    }

    #[test]
    fn a_reboot_and_an_overflow_are_told() {
        let mut r = rig_with(LocalConfig {
            events_kept: 1000,
            ..LocalConfig::default()
        });
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        r.local.set_owned(&BTreeSet::from([std]));
        r.local.poll();
        // El reloj del monitor tiene que haber avanzado para notar que volvió atrás.
        r.clock.advance(Duration::from_secs(5));
        r.local.poll();
        r.bus.reboot(r.std);
        let report = r.local.poll().remove(0);
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].kind, MonitorEventKind::Boot);
        for i in 0..u32::from(RING_SIZE) + 5 {
            let to = if i % 2 == 0 {
                Input::Hdmi
            } else {
                Input::DisplayPort
            };
            r.bus.osd_switch(r.std, to);
        }
        let report = r.local.poll().remove(0);
        let kinds: Vec<_> = report.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds[1], MonitorEventKind::Lost, "{kinds:?}");
        assert_eq!(kinds.len(), 2 + usize::from(RING_SIZE));
    }

    #[test]
    fn only_the_last_events_are_kept() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        r.local.set_owned(&BTreeSet::from([std]));
        r.local.poll();
        for _ in 0..30 {
            r.bus.osd_switch(r.std, Input::Hdmi);
        }
        let report = r.local.poll().remove(0);
        assert_eq!(report.events.len(), LocalConfig::default().events_kept);
    }

    /// Revisión de T-109: el reloj `0xEF` da la vuelta a los ~497 días. No es un reinicio:
    /// no se repiten los eventos del anillo.
    #[test]
    fn the_clock_wrap_does_not_replay_the_ring() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        r.local.set_owned(&BTreeSet::from([std]));
        r.local.poll();
        // Unos eventos viejos en el anillo.
        for to in [Input::Hdmi, Input::DisplayPort, Input::Hdmi] {
            r.bus.osd_switch(r.std, to);
        }
        r.local.poll();
        // Un minuto antes del tope del reloj (2^32 × 10 ms).
        let top = Duration::from_millis((1u64 << 32) * 10);
        let now = r.clock.now();
        r.clock.advance(top - now - Duration::from_secs(60));
        r.local.poll();
        r.clock.advance(Duration::from_secs(90));
        r.bus.osd_switch(r.std, Input::DisplayPort);
        let before = r.local.poll().remove(0).events.len();
        let report = r.local.poll().remove(0);
        assert_eq!(
            report.events.len(),
            before,
            "nada nuevo en la segunda lectura"
        );
        let last = report.events.last().unwrap();
        assert_eq!(
            (last.kind, last.input),
            (MonitorEventKind::Input, Some(MonitorInput::DisplayPort))
        );
        let inputs = report
            .events
            .iter()
            .filter(|e| e.kind == MonitorEventKind::Input)
            .count();
        assert_eq!(inputs, 4, "sin eventos repetidos: {:?}", report.events);
    }

    #[test]
    fn without_a_bus_the_scan_fails() {
        struct NoBus;
        impl DdcBus for NoBus {
            fn monitors(&self) -> Result<Vec<monitor_ddc::MonitorInfo>, DdcError> {
                Err(DdcError::NoChannel("sin backend".into()))
            }
            fn get_vcp(
                &self,
                _: MonitorHandle,
                code: u8,
            ) -> Result<monitor_ddc::VcpReply, DdcError> {
                Err(DdcError::NoReply { code })
            }
            fn set_vcp(&self, _: MonitorHandle, code: u8, _: u16) -> Result<(), DdcError> {
                Err(DdcError::NoReply { code })
            }
            fn capabilities(&self, _: MonitorHandle) -> Result<String, DdcError> {
                Err(DdcError::NoReply { code: 0xF3 })
            }
        }
        let mut local = LocalMonitors::new(
            Arc::new(NoBus),
            Arc::new(FakeClock::new()),
            system_wall(),
            LocalConfig::default(),
        );
        assert!(local.scan().is_err());
        assert!(local.channels().is_empty());
    }

    /// `test_edid` no reproduce el serial real: la identidad es la misma en DP y HDMI porque
    /// no depende del código de producto.
    #[test]
    fn the_id_does_not_depend_on_the_input() {
        let dp = monitor_ddc::Edid::parse(&test_edid("SAM", 0x7155, "LC34G55T", "X1")).unwrap();
        let hdmi = monitor_ddc::Edid::parse(&test_edid("SAM", 0x7156, "LC34G55T", "X1")).unwrap();
        assert_eq!(dp.monitor_id(), hdmi.monitor_id());
    }
}
