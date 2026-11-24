//! The simulated physical desk: each machine's power and the USB hub.
//!
//! The two machines are equal with respect to power (DESIGN §3.7): either can be
//! off (or suspended) while the other works alone.
//!
//! It is the truth against which the agents' [`State`] is compared: they only
//! know what they believe they did.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, State, Step};
use tokio::sync::watch;

/// A machine's power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Off,
    /// Wake-on-LAN arrived and it is starting up.
    Booting,
    On,
}

/// A USB device behind the hub (DESIGN §7: the server is where the hub is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: String,
}

/// What is observable on the desk, to compare before and after a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Physical {
    pub power: BTreeMap<Machine, Power>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) power: BTreeMap<Machine, Power>,
    /// Notifies every change of `power`: the simulation turns off and on what runs on each
    /// machine (the pair watch) as the system would.
    pub(crate) power_tx: watch::Sender<BTreeMap<Machine, Power>>,
    pub(crate) hub: Machine,
    pub(crate) usb: Vec<UsbDevice>,
    /// Each step requested of each machine, in order, whether or not it was applied.
    pub(crate) calls: Vec<(Machine, Step)>,
    /// Things that cannot happen on the real desk (a step on an off machine...).
    pub(crate) violations: Vec<String>,
}

/// The physical desk, shared by the simulated backends of both machines.
#[derive(Debug, Clone)]
pub struct World(Arc<Mutex<Inner>>);

impl World {
    /// The Mac on, the PC off and the hub on the Mac.
    pub fn new() -> Self {
        Self::with_primary(Machine::A)
    }

    /// `primary` on, the other off and the hub on `primary`.
    pub fn with_primary(primary: Machine) -> Self {
        let power = BTreeMap::from([(primary, Power::On), (primary.other(), Power::Off)]);
        World(Arc::new(Mutex::new(Inner {
            power_tx: watch::Sender::new(power.clone()),
            power,
            hub: primary,
            usb: vec![
                UsbDevice {
                    vid: 0x046D,
                    pid: 0xC548,
                    name: "keyboard and mouse receiver".into(),
                },
                UsbDevice {
                    vid: 0x2109,
                    pid: 0x2817,
                    name: "USB-C hub".into(),
                },
            ],
            calls: Vec::new(),
            violations: Vec::new(),
        })))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn power_of(&self, machine: Machine) -> Power {
        self.lock().power_of(machine)
    }

    /// The power of both machines, with a notice on each change.
    pub fn subscribe_power(&self) -> watch::Receiver<BTreeMap<Machine, Power>> {
        self.lock().power_tx.subscribe()
    }

    pub fn physical(&self) -> Physical {
        Physical {
            power: self.lock().power.clone(),
        }
    }

    /// Steps requested of each machine, in order (applied or not).
    pub fn calls(&self) -> Vec<(Machine, Step)> {
        self.lock().calls.clone()
    }

    pub fn violations(&self) -> Vec<String> {
        self.lock().violations.clone()
    }

    /// Connects the USB-C hub to another machine.
    pub fn plug_hub(&self, machine: Machine) {
        self.lock().hub = machine;
    }

    /// USB devices a machine sees: all those of the hub, or none.
    pub fn usb_devices(&self, machine: Machine) -> Vec<UsbDevice> {
        let w = self.lock();
        let visible = w.hub == machine && w.power_of(machine) == Power::On;
        if visible { w.usb.clone() } else { Vec::new() }
    }

    /// Compares the desk with what an agent believes. Returns each difference.
    pub fn check(&self, state: &State) -> Result<(), Vec<String>> {
        let w = self.lock();
        let mut errors = w.violations.clone();
        for machine in [Machine::A, Machine::B] {
            let on = w.power_of(machine) == Power::On;
            if on != state.is_on(machine) {
                errors.push(format!(
                    "machine {machine} is {:?} but the agent believes it {}",
                    w.power_of(machine),
                    if state.is_on(machine) { "on" } else { "off" }
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    pub(crate) fn set_power(&mut self, machine: Machine, power: Power) {
        self.power.insert(machine, power);
        self.power_tx.send_replace(self.power.clone());
    }

    pub(crate) fn power_of(&self, machine: Machine) -> Power {
        self.power.get(&machine).copied().unwrap_or(Power::Off)
    }

    /// `machine` finishes starting up (or waking).
    pub(crate) fn boot_done(&mut self, machine: Machine) {
        self.set_power(machine, Power::On);
    }

    pub(crate) fn power_off(&mut self, machine: Machine) {
        self.set_power(machine, Power::Off);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_compares_who_is_on() {
        let w = World::new();
        let mut state = State::initial(Machine::A);
        w.check(&state).unwrap();
        w.lock().boot_done(Machine::B);
        let errors = w.check(&state).unwrap_err();
        assert!(errors[0].contains("machine B is On"), "{errors:?}");
        state.on.b = true;
        w.check(&state).unwrap();
        // The impossible is also reported.
        w.lock().violations.push("something impossible".into());
        assert!(w.check(&state).is_err());
    }

    #[test]
    fn the_hub_devices_follow_the_hub() {
        let w = World::new();
        assert!(!w.usb_devices(Machine::A).is_empty());
        w.plug_hub(Machine::B);
        assert!(w.usb_devices(Machine::A).is_empty());
        // With the PC off nobody sees the keyboard.
        assert!(w.usb_devices(Machine::B).is_empty());
        w.lock().boot_done(Machine::B);
        assert!(!w.usb_devices(Machine::B).is_empty());
    }

    #[test]
    fn power_changes_are_announced() {
        let w = World::with_primary(Machine::B);
        let mut rx = w.subscribe_power();
        assert_eq!(w.power_of(Machine::A), Power::Off);
        w.lock().boot_done(Machine::A);
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update()[&Machine::A], Power::On);
        w.lock().power_off(Machine::B);
        assert_eq!(w.physical().power[&Machine::B], Power::Off);
    }
}
