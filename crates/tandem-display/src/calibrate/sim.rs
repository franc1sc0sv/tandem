//! Monitor simulado para calibrar sin hardware: lo que muestra de verdad, cómo responde a
//! cada código y qué miente al leerlo, más un usuario simulado que contesta las sondas.
//!
//! [`samsung_34`] y [`samsung_32`] reproducen lo medido en T-050: declaran códigos que no
//! obedecen, el 34" lee siempre `0x09` y el 32" lee el código de la entrada en la que
//! está, y en el 32" el `0x06` lleva a una entrada sin señal.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::Input;

use super::{ConfirmFuture, Confirmer, Probe, ProbeKind, Reported};
use crate::ddc::fake::flaky_error;
use crate::ddc::{DdcBus, DdcError, Edid, MonitorInfo, VcpValue};

/// Qué hace el monitor al recibir un código en `0x60`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Select(Input),
    /// Se va a una entrada sin señal.
    Black,
}

/// Lo que muestra el monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    Input(Input),
    /// Una entrada sin señal, elegida con este código.
    Black(u8),
}

/// Qué devuelve la lectura de `0x60`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reports {
    /// Siempre lo mismo (el 34": `0x09`).
    Fixed(u8),
    /// El código de la entrada en la que está (el 32"); en negro, el código que lo llevó.
    CurrentCode,
    /// La lectura falla siempre.
    Fails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimSpec {
    pub info: MonitorInfo,
    pub capabilities: String,
    /// Códigos con efecto; los demás no hacen nada (sin error, como los Samsung).
    pub effects: BTreeMap<u8, Effect>,
    pub reports: Reports,
    /// El código que reporta cada entrada con [`Reports::CurrentCode`].
    pub native: Reported,
}

fn samsung(product_code: u16, name: &str, description: &str) -> MonitorInfo {
    MonitorInfo {
        index: 0,
        description: description.into(),
        device: r"\\.\DISPLAY1".into(),
        edid: Some(Edid {
            manufacturer: "SAM".into(),
            product_code,
            serial_number: 0,
            name: Some(name.into()),
            serial_text: None,
        }),
    }
}

/// Samsung Odyssey G5 34" (LC34G55T, SAM711A): `docs/research/ddc-34-LC34G55T.md`.
pub fn samsung_34() -> SimSpec {
    SimSpec {
        info: samsung(0x711A, "LC34G55T", "Monitor PnP genérico"),
        capabilities: "(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 \
                       12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF) AC AE B2 B6 \
                       C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)mccs_ver(2.1)mswhql(1))"
            .into(),
        effects: [
            (0x03, Effect::Select(Input::Hdmi)),
            (0x06, Effect::Select(Input::Hdmi)),
            (0x09, Effect::Select(Input::DisplayPort)),
        ]
        .into(),
        reports: Reports::Fixed(0x09),
        native: Reported::default(),
    }
}

/// Samsung Odyssey G5 32" (SAM7489): `docs/research/ddc-32-odyssey-g5.md`.
pub fn samsung_32() -> SimSpec {
    SimSpec {
        info: samsung(0x7489, "Odyssey G5", "Monitor PnP genérico"),
        capabilities: "(prot(monitor)type(lcd)model(FALCON)cmds(01 02 03 07 0C E3 F3)vcp(02 04 \
                       05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60(01 03 04 11 12 0F 10) 62 8D \
                       FF)mswhql(1)mccs_ver(2.0)asset_eep(32)mpu_ver(01))"
            .into(),
        effects: [
            (0x05, Effect::Select(Input::Hdmi)),
            (0x06, Effect::Black),
            (0x0F, Effect::Select(Input::DisplayPort)),
        ]
        .into(),
        reports: Reports::CurrentCode,
        native: Reported {
            hdmi: Some(0x05),
            display_port: Some(0x0F),
        },
    }
}

