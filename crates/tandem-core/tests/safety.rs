//! Seguridad de pantallas (T-087, DESIGN §8): ninguna secuencia de arranques, cambios de
//! modo fallidos, correcciones del usuario y asistentes cancelados deja un monitor sin
//! salida activa en la máquina que lo muestra.
//!
//! El modelo físico lleva, por máquina, qué monitores están en su escritorio, y por monitor
//! en qué entrada está de verdad. Al principio cada máquina tiene todos sus monitores
//! activos (el fantasma: lo que ve cualquier sistema recién arrancado), sin importar lo que
//! Tandem crea. El invariante es el de la regla dura: si un monitor muestra de verdad a una
//! máquina encendida, esa máquina lo tiene en su escritorio.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use proptest::prelude::*;
use tandem_core::{
    Abilities, Config, Executor, Input, Machine, Mode, MonitorConfig, MonitorId, Plan, Powered,
    State, Step, run,
};

#[derive(Debug, Clone)]
struct World {
    /// En qué entrada está cada monitor de verdad (la máquina cableada a ella).
    shown: BTreeMap<MonitorId, Machine>,
    /// Lo que cada máquina tiene en su escritorio.
    attached: BTreeMap<Machine, BTreeSet<MonitorId>>,
    on: Powered,
    /// Lo que esta máquina quitó y no volvió a poner (el libro de `tandemd::safety`).
    ledger: BTreeMap<Machine, BTreeSet<MonitorId>>,
}

impl World {
    fn new(cfg: &Config, truth: &BTreeMap<MonitorId, Machine>, on: Powered) -> Self {
        let all: BTreeSet<_> = cfg.monitors.iter().map(|m| m.id.clone()).collect();
        World {
            shown: truth.clone(),
            attached: [(Machine::Mac, all.clone()), (Machine::Pc, all)].into(),
            on,
            ledger: BTreeMap::new(),
        }
    }

    /// La regla dura: el monitor que se ve en una máquina encendida está en su escritorio.
    fn check(&self) -> Result<(), String> {
        for (monitor, machine) in &self.shown {
            if self.on.is_on(*machine) && !self.attached[machine].contains(monitor) {
                return Err(format!(
                    "{monitor} muestra la {machine} pero ella lo quitó de su escritorio"
                ));
            }
        }
        Ok(())
    }

    /// Un arranque del agente de `machine`: reactiva lo que anotó como quitado (el estado
    /// de un arranque siempre está sin confirmar). Nunca quita nada.
    fn boot(&mut self, machine: Machine) {
        let ledger = std::mem::take(self.ledger.entry(machine).or_default());
        self.attached.entry(machine).or_default().extend(ledger);
    }
}

struct Sim {
    world: World,
    calls: usize,
    fail_at: Option<usize>,
}

impl Executor for Sim {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        let n = self.calls;
        self.calls += 1;
        let r = if Some(n) == self.fail_at {
            Err(format!("fallo inyectado en la llamada {n}"))
        } else {
            self.apply(step)
        };
        async move { r }
    }
}

impl Sim {
    fn apply(&mut self, step: &Step) -> Result<(), String> {
        let w = &mut self.world;
        match step {
            Step::PowerOn { machine } => w.on.set(*machine, true),
            Step::PowerOff { machine } => w.on.set(*machine, false),
            Step::SwitchInput {
                monitor, to, by, ..
            } => {
                if !w.on.is_on(*by) {
                    return Err(format!("{by} está apagada"));
                }
                w.shown.insert(monitor.clone(), *to);
            }
            Step::Attach { machine, monitor } => {
                if !w.on.is_on(*machine) {
                    return Err(format!("{machine} está apagada"));
                }
                w.attached
                    .entry(*machine)
                    .or_default()
                    .insert(monitor.clone());
                w.ledger.entry(*machine).or_default().remove(monitor);
            }
            Step::Detach { machine, monitor } => {
                if !w.on.is_on(*machine) {
                    return Err(format!("{machine} está apagada"));
                }
                // Primero se anota y después se quita, como `DisplaySteps`.
                w.ledger
                    .entry(*machine)
                    .or_default()
                    .insert(monitor.clone());
                w.attached.entry(*machine).or_default().remove(monitor);
            }
            Step::SetBuiltin { .. } | Step::UpdateInputGeometry => {}
        }
        Ok(())
    }
}

