//! Escenarios de punta a punta del puesto (docs/TESTING.md, «Simulación del puesto»).
//!
//! Todos con el reloj de tokio pausado: el arranque de la PC (45 s), los pings de
//! los enlaces y los tiempos máximos de los pasos corren en tiempo virtual.

use std::time::Duration;

use tandem_core::{BuiltinDisplay, Machine, Mode, MonitorId, Plan, Step};
use tandem_link::manager::LinkKind;
use tandem_sim::world::LC34G55T_READS;
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, Screen, TxnRun};
use tandemd::orchestrator::{Target, target};

fn m34() -> MonitorId {
    "m34".into()
}

fn m32() -> MonitorId {
    "m32".into()
}

/// Compartido: el 34" a la PC, el 32" se queda en la Mac.
fn shared() -> Mode {
    Mode::Shared([(m34(), Machine::Pc)].into())
}

async fn committed(d: &Desk, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}

#[tokio::test(start_paused = true)]
async fn recorrido_solo_mac_compartido_solo_windows_apagado() {
    let d = Desk::new().await.unwrap();
    d.check().await.unwrap();
    assert_eq!(d.world.power(), Power::Off);

    // Solo Mac → Compartido: Wake-on-LAN, esperar al agente de la PC y mover el 34".
    let start = tokio::time::Instant::now();
    let run = d.set_mode(Machine::Mac, shared()).await.unwrap();
    committed(&d, &run).await;
    assert!(
        start.elapsed() >= Duration::from_secs(45),
        "esperó el arranque"
    );
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    assert_eq!(d.active_link(Machine::Mac), Some(LinkKind::Usb4));
    d.pc_agrees().await.unwrap();

    // Compartido → Solo Windows: la Mac se queda sin monitores y enciende su pantalla.
    let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.builtin(), BuiltinDisplay::Extend);
    assert_eq!(d.world.geometry(Machine::Mac), Some(vec![Screen::Builtin]));
    assert_eq!(
        d.world.geometry(Machine::Pc),
        Some(vec![Screen::Monitor(m34()), Screen::Monitor(m32())])
    );
    d.pc_agrees().await.unwrap();

    // Apagado: todo vuelve a la Mac antes de apagar la PC.
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power(), Power::Off);
    assert_eq!(d.world.builtin(), BuiltinDisplay::Off);
    let calls = d.world.calls();
    let off = calls
        .iter()
        .position(|(_, s)| *s == Step::PowerOffPc)
        .unwrap();
    assert!(
        calls[..off]
            .iter()
            .filter(|(_, s)| matches!(s, Step::SwitchInput { .. }))
            .count()
            >= 4,
        "los dos monitores volvieron a la Mac antes de apagar"
    );
    // Sin la PC no queda ningún enlace con su agente.
    assert_eq!(d.active_link(Machine::Mac), None);
    assert!(d.world.violations().is_empty());
}

#[tokio::test(start_paused = true)]
async fn el_34_siempre_dice_0x09_y_manda_el_estado_del_agente() {
    let d = Desk::new().await.unwrap();
    assert_eq!(d.world.read_input_code(&m34()), Some(LC34G55T_READS));

    let run = d.set_mode(Machine::Mac, shared()).await.unwrap();
    committed(&d, &run).await;
    // En DisplayPort sigue diciendo 0x09; el agente sabe que está en la PC.
    assert_eq!(d.world.read_input_code(&m34()), Some(LC34G55T_READS));
    assert_eq!(d.mac.state().owner[&m34()], Machine::Pc);

    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.read_input_code(&m34()), Some(LC34G55T_READS));
    assert_eq!(d.world.shows(&m34()), Some(Machine::Mac));
    // Los códigos del perfil del 34" (DP 0x0F, HDMI 0x06), no los MCCS.
    assert_eq!(d.world.ddc_writes(), [(m34(), 0x0F), (m34(), 0x06)]);
}