/// Lo que pasa en el puesto simulado; lo comparten el bus y el confirmador.
#[derive(Debug)]
pub struct SimState {
    pub spec: SimSpec,
    pub showing: Showing,
    /// Resultado de las próximas escrituras; `false` = falla (sin efecto). Vacío = bien.
    pub write_script: VecDeque<bool>,
    /// Códigos escritos en `0x60` (solo los aceptados).
    pub writes: Vec<u8>,
    pub probes: Vec<Probe>,
    /// El usuario simulado hace los cambios a mano que se le piden.
    pub user_switches: bool,
    /// Si es `true`, una sonda que no se ve nunca se contesta (como en la realidad: nadie
    /// hace clic) y la corta el timeout del motor. Si es `false`, contesta `false` enseguida.
    pub silent_when_unseen: bool,
}

/// Monitor simulado. Es a la vez el bus del motor y el usuario que mira (el
/// [`Confirmer`]); los clones comparten el mismo puesto.
#[derive(Debug, Clone)]
pub struct SimMonitor {
    infos: Vec<MonitorInfo>,
    state: Arc<Mutex<SimState>>,
}

impl SimMonitor {
    pub fn new(spec: SimSpec, showing: Input) -> Self {
        let mut info = spec.info.clone();
        info.index = 0;
        Self {
            infos: vec![info],
            state: Arc::new(Mutex::new(SimState {
                spec,
                showing: Showing::Input(showing),
                write_script: VecDeque::new(),
                writes: Vec::new(),
                probes: Vec::new(),
                user_switches: true,
                silent_when_unseen: false,
            })),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, SimState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn showing(&self) -> Showing {
        self.lock().showing
    }

    pub fn writes(&self) -> Vec<u8> {
        self.lock().writes.clone()
    }

    pub fn probes(&self) -> Vec<Probe> {
        self.lock().probes.clone()
    }
}

impl DdcBus for SimMonitor {
    fn monitors(&self) -> &[MonitorInfo] {
        &self.infos
    }

    fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError> {
        self.check(monitor)?;
        Ok(self.lock().spec.capabilities.clone())
    }

    fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<VcpValue, DdcError> {
        self.check(monitor)?;
        let s = self.lock();
        if code != crate::ddc::VCP_INPUT_SOURCE {
            return Err(flaky_error("GetVCPFeatureAndVCPFeatureReply"));
        }
        let current = match (s.spec.reports, s.showing) {
            (Reports::Fixed(v), _) => v,
            (Reports::CurrentCode, Showing::Black(c)) => c,
            (Reports::CurrentCode, Showing::Input(i)) => s.spec.native.for_input(i).unwrap_or(0),
            (Reports::Fails, _) => return Err(flaky_error("GetVCPFeatureAndVCPFeatureReply")),
        };
        Ok(VcpValue {
            current: current.into(),
            maximum: 0xFF,
        })
    }

    fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError> {
        self.check(monitor)?;
        let mut s = self.lock();
        if !s.write_script.pop_front().unwrap_or(true) {
            return Err(flaky_error("SetVCPFeature"));
        }
        if code != crate::ddc::VCP_INPUT_SOURCE {
            return Ok(());
        }
        let Ok(value) = u8::try_from(value) else {
            return Ok(());
        };
        s.writes.push(value);
        match s.spec.effects.get(&value) {
            Some(Effect::Select(input)) => s.showing = Showing::Input(*input),
            Some(Effect::Black) => s.showing = Showing::Black(value),
            None => {}
        }
        Ok(())
    }
}

impl SimMonitor {
    fn check(&self, monitor: usize) -> Result<(), DdcError> {
        if monitor < self.infos.len() {
            Ok(())
        } else {
            Err(DdcError::NoSuchMonitor(monitor))
        }
    }
}

impl Confirmer for SimMonitor {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        let (seen, silent) = {
            let mut s = self.lock();
            s.probes.push(probe.clone());
            if probe.kind == ProbeKind::UserSwitch && s.user_switches {
                s.showing = Showing::Input(probe.input);
            }
            (
                s.showing == Showing::Input(probe.input),
                s.silent_when_unseen,
            )
        };
        Box::pin(async move {
            if !seen && silent {
                std::future::pending::<()>().await;
            }
            Ok(seen)
        })
    }
}
