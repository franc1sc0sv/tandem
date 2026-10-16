//! QA adversarial de punta a punta (T-062, ronda 2): lo que los escenarios de
//! `scenarios.rs` no cubren. Todo con el reloj de tokio pausado.

use std::time::Duration;

use tandem_core::{BuiltinDisplay, Input, Machine, Mode, MonitorId, Step};
use tandem_link::manager::LinkKind;
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, SimError, TxnRun};
use tandemd::orchestrator::PEER_NOT_CONNECTED;

fn m34() -> MonitorId {
    "m34".into()
}

fn m32() -> MonitorId {
    "m32".into()
}

fn shared() -> Mode {
    Mode::Shared([(m34(), Machine::Pc)].into())
}

async fn committed(d: &Desk, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check().await.unwrap();
    d.check_geometry().unwrap();
}

/// Bug de la ronda 2: cada agente solo se cuidaba de sus propias transacciones.
/// Un cambio pedido en la Mac y otro en la PC a la vez se mezclaban paso a paso en
/// el mismo puesto; los dos terminaban «aplicados», cada agente con un estado
/// distinto y el puesto sin coincidir con ninguno (el 34" en la Mac y en el
/// escritorio de la PC a la vez).
#[tokio::test(start_paused = true)]
async fn cambios_simultaneos_desde_las_dos_maquinas_no_se_mezclan() {
    for (on_mac, on_pc) in [
        (Mode::SoloMac, Mode::SoloPc),
        (Mode::SoloPc, Mode::SoloMac),
        (Mode::SoloPc, Mode::Shared([(m32(), Machine::Pc)].into())),
    ] {
        let d = Desk::new().await.unwrap();
        committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
        d.pc_agrees().await.unwrap();

        let (a, b) = tokio::join!(
            d.set_mode(Machine::Mac, on_mac.clone()),
            d.set_mode(Machine::Pc, on_pc.clone())
        );
        let case = format!("{on_mac:?} / {on_pc:?}: {a:?} {b:?}");
        for run in [&a, &b] {
            match run {
                Ok(run) => assert_ne!(run.outcome(), Some(Outcome::Inconsistent), "{case}"),
                Err(SimError::Rejected(_)) => {}
                Err(e) => panic!("{case}: {e}"),
            }
        }
        if let Err(e) = d.check().await {
            panic!("{case}: {e}");
        }
        d.check_geometry().unwrap();
        d.pc_agrees().await.unwrap();

        // Terminados los dos, se puede volver a cambiar desde cualquiera.
        committed(&d, &d.set_mode(Machine::Pc, Mode::SoloPc).await.unwrap()).await;
        d.pc_agrees().await.unwrap();
    }
}

/// Todo en la Mac según el puesto físico: los monitores la muestran, están en su
/// escritorio y la pantalla integrada sigue la preferencia (apagada).
fn all_on_mac(d: &Desk) {
    let p = d.world.physical();
    assert!(p.inputs.values().all(|i| *i == Input::Hdmi), "{p:?}");
    assert_eq!(p.desktops[&Machine::Mac], [m34(), m32()].into(), "{p:?}");
    assert_eq!(p.builtin, BuiltinDisplay::Off, "{p:?}");
}

/// Bug de la ronda 2: si la respuesta al apagado se perdía (la PC se apagó y el
/// enlace con ella), la Mac lo daba por fallido y deshacía el apagado: quitaba los
/// dos monitores de su escritorio aunque los seguían mostrando a ella, encendía la
/// pantalla integrada y le devolvía los monitores a una PC apagada. Quedaba a ciegas.
#[tokio::test(start_paused = true)]
async fn apagado_con_la_respuesta_perdida_no_deja_a_la_mac_a_ciegas() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap()).await;
    d.arm(FaultPlan::new().on(
        Machine::Pc,
        Step::PowerOffPc,
        FaultAction::ApplyThenCut(LinkKind::Usb4),
    ));
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power(), Power::Off);
    all_on_mac(&d);
    assert!(!d.mac.state().pc_on);
}

/// Si el apagado se cuelga, también se da por hecho: la Mac se queda con todo y la
/// PC, prendida y sin monitores. El siguiente cambio la espera y el puesto vuelve a
/// coincidir.
#[tokio::test(start_paused = true)]
async fn apagado_colgado_deja_todo_en_la_mac_y_el_siguiente_cambio_lo_repara() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap()).await;
    d.arm(FaultPlan::new().on(Machine::Pc, Step::PowerOffPc, FaultAction::Hang));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.settle().await;
    all_on_mac(&d);
    assert_eq!(d.world.power(), Power::On, "la PC no se apagó");
    assert!(
        d.world.violations().is_empty(),
        "{:?}",
        d.world.violations()
    );

    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    d.pc_agrees().await.unwrap();
}

