//! Cortes de corriente de la PC (T-063): la Mac concilia `pc_on` con los enlaces.
//! Todo con el reloj de tokio pausado.

use std::time::Duration;

use tandem_core::{BuiltinDisplay, Input, Machine, Mode, MonitorId, Step};
use tandem_link::manager::LinkKind;
use tandem_proto::{PowerEvent, TxnEvent};
use tandem_sim::{Desk, Outcome, Power, TxnRun};
use tokio::sync::broadcast;
use tokio::time::Instant;

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
    d.mac.config().power.pc_lost_after()
}

/// Corte de corriente en Compartido: tras el umbral la Mac da la PC por apagada y
/// avisa a la UI; el 34" sigue en DisplayPort (en negro, la Mac no puede cambiarlo
/// sin la PC) y el 32" sigue en la Mac. El siguiente cambio a Compartido empieza por
/// encender la PC y el puesto vuelve a coincidir.
#[tokio::test(start_paused = true)]
async fn corte_de_corriente_en_compartido() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    d.pc_agrees().await.unwrap();
    let mut power = d.mac.subscribe_power();
    let before = d.mac.state();

    let start = Instant::now();
    d.pull_pc_plug();
    tokio::time::sleep(lost_after(&d) - Duration::from_secs(1)).await;
    assert!(d.mac.state().pc_on, "dentro del umbral no cambia nada");
    assert!(power.try_recv().is_err());

    let PowerEvent::PcLost { reason, stranded } = next_power(&mut power).await;
    let waited = start.elapsed();
    assert!(
        waited >= lost_after(&d) && waited < lost_after(&d) + Duration::from_secs(2),
        "{waited:?}"
    );
    assert!(reason.contains("corte de corriente"), "{reason}");
    assert_eq!(stranded, [m34()]);

    let mut want = before.clone();
    want.pc_on = false;
    assert_eq!(d.mac.state(), want);
    // El puesto coincide: el 34" en negro, el 32" y la geometría en la Mac.
    d.check().await.unwrap();
    d.check_geometry().unwrap();
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    assert!(
        d.world.violations().is_empty(),
        "{:?}",
        d.world.violations()
    );

    let run = d.set_mode(Machine::Mac, shared()).await.unwrap();
    assert_eq!(planned(&run).first(), Some(&Step::PowerOnPc));
    committed(&d, &run).await;
    assert_eq!(d.world.power(), Power::On);
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    d.pc_agrees().await.unwrap();
}

/// Tras el corte, volver a Solo Mac también necesita a la PC: es la que cambia la
/// entrada del 34" (DESIGN §8). El plan la enciende primero.
#[tokio::test(start_paused = true)]
async fn tras_el_corte_solo_mac_enciende_la_pc_para_devolver_el_34() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    let mut power = d.mac.subscribe_power();
    d.pull_pc_plug();
    next_power(&mut power).await;

    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    let steps = planned(&run);
    assert_eq!(steps.first(), Some(&Step::PowerOnPc));
    assert!(steps.contains(&Step::SwitchInput {
        monitor: m34(),
        from: Machine::Pc,
        to: Machine::Mac,
        input: Input::Hdmi,
    }));
    committed(&d, &run).await;
    assert_eq!(d.world.shows(&m34()), Some(Machine::Mac));
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
}

/// En Solo Windows la Mac se queda sin monitores: tras el corte su pantalla
/// integrada sigue encendida y la UI sabe que los dos quedaron en negro.
#[tokio::test(start_paused = true)]
async fn corte_de_corriente_en_solo_windows() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap()).await;
    let mut power = d.mac.subscribe_power();
    d.pull_pc_plug();
    let PowerEvent::PcLost { stranded, .. } = next_power(&mut power).await;
    assert_eq!(stranded, [m34(), m32()]);
    assert!(!d.mac.state().pc_on);
    d.check().await.unwrap();
    assert_eq!(d.world.builtin(), BuiltinDisplay::Extend);

    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    assert_eq!(planned(&run).first(), Some(&Step::PowerOnPc));
    committed(&d, &run).await;
    assert_eq!(d.world.builtin(), BuiltinDisplay::Off);
}

/// Los tres enlaces caen un rato más corto que el umbral y vuelven: `pc_on` no cambia,
/// no hay aviso y el siguiente cambio no enciende nada.
#[tokio::test(start_paused = true)]
async fn una_caida_breve_de_los_enlaces_no_cambia_pc_on() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    let mut power = d.mac.subscribe_power();
    let before = d.mac.state();

    for kind in LinkKind::ALL {
        d.net.cut(kind);
    }
    tokio::time::sleep(lost_after(&d) - Duration::from_secs(3)).await;
    assert_eq!(d.active_link(Machine::Mac), None, "la Mac no ve a la PC");
    for kind in LinkKind::ALL {
        d.net.restore(kind);
    }
    d.wait_link(LinkKind::Usb4).await.unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;

    assert_eq!(d.mac.state(), before);
    assert!(power.try_recv().is_err());
    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    assert!(!planned(&run).contains(&Step::PowerOnPc));
    committed(&d, &run).await;
}

/// Un apagado pedido no es un corte: la PC se apaga, los enlaces caen y no hay aviso.
#[tokio::test(start_paused = true)]
async fn un_apagado_pedido_no_es_un_corte() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    let mut power = d.mac.subscribe_power();
    committed(&d, &d.shutdown_pc().await.unwrap()).await;
    tokio::time::sleep(lost_after(&d) * 4).await;
    assert_eq!(d.world.power(), Power::Off);
    assert!(!d.mac.state().pc_on);
    assert!(power.try_recv().is_err());
    d.check().await.unwrap();
}

/// Tras el corte alguien enciende la PC con su botón: en cuanto la Mac la ve, la da
/// por prendida. Windows arranca con el 34" en DisplayPort y el puesto coincide.
#[tokio::test(start_paused = true)]
async fn la_pc_encendida_a_mano_tras_el_corte_vuelve_a_contar() {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.set_mode(Machine::Mac, shared()).await.unwrap()).await;
    let mut power = d.mac.subscribe_power();
    d.pull_pc_plug();
    next_power(&mut power).await;

    d.press_pc_power_button();
    let mut states = d.mac.subscribe();
    tokio::time::timeout(Duration::from_secs(3600), states.wait_for(|s| s.pc_on))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.mac.state().owner[&m34()], Machine::Pc);
    d.check().await.unwrap();
    d.check_geometry().unwrap();
    assert!(power.try_recv().is_err());

    // Ya no hace falta encenderla.
    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    assert!(!planned(&run).contains(&Step::PowerOnPc));
    committed(&d, &run).await;
}
