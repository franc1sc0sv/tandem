//! A shared monitor's link state (T-349): which machine it shows, whether each machine's cable is
//! connected and whether it is on. Pure: the agent feeds it what the systems list, what the owner
//! (or the direct T-346 reading) read and the desk's cables, and acts on the result (geometry,
//! scenes, the input selector and the UI).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::Machine;
use crate::monitor::{Inputs, MonitorInput};

/// One machine's cable to a monitor. Kept apart from the DDC channel: a cable whose monitor does
/// not answer DDC is still `Connected`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CableState {
    /// The machine's system lists the monitor's EDID.
    Connected,
    /// The machine is reachable, has just rescanned and does not list it.
    Disconnected,
    /// The machine is unreachable, has not scanned yet, or it cannot tell (see [`own_cable`]).
    #[default]
    Unknown,
}

/// Whether the monitor is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    /// On, with a signal on the input it shows.
    On,
    /// On without a signal on the active input, or in its power-saving state.
    Standby,
    /// A probed channel stopped replying for longer than [`OFF_AFTER`] while a cable still lists
    /// the monitor.
    Off,
    #[default]
    Unknown,
}

/// The one state a monitor is in, for the UI and for acting on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// It shows this machine's picture.
    ShowsMachine(Machine),
    Standby,
    Off,
    /// The input on screen leads to this machine, whose cable is disconnected: a black screen.
    ActiveInputUnplugged(Machine),
    #[default]
    Unknown,
}

/// Each machine's cable to one monitor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cables {
    pub a: CableState,
    pub b: CableState,
}

impl Cables {
    pub fn of(&self, machine: Machine) -> CableState {
        match machine {
            Machine::A => self.a,
            Machine::B => self.b,
        }
    }

    pub fn set(&mut self, machine: Machine, state: CableState) {
        match machine {
            Machine::A => self.a = state,
            Machine::B => self.b = state,
        }
    }

    /// Some machine lists the monitor.
    pub fn any_connected(&self) -> bool {
        self.a == CableState::Connected || self.b == CableState::Connected
    }

    pub fn swapped(self) -> Self {
        Cables {
            a: self.b,
            b: self.a,
        }
    }
}

/// A monitor's derived link: the state, its power and each end's cable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonitorLink {
    pub state: LinkState,
    pub power: PowerState,
    pub cables: Cables,
}

impl MonitorLink {
    /// The link from its parts ([`derive`]).
    pub fn new(
        inputs: Inputs,
        input: Option<MonitorInput>,
        power: PowerState,
        cables: Cables,
    ) -> Self {
        MonitorLink {
            state: derive(inputs, input, power, cables),
            power,
            cables,
        }
    }

    /// Whether `machine`'s screen of this monitor belongs in its input geometry. Only a monitor
    /// that is not known to be away from `machine` stays: one that is off, in standby, unplugged
    /// or showing the other machine leaves, so the cursor never goes into a screen nobody sees.
    /// An unknown state keeps it (DESIGN §3.5: an unknown state is not guessed).
    pub fn live_for(&self, machine: Machine) -> bool {
        matches!(self.state, LinkState::Unknown) || self.state == LinkState::ShowsMachine(machine)
    }

    /// Why switching this monitor to `machine`'s input would leave a black screen: that machine's
    /// cable is disconnected. `Unknown` never refuses (T-349, scope item 1).
    pub fn refuses_switch_to(&self, machine: Machine) -> bool {
        self.cables.of(machine) == CableState::Disconnected
    }

    pub fn swapped(self) -> Self {
        let state = match self.state {
            LinkState::ShowsMachine(m) => LinkState::ShowsMachine(m.other()),
            LinkState::ActiveInputUnplugged(m) => LinkState::ActiveInputUnplugged(m.other()),
            s => s,
        };
        MonitorLink {
            state,
            power: self.power,
            cables: self.cables.swapped(),
        }
    }
}

