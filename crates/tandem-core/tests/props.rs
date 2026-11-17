//! Invariantes del núcleo con pruebas de propiedades.
//!
//! El ejecutor de prueba lleva un modelo "físico" (qué máquinas están prendidas) que solo
//! cambia cuando un paso se ejecuta con éxito. Así se comprueba lo que de verdad le importa
//! al usuario, no solo lo que `State` cree. Los estados se generan desde las dos
//! perspectivas (DESIGN §3.7).

use std::collections::BTreeMap;
use std::future::Future;

use proptest::prelude::*;
use tandem_core::{
    Abilities, Align, Arrangement, Config, Desk, DeskHistory, DeskPatch, DeskStamp, DeskVersion,
    Executor, HISTORY_LEN, InputPrefs, Machine, MachineInfo, Machines, Os, PALETTE, Plan, Powered,
    Scale, Scene, ScreenSpec, Shortcuts, Side, State, Step, TxnError, run,
};

/// Ejecutor que falla exactamente en la llamada número `fail_at` (si la hay). Lleva también
/// lo que muestra cada monitor y el acomodo, y sabe deshacer esos pasos.
struct Sim {
    on: Powered,
    calls: usize,
    fail_at: Option<usize>,
    shows: BTreeMap<String, Machine>,
    layout: Option<Arrangement>,
    /// Lo que había antes de cada paso de escena hecho, para deshacerlo.
    before: Vec<(Step, Step)>,
}

impl Sim {
    fn new(on: Powered, fail_at: Option<usize>) -> Self {
        Sim {
            on,
            calls: 0,
            fail_at,
            shows: BTreeMap::new(),
            layout: None,
            before: Vec::new(),
        }
    }

    fn set(&mut self, step: &Step) {
        match step {
            Step::ShowMachine { monitor, machine } => {
                self.shows.insert(monitor.clone(), *machine);
            }
            Step::Arrange { layout } => self.layout = *layout,
            _ => {}
        }
    }
}

impl Executor for Sim {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        let n = self.calls;
        self.calls += 1;
        let r = if Some(n) == self.fail_at {
            Err(format!("fallo inyectado en la llamada {n}"))
        } else {
            match step {
                // El Wake-on-LAN sale de la otra, que tiene que estar prendida.
                Step::PowerOn { machine } if !self.on.is_on(machine.other()) => {
                    Err(format!("{} está apagada", machine.other()))
                }
                Step::PowerOn { machine } => {
                    self.on.set(*machine, true);
                    Ok(())
                }
                Step::PowerOff { machine } => {
                    self.on.set(*machine, false);
                    Ok(())
                }
                // Los planes de energía y de escena no cambian entradas.
                Step::SwitchMonitor { .. } => Err("sin monitores".to_owned()),
                Step::ShowMachine { monitor, .. } => {
                    // Los monitores de la prueba arrancan mostrando A.
                    let was = self.shows.get(monitor).copied().unwrap_or(Machine::A);
                    let undo = Step::ShowMachine {
                        monitor: monitor.clone(),
                        machine: was,
                    };
                    self.before.push((step.clone(), undo));
                    self.set(step);
                    Ok(())
                }
                Step::Arrange { .. } => {
                    let undo = Step::Arrange {
                        layout: self.layout,
                    };
                    self.before.push((step.clone(), undo));
                    self.set(step);
                    Ok(())
                }
                Step::Geometry => Ok(()),
            }
        };
        async move { r }
    }

    fn undo(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        if let Some(at) = self.before.iter().rposition(|(done, _)| done == step) {
            let (_, undo) = self.before.remove(at);
            self.set(&undo);
        }
        async { Ok(()) }
    }
}

fn machine() -> impl Strategy<Value = Machine> {
    prop_oneof![Just(Machine::A), Just(Machine::B)]
}

/// Quien orquesta está prendida; la otra, como sea.
fn state() -> impl Strategy<Value = (Machine, State)> {
    (machine(), any::<bool>()).prop_map(|(me, other_on)| {
        let mut s = State::initial(me);
        s.on.set(me.other(), other_on);
        (me, s)
    })
}

fn info() -> impl Strategy<Value = Option<MachineInfo>> {
    proptest::option::of(
        (
            prop_oneof![Just(Os::MacOs), Just(Os::Windows)],
            "[A-Za-z ]{1,12}",
            "#[0-9a-f]{6}",
        )
            .prop_map(|(os, name, color)| MachineInfo { os, name, color }),
    )
}

fn machines() -> impl Strategy<Value = Machines> {
    (info(), info()).prop_map(|(a, b)| Machines { a, b })
}

