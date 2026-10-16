//! Escenarios espejo (T-084, DESIGN §3.7): la PC es la principal. El usuario trabaja en
//! la PC y la Mac está suspendida o apagada; los modos, la vigilancia de energía y el
//! apagado funcionan igual que con la Mac de principal (`scenarios.rs`, `power_cut.rs`).
//! El teclado y mouse espejo están en `hub.rs`.
//!
//! Lo único que no es espejo es el hardware: las entradas las sigue cambiando la PC (es
//! la que tiene DDC, según su anuncio de capacidades), así que con la PC de principal
//! nunca hace falta despertar a la Mac para mover un monitor; solo para que muestre algo.
//!
//! Todos con el reloj de tokio pausado.

use std::time::Duration;

use tandem_core::{BuiltinDisplay, Input, Machine, Mode, MonitorId, Plan, Step};
use tandem_link::manager::LinkKind;
use tandem_proto::{PowerEvent, TxnEvent};
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, Screen, TxnRun};
use tandemd::orchestrator::{Target, target};
use tokio::sync::broadcast;
use tokio::time::Instant;

fn m34() -> MonitorId {
    "m34".into()
}

fn m32() -> MonitorId {
    "m32".into()
}

/// Compartido visto desde la PC: el 32" a la Mac, el 34" se queda en la PC.
fn shared() -> Mode {
    Mode::Shared([(m34(), Machine::Pc), (m32(), Machine::Mac)].into())
}

async fn pc_desk() -> Desk {
    Desk::builder()
        .principal(Machine::Pc)
        .build()
        .await
        .unwrap()
}

async fn committed(d: &Desk, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}

fn planned(run: &TxnRun) -> &[Step] {
    match run.events.first() {
        Some(TxnEvent::Started { steps, .. }) => steps,
        other => panic!("se esperaba Started: {other:?}"),
    }
}

async fn next_power(rx: &mut broadcast::Receiver<PowerEvent>) -> PowerEvent {
    tokio::time::timeout(Duration::from_secs(3600), rx.recv())
        .await
        .expect("ningún aviso de energía")
        .unwrap()
}

fn lost_after(d: &Desk) -> Duration {
    d.pc.config().power.peer_lost_after()
}

/// Espejo de `recorrido_solo_mac_compartido_solo_windows_apagado`.
#[tokio::test(start_paused = true)]
async fn recorrido_solo_windows_compartido_solo_mac_apagado_de_la_mac() {
    let d = pc_desk().await;
    d.check().await.unwrap();
    d.check_geometry().unwrap();
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    // La PC nunca está apagada para sí misma.
    assert!(d.pc.state().is_on(Machine::Pc));
    assert!(!d.pc.state().is_on(Machine::Mac));

    // Solo Windows → Compartido: Wake-on-LAN a la Mac, esperar a su agente y mover el 32".
    let start = Instant::now();
    let run = d.set_mode(Machine::Pc, shared()).await.unwrap();
    assert_eq!(
        planned(&run).first(),
        Some(&Step::PowerOn {
            machine: Machine::Mac
        })
    );
    committed(&d, &run).await;
    assert!(
        start.elapsed() >= Duration::from_secs(8),
        "esperó a que la Mac despertara"
    );
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    assert_eq!(d.active_link(Machine::Pc), Some(LinkKind::Usb4));
    d.agrees(Machine::Mac).await.unwrap();

    // Compartido → Solo Mac: la PC se queda sin monitores; la Mac apaga su pantalla.
    let run = d.set_mode(Machine::Pc, Mode::SoloMac).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.builtin(), BuiltinDisplay::Off);
    assert_eq!(d.world.geometry(Machine::Pc), Some(vec![]));
    assert_eq!(
        d.world.geometry(Machine::Mac),
        Some(vec![Screen::Monitor(m34()), Screen::Monitor(m32())])
    );
    d.agrees(Machine::Mac).await.unwrap();

    // Apagado de la Mac pedido desde la PC: todo vuelve a la PC antes de apagarla.
    let run = d.shutdown(Machine::Pc, Machine::Mac).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    let calls = d.world.calls();
    let off = calls
        .iter()
        .position(|(_, s)| {
            *s == Step::PowerOff {
                machine: Machine::Mac,
            }
        })
        .unwrap();
    assert_eq!(calls[off].0, Machine::Mac, "se apaga ella misma");
    assert!(
        calls[..off]
            .iter()
            .filter(|(by, s)| *by == Machine::Pc && matches!(s, Step::SwitchInput { .. }))
            .count()
            >= 2,
        "los dos monitores volvieron a la PC antes de apagar"
    );
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    assert_eq!(d.active_link(Machine::Pc), None);
    assert!(
        d.world.violations().is_empty(),
        "{:?}",
        d.world.violations()
    );
}

