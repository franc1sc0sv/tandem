//! Invariantes del núcleo con pruebas de propiedades.
//!
//! El ejecutor de prueba lleva un modelo "físico" del puesto (qué muestra cada monitor,
//! qué máquinas están prendidas, qué hace la pantalla integrada) que solo cambia cuando un
//! paso se ejecuta con éxito. Así se comprueba lo que de verdad le importa al usuario, no
//! solo lo que `State` cree.
//!
//! Los estados se generan desde las dos perspectivas (DESIGN §3.7): orquesta la Mac con
//! la PC prendida o apagada, u orquesta la PC con la Mac prendida o suspendida.
//!
//! Un monitor sin confirmar (T-085) puede estar de verdad en cualquier entrada cableada,
//! no en la que cree `State`: el modelo físico parte de esa verdad, no de la creencia.

use std::collections::BTreeMap;
use std::future::Future;

use proptest::prelude::*;
use tandem_core::{
    Abilities, BuiltinDisplay, Config, Doubt, Executor, Input, Machine, Mode, MonitorConfig,
    MonitorId, Plan, Powered, State, Step, TxnError, run,
};

/// Lo que se ve en el escritorio real.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Physical {
    owner: BTreeMap<MonitorId, Machine>,
    on: Powered,
    builtin: BuiltinDisplay,
}

impl Physical {
    /// El puesto real: lo que cree `s`, salvo los monitores sin confirmar, que muestran lo
    /// que diga `truth`.
    fn new(cfg: &Config, s: &State, truth: &BTreeMap<MonitorId, Machine>) -> Self {
        let mut owner = s.owner.clone();
        for (id, m) in truth {
            if s.in_doubt(id) {
                owner.insert(id.clone(), *m);
            }
        }
        Physical {
            owner,
            on: s.on,
            builtin: s.builtin_effective(cfg),
        }
    }