fn arrangement() -> impl Strategy<Value = Option<Arrangement>> {
    proptest::option::of(
        (
            proptest::sample::select(Side::ALL.to_vec()),
            proptest::sample::select(Align::ALL.to_vec()),
        )
            .prop_map(|(a_side, align)| Arrangement { a_side, align }),
    )
}

fn screen() -> impl Strategy<Value = ScreenSpec> {
    (
        machine(),
        "[a-z0-9]{1,6}",
        any::<i32>(),
        any::<i32>(),
        1u32..10_000,
        1u32..10_000,
        1u32..4000,
    )
        .prop_map(|(machine, id, x, y, width, height, milli)| ScreenSpec {
            machine,
            id,
            x,
            y,
            width,
            height,
            scale: Scale::from_milli(milli),
        })
}

proptest! {
    /// Encender o apagar la otra deja el estado con lo que de verdad se hizo, falle donde
    /// falle.
    #[test]
    fn the_state_follows_what_really_happened(
        (me, start) in state(),
        on in any::<bool>(),
        fail in proptest::option::of(0usize..2),
    ) {
        let other = me.other();
        let plan = if on {
            Plan::power_on(&start, other, &Abilities::both_wake()).unwrap()
        } else {
            Plan::power_off(&start, other)
        };
        let mut sim = Sim::new(start.on, fail);
        let mut s = start.clone();
        let r = pollster::block_on(run(&mut s, &plan, &mut sim));
        prop_assert_eq!(s.on, sim.on);
        prop_assert!(s.is_on(me), "quien orquesta nunca se apaga sola");
        match r {
            Ok(_) => prop_assert_eq!(s.is_on(other), on),
            Err(TxnError::Failed { .. }) => prop_assert_eq!(s, start),
        }
    }

    /// Lo que se declara en `tandem.toml` vuelve igual.
    #[test]
    fn the_declared_desk_round_trips(
        screens in proptest::collection::vec(screen(), 0..5),
        arrangement in arrangement(),
        machines in machines(),
    ) {
        let cfg = Config {
            machines,
            screens,
            arrangement,
            ..Config::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        prop_assert_eq!(back, cfg);
    }

    /// DECISIONS A1: para cualquier par de claves distintas, cada máquina calcula su lado
    /// por su cuenta y las dos coinciden: una es `A`, la otra `B`, y `A` es la menor.
    #[test]
    fn both_machines_agree_on_the_sides(
        own in proptest::array::uniform32(any::<u8>()),
        peer in proptest::array::uniform32(any::<u8>()),
    ) {
        prop_assume!(own != peer);
        let mine = Machine::of_keys(&own, &peer).unwrap();
        let theirs = Machine::of_keys(&peer, &own).unwrap();
        prop_assert_eq!(theirs, mine.other());
        let a = if mine == Machine::A { own } else { peer };
        prop_assert_eq!(a, own.min(peer));
    }

    /// Cambiar los lados dos veces deja el puesto como estaba, y una vez no pierde nada:
    /// cada pantalla sigue siendo de su máquina.
    #[test]
    fn swapping_sides_is_an_involution(
        screens in proptest::collection::vec(screen(), 0..5),
        arrangement in arrangement(),
        machines in machines(),
    ) {
        let cfg = Config {
            machines,
            screens,
            arrangement,
            ..Config::default()
        };
        let swapped = cfg.clone().swap_sides();
        prop_assert_eq!(swapped.screens.len(), cfg.screens.len());
        for (before, after) in cfg.screens.iter().zip(&swapped.screens) {
            prop_assert_eq!(after.machine, before.machine.other());
        }
        prop_assert_eq!(swapped.machines.get(Machine::A), cfg.machines.get(Machine::B));
        prop_assert_eq!(swapped.swap_sides(), cfg);
    }
}

/// Las dos máquinas ya conocidas, con sus colores de omisión: el punto de partida de un
/// puesto emparejado.
fn paired_desk() -> DeskVersion {
    DeskVersion {
        stamp: DeskStamp {
            rev: 1,
            at: 0,
            by: Machine::A,
        },
        desk: Desk {
            machines: Machines {
                a: Some(MachineInfo::new(Machine::A, Os::MacOs, None)),
                b: Some(MachineInfo::new(Machine::B, Os::Windows, None)),
            },
            ..Desk::default()
        },
    }
}

fn patch() -> impl Strategy<Value = DeskPatch> {
    prop_oneof![
        arrangement().prop_map(DeskPatch::Arrangement),
        (any::<bool>(), any::<bool>()).prop_map(|(cmd_ctrl, drag_files)| {
            DeskPatch::Prefs(InputPrefs {
                cmd_ctrl,
                drag_files,
            })
        }),
        proptest::collection::vec("[A-Z][a-z]{0,5}", 0..3)
            .prop_map(|lock| DeskPatch::Shortcuts(Shortcuts { lock })),
        proptest::collection::btree_set("[a-z]{1,4}", 0..3).prop_map(|ids| {
            DeskPatch::Scenes(
                ids.into_iter()
                    .map(|id| Scene::new(id.clone(), id.to_uppercase()))
                    .collect(),
            )
        }),
        full_scene().prop_map(DeskPatch::AddScene),
        full_scene().prop_map(DeskPatch::EditScene),
        "[a-c]".prop_map(|id| DeskPatch::DeleteScene { id }),
        (machine(), "[a-z]{1,3}", any::<bool>()).prop_map(|(machine, module, on)| {
            DeskPatch::Module {
                machine,
                module,
                on,
            }
        }),
        (
            machine(),
            proptest::option::of("[A-Z][a-z]{0,6}"),
            proptest::option::of(proptest::sample::select(PALETTE.to_vec()))
        )
            .prop_map(|(side, name, color)| DeskPatch::Machine {
                side,
                name,
                color: color.map(str::to_owned),
            }),
    ]
}

/// Lo que puede pasar en el puesto: un cambio en una máquina, o que le llegue a una
/// máquina uno de los mensajes que tiene en camino (cualquiera, no el más viejo).
#[derive(Debug, Clone)]
enum Op {
    Edit {
        at: Machine,
        patch: DeskPatch,
        now: u64,
    },
    Deliver {
        to: Machine,
        pick: usize,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (machine(), patch(), 0u64..6).prop_map(|(at, patch, now)| Op::Edit { at, patch, now }),
        (machine(), any::<usize>()).prop_map(|(to, pick)| Op::Deliver { to, pick }),
    ]
}

/// Una máquina del puesto: su versión y los mensajes que le llegarán.
struct Replica {
    me: Machine,
    current: DeskVersion,
    inbox: Vec<DeskVersion>,
    history: DeskHistory,
}

impl Replica {
    fn new(me: Machine) -> Self {
        Replica {
            me,
            current: paired_desk(),
            inbox: Vec::new(),
            history: DeskHistory::new(paired_desk()),
        }
    }

    fn adopt(&mut self, v: DeskVersion) {
        self.history.record(v.clone());
        self.current = v;
    }

    fn deliver(&mut self, pick: usize) {
        if self.inbox.is_empty() {
            return;
        }
        let msg = self.inbox.remove(pick % self.inbox.len());
        if let Some(next) = self.current.receive(&msg) {
            self.adopt(next);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    })]

    /// DESIGN §4.4 / D5: con cambios en las dos máquinas a la vez y los mensajes llegando en
    /// cualquier orden, «gana el último» deja a las dos con el mismo puesto, y es el del
    /// cambio con el sello más alto.
    #[test]
    fn last_writer_wins_converges_on_both_machines(ops in proptest::collection::vec(op(), 0..40)) {
        let mut a = Replica::new(Machine::A);
        let mut b = Replica::new(Machine::B);
        let mut made = vec![paired_desk()];
        for op in ops {
            match op {
                Op::Edit { at, patch, now } => {
                    let (here, there) = if at == Machine::A { (&mut a, &mut b) } else { (&mut b, &mut a) };
                    if let Ok(Some(next)) = here.current.edit(patch, here.me, now) {
                        there.inbox.push(next.clone());
                        made.push(next.clone());
                        here.adopt(next);
                    }
                }
                Op::Deliver { to: Machine::A, pick } => a.deliver(pick),
                Op::Deliver { to: Machine::B, pick } => b.deliver(pick),
            }
        }
        while !a.inbox.is_empty() || !b.inbox.is_empty() {
            a.deliver(0);
            b.deliver(0);
        }
        prop_assert_eq!(&a.current, &b.current);
        let last = made
            .iter()
            .reduce(|x, y| if y.stamp.wins_over(&x.stamp) { y } else { x })
            .unwrap();
        prop_assert_eq!(&a.current, last);
        prop_assert!(a.history.len() <= HISTORY_LEN);
        // Ningún parche deja un puesto inválido (atajos repetidos, escenas sin nombre…).
        prop_assert!(made.iter().all(|v| v.desk.validate().is_ok()));
        // Los sellos nunca se repiten: el orden es total.
        for (i, x) in made.iter().enumerate() {
            for y in &made[i + 1..] {
                prop_assert_ne!(x.stamp, y.stamp);
            }
        }
    }

    /// Deshacer vuelve exactamente al puesto de la versión pedida, como una versión nueva
    /// que la otra máquina adopta igual.
    #[test]
    fn undo_restores_exactly_the_requested_version(
        patches in proptest::collection::vec(patch(), 1..30),
        pick in any::<usize>(),
        now in 0u64..1000,
    ) {
        let mut a = Replica::new(Machine::A);
        let mut b = Replica::new(Machine::B);
        for (i, p) in patches.into_iter().enumerate() {
            if let Ok(Some(next)) = a.current.edit(p, Machine::A, i as u64) {
                b.inbox.push(next.clone());
                a.adopt(next);
            }
        }
        while !b.inbox.is_empty() {
            b.deliver(0);
        }
        let versions: Vec<DeskVersion> = a.history.versions().cloned().collect();
        let target = &versions[pick % versions.len()];
        let undone = a.history.undo(&a.current, target.stamp.rev, Machine::B, now).unwrap();
        match undone {
            None => prop_assert_eq!(&target.desk, &a.current.desk),
            Some(v) => {
                prop_assert_eq!(&v.desk, &target.desk);
                prop_assert_eq!(v.stamp.rev, a.current.stamp.rev + 1);
                // La otra la adopta tal cual.
                prop_assert_eq!(b.current.receive(&v), Some(v.clone()));
            }
        }
    }
}

