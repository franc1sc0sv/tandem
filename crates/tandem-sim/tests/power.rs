//! Escenarios de punta a punta de la energía del puesto (docs/TESTING.md, «Simulación del
//! puesto»): encender y apagar desde cualquiera de las dos, fallos y tiempos máximos en
//! cada paso, cortes de corriente y caídas de enlace.
//!
//! Todos con el reloj de tokio pausado: el arranque de la PC (45 s), los pings de los
//! enlaces y los tiempos máximos de los pasos corren en tiempo virtual.

use std::time::Duration;

use proptest::prelude::*;
use tandem_core::{Machine, Os, Step};
use tandem_link::manager::LinkKind;
use tandem_proto::PowerEvent;
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, TxnRun};

const PC_ON: Step = Step::PowerOn {
    machine: Machine::B,
};
const PC_OFF: Step = Step::PowerOff {
    machine: Machine::B,
};

async fn committed(d: &Desk, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check().await.unwrap();
}

/// La Mac con la PC prendida y los dos agentes viéndose.
async fn both_on() -> Desk {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.power_on(Machine::B).await.unwrap()).await;
    d.pc_agrees().await.unwrap();
    d
}

#[tokio::test(start_paused = true)]
async fn encender_y_apagar_la_pc() {
    let d = Desk::new().await.unwrap();
    d.check().await.unwrap();
    assert_eq!(d.world.power_of(Machine::B), Power::Off);

    // Wake-on-LAN y esperar al agente de la PC.
    let start = tokio::time::Instant::now();
    let run = d.power_on(Machine::B).await.unwrap();
    committed(&d, &run).await;
    assert!(
        start.elapsed() >= Duration::from_secs(45),
        "esperó el arranque"
    );
    assert_eq!(d.active_link(Machine::A), Some(LinkKind::Usb4));
    d.pc_agrees().await.unwrap();

    // Apagado: el paso corre en la PC, que contesta y se apaga.
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
    assert_eq!(
        d.world.calls().last(),
        Some(&(Machine::B, PC_OFF)),
        "{:?}",
        d.world.calls()
    );
    // Sin la PC no queda ningún enlace con su agente.
    assert_eq!(d.active_link(Machine::A), None);
    assert!(d.world.violations().is_empty());
}

/// El espejo (DESIGN §3.7): con la PC de principal, ella despierta a la Mac.
#[tokio::test(start_paused = true)]
async fn la_pc_tambien_orquesta() {
    let d = Desk::builder().principal(Machine::B).build().await.unwrap();
    let run = d.power_on(Machine::A).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::A), Power::On);
    d.agrees(Machine::A).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn un_encendido_que_falla_deja_todo_como_estaba() {
    let d = Desk::new().await.unwrap();
    d.arm(FaultPlan::new().on(Machine::A, PC_ON, FaultAction::Fail("sin red".into())));
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(0));
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn un_encendido_colgado_vence_y_no_cambia_el_estado() {
    let d = Desk::new().await.unwrap();
    d.arm(FaultPlan::new().on(Machine::A, PC_ON, FaultAction::Hang));
    let start = tokio::time::Instant::now();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert!(start.elapsed() >= Duration::from_secs(120));
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();
}

/// El paquete mágico solo sale por Ethernet (DESIGN §6): sin ese cable la PC no despierta.
#[tokio::test(start_paused = true)]
async fn sin_ethernet_la_pc_no_despierta() {
    let d = Desk::builder()
        .links(&[LinkKind::Usb4, LinkKind::Wifi])
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
    d.check().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn un_apagado_que_falla_deja_la_pc_prendida() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(
        Machine::B,
        PC_OFF,
        FaultAction::Fail("acceso denegado".into()),
    ));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert!(d.mac.state().on.b);
    d.check().await.unwrap();
}

/// La PC se apaga pero la respuesta se pierde con el enlace: la Mac lo da por hecho y el
/// puesto coincide.
#[tokio::test(start_paused = true)]
async fn un_apagado_con_la_respuesta_perdida_se_da_por_hecho() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(
        Machine::B,
        PC_OFF,
        FaultAction::ApplyThenCut(LinkKind::Usb4),
    ));
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
}

/// Un apagado que no contesta se da por hecho (lo más probable es que la PC se esté
/// apagando). Si en realidad siguió prendida, el siguiente encendido lo repara: el
/// Wake-on-LAN no le hace nada y su agente ya contesta.
#[tokio::test(start_paused = true)]
async fn un_apagado_colgado_se_da_por_hecho_y_el_siguiente_encendido_lo_repara() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(Machine::B, PC_OFF, FaultAction::Hang));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert!(!d.mac.state().on.b);
    assert_eq!(d.world.power_of(Machine::B), Power::On);
    let run = d.power_on(Machine::B).await.unwrap();
    committed(&d, &run).await;
}