#[tokio::test(start_paused = true)]
async fn la_pc_tambien_orquesta() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    d.pc_agrees().await.unwrap();

    let run = d.set_mode(Machine::Pc, Mode::SoloPc).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    // La Mac recibe el estado final por el enlace y el puesto coincide.
    let mut rx = d.mac.subscribe();
    let want = d.pc.state();
    rx.wait_for(|s| *s == want).await.unwrap();
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}

/// Un tramo del recorrido, con el puesto preparado en su punto de partida.
#[derive(Debug, Clone, Copy)]
enum Leg {
    ToShared,
    ToSoloPc,
    Shutdown,
}

impl Leg {
    async fn prepare(self) -> Desk {
        let d = Desk::new().await.unwrap();
        let before: &[Mode] = match self {
            Leg::ToShared => &[],
            Leg::ToSoloPc => &[shared()],
            Leg::Shutdown => &[shared(), Mode::SoloPc],
        };
        for mode in before {
            committed(&d, &d.set_mode(Machine::Mac, mode.clone()).await.unwrap()).await;
        }
        d
    }

    fn plan(self, d: &Desk) -> Plan {
        let (cfg, from) = (d.config(), d.mac.state());
        match self {
            Leg::ToShared => Plan::to_mode(cfg, &from, &shared()),
            Leg::ToSoloPc => Plan::to_mode(cfg, &from, &Mode::SoloPc),
            Leg::Shutdown => Plan::shutdown_pc(cfg, &from),
        }
        .unwrap()
    }

    async fn run(self, d: &Desk) -> TxnRun {
        match self {
            Leg::ToShared => d.set_mode(Machine::Mac, shared()).await,
            Leg::ToSoloPc => d.set_mode(Machine::Mac, Mode::SoloPc).await,
            Leg::Shutdown => d.shutdown_pc().await,
        }
        .unwrap()
    }
}

/// Inyecta `action` en cada paso del tramo, en cada máquina que lo ejecuta, y
/// comprueba que todo vuelve al punto de partida.
async fn every_step_rolls_back(leg: Leg, action: FaultAction) {
    let steps = leg.plan(&leg.prepare().await).steps;
    assert!(!steps.is_empty());
    for (index, step) in steps.iter().enumerate() {
        let machines = match target(step) {
            Target::On(m) => vec![m],
            Target::Both => vec![Machine::Mac, Machine::Pc],
        };
        for machine in machines {
            let case = format!("{leg:?}: {action:?} en {machine} al {step:?}");
            let d = leg.prepare().await;
            let (before, physical) = (d.mac.state(), d.world.physical());
            d.arm(FaultPlan::new().on(machine, step.clone(), action.clone()));

            let run = leg.run(&d).await;
            assert_eq!(
                run.outcome(),
                Some(Outcome::RolledBack),
                "{case}\n{:#?}",
                run.events
            );
            assert_eq!(run.failure().map(|f| f.0), Some(index as u32), "{case}");

            // El agente vuelve al estado de partida (la PC encendida puede quedar
            // encendida) y el puesto físico coincide con él, también la geometría de
            // los motores de entrada.
            let after = d.mac.state();
            assert_eq!(after.owner, before.owner, "{case}");
            assert_eq!(after.builtin_pref, before.builtin_pref, "{case}");
            let powered_on = steps[..index].contains(&Step::PowerOnPc);
            assert_eq!(after.pc_on, before.pc_on || powered_on, "{case}");
            if let Err(e) = d.check().await {
                panic!("{case}: {e}");
            }
            if let Err(e) = d.check_geometry() {
                panic!("{case}: {e}");
            }
            let now = d.world.physical();
            assert_eq!(now.inputs, physical.inputs, "{case}");
            assert_eq!(now.desktops, physical.desktops, "{case}");
            assert_eq!(now.builtin, physical.builtin, "{case}");
            if after.pc_on {
                d.pc_agrees().await.unwrap();
            }
        }
    }
}

fn fail() -> FaultAction {
    FaultAction::Fail("falla inyectada".into())
}