fn scene() -> impl Strategy<Value = Scene> {
    (
        proptest::collection::btree_map("m[0-3]", machine(), 0..4),
        arrangement(),
        proptest::option::of(machine()),
    )
        .prop_map(|(monitors, layout, require_on)| Scene {
            monitors: monitors.into_iter().collect(),
            layout,
            require_on,
            ..Scene::new("s", "S")
        })
}

/// Una escena entera con id de un conjunto chico (para que editar y borrar acierten) y, a
/// veces, un atajo que puede chocar con el de otra escena.
fn full_scene() -> impl Strategy<Value = Scene> {
    (
        scene(),
        "[a-c]",
        "[A-Z][a-z]{0,5}",
        proptest::option::of(proptest::sample::select(vec!["Digit1", "Digit2", "Digit3"])),
        proptest::sample::select(vec!["puesto", "laptop", "play"]),
    )
        .prop_map(|(scene, id, name, key, icon)| Scene {
            id,
            name,
            icon: icon.to_owned(),
            shortcut: key
                .map(|k| vec!["ControlLeft".to_owned(), "AltLeft".to_owned(), k.to_owned()])
                .unwrap_or_default(),
            ..scene
        })
}

proptest! {
    /// Aplicar una escena, falle donde falle: con éxito, los monitores y el acomodo quedan
    /// como dice la escena y lo que necesita está encendido; con un fallo, los monitores y el
    /// acomodo vuelven exactamente a como estaban (la energía queda como se hizo).
    #[test]
    fn a_scene_is_all_or_nothing(
        (me, start) in state(),
        scene in scene(),
        current in arrangement(),
        fail in proptest::option::of(0usize..8),
    ) {
        let plan = scene.plan(&start, current, &Abilities::both_wake()).unwrap();
        let mut sim = Sim::new(start.on, fail);
        sim.layout = current;
        let shows = sim.shows.clone();
        let mut s = start.clone();
        let r = pollster::block_on(run(&mut s, &plan, &mut sim));
        prop_assert_eq!(s.on, sim.on);
        prop_assert!(s.is_on(me));
        match r {
            Ok(_) => {
                prop_assert!(scene.holds(&s));
                for (monitor, machine) in &scene.monitors {
                    prop_assert_eq!(sim.shows.get(monitor), Some(machine));
                }
                if scene.layout.is_some() {
                    prop_assert_eq!(sim.layout, scene.layout);
                }
            }
            Err(_) => {
                // Lo que no había se queda mostrando lo de antes (A en la prueba).
                for (monitor, machine) in &sim.shows {
                    prop_assert_eq!(shows.get(monitor).unwrap_or(&Machine::A), machine);
                }
                prop_assert_eq!(sim.layout, current);
            }
        }
    }

    /// Los lados cambiados dos veces dejan la escena igual, y lo que necesita sigue siendo
    /// lo mismo visto desde el otro lado.
    #[test]
    fn swapping_a_scene_is_an_involution(scene in scene()) {
        let swapped = scene.clone().swapped();
        let needs: std::collections::BTreeSet<Machine> =
            scene.needs().into_iter().map(Machine::other).collect();
        prop_assert_eq!(swapped.needs(), needs);
        prop_assert_eq!(swapped.swapped(), scene);
    }
}
