//! The monitors this machine sees over its DDC bus.
//!
//! - [`LocalMonitors::scan`] probes each monitor (reads only: `Get 0xEA`) and builds the
//!   channel announcement. One that does not answer is announced as present without a channel
//!   (only its EDID); one that answers without the standard is announced as "not compatible".
//!   Neither is ever opened for writing.
//! - [`LocalMonitors::set_owned`] opens the ones that belong to this machine and drops the rest.
//!   A new owner carries on from the cursor the previous owner last reported
//!   ([`LocalMonitors::hand_over`], T-346); without one it asks for a new cursor: its own may have
//!   gone stale while the other was reading (the firmware's cursor is one per monitor).
//! - [`LocalMonitors::poll`] reads events and state of its own.
//! - [`LocalMonitors::switch`] changes the input and waits for the firmware's verdict.
//! - [`LocalMonitors::read_direct`] and [`LocalMonitors::switch_unowned`]: what a machine with a
//!   channel does on a monitor the other owns while the owner is unreachable (T-346). The
//!   34"'s DP and HDMI share one DDC/CI request buffer, so two hosts talking at once can get
//!   each other's replies: the caller only uses these while the owner cannot be talking to it
//!   through Tandem, and a direct reading counts only if its replies agree.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use monitor_ddc::{
    Clock, DdcBus, DdcError, Event, EventCursor, EventKind, Incompatibility, Input, Link,
    LinkConfig, MonitorHandle, Probe, StandardMonitor, VCP_INPUT, VCP_LIVE, Verdict,
};
use tandem_core::{DisplayKey, MonitorInput};
use tandem_proto::{
    MonitorChannel, MonitorCompat, MonitorCursor, MonitorEvent, MonitorEventKind, MonitorReport,
};

use crate::{ring, to_core, to_ddc};

/// Start of the error for a switch without a verdict: the monitor may or may not have changed,
/// and whoever orchestrates returns to the previous input.
pub const NO_VERDICT: &str = "the monitor gave no verdict";
/// This machine is not the owner of the monitor.
pub const NOT_OWNER: &str = "this machine is not the owner of the monitor";

/// The machine's time, in Unix milliseconds.
pub type WallClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The monitor input through which this EDID arrives, according to the E-EDID 1.4 interface
/// type (byte 20: digital, bits 0–3: `2`/`3` HDMI, `5` DisplayPort). Each input has its own
/// EDID, so it says which input this machine's cable goes to. Before 1.4 those bits say
/// nothing, and many monitors do not fill them in: then `None` (T-111).
pub fn edid_input(raw: &[u8]) -> Option<MonitorInput> {
    let (version, revision, video) = (*raw.get(18)?, *raw.get(19)?, *raw.get(20)?);
    if version != 1 || revision < 4 || video & 0x80 == 0 {
        return None;
    }
    match video & 0x0F {
        0x2 | 0x3 => Some(MonitorInput::Hdmi),
        0x5 => Some(MonitorInput::DisplayPort),
        _ => None,
    }
}

/// Writes into an EDID (128-byte base block) the interface type of `input` as E-EDID
/// 1.4, with its checksum: for test doubles and the simulation. `None` leaves it
/// undefined.
pub fn set_edid_input(edid: &mut [u8], input: Option<MonitorInput>) {
    if edid.len() < 128 {
        return;
    }
    edid[18] = 1;
    edid[19] = 4;
    edid[20] = match input {
        Some(MonitorInput::Hdmi) => 0xA2,
        Some(MonitorInput::DisplayPort) => 0xA5,
        None => 0x80,
    };
    let sum = edid[..127].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    edid[127] = 0u8.wrapping_sub(sum);
}

/// The system's wall clock.
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
    /// How long to wait for the verdict of an input switch.
    pub switch_timeout: Duration,
    /// How many events are kept per monitor for the UI.
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

/// A monitor that this machine reads because it is the owner.
struct Reader {
    monitor: StandardMonitor,
    /// `None` until the first read: the cursor is requested when reading starts, unless the
    /// previous owner handed one over.
    cursor: Option<EventCursor>,
    /// The cursor as of the read that last moved it: what goes in the report. The monitor clock
    /// of later reads without events is left out, so the report only changes with the events.
    reported: Option<MonitorCursor>,
    /// When it was last read (`monitor-ddc` clock).
    read_at: Duration,
    events: VecDeque<MonitorEvent>,
}

