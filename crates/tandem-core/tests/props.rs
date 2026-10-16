//! Invariantes del núcleo con pruebas de propiedades.
//!
//! El ejecutor de prueba lleva un modelo "físico" del puesto (qué muestra cada monitor,
//! si la PC está prendida, qué hace la pantalla integrada) que solo cambia cuando un paso
//! se ejecuta con éxito. Así se comprueba lo que de verdad le importa al usuario, no solo
//! lo que `State` cree.

use std::collections::BTreeMap;
use std::future::Future;

use proptest::prelude::*;
use tandem_core::{
    BuiltinDisplay, Config, Executor, Input, Machine, Mode, MonitorConfig, MonitorId, Plan, State,
    Step, TxnError, run,
};

/// Lo que se ve en el escritorio real.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Physical {
    owner: BTreeMap<MonitorId, Machine>,
    pc_on: bool,
    builtin: BuiltinDisplay,
}

impl Physical {
    fn from_state(cfg: &Config, s: &State) -> Self {
        Physical {
            owner: s.owner.clone(),
            pc_on: s.pc_on,
            builtin: s.builtin_effective(cfg),
        }
    }

    /// La Mac tiene dónde mirar: un monitor externo o la pantalla integrada encendida.
    fn mac_visible(&self) -> bool {
        self.owner.values().any(|m| *m == Machine::Mac) || self.builtin != BuiltinDisplay::Off
    }
}

/// Ejecutor que falla exactamente en la llamada número `fail_at` (si la hay).
struct Sim {
    phys: Physical,
    calls: usize,
    fail_at: Option<usize>,
}

impl Executor for Sim {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        let n = self.calls;
        self.calls += 1;
        let r = if Some(n) == self.fail_at {
            Err(format!("fallo inyectado en la llamada {n}"))
        } else if matches!(step, Step::SwitchInput { .. }) && !self.phys.pc_on {
            // Las entradas las cambia la PC (DESIGN §8): apagada no puede.
            Err(format!("la PC está apagada (llamada {n})"))
        } else {
            match step {
                Step::PowerOnPc => self.phys.pc_on = true,
                Step::PowerOffPc => self.phys.pc_on = false,
                Step::SwitchInput { monitor, to, .. } => {
                    self.phys.owner.insert(monitor.clone(), *to);
                }
                Step::SetBuiltin { to, .. } => self.phys.builtin = *to,
                Step::Attach { .. } | Step::Detach { .. } | Step::UpdateInputGeometry => {}
            }
            Ok(())
        };
        async move { r }
    }
}

fn machine() -> impl Strategy<Value = Machine> {
    prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
}

fn input() -> impl Strategy<Value = Input> {
    prop_oneof![Just(Input::Hdmi), Just(Input::DisplayPort)]
}

fn builtin() -> impl Strategy<Value = BuiltinDisplay> {
    prop_oneof![
        Just(BuiltinDisplay::Off),
        Just(BuiltinDisplay::Mirror),
        Just(BuiltinDisplay::Extend),
    ]
}

/// Cableado no vacío: solo a la Mac, solo a la PC o a las dos.
fn wiring() -> impl Strategy<Value = BTreeMap<Machine, Input>> {
    (proptest::option::of(input()), proptest::option::of(input())).prop_filter_map(
        "monitor sin cablear",
        |(mac, pc)| {
            let mut w = BTreeMap::new();
            if let Some(i) = mac {
                w.insert(Machine::Mac, i);
            }
            if let Some(i) = pc {
                w.insert(Machine::Pc, i);
            }
            (!w.is_empty()).then_some(w)
        },
    )
}

fn config() -> impl Strategy<Value = Config> {
    proptest::collection::vec(wiring(), 1..=4).prop_map(|wirings| Config {
        monitors: wirings
            .into_iter()
            .enumerate()
            .map(|(i, wiring)| MonitorConfig {
                id: MonitorId(format!("m{i}")),
                label: format!("monitor {i}"),
                edid_model: None,
                wiring,
                ddc: None,
            })
            .collect(),
    })
}

/// Una máquina a la que el monitor está cableado (un monitor no puede mostrar otra cosa).
fn wired_machine(m: &MonitorConfig, pick: bool) -> Machine {
    let wired: Vec<Machine> = m.wiring.keys().copied().collect();
    wired[usize::from(pick) % wired.len()]
}

/// Estado coherente con el cableado: cada monitor muestra una máquina cableada y,
/// si alguno muestra la PC, la PC está prendida, salvo tras un corte de corriente
/// (`cut`): la PC se apagó sola y sus monitores siguen en su entrada, en negro.
fn state(cfg: &Config) -> impl Strategy<Value = State> + use<> {
    let cfg = cfg.clone();
    let n = cfg.monitors.len();
    (
        proptest::collection::vec(any::<bool>(), n),
        any::<bool>(),
        builtin(),
        proptest::bool::weighted(0.2),
    )
        .prop_map(move |(picks, pc_on, builtin_pref, cut)| {
            let owner: BTreeMap<_, _> = cfg
                .monitors
                .iter()
                .zip(picks)
                .map(|(m, p)| (m.id.clone(), wired_machine(m, p)))
                .collect();
            let pc_on = !cut && (pc_on || owner.values().any(|o| *o == Machine::Pc));
            State {
                owner,
                pc_on,
                builtin_pref,
            }
        })
}