fn machine() -> impl Strategy<Value = Machine> {
    prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
}

fn config() -> impl Strategy<Value = Config> {
    proptest::collection::vec(any::<bool>(), 1..=3).prop_map(|v| Config {
        monitors: v
            .into_iter()
            .enumerate()
            .map(|(i, flip)| MonitorConfig {
                id: MonitorId(format!("m{i}")),
                label: format!("{i}"),
                edid_model: None,
                edid_aliases: Vec::new(),
                wiring: [
                    (
                        Machine::Pc,
                        if flip {
                            Input::Hdmi
                        } else {
                            Input::DisplayPort
                        },
                    ),
                    (
                        Machine::Mac,
                        if flip {
                            Input::DisplayPort
                        } else {
                            Input::Hdmi
                        },
                    ),
                ]
                .into(),
                ddc: None,
            })
            .collect(),
    })
}

#[derive(Debug, Clone)]
enum Op {
    /// «Cambiar de modo» o «Aplicar modo», con un fallo en el paso `fail` si lo hay.
    Change {
        mode: Vec<Machine>,
        apply: bool,
        fail: Option<prop::sample::Index>,
    },
    /// El usuario dice lo que muestra cada monitor (el agente solo corrige su creencia).
    Say(Vec<Option<Machine>>),
    /// El agente de `0` arranca de nuevo (el estado vuelve a quedar sin confirmar).
    Boot(Machine),
    /// Sincronizar con la otra máquina, escanear, agregar, quitar, el asistente cancelado:
    /// no tocan ningún escritorio.
    Wizard,
}

fn op(n: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (
            proptest::collection::vec(machine(), n),
            any::<bool>(),
            proptest::option::of(any::<prop::sample::Index>()),
        )
            .prop_map(|(mode, apply, fail)| Op::Change { mode, apply, fail }),
        2 => proptest::collection::vec(proptest::option::of(machine()), n).prop_map(Op::Say),
        2 => machine().prop_map(Op::Boot),
        1 => Just(Op::Wizard),
    ]
}

fn abilities(cfg: &Config) -> Abilities {
    Abilities::pc_switches(cfg)
}