/// The state from its parts. `inputs` says which input each machine's cable goes to, `input` is
/// the input on screen (`None` when nobody could read it) and `power` comes from a
/// [`PowerTracker`]. An unknown input never gives [`LinkState::ShowsMachine`].
///
/// Order: off wins (whatever was read last is stale); then an input that leads to an unplugged
/// cable (the monitor is also without signal, but the cause is what the user must see); then
/// standby; then the machine shown.
pub fn derive(
    inputs: Inputs,
    input: Option<MonitorInput>,
    power: PowerState,
    cables: Cables,
) -> LinkState {
    if power == PowerState::Off {
        return LinkState::Off;
    }
    let shown = input.and_then(|i| inputs.machine_on(i));
    if let Some(m) = shown
        && cables.of(m) == CableState::Disconnected
    {
        return LinkState::ActiveInputUnplugged(m);
    }
    if power == PowerState::Standby {
        return LinkState::Standby;
    }
    match shown {
        Some(m) => LinkState::ShowsMachine(m),
        None => LinkState::Unknown,
    }
}

/// The state of this machine's own end. `listed`: its system lists the monitor's EDID after a
/// rescan; `shows`: which machine the monitor shows, if known.
///
/// Safety rule (T-349): only the Mac has been shown to keep listing the monitor while it shows
/// the other input. Until hardware confirms both sides, a machine that does not list a monitor
/// that shows the other machine says `Unknown`, never `Disconnected`, so a switch back to it is
/// never refused because of it.
pub fn own_cable(listed: bool, shows: Option<Machine>, me: Machine) -> CableState {
    if listed {
        CableState::Connected
    } else if shows == Some(me.other()) {
        CableState::Unknown
    } else {
        CableState::Disconnected
    }
}

/// What a live reading says about power, if it says anything: `on` with power and signal,
/// `standby` without power or without signal on the active input.
pub fn live_power(power_on: Option<bool>, signal: Option<bool>) -> Option<PowerState> {
    match (power_on?, signal) {
        (false, _) | (true, Some(false)) => Some(PowerState::Standby),
        (true, _) => Some(PowerState::On),
    }
}

/// How long a channel that replied has to stay silent before the monitor counts as off. A single
/// failed read is not "off": the owner polls every second and probes every 5 s.
pub const OFF_AFTER: Duration = Duration::from_secs(10);

/// One turn's observation of a monitor's power.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PowerSample {
    /// What the live reading this turn said ([`live_power`]); `None` without a reading.
    pub live: Option<PowerState>,
    /// The machine the input on screen leads to has a probed channel (or the input is unknown).
    /// A reading through such a channel arms the "off" detection; a monitor that moved to an
    /// input whose machine has no channel goes quiet for that reason, not because it is off.
    pub reader_on_input: bool,
    /// Some machine's cable still lists the monitor.
    pub listed: bool,
}

/// Debounced power of one monitor: `off` takes [`OFF_AFTER`] of silence from a channel that
/// replied, and a monitor that never replied over a probed channel (all its ends `NoChannel`) is
/// `unknown`, never `off`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PowerTracker {
    state: PowerState,
    /// A probed channel on the shown input replied: its silence may mean "off".
    armed: bool,
    /// Since when (caller's clock) no reading arrived.
    silent_since: Option<Duration>,
}

impl PowerTracker {
    pub fn new() -> Self {
        PowerTracker::default()
    }

    pub fn state(&self) -> PowerState {
        self.state
    }

