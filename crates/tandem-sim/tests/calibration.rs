//! Calibración DDC con la confirmación desde la otra máquina (T-078), de punta a punta:
//! el motor de T-077 corre en el agente de la PC sobre un monitor simulado, y cada sonda
//! la muestra la máquina de la entrada que debería verse, por la red simulada.
//!
//! Un usuario simulado mira cada UI: hace clic en la sonda solo si el monitor muestra de
//! verdad la máquina de esa UI, contesta en la PC qué ve («¿qué ves?», T-081) y cambia la
//! entrada desde el menú cuando se le pide.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tandem_core::{DdcProfile, Input, Machine};
use tandem_display::calibrate::sim::Showing;
use tandem_display::calibrate::sim::{SimMonitor, SimSpec, samsung_32, samsung_34};
use tandem_display::calibrate::{
    CONFIRM_TIMEOUT, CalibrationError, CalibrationEvent, CalibrationOptions, CalibrationRequest,
    CancelToken, calibrate,
};
use tandem_proto::{ProbeAnswer, ProbeAsk, ProbeEvent, ProbePlace};
use tandem_sim::Desk;
use tandemd::{Agent, AgentConfirmer};
use tokio::sync::broadcast::error::RecvError;

/// Lo que vio la UI de una máquina.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<ProbeEvent>>>);

impl Seen {
    fn events(&self) -> Vec<ProbeEvent> {
        self.0.lock().unwrap().clone()
    }

    /// Las sondas mostradas (sin repetir el mismo id).
    fn shown(&self) -> Vec<(u64, ProbeAsk, Input, ProbePlace)> {
        let mut out: Vec<(u64, ProbeAsk, Input, ProbePlace)> = Vec::new();
        for e in self.events() {
            if let ProbeEvent::Show { id, probe, place } = e
                && !out.iter().any(|(i, ..)| *i == id)
            {
                out.push((id, probe.ask, probe.input, place));
            }
        }
        out
    }

    /// Toda sonda mostrada terminó quitándose.
    fn all_hidden(&self) -> bool {
        let events = self.events();
        self.shown()
            .iter()
            .all(|(id, ..)| events.contains(&ProbeEvent::Hide { id: *id }))
    }
}

/// El usuario frente a la UI de una máquina, que en el monitor ve la entrada `mine`.
/// En la sonda a pantalla completa hace clic si la ve. En la pregunta de la PC («¿qué
/// ves?») dice lo que ve, salvo que vea la Mac y `click_on_mac`: entonces hace clic en
/// la sonda de la Mac.
fn user(agent: &Arc<Agent>, sim: &SimMonitor, mine: Input, click_on_mac: bool) -> Seen {
    let seen = Seen::default();
    let mut rx = agent.subscribe_probes();
    let (agent, sim, log) = (agent.clone(), sim.clone(), seen.clone());
    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            log.0.lock().unwrap().push(event.clone());
            let ProbeEvent::Show { id, probe, place } = event else {
                continue;
            };
            if probe.ask == ProbeAsk::UserSwitch {
                // Lo pide el motor: el usuario cambia la entrada desde el menú.
                sim.lock().showing = Showing::Input(probe.input);
            }
            let showing = sim.showing();
            let answer = match place {
                // En la pantalla completa solo hay clic si se ve; si no, nadie la ve.
                ProbePlace::Monitor { .. } => (showing == Showing::Input(probe.input)
                    && probe.input == mine)
                    .then_some(ProbeAnswer::Seen),
                // Fuera del escritorio no se muestra en ninguna parte: nadie la ve.
                ProbePlace::Offscreen => None,
                // La pregunta se contesta mirando el monitor (la PC va por DisplayPort).
                ProbePlace::Question => match showing {
                    Showing::Input(Input::Hdmi) if click_on_mac => None,
                    Showing::Input(Input::Hdmi) => Some(ProbeAnswer::Machine(Machine::Mac)),
                    Showing::Input(Input::DisplayPort) => Some(ProbeAnswer::Machine(Machine::Pc)),
                    Showing::Black(_) => Some(ProbeAnswer::Black),
                },
            };
            if let Some(answer) = answer {
                let _ = agent.probes().answer(id, answer);
            }
        }
    });
    seen
}

fn request() -> CalibrationRequest {
    CalibrationRequest {
        monitor: 0,
        start: Input::DisplayPort,
        known: Vec::new(),
        options: CalibrationOptions::default(),
    }
}