proptest! {
    /// La regla dura, con cualquier secuencia y cualquier fallo, y con cualquier cosa que
    /// Tandem crea de lo que muestra cada monitor.
    #[test]
    fn no_sequence_leaves_a_monitor_without_an_active_output(
        (cfg, me, truth, ops) in config().prop_flat_map(|cfg| {
            let n = cfg.monitors.len();
            let truth = proptest::collection::vec(machine(), n);
            (Just(cfg), machine(), truth, proptest::collection::vec(op(n), 1..8))
        })
    ) {
        let truth: BTreeMap<MonitorId, Machine> = cfg
            .monitors
            .iter()
            .zip(truth)
            .map(|(m, t)| (m.id.clone(), t))
            .collect();
        let mut state = State::initial(&cfg, me);
        let mut world = World::new(&cfg, &truth, Powered::both());
        prop_assert!(world.check().is_ok());
        for op in ops {
            match op {
                Op::Boot(machine) => {
                    world.boot(machine);
                    // Arrancar tampoco quita nada, y el estado sigue sin confirmar.
                    state.forget();
                }
                Op::Wizard => {}
                Op::Say(says) => {
                    let mut to = state.clone();
                    for (m, say) in cfg.monitors.iter().zip(says) {
                        to.record_shown(&m.id, say.filter(|s| m.input_for(*s).is_some()));
                    }
                    let plan = Plan::follow(&cfg, &state, &to);
                    // Lo dicho por el usuario no se comprobó con ningún cambio de entrada:
                    // nunca quita una salida.
                    let detaches = plan.steps.iter().filter(|s| matches!(s, Step::Detach { .. })).count();
                    prop_assert_eq!(detaches, 0);
                    let mut sim = Sim { world: world.clone(), calls: 0, fail_at: None };
                    let mut next = state.clone();
                    if pollster::block_on(run(&cfg, &mut next, &plan, &mut sim)).is_ok() {
                        world = sim.world;
                        state = to;
                    }
                }
                Op::Change { mode, apply, fail } => {
                    let assign: BTreeMap<_, _> = cfg
                        .monitors
                        .iter()
                        .zip(mode)
                        .map(|(m, to)| (m.id.clone(), to))
                        .collect();
                    let mode = Mode::Shared(assign);
                    let plan = if apply {
                        Plan::apply_mode(&cfg, &state, &mode, &abilities(&cfg))
                    } else {
                        Plan::to_mode(&cfg, &state, &mode, &abilities(&cfg))
                    };
                    let Ok(plan) = plan else { continue };
                    let fail_at = fail.filter(|_| !plan.steps.is_empty()).map(|f| f.index(plan.steps.len()));
                    let mut sim = Sim { world: world.clone(), calls: 0, fail_at };
                    let mut next = state.clone();
                    let out = pollster::block_on(run(&cfg, &mut next, &plan, &mut sim));
                    world = sim.world;
                    if out.is_ok() {
                        state = next;
                    } else {
                        // Se deshizo: lo que Tandem cree no cambia, salvo máquinas encendidas.
                        state.on = next.on;
                    }
                }
            }
            if let Err(e) = world.check() {
                return Err(TestCaseError::fail(e));
            }
        }
    }

    /// En todo plan, el `Detach` de un monitor va después del `SwitchInput` de ese
    /// monitor a la otra máquina: nunca se quita una salida por un estado sin comprobar.
    #[test]
    fn every_detach_follows_the_switch_of_its_monitor(
        (cfg, me, mode, apply) in config().prop_flat_map(|cfg| {
            let n = cfg.monitors.len();
            (Just(cfg), machine(), proptest::collection::vec(machine(), n), any::<bool>())
        })
    ) {
        let assign: BTreeMap<_, _> = cfg
            .monitors
            .iter()
            .zip(mode)
            .map(|(m, to)| (m.id.clone(), to))
            .collect();
        let mode = Mode::Shared(assign);
        let mut state = State::initial(&cfg, me);
        state.on = Powered::both();
        for confirmed in [false, true] {
            let from = if confirmed { state.clone().confirmed() } else { state.clone() };
            let plans = [
                if apply {
                    Plan::apply_mode(&cfg, &from, &mode, &abilities(&cfg))
                } else {
                    Plan::to_mode(&cfg, &from, &mode, &abilities(&cfg))
                },
                Plan::power_off(&cfg, &from, me.other(), &abilities(&cfg)),
            ];
            for plan in plans.into_iter().flatten() {
                for (i, step) in plan.steps.iter().enumerate() {
                    let Step::Detach { machine, monitor } = step else { continue };
                    let switched = plan.steps[..i].iter().any(|s| matches!(
                        s,
                        Step::SwitchInput { monitor: m, to, .. } if m == monitor && *to == machine.other()
                    ));
                    prop_assert!(switched, "{step:?} sin su SwitchInput antes: {:?}", plan.steps);
                }
            }
        }
    }
}

/// El caso que encontró el diseño de la vuelta atrás: un «Aplicar modo» con el estado sin
/// confirmar pone `Attach` en la máquina que ya mostraba el monitor. Si falla un paso
/// después, deshacer ese `Attach` con un `Detach` la dejaba sin su monitor.
#[test]
fn undoing_an_attach_never_strips_the_monitor_the_machine_already_showed() {
    let cfg = Config {
        monitors: vec![MonitorConfig {
            id: "m0".into(),
            label: "0".into(),
            edid_model: None,
            edid_aliases: Vec::new(),
            wiring: [
                (Machine::Pc, Input::DisplayPort),
                (Machine::Mac, Input::Hdmi),
            ]
            .into(),
            ddc: None,
        }],
    };
    // La Mac cree que muestra m0 (sin confirmar) y de verdad lo muestra.
    let mut state = State::initial(&cfg, Machine::Mac);
    state.on = Powered::both();
    let truth: BTreeMap<_, _> = [("m0".into(), Machine::Mac)].into();
    let plan = Plan::apply_mode(&cfg, &state, &Mode::SoloMac, &abilities(&cfg)).unwrap();
    // [SwitchInput, Attach(Mac), Detach(Pc), UpdateInputGeometry]
    for fail_at in 0..plan.steps.len() {
        let mut sim = Sim {
            world: World::new(&cfg, &truth, Powered::both()),
            calls: 0,
            fail_at: Some(fail_at),
        };
        let mut next = state.clone();
        let _ = pollster::block_on(run(&cfg, &mut next, &plan, &mut sim));
        sim.world
            .check()
            .unwrap_or_else(|e| panic!("fallo en {fail_at}: {e}"));
    }
}