    /// La Mac tiene dónde mirar si está prendida: un monitor externo o la pantalla
    /// integrada encendida.
    fn mac_visible(&self) -> bool {
        !self.on.mac
            || self.owner.values().any(|m| *m == Machine::Mac)
            || self.builtin != BuiltinDisplay::Off
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
        } else if let Some(m) = needs(step).filter(|m| !self.phys.on.is_on(*m)) {
            // Una máquina apagada no hace pasos (ni cambia entradas, ni toca su escritorio).
            Err(format!("{m} está apagada (llamada {n})"))
        } else {
            match step {
                Step::PowerOn { machine } => self.phys.on.set(*machine, true),
                Step::PowerOff { machine } => self.phys.on.set(*machine, false),
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

/// La máquina que tiene que estar prendida para hacer el paso.
fn needs(step: &Step) -> Option<Machine> {
    match step {
        Step::PowerOn { machine } => Some(machine.other()),
        Step::PowerOff { machine } => Some(*machine),
        Step::SwitchInput { by, .. } => Some(*by),
        Step::Attach { machine, .. } | Step::Detach { machine, .. } => Some(*machine),
        Step::SetBuiltin { .. } => Some(Machine::Mac),
        Step::UpdateInputGeometry => None,
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

/// Estado coherente con el cableado visto desde `me`: `me` está prendida, cada monitor
/// muestra una máquina cableada y, si alguno muestra la otra, la otra está prendida, salvo
/// tras un corte de corriente (`cut`): se apagó sola y sus monitores siguen en su entrada,
/// en negro.
fn state(cfg: &Config, me: Machine) -> impl Strategy<Value = State> + use<> {
    let cfg = cfg.clone();
    let n = cfg.monitors.len();
    (
        proptest::collection::vec(any::<bool>(), n),
        any::<bool>(),
        builtin(),
        proptest::bool::weighted(0.2),
        proptest::collection::vec(proptest::option::of(doubt()), n),
    )
        .prop_map(move |(picks, other_on, builtin_pref, cut, doubts)| {
            let owner: BTreeMap<_, _> = cfg
                .monitors
                .iter()
                .zip(picks)
                .map(|(m, p)| (m.id.clone(), wired_machine(m, p)))
                .collect();
            let other = me.other();
            let other_on = !cut && (other_on || owner.values().any(|o| *o == other));
            let mut on = Powered::only(me);
            on.set(other, other_on);
            let doubt = cfg
                .monitors
                .iter()
                .zip(doubts)
                .filter_map(|(m, d)| Some((m.id.clone(), d?)))
                .collect();
            State {
                owner,
                on,
                builtin_pref,
                doubt,
            }
        })
}

fn doubt() -> impl Strategy<Value = Doubt> {
    prop_oneof![Just(Doubt::Unknown), Just(Doubt::Stale), Just(Doubt::Blank)]
}

/// Lo que muestra de verdad cada monitor sin confirmar: cualquier máquina cableada.
fn truth(cfg: &Config) -> impl Strategy<Value = BTreeMap<MonitorId, Machine>> + use<> {
    let cfg = cfg.clone();
    proptest::collection::vec(any::<bool>(), cfg.monitors.len()).prop_map(move |picks| {
        cfg.monitors
            .iter()
            .zip(picks)
            .map(|(m, p)| (m.id.clone(), wired_machine(m, p)))
            .collect()
    })
}

/// Cualquier modo, incluidos repartos con monitores que no existen o que no se pueden cumplir.
fn mode(cfg: &Config) -> impl Strategy<Value = Mode> + use<> {
    let ids: Vec<MonitorId> = cfg.monitors.iter().map(|m| m.id.clone()).collect();
    let shared = proptest::collection::vec((proptest::sample::select(ids), machine()), 0..=4)
        .prop_map(|a| Mode::Shared(a.into_iter().collect()));
    prop_oneof![Just(Mode::SoloMac), Just(Mode::SoloPc), shared]
}

/// Puesto completo: configuración, estado de partida (desde la Mac o desde la PC), lo que
/// muestran de verdad los monitores sin confirmar y modo destino.
type Scenario = (Config, State, BTreeMap<MonitorId, Machine>, Mode);

fn scenario() -> impl Strategy<Value = Scenario> {
    (config(), machine()).prop_flat_map(|(cfg, me)| {
        let s = state(&cfg, me);
        let t = truth(&cfg);
        let m = mode(&cfg);
        (Just(cfg), s, t, m)
    })
}

/// El plan, o `None` si el modo pide algo que el cableado no permite (y entonces
/// el error debe decir exactamente eso).
fn plan_or_skip(cfg: &Config, s: &State, mode: &Mode) -> Option<Plan> {
    match Plan::to_mode(cfg, s, mode, &Abilities::pc_switches(cfg)) {
        Ok(p) => Some(p),
        Err(e) => {
            let target = mode.assignment(cfg);
            let unwired = cfg.monitors.iter().any(|m| {
                (s.owner[&m.id] != target[&m.id] || s.in_doubt(&m.id))
                    && m.input_for(target[&m.id]).is_none()
            });
            assert!(unwired, "error inesperado al planear: {e}");
            None
        }
    }
}

proptest! {
    /// Sin fallos, el plan deja el puesto exactamente en el reparto del modo, `State`
    /// coincide con la realidad y la Mac tiene pantalla.
    #[test]
    fn plan_reaches_mode((cfg, start, truth, mode) in scenario()) {
        let Some(plan) = plan_or_skip(&cfg, &start, &mode) else { return Ok(()) };
        let mut s = start.clone();
        let mut exec = Sim { phys: Physical::new(&cfg, &start, &truth), calls: 0, fail_at: None };

        let report = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec));
        prop_assert!(report.is_ok());
        prop_assert_eq!(&s.owner, &mode.assignment(&cfg));
        // Aunque Tandem creyera otra cosa de un monitor sin confirmar, termina donde va y
        // el estado queda confirmado (T-085).
        prop_assert_eq!(&exec.phys.owner, &s.owner);
        prop_assert!(s.doubt.is_empty(), "quedó sin confirmar: {:?}", s.doubt);
        // La pantalla integrada de una Mac apagada no se toca: no hay nada que ver.
        if s.is_on(Machine::Mac) {
            prop_assert_eq!(exec.phys.builtin, s.builtin_effective(&cfg));
        }
        for m in [Machine::Mac, Machine::Pc] {
            if s.owner.values().any(|o| *o == m) {
                prop_assert!(s.is_on(m) && exec.phys.on.is_on(m), "{} muestra algo, así que debe estar prendida", m);
            }
        }
        prop_assert_eq!(exec.phys.on, s.on);
        prop_assert!(exec.phys.mac_visible(), "la Mac quedó sin pantalla: {:?}", exec.phys);
        if s.is_on(Machine::Mac) {
            prop_assert!(s.is_visible(&cfg, Machine::Mac));
        }
    }

    /// Falle el paso que falle, la vuelta atrás deja el puesto (y `State`) como estaba.
    /// Lo único que puede quedar es una máquina prendida, porque encenderla no se deshace.
    #[test]
    fn rollback_restores_state((cfg, start, truth, mode) in scenario(), fail in any::<prop::sample::Index>()) {
        let Some(plan) = plan_or_skip(&cfg, &start, &mode) else { return Ok(()) };
        prop_assume!(!plan.steps.is_empty());
        let k = fail.index(plan.steps.len());

        let before = Physical::new(&cfg, &start, &truth);
        let mut s = start.clone();
        let mut exec = Sim { phys: before.clone(), calls: 0, fail_at: Some(k) };

        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        prop_assert_eq!(
            err,
            TxnError::RolledBack { step: plan.steps[k].clone(), error: format!("fallo inyectado en la llamada {k}") }
        );

        prop_assert_eq!(&s.owner, &start.owner);
        prop_assert_eq!(&s.doubt, &start.doubt);
        prop_assert_eq!(s.builtin_pref, start.builtin_pref);
        for m in [Machine::Mac, Machine::Pc] {
            let powered_on = plan.steps[..k].contains(&Step::PowerOn { machine: m });
            prop_assert_eq!(s.is_on(m), start.is_on(m) || powered_on);
        }

        // Un monitor sin confirmar no tiene a qué volver: si se le mandó la entrada, el
        // deshacer lo deja en la que creía Tandem; si no, sigue donde estaba.
        for (id, m) in &exec.phys.owner {
            if start.in_doubt(id) {
                prop_assert!(*m == before.owner[id] || *m == start.owner[id], "{} quedó en {}", id, m);
            } else {
                prop_assert_eq!(*m, before.owner[id]);
            }
        }
        prop_assert_eq!(exec.phys.builtin, before.builtin);
        prop_assert_eq!(exec.phys.on, s.on);
        // Si Tandem creía mal lo de un monitor sin confirmar, no hay garantía: la Mac pudo
        // empezar sin pantalla, o encenderse para un monitor que no estaba donde creía.
        if before.owner == start.owner {
            prop_assert!(exec.phys.mac_visible(), "la Mac quedó sin pantalla: {:?}", exec.phys);
        }
    }

    /// El apagado limpio de la otra máquina nunca la apaga mientras muestra algo, y la
    /// que queda tiene todos los monitores.
    #[test]
    fn shutdown_leaves_everything_on_the_machine_that_stays((cfg, start, truth, _mode) in scenario()) {
        let me = [Machine::Mac, Machine::Pc].into_iter().find(|m| start.is_on(*m) && !start.is_on(m.other())).unwrap_or(Machine::Mac);
        let off = me.other();
        let Some(plan) = Plan::power_off(&cfg, &start, off, &Abilities::pc_switches(&cfg)).ok() else {
            // Solo puede fallar si algún monitor de la que se apaga no está cableado a la otra.
            // (o uno sin confirmar, que se manda igual).
            prop_assert!(cfg.monitors.iter().any(|m| (start.owner[&m.id] == off || start.in_doubt(&m.id)) && m.input_for(me).is_none()));
            return Ok(());
        };
        let mut s = start.clone();
        let mut exec = Sim { phys: Physical::new(&cfg, &start, &truth), calls: 0, fail_at: None };
        pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap();

        prop_assert!(!exec.phys.on.is_on(off));
        prop_assert!(exec.phys.owner.values().all(|m| *m == me));
        prop_assert!(exec.phys.mac_visible());
        if let Some(pos) = plan.steps.iter().position(|st| *st == Step::PowerOff { machine: off }) {
            prop_assert_eq!(pos, plan.steps.len() - 1, "apagar es lo último");
        }
    }

    /// «Aplicar modo» manda la entrada de todos los monitores, y con el puesto en
    /// cualquier entrada lo deja en el reparto del modo, confirmado.
    #[test]
    fn apply_mode_switches_every_monitor_and_confirms((cfg, start, truth, mode) in scenario()) {
        let Ok(plan) = Plan::apply_mode(&cfg, &start, &mode, &Abilities::pc_switches(&cfg)) else { return Ok(()) };
        for m in &cfg.monitors {
            prop_assert!(plan.steps.iter().any(|st| matches!(st, Step::SwitchInput { monitor, .. } if *monitor == m.id)), "{} sin cambio de entrada", m.id);
        }
        // Todo lo que muestra el puesto es desconocido, no solo lo marcado.
        let all_unknown = State { doubt: cfg.monitors.iter().map(|m| (m.id.clone(), Doubt::Unknown)).collect(), ..start.clone() };
        let mut s = start.clone();
        let mut exec = Sim { phys: Physical::new(&cfg, &all_unknown, &truth), calls: 0, fail_at: None };
        pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap();
        prop_assert_eq!(&s.owner, &mode.assignment(&cfg));
        prop_assert_eq!(&exec.phys.owner, &s.owner);
        prop_assert!(s.is_confirmed() && s.doubt.is_empty());
    }

    /// Corregir lo que muestra un monitor nunca manda una entrada (T-085).
    #[test]
    fn following_a_correction_never_switches_an_input((cfg, start, truth, _mode) in scenario()) {
        let mut to = start.clone();
        for (id, m) in &truth {
            to.record_shown(id, Some(*m));
        }
        let plan = Plan::follow(&cfg, &start, &to);
        prop_assert!(!plan.steps.iter().any(|st| matches!(st, Step::SwitchInput { .. } | Step::PowerOn { .. } | Step::PowerOff { .. })), "{:?}", plan.steps);
        prop_assert!(to.is_confirmed());
        for st in &plan.steps {
            if let Step::Attach { machine, .. } | Step::Detach { machine, .. } = st {
                prop_assert!(to.is_on(*machine), "toca el escritorio de {} apagada", machine);
            }
        }
    }
}
