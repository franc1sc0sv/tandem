//! El puesto físico simulado: la energía de cada máquina y el hub USB.
//!
//! Las dos máquinas son iguales ante la energía (DESIGN §3.7): cualquiera puede estar
//! apagada (o suspendida) mientras la otra trabaja sola.
//!
//! Es la verdad contra la que se compara el [`State`] de los agentes: ellos solo
//! saben lo que creen haber hecho.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, State, Step};
use tokio::sync::watch;

/// Energía de una máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Off,
    /// Llegó el Wake-on-LAN y está arrancando.
    Booting,
    On,
}

/// Un dispositivo USB detrás del hub (DESIGN §7: el servidor es donde está el hub).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: String,
}

/// Lo observable del puesto, para comparar antes y después de un fallo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Physical {
    pub power: BTreeMap<Machine, Power>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) power: BTreeMap<Machine, Power>,
    /// Avisa cada cambio de `power`: la simulación apaga y prende lo que corre en cada
    /// máquina (la vigilancia del par) como lo haría el sistema.
    pub(crate) power_tx: watch::Sender<BTreeMap<Machine, Power>>,
    pub(crate) hub: Machine,
    pub(crate) usb: Vec<UsbDevice>,
    /// Cada paso pedido a cada máquina, en orden, se haya aplicado o no.
    pub(crate) calls: Vec<(Machine, Step)>,
    /// Cosas que no pueden pasar en el puesto real (un paso en una máquina apagada...).
    pub(crate) violations: Vec<String>,
}

/// El puesto físico, compartido por los backends simulados de las dos máquinas.
#[derive(Debug, Clone)]
pub struct World(Arc<Mutex<Inner>>);

impl World {
    /// La Mac prendida, la PC apagada y el hub en la Mac.
    pub fn new() -> Self {
        Self::with_principal(Machine::A)
    }

    /// `principal` prendida, la otra apagada y el hub en `principal`.
    pub fn with_principal(principal: Machine) -> Self {
        let power = BTreeMap::from([(principal, Power::On), (principal.other(), Power::Off)]);
        World(Arc::new(Mutex::new(Inner {
            power_tx: watch::Sender::new(power.clone()),
            power,
            hub: principal,
            usb: vec![
                UsbDevice {
                    vid: 0x046D,
                    pid: 0xC548,
                    name: "receptor del teclado y mouse".into(),
                },
                UsbDevice {
                    vid: 0x2109,
                    pid: 0x2817,
                    name: "hub USB-C".into(),
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

    /// La energía de las dos máquinas, con aviso en cada cambio.
    pub fn subscribe_power(&self) -> watch::Receiver<BTreeMap<Machine, Power>> {
        self.lock().power_tx.subscribe()
    }

    pub fn physical(&self) -> Physical {
        Physical {
            power: self.lock().power.clone(),
        }
    }

    /// Pasos pedidos a cada máquina, en orden (aplicados o no).
    pub fn calls(&self) -> Vec<(Machine, Step)> {
        self.lock().calls.clone()
    }

    pub fn violations(&self) -> Vec<String> {
        self.lock().violations.clone()
    }

    /// Conecta el hub USB-C a otra máquina.
    pub fn plug_hub(&self, machine: Machine) {
        self.lock().hub = machine;
    }

    /// Dispositivos USB que ve una máquina: todos los del hub, o ninguno.
    pub fn usb_devices(&self, machine: Machine) -> Vec<UsbDevice> {
        let w = self.lock();
        let visible = w.hub == machine && w.power_of(machine) == Power::On;
        if visible { w.usb.clone() } else { Vec::new() }
    }

    /// Compara el puesto con lo que cree un agente. Devuelve cada diferencia.
    pub fn check(&self, state: &State) -> Result<(), Vec<String>> {
        let w = self.lock();
        let mut errors = w.violations.clone();
        for machine in [Machine::A, Machine::B] {
            let on = w.power_of(machine) == Power::On;
            if on != state.is_on(machine) {
                errors.push(format!(
                    "la máquina {machine} está {:?} pero el agente la cree {}",
                    w.power_of(machine),
                    if state.is_on(machine) {
                        "prendida"
                    } else {
                        "apagada"
                    }
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

    /// `machine` termina de arrancar (o de despertarse).
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
        assert!(errors[0].contains("la máquina B está On"), "{errors:?}");
        state.on.b = true;
        w.check(&state).unwrap();
        // Lo imposible también se informa.
        w.lock().violations.push("algo imposible".into());
        assert!(w.check(&state).is_err());
    }

    #[test]
    fn the_hub_devices_follow_the_hub() {
        let w = World::new();
        assert!(!w.usb_devices(Machine::A).is_empty());
        w.plug_hub(Machine::B);
        assert!(w.usb_devices(Machine::A).is_empty());
        // Con la PC apagada nadie ve el teclado.
        assert!(w.usb_devices(Machine::B).is_empty());
        w.lock().boot_done(Machine::B);
        assert!(!w.usb_devices(Machine::B).is_empty());
    }

    #[test]
    fn power_changes_are_announced() {
        let w = World::with_principal(Machine::B);
        let mut rx = w.subscribe_power();
        assert_eq!(w.power_of(Machine::A), Power::Off);
        w.lock().boot_done(Machine::A);
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update()[&Machine::A], Power::On);
        w.lock().power_off(Machine::B);
        assert_eq!(w.physical().power[&Machine::B], Power::Off);
    }
}