#[tokio::test(start_paused = true)]
async fn fallo_en_cada_paso_hacia_compartido() {
    every_step_rolls_back(Leg::ToShared, fail()).await;
}

#[tokio::test(start_paused = true)]
async fn fallo_en_cada_paso_hacia_solo_windows() {
    every_step_rolls_back(Leg::ToSoloPc, fail()).await;
}

#[tokio::test(start_paused = true)]
async fn fallo_en_cada_paso_del_apagado() {
    every_step_rolls_back(Leg::Shutdown, fail()).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_en_cada_paso_hacia_compartido() {
    every_step_rolls_back(Leg::ToShared, FaultAction::Hang).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_en_cada_paso_hacia_solo_windows() {
    every_step_rolls_back(Leg::ToSoloPc, FaultAction::Hang).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_en_cada_paso_del_apagado() {
    every_step_rolls_back(Leg::Shutdown, FaultAction::Hang).await;
}

#[tokio::test(start_paused = true)]
async fn caida_del_enlace_preferido_entre_pasos_sigue_por_ethernet() {
    let d = Leg::ToSoloPc.prepare().await;
    assert_eq!(d.active_link(Machine::Mac), Some(LinkKind::Usb4));
    // Mientras la Mac quita el 32" de su escritorio se desenchufa el USB4; el paso
    // tarda lo bastante para que el gestor de enlaces lo note.
    let detach = Step::Detach {
        machine: Machine::Mac,
        monitor: m32(),
    };
    d.arm(
        FaultPlan::new()
            .on(
                Machine::Mac,
                detach.clone(),
                FaultAction::CutLink(LinkKind::Usb4),
            )
            .on(
                Machine::Mac,
                detach,
                FaultAction::Delay(Duration::from_secs(3)),
            ),
    );
    let run = Leg::ToSoloPc.run(&d).await;
    committed(&d, &run).await;
    assert_eq!(d.active_link(Machine::Mac), Some(LinkKind::Ethernet));
    d.pc_agrees().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn caida_del_enlace_preferido_con_la_respuesta_en_el_aire() {
    let d = Leg::ToSoloPc.prepare().await;
    // La PC agrega el 32" a su escritorio y el USB4 se cae antes de que la respuesta
    // llegue: la Mac no sabe que el paso se hizo, lo da por vencido y vuelve atrás
    // por Ethernet. La PC no puede quedarse con el 32" de fantasma.
    let attach = Step::Attach {
        machine: Machine::Pc,
        monitor: m32(),
    };
    d.arm(FaultPlan::new().on(
        Machine::Pc,
        attach,
        FaultAction::ApplyThenCut(LinkKind::Usb4),
    ));
    let before = d.world.physical();
    let run = Leg::ToSoloPc.run(&d).await;
    assert_eq!(
        run.outcome(),
        Some(Outcome::RolledBack),
        "{:#?}",
        run.events
    );
    assert!(
        run.failure()
            .is_some_and(|(_, e)| e.contains("sin respuesta")),
        "{:?}",
        run.failure()
    );
    d.check().await.unwrap();
    assert_eq!(d.world.physical(), before);
    assert_eq!(d.active_link(Machine::Mac), Some(LinkKind::Ethernet));
    d.pc_agrees().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn solo_con_wifi_el_control_sigue() {
    let d = Leg::ToSoloPc.prepare().await;
    d.arm(
        FaultPlan::new()
            .cut_link_at(Duration::ZERO, LinkKind::Usb4)
            .cut_link_at(Duration::from_secs(1), LinkKind::Ethernet),
    );
    d.wait_link(LinkKind::Wifi).await.unwrap();
    let run = Leg::ToSoloPc.run(&d).await;
    committed(&d, &run).await;
    d.pc_agrees().await.unwrap();

    // Vuelve el USB4 y el gestor regresa a él.
    d.net.restore(LinkKind::Usb4);
    d.wait_link(LinkKind::Usb4).await.unwrap();
}
