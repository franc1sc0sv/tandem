//! El puesto físico simulado: lo que de verdad muestran los monitores, qué tiene
//! cada máquina en su escritorio, la pantalla integrada, la energía de la PC, la
//! geometría de cada motor de entrada y el hub USB.
//!
//! Es la verdad contra la que se compara el [`State`] de los agentes: ellos solo
//! saben lo que creen haber hecho.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{BuiltinDisplay, Config, Input, Machine, MonitorId, State, Step};

/// Lo que el 34" (LC34G55T) contesta siempre al leer `0x60`, esté en la entrada
/// que esté (docs/research/ddc-34-LC34G55T.md).
pub const LC34G55T_READS: u8 = 0x09;

/// Códigos MCCS para los monitores sin perfil propio.
const MCCS_HDMI: u8 = 0x11;
const MCCS_DP: u8 = 0x0F;

/// Un monitor físico con su comportamiento DDC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimMonitor {
    pub input: Input,
    /// Códigos de `0x60` que obedece y a qué entrada lo llevan; los demás se ignoran
    /// sin error, como hacen los Samsung.
    pub accepts: BTreeMap<u8, Input>,
    /// Si la lectura de `0x60` miente siempre con el mismo valor.
    pub reads_as: Option<u8>,
    /// Último código escrito, para la lectura de los monitores honestos.
    pub last_code: Option<u8>,
}

impl SimMonitor {
    /// El monitor tal como sale de la configuración, mostrando HDMI.
    fn from_model(model: Option<&str>) -> Self {
        let is_34 = model.is_some_and(|m| m.to_ascii_uppercase().contains("LC34G55T"));
        let accepts = if is_34 {
            BTreeMap::from([
                (0x03, Input::Hdmi),
                (0x06, Input::Hdmi),
                (0x09, Input::DisplayPort),
                (0x0F, Input::DisplayPort),
            ])
        } else {
            BTreeMap::from([(MCCS_HDMI, Input::Hdmi), (MCCS_DP, Input::DisplayPort)])
        };
        SimMonitor {
            input: Input::Hdmi,
            accepts,
            reads_as: is_34.then_some(LC34G55T_READS),
            last_code: None,
        }
    }
}

/// Energía de la PC.
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

/// Lo observable del puesto, para comparar antes y después de una vuelta atrás.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Physical {
    pub inputs: BTreeMap<MonitorId, Input>,
    pub desktops: BTreeMap<Machine, BTreeSet<MonitorId>>,
    pub builtin: BuiltinDisplay,
    pub power: Power,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) cfg: Config,
    pub(crate) monitors: BTreeMap<MonitorId, SimMonitor>,
    pub(crate) desktops: BTreeMap<Machine, BTreeSet<MonitorId>>,
    pub(crate) builtin: BuiltinDisplay,
    pub(crate) power: Power,
    /// Pantallas que cada motor de entrada usó la última vez que recalculó.
    pub(crate) geometry: BTreeMap<Machine, Vec<Screen>>,
    pub(crate) hub: Machine,
    pub(crate) usb: Vec<UsbDevice>,
    /// Cada paso pedido a cada máquina, en orden, se haya aplicado o no.
    pub(crate) calls: Vec<(Machine, Step)>,
    /// Escrituras DDC: (monitor, código).
    pub(crate) ddc_writes: Vec<(MonitorId, u8)>,
    /// Cosas que no pueden pasar en el puesto real (un paso en la PC apagada...).
    pub(crate) violations: Vec<String>,
}

/// Una pantalla para el motor de entrada.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Screen {
    Monitor(MonitorId),
    Builtin,
}

/// El puesto físico, compartido por los backends simulados de las dos máquinas.
#[derive(Debug, Clone)]
pub struct World(Arc<Mutex<Inner>>);