struct Entry {
    handle: MonitorHandle,
    key: Option<DisplayKey>,
    channel: MonitorChannel,
    reader: Option<Reader>,
}

pub struct LocalMonitors {
    bus: Arc<dyn DdcBus>,
    clock: Arc<dyn Clock>,
    wall: WallClock,
    cfg: LocalConfig,
    entries: BTreeMap<String, Entry>,
    /// Cursors the previous owners reported, for the next [`LocalMonitors::set_owned`].
    handover: BTreeMap<String, MonitorCursor>,
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
            handover: BTreeMap::new(),
        }
    }

    fn link(&self, handle: MonitorHandle) -> Link {
        Link::new(self.bus.clone(), self.clock.clone(), handle).with_config(self.cfg.link)
    }

    /// The live channels, sorted by id.
    pub fn channels(&self) -> Vec<MonitorChannel> {
        self.entries.values().map(|e| e.channel.clone()).collect()
    }

    /// The EDID identity of each monitor it sees, with or without a channel (T-340).
    pub fn keys(&self) -> BTreeMap<String, DisplayKey> {
        self.entries
            .iter()
            .filter_map(|(id, e)| Some((id.clone(), e.key?)))
            .collect()
    }

    /// The monitors this machine is reading.
    pub fn reading(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|(_, e)| e.reader.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Enumerates and probes again. `Err` if the system does not allow enumerating (no DDC channel).
    pub fn scan(&mut self) -> Result<Vec<MonitorChannel>, DdcError> {
        let infos = self.bus.monitors()?;
        let mut next = BTreeMap::new();
        for info in infos {
            // Without an EDID there is no stable identity: it cannot be known whether it is the same one the other sees.
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
                // The EDID is there even if DDC does not answer (the 34" only answers on the input
                // it shows): it is announced as present, so the desk learns this machine's input
                // and screen. It never owns it and is never opened (T-340).
                Probe::NoChannel(e) => {
                    tracing::debug!(monitor = %id, "no DDC channel: {e}");
                    MonitorCompat::NoChannel
                }
            };
            // Latency is measured when the channel opens and kept while it stays alive: the
            // owner election must not oscillate with every probe. The monitor is recognized
            // by its id, not its number: on Windows and macOS every enumeration gives new
            // numbers. With another number only the connection is redone; the cursor and events carry on.
            let (latency_ms, reader) = match self.entries.remove(&id) {
                Some(prev) if prev.channel.compat == compat => {
                    let reader = match prev.reader {
                        Some(r) if prev.handle != info.handle => self.reconnect(r, &link),
                        same => same,
                    };
                    (prev.channel.latency_ms, reader)
                }
                _ if compat == MonitorCompat::NoChannel => (0, None),
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
                    key: DisplayKey::from_edid(edid.raw()),
                    channel: MonitorChannel {
                        id,
                        name,
                        compat,
                        latency_ms,
                        via: edid_input(edid.raw()),
                    },
                    reader,
                },
            );
        }
        self.entries = next;
        Ok(self.channels())
    }

    /// The same monitor with another number: it is opened again (another probe, reads only) and
    /// what was read is kept. If it can no longer be opened, it is dropped: the next `set_owned`
    /// retries it with a new cursor.
    fn reconnect(&self, reader: Reader, link: &Link) -> Option<Reader> {
        match link.clone().open_standard() {
            Ok(monitor) => Some(Reader { monitor, ..reader }),
            Err(e) => {
                tracing::debug!("could not reopen the monitor: {e}");
                None
            }
        }
    }

    /// The cursors the previous owners last reported (T-346): the next [`Self::set_owned`] that
    /// opens one of these monitors carries on from there, so what happened between the old
    /// owner's last read and now is read too instead of skipped. One not taken by that call is dropped.
    pub fn hand_over(&mut self, cursors: BTreeMap<String, MonitorCursor>) {
        self.handover.extend(cursors);
    }

    /// Reads the monitors in `owned` (the compatible ones it sees) and drops the rest.
    pub fn set_owned(&mut self, owned: &BTreeSet<String>) {
        let mut handover = std::mem::take(&mut self.handover);
        let ids: Vec<String> = self.entries.keys().cloned().collect();
        for id in ids {
            let want = owned.contains(&id);
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            if !want {
                if entry.reader.is_some() {
                    tracing::info!(monitor = %id, "stops reading the monitor: the owner is the other machine");
                }
                if let Some(e) = self.entries.get_mut(&id) {
                    e.reader = None;
                }
                continue;
            }
            if entry.reader.is_some() || !entry.channel.compat.is_compatible() {
                continue;
            }
            // It is only opened after another probe that confirms it: a monitor that stopped speaking
            // the standard never receives a Set.
            match self.link(entry.handle).open_standard() {
                Ok(monitor) => {
                    let reported = handover.remove(&id);
                    tracing::info!(
                        monitor = %id,
                        carried_on = reported.is_some(),
                        "this machine is the owner: starts reading"
                    );
                    let read_at = self.clock.now();
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.reader = Some(Reader {
                            monitor,
                            cursor: reported.map(|c| EventCursor {
                                last: c.last,
                                clock_10ms: c.clock_10ms,
                            }),
                            reported,
                            read_at,
                            events: VecDeque::new(),
                        });
                    }
                }
                Err(e) => tracing::debug!(monitor = %id, "could not open: {e}"),
            }
        }
    }

    /// Reads events and state of each monitor of its own.
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
                    cursor: reader.reported,
                })
            })
            .collect()
    }

    /// Reads the live state of the monitors in `ids` that this machine sees with a compatible
    /// channel but does not read as the owner (T-346). Reads only `Get 0xEE`, `Get 0x60` and
    /// `Get 0xEE` again: never the event registers, whose cursor is global to the monitor and
    /// stays with the owner.
    ///
    /// The 34"'s DP and HDMI share one request buffer, so a reply may be the answer to the other
    /// host's request. Besides the backend's own check (the code echoed in the reply), a reading
    /// counts only if [`live_agrees`]; otherwise it is dropped and the monitor is left out of the
    /// result.
    pub fn read_direct(&self, ids: &BTreeSet<String>) -> Vec<MonitorReport> {
        ids.iter()
            .filter_map(|id| {
                let entry = self.entries.get(id)?;
                if entry.reader.is_some() || !entry.channel.compat.is_compatible() {
                    return None;
                }
                let link = self.link(entry.handle);
                let read = || -> Result<_, DdcError> {
                    Ok((
                        link.get(VCP_LIVE)?,
                        link.get(VCP_INPUT)?,
                        link.get(VCP_LIVE)?,
                    ))
                };
                let (live, input, again) = match read() {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::debug!(monitor = %id, "direct reading failed: {e}");
                        return None;
                    }
                };
                let Some(on_screen) = live_agrees(live, input, again) else {
                    tracing::debug!(
                        monitor = %id,
                        live = ?live,
                        input = ?input,
                        again = ?again,
                        "direct reading dropped: the replies do not agree (another host's reply?)"
                    );
                    return None;
                };
                Some(MonitorReport {
                    id: id.clone(),
                    input: Some(to_core(on_screen)),
                    power_on: Some(live.ch() & 0x01 != 0),
                    signal: Some(live.cl() != 0),
                    events: Vec::new(),
                    cursor: None,
                })
            })
            .collect()
    }

    /// Changes the input of a monitor the other machine owns, while the owner is unreachable
    /// (T-346): it is opened after another probe (reads only), so a monitor that stopped speaking
    /// the standard never receives a Set, and it waits for the firmware's verdict like the owner.
    pub fn switch_unowned(&self, id: &str, input: MonitorInput) -> Result<(), String> {
        let Some(entry) = self.entries.get(id) else {
            return Err(format!("this machine does not see monitor {id}"));
        };
        if !entry.channel.compat.is_compatible() {
            return Err(format!("{id}: this machine has no channel to the monitor"));
        }
        let monitor = self
            .link(entry.handle)
            .open_standard()
            .map_err(|e| format!("{id}: could not open the monitor: {e}"))?;
        tracing::info!(monitor = %id, input = %input, "input switch without being the owner");
        self.verdict(&monitor, input)
    }

    /// Changes the input of a monitor of its own and waits for the firmware's verdict. An error that
    /// starts with [`NO_VERDICT`] leaves doubt about whether it changed.
    pub fn switch(&mut self, id: &str, input: MonitorInput) -> Result<(), String> {
        let Some(entry) = self.entries.get(id) else {
            return Err(format!("this machine does not see monitor {id}"));
        };
        let Some(reader) = &entry.reader else {
            return Err(format!("{id}: {NOT_OWNER}"));
        };
        tracing::info!(monitor = %id, input = %input, "input switch");
        self.verdict(&reader.monitor, input)
    }

    /// Set `0x60` and the firmware's verdict, as the text the orchestrator shows.
    fn verdict(&self, monitor: &StandardMonitor, input: MonitorInput) -> Result<(), String> {
        let timeout = self.cfg.switch_timeout;
        match monitor.switch_input(to_ddc(input), timeout) {
            Ok(Verdict::Confirmed { .. }) => Ok(()),
            Ok(Verdict::Failed { current, .. }) => Err(match current {
                Some(c) => format!(
                    "the monitor showed no image on {input}; it stays on {}",
                    to_core(c)
                ),
                None => format!("the monitor showed no image on {input}"),
            }),
            Ok(Verdict::TimedOut) => Err(format!("{NO_VERDICT} in {} s", timeout.as_secs())),
            Err(e) => Err(format!("could not request the switch over DDC: {e}")),
        }
    }
}