/// Corte de corriente: tras el umbral la Mac da la PC por apagada y avisa a la UI; una vez
/// encendida a mano, vuelve a contar.
#[tokio::test(start_paused = true)]
async fn corte_de_corriente_y_vuelta_a_mano() {
    let d = both_on().await;
    let mut power = d.mac.subscribe_power();
    let lost_after = d.mac.config().power.peer_lost_after();
    d.pull_pc_plug();
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.mac.state().on.b, "dentro del umbral no cambia nada");
    let PowerEvent::PeerLost { machine, reason } =
        tokio::time::timeout(Duration::from_secs(10), power.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(machine, Machine::B);
    assert!(reason.contains("power cut"), "{reason}");
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();

    d.press_pc_power_button();
    let mut rx = d.mac.subscribe();
    tokio::time::timeout(Duration::from_secs(120), rx.wait_for(|s| s.on.b))
        .await
        .unwrap()
        .unwrap();
    d.check().await.unwrap();
}

/// Sin USB4 ni Ethernet el plano de control sigue por Wi-Fi, y al volver el USB4 el gestor
/// regresa a él.
#[tokio::test(start_paused = true)]
async fn solo_con_wifi_el_control_sigue() {
    let d = both_on().await;
    d.arm(
        FaultPlan::new()
            .cut_link_at(Duration::ZERO, LinkKind::Usb4)
            .cut_link_at(Duration::from_secs(1), LinkKind::Ethernet),
    );
    d.wait_link(LinkKind::Wifi).await.unwrap();
    d.net.restore(LinkKind::Usb4);
    d.wait_link(LinkKind::Usb4).await.unwrap();
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
}

/// Lo que se le pide al puesto en cada vuelta de la propiedad.
#[derive(Debug, Clone)]
struct Op {
    from: Machine,
    on: bool,
    fail: bool,
}

fn op() -> impl Strategy<Value = Op> {
    (
        prop_oneof![Just(Machine::A), Just(Machine::B)],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(from, on, fail)| Op { from, on, fail })
}

async fn run_ops(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let d = Desk::new()
        .await
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
    for (i, op) in ops.iter().enumerate() {
        let case = format!("paso {i}: {op:?}");
        // Solo puede pedir algo una máquina prendida.
        if d.world.power_of(op.from) != Power::On {
            continue;
        }
        let other = op.from.other();
        if op.fail {
            let (machine, step) = if op.on {
                (op.from, Step::PowerOn { machine: other })
            } else {
                (other, Step::PowerOff { machine: other })
            };
            d.arm(FaultPlan::new().on(machine, step, FaultAction::Fail("falla".into())));
        } else {
            d.arm(FaultPlan::new());
        }
        let run = if op.on {
            d.set_power(op.from, other, true).await
        } else {
            d.shutdown(op.from, other).await
        };
        let run = run.map_err(|e| TestCaseError::fail(format!("{case}: {e}")))?;
        prop_assert!(run.outcome().is_some(), "{case}: {:#?}", run.events);
        d.check_from(op.from)
            .await
            .map_err(|e| TestCaseError::fail(format!("{case}: {e}")))?;
        prop_assert!(d.world.violations().is_empty(), "{case}");
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Cualquier secuencia de encendidos y apagados, pedidos desde cualquiera de las dos y
    /// con fallos inyectados, deja la energía como la cree quien la pidió.
    #[test]
    fn cualquier_secuencia_deja_la_energia_coherente(ops in proptest::collection::vec(op(), 1..6)) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(run_ops(ops))?;
    }
}

/// DECISIONS D2: el puesto funciona igual con cualquier sistema en cada lado. Con macOS +
/// Windows, Windows + Windows y macOS + macOS: encender B desde A y apagarla, cada una
/// anuncia su sistema y queda anotada en el puesto de las dos.
#[tokio::test(start_paused = true)]
async fn cualquier_sistema_en_cada_lado() {
    for (a, b) in [
        (Os::MacOs, Os::Windows),
        (Os::Windows, Os::Windows),
        (Os::MacOs, Os::MacOs),
    ] {
        let d = Desk::builder().os(a, b).build().await.unwrap();
        let run = d.power_on(Machine::B).await.unwrap();
        committed(&d, &run).await;
        d.pc_agrees().await.unwrap();
        // Cada una ve el sistema de la otra, y el puesto de las dos lo guarda.
        let caps = d.agent(Machine::A).capabilities();
        assert_eq!(caps.here.info.os, a, "{a:?}+{b:?}");
        assert_eq!(caps.peer.map(|p| p.info.os), Some(b), "{a:?}+{b:?}");
        for m in Machine::ALL {
            let desk = d.agent(m).desk();
            assert_eq!(desk.machines.get(Machine::A).map(|i| i.os), Some(a));
            assert_eq!(desk.machines.get(Machine::B).map(|i| i.os), Some(b));
        }
        // Solo Windows sabe apagarse sola: lo anuncia según su sistema, no su lado.
        let b_caps = d.agent(Machine::B).capabilities().here;
        assert_eq!(b_caps.power.power_off, b == Os::Windows, "{a:?}+{b:?}");
        let run = d.shutdown_pc().await.unwrap();
        committed(&d, &run).await;
        assert_eq!(d.world.power_of(Machine::B), Power::Off, "{a:?}+{b:?}");
        assert!(d.world.violations().is_empty());
    }
}