/// Enciende la PC y espera a que cada agente vea al otro.
async fn both_up(d: &Desk) {
    d.press_pc_power_button();
    for _ in 0..600 {
        if d.active_link(Machine::Mac).is_some() && d.active_link(Machine::Pc).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("los agentes no se vieron");
}

struct Run {
    result: Result<tandem_display::calibrate::CalibratedProfile, CalibrationError>,
    events: Vec<CalibrationEvent>,
    sim: SimMonitor,
    mac: Seen,
    pc: Seen,
}

async fn run(d: &Desk, spec: SimSpec, mac_up: bool) -> Run {
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let mac = user(&d.mac, &sim, Input::Hdmi, mac_up);
    let pc = user(&d.pc, &sim, Input::DisplayPort, mac_up);
    let confirmer = AgentConfirmer::new(d.pc.clone(), Input::DisplayPort);
    let mut bus = sim.clone();
    let mut events = Vec::new();
    let result = calibrate(
        &mut bus,
        &request(),
        &confirmer,
        &CancelToken::new(),
        &mut |e| events.push(e.clone()),
    )
    .await;
    // Que lleguen los últimos avisos de quitar la sonda.
    tokio::time::sleep(Duration::from_secs(1)).await;
    Run {
        result,
        events,
        sim,
        mac,
        pc,
    }
}

fn no_waits(r: &Run) -> bool {
    !r.events
        .iter()
        .any(|e| matches!(e, CalibrationEvent::NoAnswer { .. }))
}

/// El 32" real (lee el código viejo tras el cambio desde el menú): HDMI se busca entre
/// varios códigos. Lo que no hace nada lo descarta el usuario diciendo en la PC que ve
/// la PC; el bueno lo confirma con un clic en la sonda de la Mac.
#[tokio::test(start_paused = true)]
async fn the_32_is_calibrated_with_the_mac_confirming_over_the_link() {
    let d = Desk::new().await.unwrap();
    both_up(&d).await;
    let r = run(&d, samsung_32(), true).await;

    let profile = r.result.as_ref().unwrap();
    assert_eq!(
        profile.ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    assert_eq!(r.sim.showing(), Showing::Input(Input::DisplayPort));
    assert!(no_waits(&r));

    // Todo lo de HDMI lo mostró la Mac en su monitor, a pantalla completa: el cambio a
    // mano, cada candidato y la verificación del 0x05.
    let mac = r.mac.shown();
    assert!(
        mac.iter().all(|(_, _, input, place)| *input == Input::Hdmi
            && matches!(place, ProbePlace::Monitor { .. }))
    );
    let asks: Vec<ProbeAsk> = mac.iter().map(|(_, ask, ..)| *ask).collect();
    assert_eq!(asks[0], ProbeAsk::UserSwitch);
    assert!(asks.contains(&ProbeAsk::Candidate { code: 0x05 }));
    assert_eq!(asks.last(), Some(&ProbeAsk::Verify { code: 0x05 }));
    // La PC mostró a pantalla completa solo lo de DisplayPort, y preguntó todo.
    let pc = r.pc.shown();
    assert!(
        pc.iter()
            .filter(|(.., place)| matches!(place, ProbePlace::Monitor { .. }))
            .all(|(_, _, input, _)| *input == Input::DisplayPort)
    );
    let questions = pc
        .iter()
        .filter(|(.., place)| *place == ProbePlace::Question)
        .count();
    assert_eq!(questions, mac.len() + pc.len() - questions);
    assert!(r.mac.all_hidden() && r.pc.all_hidden());
}

/// El 34" obedece pocos códigos: con los que no hacen nada el usuario dice en la PC que
/// sigue viendo la PC y se pasa al siguiente sin esperar el plazo (T-081: antes cada uno
/// esperaba 8 s sin respuesta).
#[tokio::test(start_paused = true)]
async fn seeing_the_pc_rejects_a_code_without_waiting() {
    let d = Desk::new().await.unwrap();
    both_up(&d).await;
    let start = tokio::time::Instant::now();
    let r = run(&d, samsung_34(), true).await;

    // Sin la base de perfiles, el barrido encuentra 0x03 (T-077).
    let profile = r.result.as_ref().unwrap();
    assert_eq!(
        profile.ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x03,
            display_port: 0x09
        })
    );
    assert_eq!(r.sim.showing(), Showing::Input(Input::DisplayPort));
    let hdmi: Vec<u8> = r
        .mac
        .shown()
        .into_iter()
        .filter_map(|(_, ask, ..)| match ask {
            ProbeAsk::Candidate { code } => Some(code),
            _ => None,
        })
        .collect();
    // Varios códigos sin efecto antes del bueno, y todos mostrados en la Mac.
    assert_eq!(hdmi.last(), Some(&0x03));
    assert!(hdmi.len() > 3, "{hdmi:x?}");
    assert!(no_waits(&r));
    assert!(r.mac.all_hidden() && r.pc.all_hidden());
    // Ningún código esperó su plazo: solo las pausas tras cada vuelta.
    assert!(
        start.elapsed() < CONFIRM_TIMEOUT * 4,
        "{:?}",
        start.elapsed()
    );
}

/// Con la Mac apagada no hay quién muestre la sonda de HDMI: queda la pregunta de la PC,
/// que el usuario contesta mirando el monitor.
#[tokio::test(start_paused = true)]
async fn without_the_other_machine_the_pc_asks_in_its_own_window() {
    let d = Desk::new().await.unwrap();
    // La PC no está encendida en la simulación de red: ningún enlace llega a la Mac.
    assert_eq!(d.active_link(Machine::Pc), None);
    let r = run(&d, samsung_32(), false).await;

    assert_eq!(
        r.result.as_ref().unwrap().ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    assert!(no_waits(&r));
    assert!(r.mac.shown().is_empty());
    let pc = r.pc.shown();
    // Lo de HDMI, solo como pregunta; lo de DisplayPort, también a pantalla completa.
    for (_, ask, input, place) in &pc {
        if *input == Input::Hdmi {
            assert_eq!(*place, ProbePlace::Question, "{ask:?}");
        }
    }
    assert_eq!(
        pc.iter()
            .filter(|(_, ask, input, _)| *ask == ProbeAsk::UserSwitch && *input == Input::Hdmi)
            .count(),
        1
    );
    assert!(pc.iter().any(|(_, ask, input, place)| {
        *ask == ProbeAsk::Candidate { code: 0x0F }
            && *input == Input::DisplayPort
            && matches!(place, ProbePlace::Monitor { .. })
    }));
    assert!(r.pc.all_hidden());
}