/// Whether three replies read as `0xEE`, `0x60`, `0xEE` all answer what was asked (T-346), and
/// then the input on screen. A reply to another host's request is very unlikely to pass:
/// - both `0xEE` replies are the same (the clock, the cursor and the records the other host reads
///   move between two requests);
/// - `0xEE` says DDC/CI is enabled (power flags bit 6: the monitor is answering it) and names a
///   known port;
/// - `0x60` names the same input (`0x06`/`0x09`, with nothing in the high byte).
///
/// If both `0xEE` replies are real, the input is right whatever `0x60` got; the check only drops.
pub fn live_agrees(
    live: monitor_ddc::VcpReply,
    input: monitor_ddc::VcpReply,
    again: monitor_ddc::VcpReply,
) -> Option<Input> {
    const DDC_ENABLED: u8 = 0x40;
    if live != again || live.ch() & DDC_ENABLED == 0 || input.current > 0xFF {
        return None;
    }
    let on_screen = Input::from_port(live.mh())?;
    (Input::from_vcp(input.current) == Some(on_screen)).then_some(on_screen)
}

/// Reads what is new in the ring. The first time it only takes the cursor: anything from before this
/// machine became the owner is no longer its own.
fn read_events(reader: &mut Reader, clock: &dyn Clock, wall_ms: u64, kept: usize) {
    let now = clock.now();
    let Some(cursor) = reader.cursor.as_mut() else {
        match reader.monitor.cursor() {
            Ok(c) => {
                reader.cursor = Some(c);
                reader.reported = Some(MonitorCursor {
                    last: c.last,
                    clock_10ms: c.clock_10ms,
                });
                reader.read_at = now;
            }
            Err(e) => tracing::debug!("could not take the event cursor: {e}"),
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
        // Without a previous clock, `read_events` does not take it for a restart and goes on from the cursor.
        tracing::info!("the monitor clock wrapped around (497 days)");
        cursor.clock_10ms = None;
    }
    let batch = match reader.monitor.read_events(cursor) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("could not read the events: {e}");
            return;
        }
    };
    reader.read_at = now;
    if reader.reported.is_none_or(|r| r.last != cursor.last) {
        reader.reported = Some(MonitorCursor {
            last: cursor.last,
            clock_10ms: cursor.clock_10ms,
        });
    }
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
        // `old` is the requested input; `new`, the one that stayed.
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