/// Cualquier modo, incluidos repartos con monitores que no existen o que no se pueden cumplir.
fn mode(cfg: &Config) -> impl Strategy<Value = Mode> + use<> {
    let ids: Vec<MonitorId> = cfg.monitors.iter().map(|m| m.id.clone()).collect();
    let shared = proptest::collection::vec((proptest::sample::select(ids), machine()), 0..=4)
        .prop_map(|a| Mode::Shared(a.into_iter().collect()));
    prop_oneof![Just(Mode::SoloMac), Just(Mode::SoloPc), shared]
}

/// Puesto completo: configuración, estado de partida y modo destino.
fn scenario() -> impl Strategy<Value = (Config, State, Mode)> {
    config().prop_flat_map(|cfg| {
        let s = state(&cfg);
        let m = mode(&cfg);
        (Just(cfg), s, m)
    })
}

/// El plan, o `None` si el modo pide algo que el cableado no permite (y entonces
/// el error debe decir exactamente eso).
fn plan_or_skip(cfg: &Config, s: &State, mode: &Mode) -> Option<Plan> {
    match Plan::to_mode(cfg, s, mode) {
        Ok(p) => Some(p),
        Err(e) => {
            let target = mode.assignment(cfg);
            let unwired = cfg
                .monitors
                .iter()
                .any(|m| s.owner[&m.id] != target[&m.id] && m.input_for(target[&m.id]).is_none());
            assert!(unwired, "error inesperado al planear: {e}");
            None
        }
    }
}

proptest! {
    /// Sin fallos, el plan deja el puesto exactamente en el reparto del modo, `State`
    /// coincide con la realidad y la Mac tiene pantalla.
    #[test]
    fn plan_reaches_mode((cfg, start, mode) in scenario()) {
        let Some(plan) = plan_or_skip(&cfg, &start, &mode) else { return Ok(()) };
        let mut s = start.clone();
        let mut exec = Sim { phys: Physical::from_state(&cfg, &start), calls: 0, fail_at: None };

        let report = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec));
        prop_assert!(report.is_ok());
        prop_assert_eq!(&s.owner, &mode.assignment(&cfg));
        prop_assert_eq!(&exec.phys.owner, &s.owner);
        prop_assert_eq!(exec.phys.builtin, s.builtin_effective(&cfg));
        if s.owner.values().any(|m| *m == Machine::Pc) {
            prop_assert!(s.pc_on && exec.phys.pc_on, "la PC muestra algo, así que debe estar prendida");
        }
        prop_assert!(exec.phys.mac_visible(), "la Mac quedó sin pantalla: {:?}", exec.phys);
        prop_assert!(s.is_visible(&cfg, Machine::Mac));
    }

    /// Falle el paso que falle, la vuelta atrás deja el puesto (y `State`) como estaba.
    /// Lo único que puede quedar es la PC prendida, porque encenderla no se deshace.
    #[test]
    fn rollback_restores_state((cfg, start, mode) in scenario(), fail in any::<prop::sample::Index>()) {
        let Some(plan) = plan_or_skip(&cfg, &start, &mode) else { return Ok(()) };
        prop_assume!(!plan.steps.is_empty());
        let k = fail.index(plan.steps.len());

        let before = Physical::from_state(&cfg, &start);
        let mut s = start.clone();
        let mut exec = Sim { phys: before.clone(), calls: 0, fail_at: Some(k) };

        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        prop_assert_eq!(
            err,
            TxnError::RolledBack { step: plan.steps[k].clone(), error: format!("fallo inyectado en la llamada {k}") }
        );

        let powered_on = plan.steps[..k].contains(&Step::PowerOnPc);
        prop_assert_eq!(&s.owner, &start.owner);
        prop_assert_eq!(s.builtin_pref, start.builtin_pref);
        prop_assert_eq!(s.pc_on, start.pc_on || powered_on);

        prop_assert_eq!(&exec.phys.owner, &before.owner);
        prop_assert_eq!(exec.phys.builtin, before.builtin);
        prop_assert_eq!(exec.phys.pc_on, s.pc_on);
        prop_assert!(exec.phys.mac_visible(), "la Mac quedó sin pantalla: {:?}", exec.phys);
    }

    /// El apagado limpio nunca apaga la PC mientras muestra algo y deja la Mac con pantalla.
    #[test]
    fn shutdown_leaves_mac_visible((cfg, start, _mode) in scenario()) {
        let Some(plan) = Plan::shutdown_pc(&cfg, &start).ok() else {
            // Solo puede fallar si algún monitor de la PC no está cableado a la Mac.
            prop_assert!(cfg.monitors.iter().any(|m| start.owner[&m.id] == Machine::Pc && m.input_for(Machine::Mac).is_none()));
            return Ok(());
        };
        let mut s = start.clone();
        let mut exec = Sim { phys: Physical::from_state(&cfg, &start), calls: 0, fail_at: None };
        pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap();

        prop_assert!(!exec.phys.pc_on);
        prop_assert!(exec.phys.owner.values().all(|m| *m == Machine::Mac));
        prop_assert!(exec.phys.mac_visible());
        if let Some(pos) = plan.steps.iter().position(|st| *st == Step::PowerOffPc) {
            prop_assert_eq!(pos, plan.steps.len() - 1, "apagar la PC es lo último");
        }
    }
}