/// Con la Mac dormida, todo lo que es de la PC se hace sin despertarla: la PC cambia
/// las entradas y no toca el escritorio de la Mac.
#[tokio::test(start_paused = true)]
async fn con_la_mac_dormida_la_pc_no_la_despierta_para_lo_suyo() {
    let d = pc_desk().await;
    // Lo que creía la Mac al dormirse no importa: la PC sabe que todo es suyo.
    let run = d.set_mode(Machine::Pc, Mode::SoloPc).await.unwrap();
    assert!(planned(&run).is_empty(), "{:?}", planned(&run));
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    assert!(
        d.world.calls().iter().all(|(m, _)| *m == Machine::Pc),
        "{:?}",
        d.world.calls()
    );
}

/// Espejo de `la_pc_tambien_orquesta`: con las dos despiertas, la Mac también orquesta.
#[tokio::test(start_paused = true)]
async fn la_mac_tambien_orquesta() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    d.agrees(Machine::Mac).await.unwrap();

    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let mut rx = d.pc.subscribe();
    let want = d.mac.state();
    rx.wait_for(|s| *s == want).await.unwrap();
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}

/// Espejo de `every_step_rolls_back`: falle el paso que falle del tramo, en la máquina
/// que sea, todo vuelve al punto de partida.
async fn every_step_rolls_back(mode: Mode, action: FaultAction) {
    let prepare = || async { pc_desk().await };
    let steps = {
        let d = prepare().await;
        let from = d.pc.state();
        Plan::to_mode(d.config(), &from, &mode, &d.pc.abilities())
            .unwrap()
            .steps
    };
    assert!(!steps.is_empty());
    for (index, step) in steps.iter().enumerate() {
        let machines = match target(step) {
            Target::On(m) => vec![m],
            Target::Both => vec![Machine::Mac, Machine::Pc],
        };
        for machine in machines {
            let case = format!("{mode:?}: {action:?} en {machine} al {step:?}");
            let d = prepare().await;
            let (before, physical) = (d.pc.state(), d.world.physical());
            d.arm(FaultPlan::new().on(machine, step.clone(), action.clone()));

            let run = d.set_mode(Machine::Pc, mode.clone()).await.unwrap();
            assert_eq!(
                run.outcome(),
                Some(Outcome::RolledBack),
                "{case}\n{:#?}",
                run.events
            );
            assert_eq!(run.failure().map(|f| f.0), Some(index as u32), "{case}");

            let after = d.pc.state();
            assert_eq!(after.owner, before.owner, "{case}");
            assert!(after.is_on(Machine::Pc), "{case}");
            if let Err(e) = d.check().await {
                panic!("{case}: {e}");
            }
            if let Err(e) = d.check_geometry() {
                panic!("{case}: {e}");
            }
            let now = d.world.physical();
            assert_eq!(now.inputs, physical.inputs, "{case}");
            assert_eq!(
                now.desktops[&Machine::Pc],
                physical.desktops[&Machine::Pc],
                "{case}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn fallo_en_cada_paso_hacia_compartido_desde_la_pc() {
    every_step_rolls_back(shared(), FaultAction::Fail("falla inyectada".into())).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_en_cada_paso_hacia_compartido_desde_la_pc() {
    every_step_rolls_back(shared(), FaultAction::Hang).await;
}

// La vigilancia de energía, espejo de `power_cut.rs`: ahora la que se queda sin corriente
// (o se suspende) es la Mac y la que vigila es la PC.

/// Espejo de `corte_de_corriente_en_compartido`.
#[tokio::test(start_paused = true)]
async fn la_mac_se_suspende_en_compartido() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    d.agrees(Machine::Mac).await.unwrap();
    let mut power = d.pc.subscribe_power();
    let before = d.pc.state();

    let start = Instant::now();
    d.pull_plug(Machine::Mac);
    tokio::time::sleep(lost_after(&d) - Duration::from_secs(1)).await;
    assert!(
        d.pc.state().is_on(Machine::Mac),
        "dentro del umbral no cambia nada"
    );
    assert!(power.try_recv().is_err());

    let PowerEvent::PeerLost {
        machine,
        reason,
        stranded,
    } = next_power(&mut power).await;
    let waited = start.elapsed();
    assert!(
        waited >= lost_after(&d) && waited < lost_after(&d) + Duration::from_secs(2),
        "{waited:?}"
    );
    assert_eq!(machine, Machine::Mac);
    assert!(reason.starts_with("La Mac no responde"), "{reason}");
    assert_eq!(stranded, [m32()]);

    let mut want = before.clone();
    want.on.mac = false;
    assert_eq!(d.pc.state(), want);
    // El 32" en negro; el 34" y la geometría en la PC, que sigue prendida.
    d.check().await.unwrap();
    d.check_geometry().unwrap();
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    assert!(
        d.world.violations().is_empty(),
        "{:?}",
        d.world.violations()
    );

    // Diferencia de hardware con el espejo: la PC cambia la entrada del 32" ella sola,
    // sin despertar a la Mac.
    let run = d.set_mode(Machine::Pc, Mode::SoloPc).await.unwrap();
    assert!(
        !planned(&run)
            .iter()
            .any(|s| matches!(s, Step::PowerOn { .. }))
    );
    assert!(planned(&run).contains(&Step::SwitchInput {
        monitor: m32(),
        from: Machine::Mac,
        to: Machine::Pc,
        input: Input::DisplayPort,
        by: Machine::Pc,
    }));
    committed(&d, &run).await;
    assert_eq!(d.world.shows(&m32()), Some(Machine::Pc));
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
}

/// Espejo de `corte_de_corriente_en_compartido`, segunda mitad: volver a Compartido
/// empieza por despertar a la Mac.
#[tokio::test(start_paused = true)]
async fn tras_la_suspension_compartido_despierta_a_la_mac() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    let mut power = d.pc.subscribe_power();
    d.pull_plug(Machine::Mac);
    next_power(&mut power).await;

    let run = d.set_mode(Machine::Pc, shared()).await.unwrap();
    assert_eq!(
        planned(&run).first(),
        Some(&Step::PowerOn {
            machine: Machine::Mac
        })
    );
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::On);
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    d.agrees(Machine::Mac).await.unwrap();
}

/// Espejo de `una_caida_breve_de_los_enlaces_no_cambia_pc_on`.
#[tokio::test(start_paused = true)]
async fn una_caida_breve_de_los_enlaces_no_apaga_a_la_mac() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    let mut power = d.pc.subscribe_power();
    let before = d.pc.state();

    for kind in LinkKind::ALL {
        d.net.cut(kind);
    }
    tokio::time::sleep(lost_after(&d) - Duration::from_secs(3)).await;
    assert_eq!(d.active_link(Machine::Pc), None, "la PC no ve a la Mac");
    for kind in LinkKind::ALL {
        d.net.restore(kind);
    }
    d.wait_link(LinkKind::Usb4).await.unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;

    assert_eq!(d.pc.state(), before);
    assert!(power.try_recv().is_err());
    let run = d.set_mode(Machine::Pc, Mode::SoloPc).await.unwrap();
    assert!(
        !planned(&run)
            .iter()
            .any(|s| matches!(s, Step::PowerOn { .. }))
    );
    committed(&d, &run).await;
}

/// Espejo de `un_apagado_pedido_no_es_un_corte`.
#[tokio::test(start_paused = true)]
async fn apagar_la_mac_desde_la_pc_no_es_un_corte() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    let mut power = d.pc.subscribe_power();
    committed(&d, &d.shutdown(Machine::Pc, Machine::Mac).await.unwrap()).await;
    tokio::time::sleep(lost_after(&d) * 4).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    assert!(!d.pc.state().is_on(Machine::Mac));
    assert!(power.try_recv().is_err());
    d.check().await.unwrap();
}

/// Espejo de `la_pc_encendida_a_mano_tras_el_corte_vuelve_a_contar`.
#[tokio::test(start_paused = true)]
async fn la_mac_despertada_a_mano_vuelve_a_contar() {
    let d = pc_desk().await;
    committed(&d, &d.set_mode(Machine::Pc, shared()).await.unwrap()).await;
    let mut power = d.pc.subscribe_power();
    d.pull_plug(Machine::Mac);
    next_power(&mut power).await;

    d.press_power_button(Machine::Mac);
    let mut states = d.pc.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3600),
        states.wait_for(|s| s.is_on(Machine::Mac)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(d.pc.state().owner[&m32()], Machine::Mac);
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}
