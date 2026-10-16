//! Propiedades de punta a punta sobre el puesto simulado (QA ronda 2): secuencias
//! cualesquiera de cambios de modo y apagados, pedidos desde cualquiera de las dos
//! máquinas, con un fallo inyectado en cualquier paso.
//!
//! Tras cada operación: el desenlace es el esperado, el puesto físico coincide con
//! el estado del agente (también la geometría de los motores de entrada), la PC
//! está de acuerdo, nada imposible ocurrió y la Mac siempre tiene una pantalla.

use proptest::prelude::*;
use tandem_core::Abilities;
use tandem_core::{Machine, Mode, MonitorId, Plan, Step};
use tandem_link::manager::LinkKind;
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, SimError, TxnRun};
use tandemd::orchestrator::{Target, target};

#[derive(Debug, Clone)]
enum Op {
    Mode(Mode),
    Shutdown,
}

#[derive(Debug, Clone)]
struct Fault {
    index: prop::sample::Index,
    /// En los pasos que corren en las dos máquinas: cuál falla.
    on_mac: bool,
    kind: FaultKind,
}

#[derive(Debug, Clone, Copy)]
enum FaultKind {
    Fail,
    Hang,
    /// Se aplica y se cae el enlace activo antes de la respuesta.
    LostReply,
}

#[derive(Debug, Clone)]
struct Action {
    op: Op,
    /// Si la PC está prendida, pedirlo desde ella.
    from_pc: bool,
    fault: Option<Fault>,
}

fn mode() -> impl Strategy<Value = Mode> {
    let m = |id: &str| MonitorId::from(id);
    let machine = || prop_oneof![Just(Machine::Mac), Just(Machine::Pc)];
    prop_oneof![
        Just(Mode::SoloMac),
        Just(Mode::SoloPc),
        (machine(), machine())
            .prop_map(move |(a, b)| Mode::Shared([(m("m34"), a), (m("m32"), b)].into())),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    let op = prop_oneof![4 => mode().prop_map(Op::Mode), 1 => Just(Op::Shutdown)];
    let kind = prop_oneof![
        Just(FaultKind::Fail),
        Just(FaultKind::Hang),
        Just(FaultKind::LostReply)
    ];
    let fault =
        (any::<prop::sample::Index>(), any::<bool>(), kind).prop_map(|(index, on_mac, kind)| {
            Fault {
                index,
                on_mac,
                kind,
            }
        });
    (op, any::<bool>(), prop::option::weighted(0.5, fault))
        .prop_map(|(op, from_pc, fault)| Action { op, from_pc, fault })
}

async fn check_all(d: &Desk, case: &str) -> Result<(), TestCaseError> {
    let fail = |e: SimError| TestCaseError::fail(format!("{case}: {e}"));
    d.check().await.map_err(fail)?;
    d.check_geometry().map_err(fail)?;
    if d.mac.state().on.pc {
        d.pc_agrees().await.map_err(fail)?;
    }
    prop_assert!(
        d.world
            .geometry(Machine::Mac)
            .is_some_and(|g| !g.is_empty()),
        "{case}: la Mac se quedó sin pantallas"
    );
    prop_assert!(
        d.world.violations().is_empty(),
        "{case}: {:?}",
        d.world.violations()
    );
    Ok(())
}

async fn run(actions: Vec<Action>) -> Result<(), TestCaseError> {
    let d = Desk::new()
        .await
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
    for (i, a) in actions.into_iter().enumerate() {
        // Que llegue el estado final del cambio anterior a la otra máquina.
        d.settle().await;
        let on = if a.from_pc
            && d.world.power_of(Machine::Pc) == Power::On
            && !matches!(a.op, Op::Shutdown)
        {
            Machine::Pc
        } else {
            Machine::Mac
        };
        let from = d.agent(on).state();
        let plan = match &a.op {
            Op::Mode(m) => Plan::to_mode(d.config(), &from, m, &Abilities::pc_switches(d.config())),
            Op::Shutdown => Plan::power_off(
                d.config(),
                &from,
                Machine::Pc,
                &Abilities::pc_switches(d.config()),
            ),
        }
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let mut faulted = None;
        if let Some(f) = &a.fault
            && !plan.steps.is_empty()
        {
            let step = f.index.get(&plan.steps).clone();
            let machine = match target(&step) {
                Target::On(m) => m,
                Target::Both if f.on_mac => Machine::Mac,
                Target::Both => Machine::Pc,
            };
            let link = d.active_link(Machine::Mac).unwrap_or(LinkKind::Usb4);
            let action = match f.kind {
                // Un apagado colgado se da por hecho (lo prueba `adversarial.rs`).
                FaultKind::Hang
                    if step
                        != Step::PowerOff {
                            machine: Machine::Pc,
                        } =>
                {
                    FaultAction::Hang
                }
                FaultKind::Fail | FaultKind::Hang => FaultAction::Fail("falla inyectada".into()),
                FaultKind::LostReply => FaultAction::ApplyThenCut(link),
            };
            d.arm(FaultPlan::new().on(machine, step.clone(), action.clone()));
            faulted = Some((machine, step, action));
        }
        let case = format!("#{i} {:?} desde {on}, fallo {faulted:?}", a.op);
        let run: TxnRun = match &a.op {
            Op::Mode(m) => d.set_mode(on, m.clone()).await,
            Op::Shutdown => d.shutdown_pc().await,
        }
        .map_err(|e| TestCaseError::fail(format!("{case}: {e}")))?;
        let outcome = run.outcome();
        match &faulted {
            None => prop_assert_eq!(
                outcome,
                Some(Outcome::Committed),
                "{}\n{:#?}",
                case,
                run.events
            ),
            // Un corte puede no tocar la respuesta (paso local) o caer sobre un paso
            // posterior; cualquiera de los dos desenlaces limpios vale.
            Some((_, _, FaultAction::ApplyThenCut(_))) => prop_assert!(
                matches!(outcome, Some(Outcome::Committed | Outcome::RolledBack)),
                "{}\n{:#?}",
                case,
                run.events
            ),
            Some(_) => prop_assert_eq!(
                outcome,
                Some(Outcome::RolledBack),
                "{}\n{:#?}",
                case,
                run.events
            ),
        }
        check_all(&d, &case).await?;
        // El cable vuelve para la operación siguiente.
        for kind in LinkKind::ALL {
            d.net.restore(kind);
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        ..ProptestConfig::default()
    })]

    #[test]
    fn cualquier_secuencia_con_fallos_deja_el_puesto_coherente(
        actions in prop::collection::vec(action(), 1..6)
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        rt.block_on(run(actions))?;
    }
}