    /// The power after `sample`, seen at `now` (any monotonic clock, the same on every call).
    pub fn observe(&mut self, now: Duration, sample: PowerSample) -> PowerState {
        if let Some(live) = sample.live {
            self.state = live;
            self.armed = sample.reader_on_input;
            self.silent_since = None;
            return self.state;
        }
        if !self.armed {
            self.state = PowerState::Unknown;
            self.silent_since = None;
            return self.state;
        }
        let since = *self.silent_since.get_or_insert(now);
        if now.saturating_sub(since) >= OFF_AFTER {
            self.state = if sample.listed {
                PowerState::Off
            } else {
                PowerState::Unknown
            };
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const DP: MonitorInput = MonitorInput::DisplayPort;
    const HDMI: MonitorInput = MonitorInput::Hdmi;

    /// A on DP, B on HDMI.
    fn wired() -> Inputs {
        Inputs {
            a: Some(DP),
            b: Some(HDMI),
        }
    }

    fn cables(a: CableState, b: CableState) -> Cables {
        Cables { a, b }
    }

    const C: CableState = CableState::Connected;
    const D: CableState = CableState::Disconnected;
    const U: CableState = CableState::Unknown;

    #[test]
    fn each_state_from_its_parts() {
        let on = PowerState::On;
        assert_eq!(
            derive(wired(), Some(DP), on, cables(C, C)),
            LinkState::ShowsMachine(Machine::A)
        );
        assert_eq!(
            derive(wired(), Some(HDMI), on, cables(C, U)),
            LinkState::ShowsMachine(Machine::B)
        );
        assert_eq!(
            derive(wired(), Some(HDMI), PowerState::Standby, cables(C, D)),
            LinkState::ActiveInputUnplugged(Machine::B)
        );
        assert_eq!(
            derive(wired(), Some(DP), PowerState::Standby, cables(C, D)),
            LinkState::Standby
        );
        assert_eq!(
            derive(wired(), Some(DP), PowerState::Off, cables(C, C)),
            LinkState::Off
        );
        assert_eq!(derive(wired(), None, on, cables(C, C)), LinkState::Unknown);
        // The input is read, but nobody knows whose it is.
        assert_eq!(
            derive(Inputs::default(), Some(DP), on, cables(C, C)),
            LinkState::Unknown
        );
        // Power unknown with a known input: what was read is shown.
        assert_eq!(
            derive(wired(), Some(DP), PowerState::Unknown, cables(C, C)),
            LinkState::ShowsMachine(Machine::A)
        );
    }

    #[test]
    fn own_cable_never_says_disconnected_for_a_monitor_showing_the_other() {
        let me = Machine::B;
        assert_eq!(own_cable(true, Some(Machine::A), me), C);
        assert_eq!(own_cable(false, Some(Machine::A), me), U);
        assert_eq!(own_cable(false, Some(me), me), D);
        assert_eq!(own_cable(false, None, me), D);
    }

    #[test]
    fn live_power_from_the_reading() {
        assert_eq!(live_power(Some(true), Some(true)), Some(PowerState::On));
        assert_eq!(live_power(Some(true), None), Some(PowerState::On));
        assert_eq!(
            live_power(Some(true), Some(false)),
            Some(PowerState::Standby)
        );
        assert_eq!(
            live_power(Some(false), Some(true)),
            Some(PowerState::Standby)
        );
        assert_eq!(live_power(None, Some(true)), None);
    }

    fn on() -> PowerSample {
        PowerSample {
            live: Some(PowerState::On),
            reader_on_input: true,
            listed: true,
        }
    }

    fn silent() -> PowerSample {
        PowerSample {
            live: None,
            reader_on_input: true,
            listed: true,
        }
    }

    #[test]
    fn a_single_failed_read_is_not_off_it_takes_the_timeout() {
        let mut t = PowerTracker::new();
        let s = Duration::from_secs;
        assert_eq!(t.observe(s(0), on()), PowerState::On);
        assert_eq!(t.observe(s(1), silent()), PowerState::On);
        assert_eq!(t.observe(s(2), on()), PowerState::On, "it answered again");
        assert_eq!(t.observe(s(3), silent()), PowerState::On);
        assert_eq!(t.observe(s(12), silent()), PowerState::On, "9 s of silence");
        assert_eq!(t.observe(s(13), silent()), PowerState::Off);
        assert_eq!(t.observe(s(14), on()), PowerState::On, "back on");
    }

    #[test]
    fn only_no_channel_ends_stay_unknown_never_off() {
        let mut t = PowerTracker::new();
        let quiet = PowerSample {
            live: None,
            reader_on_input: false,
            listed: true,
        };
        for secs in [0, 5, 30, 600] {
            assert_eq!(
                t.observe(Duration::from_secs(secs), quiet),
                PowerState::Unknown
            );
        }
    }

    #[test]
    fn a_monitor_moved_to_an_input_without_a_channel_is_not_off() {
        let mut t = PowerTracker::new();
        t.observe(Duration::ZERO, on());
        // The confirmed switch: the new input's machine has no channel.
        let moved = PowerSample {
            reader_on_input: false,
            ..on()
        };
        t.observe(Duration::from_secs(1), moved);
        assert_eq!(
            t.observe(Duration::from_secs(60), silent()),
            PowerState::Unknown
        );
    }

    #[test]
    fn silence_without_any_cable_listing_it_is_unknown() {
        let mut t = PowerTracker::new();
        t.observe(Duration::ZERO, on());
        let gone = PowerSample {
            listed: false,
            ..silent()
        };
        assert_eq!(t.observe(Duration::from_secs(20), gone), PowerState::On);
        assert_eq!(
            t.observe(Duration::from_secs(40), gone),
            PowerState::Unknown
        );
    }

    #[test]
    fn geometry_and_switch_rules() {
        let link = |state, a, b| MonitorLink {
            state,
            power: PowerState::On,
            cables: cables(a, b),
        };
        let shows_a = link(LinkState::ShowsMachine(Machine::A), C, C);
        assert!(shows_a.live_for(Machine::A));
        assert!(!shows_a.live_for(Machine::B));
        for away in [
            LinkState::Off,
            LinkState::Standby,
            LinkState::ActiveInputUnplugged(Machine::A),
            LinkState::ActiveInputUnplugged(Machine::B),
        ] {
            let l = link(away, C, C);
            assert!(
                !l.live_for(Machine::A) && !l.live_for(Machine::B),
                "{away:?}"
            );
        }
        assert!(link(LinkState::Unknown, U, U).live_for(Machine::A));
        assert!(link(LinkState::Unknown, C, D).refuses_switch_to(Machine::B));
        assert!(!link(LinkState::Unknown, C, U).refuses_switch_to(Machine::B));
    }

    #[test]
    fn swapping_sides_keeps_the_meaning() {
        let l = MonitorLink::new(wired(), Some(HDMI), PowerState::On, cables(C, U));
        let s = l.swapped();
        assert_eq!(s.state, LinkState::ShowsMachine(Machine::A));
        assert_eq!(s.cables, cables(U, C));
        assert_eq!(s.swapped(), l);
    }

    #[test]
    fn the_wire_names_are_stable() {
        let l = MonitorLink {
            state: LinkState::ActiveInputUnplugged(Machine::A),
            power: PowerState::Standby,
            cables: cables(D, C),
        };
        assert_eq!(
            serde_json::to_value(l).unwrap(),
            serde_json::json!({
                "state": {"active_input_unplugged": "a"},
                "power": "standby",
                "cables": {"a": "disconnected", "b": "connected"},
            })
        );
        assert_eq!(
            serde_json::to_value(LinkState::Unknown).unwrap(),
            serde_json::json!("unknown")
        );
    }

    fn any_machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::A), Just(Machine::B)]
    }

    fn any_cable() -> impl Strategy<Value = CableState> {
        prop_oneof![Just(C), Just(D), Just(U)]
    }

    fn any_power() -> impl Strategy<Value = PowerState> {
        prop_oneof![
            Just(PowerState::On),
            Just(PowerState::Standby),
            Just(PowerState::Off),
            Just(PowerState::Unknown),
        ]
    }

    fn any_input() -> impl Strategy<Value = Option<MonitorInput>> {
        prop_oneof![Just(None), Just(Some(DP)), Just(Some(HDMI))]
    }

    fn any_inputs() -> impl Strategy<Value = Inputs> {
        (any_input(), any_input()).prop_map(|(a, b)| {
            Inputs::resolve(None, |m| match m {
                Machine::A => a,
                Machine::B => b,
            })
        })
    }

    proptest! {
        /// Every combination gives exactly one state, consistent with its parts.
        #[test]
        fn every_combination_maps_to_one_consistent_state(
            inputs in any_inputs(), input in any_input(), power in any_power(),
            a in any_cable(), b in any_cable(),
        ) {
            let cables = cables(a, b);
            let state = derive(inputs, input, power, cables);
            let shown = input.and_then(|i| inputs.machine_on(i));
            match state {
                LinkState::ShowsMachine(m) => {
                    prop_assert!(input.is_some(), "an unknown input never shows a machine");
                    prop_assert_eq!(shown, Some(m));
                    prop_assert!(matches!(power, PowerState::On | PowerState::Unknown));
                    prop_assert_ne!(cables.of(m), D);
                }
                LinkState::ActiveInputUnplugged(m) => {
                    prop_assert_eq!(shown, Some(m));
                    prop_assert_eq!(cables.of(m), D);
                    prop_assert_ne!(power, PowerState::Off);
                }
                LinkState::Off => prop_assert_eq!(power, PowerState::Off),
                LinkState::Standby => prop_assert_eq!(power, PowerState::Standby),
                LinkState::Unknown => {
                    prop_assert!(shown.is_none());
                    prop_assert!(matches!(power, PowerState::On | PowerState::Unknown));
                }
            }
            prop_assert_eq!(input.is_none() && power != PowerState::Off && power != PowerState::Standby,
                input.is_none() && state == LinkState::Unknown);
        }

        /// The geometry keeps a screen only if its monitor shows that machine or is unknown, and a
        /// switch is refused only for a disconnected cable, never an unknown one.
        #[test]
        fn acting_on_the_state_is_safe(
            inputs in any_inputs(), input in any_input(), power in any_power(),
            a in any_cable(), b in any_cable(), m in any_machine(),
        ) {
            let link = MonitorLink::new(inputs, input, power, cables(a, b));
            if link.live_for(m) {
                prop_assert!(matches!(link.state, LinkState::Unknown) || link.state == LinkState::ShowsMachine(m));
            }
            if matches!(power, PowerState::Off | PowerState::Standby) {
                prop_assert!(!link.live_for(m));
            }
            prop_assert_eq!(link.refuses_switch_to(m), link.cables.of(m) == D);
            prop_assert_eq!(link.swapped().swapped(), link);
        }

        /// The safety rule: a machine that stops listing a monitor showing the other machine
        /// never reports its cable as disconnected.
        #[test]
        fn own_cable_is_safe(listed in any::<bool>(), shows in proptest::option::of(any_machine()), me in any_machine()) {
            let c = own_cable(listed, shows, me);
            prop_assert_eq!(c == C, listed);
            if shows == Some(me.other()) {
                prop_assert_ne!(c, D);
            }
        }

        /// Off only after `OFF_AFTER` of silence from an armed channel, with a cable listing it.
        #[test]
        fn off_needs_the_full_timeout(
            steps in proptest::collection::vec((0u64..4000, proptest::option::of(any::<bool>()), any::<bool>(), any::<bool>()), 1..40),
        ) {
            let mut t = PowerTracker::new();
            let mut now = Duration::ZERO;
            let mut last_live: Option<Duration> = None;
            let mut armed = false;
            for (dt, live, reader, listed) in steps {
                now += Duration::from_millis(dt);
                let live = live.map(|on| if on { PowerState::On } else { PowerState::Standby });
                let state = t.observe(now, PowerSample { live, reader_on_input: reader, listed });
                if live.is_some() {
                    last_live = Some(now);
                    armed = reader;
                    prop_assert_eq!(Some(state), live);
                }
                if state == PowerState::Off {
                    prop_assert!(armed && listed);
                    let since = last_live.unwrap_or(Duration::ZERO);
                    prop_assert!(now.saturating_sub(since) >= OFF_AFTER);
                }
                if !armed {
                    prop_assert_ne!(state, PowerState::Off);
                }
            }
        }
    }
}