/// Cuando falla un paso y además falla deshacerlo, el agente lo dice
/// (`Inconsistent`) y el puesto no coincide; el siguiente cambio lo arregla porque
/// todos los pasos son idempotentes.
#[tokio::test(start_paused = true)]
async fn tras_un_cambio_inconsistente_el_siguiente_repara_el_puesto() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    let back_32 = Step::SwitchInput {
        monitor: m32(),
        from: Machine::Pc,
        to: Machine::Mac,
        input: Input::Hdmi,
    };
    d.arm(
        FaultPlan::new()
            .on(
                Machine::Mac,
                Step::Detach {
                    machine: Machine::Mac,
                    monitor: m32(),
                },
                FaultAction::Fail("macOS no suelta el 32\"".into()),
            )
            .on(
                Machine::Pc,
                back_32.clone(),
                FaultAction::Fail("DDC".into()),
            ),
    );
    let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
    assert_eq!(
        run.outcome(),
        Some(Outcome::Inconsistent),
        "{:#?}",
        run.events
    );
    assert!(d.check().await.is_err(), "el puesto quedó a medias");
    // El agente cree lo que de verdad quedó hecho: el 32" sigue en la PC.
    assert_eq!(d.mac.state().owner[&m32()], Machine::Pc);

    committed(&d, &d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap()).await;
    all_on_mac(&d);
    d.pc_agrees().await.unwrap();
}

/// Wake-on-LAN solo sale por Ethernet (DESIGN §6). Sin ese cable la PC no arranca:
/// el encendido vence a los 120 s y todo vuelve atrás.
#[tokio::test(start_paused = true)]
async fn sin_ethernet_la_pc_no_despierta() {
    let d = Desk::builder()
        .links(&[LinkKind::Usb4, LinkKind::Wifi])
        .build()
        .await
        .unwrap();
    let start = tokio::time::Instant::now();
    let run = d.set_mode(Machine::Mac, shared()).await.unwrap();
    assert_eq!(
        run.outcome(),
        Some(Outcome::RolledBack),
        "{:#?}",
        run.events
    );
    assert_eq!(run.failure().map(|f| f.0), Some(0));
    assert!(start.elapsed() >= Duration::from_secs(120));
    assert_eq!(d.world.power(), Power::Off);
    d.check().await.unwrap();
    all_on_mac(&d);

    // Con Ethernet cortado pasa lo mismo.
    let d = Desk::new().await.unwrap();
    d.net.cut(LinkKind::Ethernet);
    let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
    assert_eq!(
        run.outcome(),
        Some(Outcome::RolledBack),
        "{:#?}",
        run.events
    );
    d.check().await.unwrap();
}

/// La PC se queda sin corriente en Compartido. Un cambio que necesita a la PC
/// falla enseguida y limpio (no hay con quién hablar), y la Mac conserva su monitor.
#[tokio::test(start_paused = true)]
async fn la_pc_se_queda_sin_corriente() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    d.pull_pc_plug();
    d.settle().await;
    assert_eq!(d.active_link(Machine::Mac), None);
    assert_eq!(
        d.world.shows(&m34()),
        None,
        "el 34\" en DisplayPort queda en negro"
    );

    let before = d.world.physical();
    let start = tokio::time::Instant::now();
    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    assert_eq!(
        run.outcome(),
        Some(Outcome::RolledBack),
        "{:#?}",
        run.events
    );
    assert!(
        run.failure()
            .is_some_and(|(_, e)| e.contains(PEER_NOT_CONNECTED)),
        "{:?}",
        run.failure()
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "sin esperas largas"
    );
    assert_eq!(d.world.physical(), before);
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    assert!(
        d.world.violations().is_empty(),
        "{:?}",
        d.world.violations()
    );
}

/// El USB4 parpadea (se corta y vuelve cada 700 ms) durante un cambio, con
/// Ethernet sano. Pase lo que pase con ese cambio, si el agente dice que terminó
/// bien o que volvió atrás, el puesto coincide; y en cuanto el cable se queda
/// quieto, el siguiente cambio funciona.
#[tokio::test(start_paused = true)]
async fn usb4_intermitente_durante_un_cambio() {
    for offset_ms in [0u64, 200, 450] {
        let d = Desk::new().await.unwrap();
        committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
        let net = d.net.clone();
        let flapping = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(offset_ms)).await;
            for _ in 0..30 {
                net.cut(LinkKind::Usb4);
                tokio::time::sleep(Duration::from_millis(350)).await;
                net.restore(LinkKind::Usb4);
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
        });
        let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
        flapping.await.unwrap();
        let case = format!("desfase {offset_ms} ms: {:?}", run.outcome());
        match run.outcome() {
            Some(Outcome::Committed | Outcome::RolledBack) => {
                if let Err(e) = d.check().await {
                    panic!("{case}: {e}");
                }
            }
            Some(Outcome::Inconsistent) => {}
            None => panic!("{case}"),
        }
        // Justo después del último corte el gestor todavía puede tener el USB4
        // muerto como activo (lo nota en hasta 1,25 s): se deja asentar.
        d.settle().await;
        d.wait_link(LinkKind::Usb4).await.unwrap();
        for mode in [Mode::SoloMac, Mode::SoloPc] {
            committed(&d, &d.set_mode(Machine::Mac, mode).await.unwrap()).await;
            d.pc_agrees().await.unwrap();
        }
    }
}