impl World {
    /// Todo en HDMI (la Mac), la PC apagada y el hub en la Mac.
    pub fn new(cfg: &Config) -> Self {
        let monitors = cfg
            .monitors
            .iter()
            .map(|m| {
                (
                    m.id.clone(),
                    SimMonitor::from_model(m.edid_model.as_deref()),
                )
            })
            .collect();
        let mac_desktop = cfg.monitors.iter().map(|m| m.id.clone()).collect();
        let world = World(Arc::new(Mutex::new(Inner {
            cfg: cfg.clone(),
            monitors,
            desktops: BTreeMap::from([(Machine::Mac, mac_desktop), (Machine::Pc, BTreeSet::new())]),
            builtin: BuiltinDisplay::Off,
            power: Power::Off,
            geometry: BTreeMap::new(),
            hub: Machine::Mac,
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
            ddc_writes: Vec::new(),
            violations: Vec::new(),
        })));
        // El agente de la Mac calcula su geometría al arrancar.
        {
            let mut w = world.lock();
            let screens = w.screens(Machine::Mac);
            w.geometry.insert(Machine::Mac, screens);
        }
        world
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn power(&self) -> Power {
        self.lock().power
    }

    pub fn physical(&self) -> Physical {
        let w = self.lock();
        Physical {
            inputs: w
                .monitors
                .iter()
                .map(|(id, m)| (id.clone(), m.input))
                .collect(),
            desktops: w.desktops.clone(),
            builtin: w.builtin,
            power: w.power,
        }
    }

    /// La máquina que se ve en el monitor, o `None` si está en negro (la PC apagada).
    pub fn shows(&self, monitor: &MonitorId) -> Option<Machine> {
        self.lock().shows(monitor)
    }

    /// Lo que devolvería una lectura DDC de `0x60`.
    pub fn read_input_code(&self, monitor: &MonitorId) -> Option<u8> {
        let w = self.lock();
        let m = w.monitors.get(monitor)?;
        m.reads_as.or(m.last_code)
    }

    pub fn desktop(&self, machine: Machine) -> BTreeSet<MonitorId> {
        self.lock()
            .desktops
            .get(&machine)
            .cloned()
            .unwrap_or_default()
    }

    pub fn builtin(&self) -> BuiltinDisplay {
        self.lock().builtin
    }

    pub fn geometry(&self, machine: Machine) -> Option<Vec<Screen>> {
        self.lock().geometry.get(&machine).cloned()
    }

    /// Pasos pedidos a cada máquina, en orden (aplicados o no).
    pub fn calls(&self) -> Vec<(Machine, Step)> {
        self.lock().calls.clone()
    }

    pub fn ddc_writes(&self) -> Vec<(MonitorId, u8)> {
        self.lock().ddc_writes.clone()
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
        let visible = w.hub == machine && (machine == Machine::Mac || w.power == Power::On);
        if visible { w.usb.clone() } else { Vec::new() }
    }

    /// Compara el puesto con lo que cree un agente. Devuelve cada diferencia.
    pub fn check(&self, state: &State) -> Result<(), Vec<String>> {
        let w = self.lock();
        let mut errors = w.violations.clone();
        let pc_on = w.power == Power::On;
        if pc_on != state.pc_on {
            errors.push(format!(
                "la PC está {:?} pero el agente cree pc_on = {}",
                w.power, state.pc_on
            ));
        }
        for m in &w.cfg.monitors {
            let want = state.owner.get(&m.id).copied();
            let shows = w.shows(&m.id);
            if shows != want {
                errors.push(format!(
                    "{} muestra {shows:?} y el agente cree {want:?}",
                    m.id
                ));
            }
        }
        for machine in [Machine::Mac, Machine::Pc] {
            if machine == Machine::Pc && !pc_on {
                continue;
            }
            let want: BTreeSet<_> = state.monitors_of(&w.cfg, machine).cloned().collect();
            let got = w.desktops.get(&machine).cloned().unwrap_or_default();
            if got != want {
                errors.push(format!(
                    "el escritorio de {machine} tiene {got:?} y debería tener {want:?}"
                ));
            }
        }
        let builtin = state.builtin_effective(&w.cfg);
        if w.builtin != builtin {
            errors.push(format!(
                "la pantalla integrada está {:?} y debería estar {builtin:?}",
                w.builtin
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Cada motor de entrada encendido usa exactamente las pantallas que su máquina muestra.
    pub fn check_geometry(&self) -> Result<(), Vec<String>> {
        let w = self.lock();
        let mut errors = Vec::new();
        for machine in [Machine::Mac, Machine::Pc] {
            if machine == Machine::Pc && w.power != Power::On {
                continue;
            }
            let want = w.screens(machine);
            let got = w.geometry.get(&machine);
            if got != Some(&want) {
                errors.push(format!(
                    "la geometría de {machine} es {got:?} y debería ser {want:?}"
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

impl Inner {
    pub(crate) fn shows(&self, monitor: &MonitorId) -> Option<Machine> {
        let input = self.monitors.get(monitor)?.input;
        let machine = self
            .cfg
            .monitor(monitor)?
            .wiring
            .iter()
            .find(|(_, i)| **i == input)
            .map(|(m, _)| *m)?;
        match machine {
            Machine::Pc if self.power != Power::On => None,
            m => Some(m),
        }
    }

    /// Pantallas del escritorio de una máquina, en el orden del puesto.
    pub(crate) fn screens(&self, machine: Machine) -> Vec<Screen> {
        let desktop = self.desktops.get(&machine);
        let mut screens: Vec<Screen> = self
            .cfg
            .monitors
            .iter()
            .filter(|m| desktop.is_some_and(|d| d.contains(&m.id)))
            .map(|m| Screen::Monitor(m.id.clone()))
            .collect();
        if machine == Machine::Mac && self.builtin != BuiltinDisplay::Off {
            screens.push(Screen::Builtin);
        }
        screens
    }

    /// Escribe `0x60` en un monitor. Un código que no conoce se ignora sin error.
    pub(crate) fn write_input(&mut self, monitor: &MonitorId, code: u8) -> Result<(), String> {
        let m = self
            .monitors
            .get_mut(monitor)
            .ok_or_else(|| format!("no hay monitor {monitor}"))?;
        m.last_code = Some(code);
        if let Some(input) = m.accepts.get(&code) {
            m.input = *input;
        }
        self.ddc_writes.push((monitor.clone(), code));
        Ok(())
    }

    /// La PC termina de arrancar: Windows activa los monitores que están en DisplayPort.
    pub(crate) fn boot_done(&mut self) {
        self.power = Power::On;
        let on_dp: BTreeSet<_> = self
            .cfg
            .monitors
            .iter()
            .filter(|m| {
                m.input_for(Machine::Pc)
                    .is_some_and(|i| self.monitors.get(&m.id).is_some_and(|s| s.input == i))
            })
            .map(|m| m.id.clone())
            .collect();
        self.desktops.insert(Machine::Pc, on_dp);
        // Su agente arranca con Windows y calcula su geometría.
        let screens = self.screens(Machine::Pc);
        self.geometry.insert(Machine::Pc, screens);
    }

    pub(crate) fn power_off(&mut self) {
        self.power = Power::Off;
        self.desktops.insert(Machine::Pc, BTreeSet::new());
        self.geometry.remove(&Machine::Pc);
    }
}

/// Código DDC que manda la PC para llevar un monitor a `input`: el del perfil o el MCCS.
pub(crate) fn code_for(cfg: &Config, monitor: &MonitorId, input: Input) -> Option<u8> {
    let m = cfg.monitor(monitor)?;
    Some(match &m.ddc {
        Some(p) => p.code_for(input),
        None => match input {
            Input::Hdmi => MCCS_HDMI,
            Input::DisplayPort => MCCS_DP,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk::DESK_TOML;

    fn world() -> (Config, World) {
        let cfg = Config::from_toml(DESK_TOML).unwrap();
        let w = World::new(&cfg);
        (cfg, w)
    }

    #[test]
    fn the_34_always_reads_0x09_and_the_32_tells_the_truth() {
        let (_, w) = world();
        let (m34, m32) = (MonitorId::from("m34"), MonitorId::from("m32"));
        w.lock().write_input(&m34, 0x06).unwrap();
        w.lock().write_input(&m32, MCCS_HDMI).unwrap();
        assert_eq!(w.read_input_code(&m34), Some(LC34G55T_READS));
        assert_eq!(w.read_input_code(&m32), Some(MCCS_HDMI));
        w.lock().write_input(&m34, 0x0F).unwrap();
        assert_eq!(w.read_input_code(&m34), Some(LC34G55T_READS));
        assert_eq!(w.physical().inputs[&m34], Input::DisplayPort);
    }

    #[test]
    fn unknown_codes_are_ignored_without_error() {
        let (_, w) = world();
        let m34 = MonitorId::from("m34");
        // El MCCS de HDMI-1 no lo obedece el 34".
        w.lock().write_input(&m34, 0x0F).unwrap();
        w.lock().write_input(&m34, MCCS_HDMI).unwrap();
        assert_eq!(w.physical().inputs[&m34], Input::DisplayPort);
    }

    #[test]
    fn displayport_is_black_while_the_pc_is_off() {
        let (cfg, w) = world();
        let m34 = MonitorId::from("m34");
        w.lock().write_input(&m34, 0x0F).unwrap();
        assert_eq!(w.shows(&m34), None);
        w.lock().boot_done();
        assert_eq!(w.shows(&m34), Some(Machine::Pc));
        // Windows arrancó con el 34" en DisplayPort: lo tiene en su escritorio.
        assert_eq!(w.desktop(Machine::Pc), BTreeSet::from([m34.clone()]));
        let mut state = State::initial(&cfg);
        assert!(w.check(&state).is_err());
        state.pc_on = true;
        state.owner.insert(m34.clone(), Machine::Pc);
        // La Mac todavía tiene el 34" en su escritorio: fantasma.
        let errors = w.check(&state).unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("escritorio de mac")),
            "{errors:?}"
        );
        w.lock()
            .desktops
            .entry(Machine::Mac)
            .or_default()
            .remove(&m34);
        w.check(&state).unwrap();
    }

    #[test]
    fn the_hub_devices_follow_the_hub() {
        let (_, w) = world();
        assert!(!w.usb_devices(Machine::Mac).is_empty());
        w.plug_hub(Machine::Pc);
        assert!(w.usb_devices(Machine::Mac).is_empty());
        // Con la PC apagada nadie ve el teclado.
        assert!(w.usb_devices(Machine::Pc).is_empty());
        w.lock().boot_done();
        assert!(!w.usb_devices(Machine::Pc).is_empty());
    }

    #[test]
    fn geometry_includes_the_builtin_only_when_on() {
        let (_, w) = world();
        assert_eq!(
            w.geometry(Machine::Mac),
            Some(vec![
                Screen::Monitor("m34".into()),
                Screen::Monitor("m32".into())
            ])
        );
        let mut inner = w.lock();
        inner.builtin = BuiltinDisplay::Extend;
        assert_eq!(inner.screens(Machine::Mac).last(), Some(&Screen::Builtin));
    }
}