/// Why it is not compatible, for the UI.
fn incompatibility(why: Incompatibility) -> String {
    match why {
        Incompatibility::NoEventRing => "Firmware without the Tandem standard".into(),
        Incompatibility::UnsupportedProtocol(v) => {
            format!("Firmware protocol {v}, which this version of Tandem does not speak")
        }
        Incompatibility::UnknownRing(_) => "Does not speak the own-firmware standard".into(),
    }
}

#[cfg(test)]
mod tests {
    use monitor_ddc::RING_SIZE;
    use monitor_ddc::fake::{
        BusOp, FakeBus, FakeClock, FakeMonitorConfig, Personality, SwitchBehavior, test_edid,
    };
    use std::sync::Mutex;

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
        gone.edid = test_edid("SAM", 1, "Off", "FAKE0000003");
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
            .filter(|c| c.compat != MonitorCompat::NoChannel)
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
        assert_eq!(
            chans.len(),
            3,
            "the silent one is announced by its EDID: {chans:?}"
        );
        let std = chans.iter().find(|c| c.compat.is_compatible()).unwrap();
        assert_eq!(std.name, "LC34G55T");
        assert!(std.id.starts_with("lc34g55t-"), "{}", std.id);
        let fab = chans
            .iter()
            .find(|c| matches!(c.compat, MonitorCompat::NotCompatible { .. }))
            .unwrap();
        assert_eq!(fab.name, "Odyssey G5");
        assert_eq!(
            fab.compat,
            MonitorCompat::NotCompatible {
                reason: "Firmware without the Tandem standard".into()
            }
        );
        // The probe only reads.
        assert!(sets(&r.bus, r.std).is_empty() && sets(&r.bus, r.factory).is_empty());
    }

    /// T-340: the 34" only answers DDC on the input it shows. The machine on the other input
    /// still has its EDID: it announces it as present without a channel, never opens it, never
    /// reads it and never writes to it; and it can tell which of its screens it is.
    #[test]
    fn a_monitor_without_a_channel_is_present_but_never_opened() {
        let mut r = rig();
        let chans = r.local.scan().unwrap();
        let silent = chans
            .iter()
            .find(|c| c.name == "Off")
            .expect("the silent monitor is announced");
        assert_eq!(silent.compat, MonitorCompat::NoChannel);
        assert!(!silent.compat.is_compatible());
        assert_eq!(crate::elect(Some(silent), None), None, "it never owns it");
        let all: BTreeSet<String> = chans.iter().map(|c| c.id.clone()).collect();
        r.local.set_owned(&all);
        assert!(!r.local.reading().contains(&silent.id));
        assert!(r.local.poll().iter().all(|rep| rep.id != silent.id));
        let e = r.local.switch(&silent.id, MonitorInput::Hdmi).unwrap_err();
        assert!(e.contains(NOT_OWNER), "{e}");
        let handle = r
            .bus
            .monitors()
            .unwrap()
            .into_iter()
            .find(|m| m.edid.as_ref().and_then(|e| e.name()) == Some("Off"))
            .unwrap()
            .handle;
        let ops = r.bus.ops_for(handle);
        assert!(
            ops.iter().all(|op| !matches!(op, BusOp::Set { .. })),
            "{ops:?}"
        );
        let keys = r.local.keys();
        assert_eq!(keys.len(), 3, "each one with its EDID identity: {keys:?}");
        assert_eq!(keys[&silent.id].product, 1);
    }

    #[test]
    fn the_latency_is_kept_while_the_channel_lives() {
        let mut r = rig();
        r.local.scan().unwrap();
        let first: Vec<u32> = r.local.channels().iter().map(|c| c.latency_ms).collect();
        r.bus.drop_next(r.std, 1); // one retry: the probe takes longer
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
            "what was there before is not from this owner"
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
        // The time: the switch was confirmed 300 ms after the request.
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
        assert_eq!(e, "the monitor showed no image on HDMI; it stays on DP");
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

    /// Review of T-109: on becoming the owner again a new cursor is requested, so that what the
    /// other machine read in the meantime is not repeated.
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

    /// T-346: a new owner carries on from the cursor the previous owner last reported, so what
    /// happened while nobody read (here, a menu switch) is not lost.
    #[test]
    fn a_handed_over_cursor_carries_on() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        let mine = BTreeSet::from([std.clone()]);
        r.local.set_owned(&mine);
        let cursor = r.local.poll().remove(0).cursor.unwrap();
        r.local.set_owned(&BTreeSet::new());
        r.bus.osd_switch(r.std, Input::Hdmi);
        r.local.hand_over(BTreeMap::from([
            (std.clone(), cursor),
            ("gone".into(), cursor),
        ]));
        r.local.set_owned(&mine);
        let report = r.local.poll().remove(0);
        let inputs: Vec<_> = report
            .events
            .iter()
            .filter(|e| e.kind == MonitorEventKind::Input)
            .map(|e| e.input)
            .collect();
        assert_eq!(inputs, [Some(MonitorInput::Hdmi)], "{:?}", report.events);
        assert!(report.cursor.unwrap().last > cursor.last);
        // A hand-over not taken is dropped: the next owner without one starts afresh.
        assert!(r.local.handover.is_empty());
    }

    /// The reported cursor only moves with the events: the owner does not send its report again on
    /// every read only because the monitor clock moved.
    #[test]
    fn the_reported_cursor_moves_only_with_the_events() {
        let mut r = rig();
        r.local.scan().unwrap();
        r.local.set_owned(&BTreeSet::from([id_of(&r, true)]));
        let first = r.local.poll();
        r.clock.advance(Duration::from_secs(3));
        assert_eq!(r.local.poll(), first);
        r.bus.osd_switch(r.std, Input::Hdmi);
        let moved = r.local.poll();
        assert_ne!(moved[0].cursor, first[0].cursor);
        assert!(moved[0].cursor.unwrap().clock_10ms > first[0].cursor.unwrap().clock_10ms);
    }

    /// T-346: a direct reading asks only `0xEE` and `0x60`, never an event register nor a Set, and
    /// never touches what this machine does not see as compatible or already reads as the owner.
    #[test]
    fn a_direct_reading_asks_only_the_live_state() {
        let mut r = rig();
        r.local.scan().unwrap();
        let (std, fab) = (id_of(&r, true), id_of(&r, false));
        r.bus.osd_switch(r.std, Input::Hdmi);
        r.bus.clear_ops();
        let read = r
            .local
            .read_direct(&BTreeSet::from([std.clone(), fab, "unknown".into()]));
        assert_eq!(read.len(), 1, "{read:?}");
        assert_eq!(read[0].id, std);
        assert_eq!(read[0].input, Some(MonitorInput::Hdmi));
        assert_eq!((read[0].power_on, read[0].signal), (Some(true), Some(true)));
        assert!(read[0].events.is_empty() && read[0].cursor.is_none());
        let codes: Vec<_> = r
            .bus
            .ops()
            .into_iter()
            .map(|op| match op {
                BusOp::Get { code, .. } => code,
                other => panic!("only Gets: {other:?}"),
            })
            .collect();
        assert_eq!(codes, [VCP_LIVE, VCP_INPUT, VCP_LIVE]);
        assert!(r.bus.ops_for(r.factory).is_empty());
        // Owned here, it is read by `poll`, not directly.
        r.local.set_owned(&BTreeSet::from([std.clone()]));
        assert!(r.local.read_direct(&BTreeSet::from([std])).is_empty());
    }

    /// The 34"'s DP and HDMI share one request buffer (monitor-firmware, inactive-hdmi-ddc.md): a
    /// reply may answer the other host's request. Such a reading is dropped, never shown.
    #[test]
    fn a_direct_reading_with_a_crossed_reply_is_dropped() {
        let clock = FakeClock::new();
        let fake = FakeBus::new(clock.clone());
        let h = fake.add(FakeMonitorConfig::new(Personality::Standard));
        let crossed = Crossed {
            inner: fake.clone(),
            plan: Mutex::new(VecDeque::new()),
        };
        let crossed = Arc::new(crossed);
        let mut local = LocalMonitors::new(
            crossed.clone(),
            Arc::new(clock),
            system_wall(),
            LocalConfig::default(),
        );
        let id = local.scan().unwrap()[0].id.clone();
        let ids = BTreeSet::from([id]);
        // The replies to 0xEE, 0x60, 0xEE: `None` is the honest one, `Some(c)` the other host's `c`.
        for plan in [
            [None, Some(VCP_LIVE), None],
            [Some(0xEB), None, Some(0xEB)],
            [None, None, Some(0xEF)],
            [Some(0xEF), None, None],
        ] {
            crossed.plan(&plan);
            assert!(local.read_direct(&ids).is_empty(), "{plan:?}");
        }
        // With honest replies it reads again; the monitor never got a Set.
        assert_eq!(
            local.read_direct(&ids)[0].input,
            Some(MonitorInput::DisplayPort)
        );
        assert!(
            !fake
                .ops_for(h)
                .iter()
                .any(|op| matches!(op, BusOp::Set { .. }))
        );
    }

    /// T-346: with the owner unreachable, a machine with a channel switches by itself, with the
    /// firmware's verdict; one it does not see as compatible is never written to.
    #[test]
    fn a_switch_without_being_the_owner_waits_for_the_verdict() {
        let mut r = rig();
        r.local.scan().unwrap();
        let (std, fab) = (id_of(&r, true), id_of(&r, false));
        r.local.switch_unowned(&std, MonitorInput::Hdmi).unwrap();
        assert_eq!(r.bus.screen_input(r.std), Some(Input::Hdmi));
        assert!(r.local.reading().is_empty(), "it does not become the owner");
        let e = r
            .local
            .switch_unowned(&fab, MonitorInput::Hdmi)
            .unwrap_err();
        assert!(e.contains("no channel"), "{e}");
        assert!(sets(&r.bus, r.factory).is_empty());
        assert!(
            r.local
                .switch_unowned("unknown", MonitorInput::Hdmi)
                .is_err()
        );
    }

    proptest::proptest! {
        /// Whatever three replies arrive, an accepted reading names the input both `0xEE` replies
        /// and `0x60` agree on.
        #[test]
        fn an_accepted_direct_reading_is_consistent(
            live in proptest::prelude::any::<[u8; 4]>(),
            input in proptest::prelude::any::<u16>(),
            again in proptest::prelude::any::<[u8; 4]>(),
        ) {
            let r = |b: [u8; 4]| monitor_ddc::VcpReply::from_bytes(b[0], b[1], b[2], b[3]);
            let (l, i, a) = (r(live), monitor_ddc::VcpReply { max: 0, current: input }, r(again));
            if let Some(on_screen) = live_agrees(l, i, a) {
                proptest::prop_assert_eq!(l, a);
                proptest::prop_assert_eq!(Input::from_port(l.mh()), Some(on_screen));
                proptest::prop_assert_eq!(Input::from_vcp(input), Some(on_screen));
                proptest::prop_assert!(l.ch() & 0x40 != 0);
            }
        }
    }

    /// A bus whose next reply to a code is the one to another code: what the firmware's shared
    /// request buffer does when two hosts ask at once.
    struct Crossed {
        inner: FakeBus,
        plan: Mutex<VecDeque<Option<u8>>>,
    }

    impl Crossed {
        fn plan(&self, replies: &[Option<u8>]) {
            *self.plan.lock().unwrap() = replies.iter().copied().collect();
        }
    }

    impl DdcBus for Crossed {
        fn monitors(&self) -> Result<Vec<monitor_ddc::MonitorInfo>, DdcError> {
            self.inner.monitors()
        }
        fn get_vcp(&self, h: MonitorHandle, code: u8) -> Result<monitor_ddc::VcpReply, DdcError> {
            let answered = self
                .plan
                .lock()
                .unwrap()
                .pop_front()
                .flatten()
                .unwrap_or(code);
            self.inner.get_vcp(h, answered)
        }
        fn set_vcp(&self, h: MonitorHandle, code: u8, value: u16) -> Result<(), DdcError> {
            self.inner.set_vcp(h, code, value)
        }
        fn capabilities(&self, h: MonitorHandle) -> Result<String, DdcError> {
            self.inner.capabilities(h)
        }
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
        // The monitor clock must have advanced to notice that it went back.
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

    /// Review of T-109: the `0xEF` clock wraps around at ~497 days. It is not a restart:
    /// the ring's events are not repeated.
    #[test]
    fn the_clock_wrap_does_not_replay_the_ring() {
        let mut r = rig();
        r.local.scan().unwrap();
        let std = id_of(&r, true);
        r.local.set_owned(&BTreeSet::from([std]));
        r.local.poll();
        // Some old events in the ring.
        for to in [Input::Hdmi, Input::DisplayPort, Input::Hdmi] {
            r.bus.osd_switch(r.std, to);
        }
        r.local.poll();
        // One minute before the clock's limit (2^32 × 10 ms).
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
            "nothing new on the second read"
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
        assert_eq!(inputs, 4, "no repeated events: {:?}", report.events);
    }

    /// Like `WindowsBus` and `MacBus`: every enumeration gives new numbers to the same monitors.
    struct Renumbering {
        inner: FakeBus,
        map: std::sync::Mutex<BTreeMap<MonitorHandle, MonitorHandle>>,
        next: std::sync::atomic::AtomicU64,
    }

    impl Renumbering {
        fn new(inner: FakeBus) -> Self {
            Renumbering {
                inner,
                map: std::sync::Mutex::new(BTreeMap::new()),
                next: std::sync::atomic::AtomicU64::new(1000),
            }
        }

        fn real(&self, h: MonitorHandle) -> Result<MonitorHandle, DdcError> {
            self.map
                .lock()
                .unwrap()
                .get(&h)
                .copied()
                .ok_or(DdcError::UnknownMonitor(h))
        }
    }

    impl DdcBus for Renumbering {
        fn monitors(&self) -> Result<Vec<monitor_ddc::MonitorInfo>, DdcError> {
            let mut map = self.map.lock().unwrap();
            map.clear();
            Ok(self
                .inner
                .monitors()?
                .into_iter()
                .map(|mut m| {
                    let outer =
                        MonitorHandle(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
                    map.insert(outer, m.handle);
                    m.handle = outer;
                    m
                })
                .collect())
        }
        fn get_vcp(&self, h: MonitorHandle, code: u8) -> Result<monitor_ddc::VcpReply, DdcError> {
            self.inner.get_vcp(self.real(h)?, code)
        }
        fn set_vcp(&self, h: MonitorHandle, code: u8, value: u16) -> Result<(), DdcError> {
            self.inner.set_vcp(self.real(h)?, code, value)
        }
        fn capabilities(&self, h: MonitorHandle) -> Result<String, DdcError> {
            self.inner.capabilities(self.real(h)?)
        }
    }

    /// Review of #80: on Windows and macOS the number of each monitor changes on every
    /// enumeration. The probe must not throw away the latency, the cursor or the events.
    #[test]
    fn a_rescan_with_new_handles_keeps_latency_cursor_and_events() {
        let clock = FakeClock::new();
        let fake = FakeBus::new(clock.clone());
        let h = fake.add(FakeMonitorConfig::new(Personality::Standard));
        let mut local = LocalMonitors::new(
            Arc::new(Renumbering::new(fake.clone())),
            Arc::new(clock.clone()),
            system_wall(),
            LocalConfig::default(),
        );
        local.scan().unwrap();
        let before = local.channels();
        let id = before[0].id.clone();
        local.set_owned(&BTreeSet::from([id.clone()]));
        local.poll();
        fake.osd_switch(h, Input::Hdmi);
        assert_eq!(local.poll()[0].events.len(), 1);
        fake.osd_switch(h, Input::DisplayPort);
        // A retry in the probe: if it were measured again, the latency would change.
        fake.drop_next(h, 1);
        assert_eq!(local.scan().unwrap(), before, "same latency");
        local.set_owned(&BTreeSet::from([id.clone()]));
        let report = local.poll().remove(0);
        let inputs: Vec<_> = report.events.iter().map(|e| e.input).collect();
        assert_eq!(
            inputs,
            [Some(MonitorInput::Hdmi), Some(MonitorInput::DisplayPort)],
            "{:?}",
            report.events
        );
        // And it is still controllable with the new number.
        local.switch(&id, MonitorInput::Hdmi).unwrap();
        assert_eq!(fake.screen_input(h), Some(Input::Hdmi));
    }

    #[test]
    fn without_a_bus_the_scan_fails() {
        struct NoBus;
        impl DdcBus for NoBus {
            fn monitors(&self) -> Result<Vec<monitor_ddc::MonitorInfo>, DdcError> {
                Err(DdcError::NoChannel("no backend".into()))
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

    #[test]
    fn the_edid_says_which_input_the_cable_goes_to() {
        let mut e = test_edid("SAM", 0x7155, "LC34G55T", "X1");
        assert_eq!(edid_input(&e), None, "no interface type");
        for input in [
            Some(MonitorInput::Hdmi),
            Some(MonitorInput::DisplayPort),
            None,
        ] {
            set_edid_input(&mut e, input);
            assert_eq!(edid_input(&e), input);
            assert!(
                monitor_ddc::Edid::parse(&e).is_ok(),
                "the checksum is still right"
            );
        }
        // Before E-EDID 1.4 the bits say nothing; an analog one, neither.
        set_edid_input(&mut e, Some(MonitorInput::Hdmi));
        e[19] = 3;
        assert_eq!(edid_input(&e), None);
        e[19] = 4;
        e[20] &= 0x7F;
        assert_eq!(edid_input(&e), None);
        assert_eq!(edid_input(&e[..10]), None);
        // The probe announces it on the channel.
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let mut config = FakeMonitorConfig::new(Personality::Standard);
        set_edid_input(&mut config.edid, Some(MonitorInput::DisplayPort));
        bus.add(config);
        let mut local = LocalMonitors::new(
            Arc::new(bus),
            Arc::new(clock),
            system_wall(),
            LocalConfig::default(),
        );
        let channels = local.scan().unwrap();
        assert_eq!(channels[0].via, Some(MonitorInput::DisplayPort));
    }

    /// `test_edid` does not reproduce the real serial: the identity is the same on DP and HDMI because
    /// it does not depend on the product code.
    #[test]
    fn the_id_does_not_depend_on_the_input() {
        let dp = monitor_ddc::Edid::parse(&test_edid("SAM", 0x7155, "LC34G55T", "X1")).unwrap();
        let hdmi = monitor_ddc::Edid::parse(&test_edid("SAM", 0x7156, "LC34G55T", "X1")).unwrap();
        assert_eq!(dp.monitor_id(), hdmi.monitor_id());
    }
}
