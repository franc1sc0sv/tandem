//! Calibración DDC con la confirmación desde la otra máquina (T-078), de punta a punta:
//! el motor de T-077 corre en el agente de la PC sobre un monitor simulado, y cada sonda
//! la muestra la máquina de la entrada que debería verse, por la red simulada.
//!
//! Un usuario simulado mira cada UI: hace clic en la sonda solo si el monitor muestra de
//! verdad la máquina de esa UI, y cambia la entrada desde el menú cuando se le pide.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tandem_core::{DdcProfile, Input, Machine};
use tandem_display::calibrate::sim::Showing;
use tandem_display::calibrate::sim::{SimMonitor, SimSpec, samsung_32, samsung_34};
use tandem_display::calibrate::{
    CalibrationError, CalibrationOptions, CalibrationRequest, CancelToken, calibrate,
};
use tandem_proto::{ProbeAsk, ProbeEvent, ProbePlace};
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

/// El usuario frente a la UI de `machine`, que en el monitor ve la entrada `mine`.
fn user(agent: &Arc<Agent>, sim: &SimMonitor, mine: Input) -> Seen {
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
            let visible = sim.showing() == Showing::Input(probe.input);
            match place {
                // En la pantalla completa solo hay clic si se ve; si no, nadie la ve.
                ProbePlace::Monitor { .. } => {
                    if visible && probe.input == mine {
                        let _ = agent.probes().answer(id, true);
                    }
                }
                // Fuera del escritorio no se muestra en ninguna parte: nadie la ve.
                ProbePlace::Offscreen => {}
                // La pregunta de respaldo se contesta mirando el monitor.
                ProbePlace::Question => {
                    let _ = agent.probes().answer(id, visible);
                }
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
    sim: SimMonitor,
    mac: Seen,
    pc: Seen,
}

async fn run(d: &Desk, spec: SimSpec) -> Run {
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let mac = user(&d.mac, &sim, Input::Hdmi);
    let pc = user(&d.pc, &sim, Input::DisplayPort);
    let confirmer = AgentConfirmer::new(d.pc.clone(), Input::DisplayPort);
    let mut bus = sim.clone();
    let result = calibrate(
        &mut bus,
        &request(),
        &confirmer,
        &CancelToken::new(),
        &mut |_| {},
    )
    .await;
    // Que lleguen los últimos avisos de quitar la sonda.
    tokio::time::sleep(Duration::from_secs(1)).await;
    Run {
        result,
        sim,
        mac,
        pc,
    }
}

#[tokio::test(start_paused = true)]
async fn the_32_is_calibrated_with_the_mac_confirming_over_the_link() {
    let d = Desk::new().await.unwrap();
    both_up(&d).await;
    let r = run(&d, samsung_32()).await;

    let profile = r.result.unwrap();
    assert_eq!(
        profile.ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    assert_eq!(r.sim.showing(), Showing::Input(Input::DisplayPort));

    // Lo de HDMI lo vio la Mac en su monitor (el cambio a mano y el código 0x05); lo de
    // DisplayPort, la PC.
    let on_monitor = |s: &Seen| {
        s.shown()
            .into_iter()
            .map(|(_, ask, input, place)| {
                assert!(matches!(place, ProbePlace::Monitor { .. }), "{place:?}");
                (ask, input)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        on_monitor(&r.mac),
        [
            (ProbeAsk::UserSwitch, Input::Hdmi),
            (ProbeAsk::Candidate { code: 0x05 }, Input::Hdmi),
        ]
    );
    assert_eq!(
        on_monitor(&r.pc),
        [(ProbeAsk::Candidate { code: 0x0F }, Input::DisplayPort)]
    );
    assert!(r.mac.all_hidden() && r.pc.all_hidden());
}

/// El 34" obedece pocos códigos: los que no hacen nada dejan la sonda de la Mac sin
/// clic, cada una cuenta como no vista a los 8 s y desaparece.
#[tokio::test(start_paused = true)]
async fn without_a_click_the_candidate_is_unseen_and_the_probe_goes_away() {
    let d = Desk::new().await.unwrap();
    both_up(&d).await;
    let start = tokio::time::Instant::now();
    let r = run(&d, samsung_34()).await;

    // Sin la base de perfiles, el barrido encuentra 0x03 (T-077).
    let profile = r.result.unwrap();
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
            ProbeAsk::UserSwitch => None,
        })
        .collect();
    // Varios códigos sin efecto antes del bueno, y todos mostrados en la Mac.
    assert_eq!(hdmi.last(), Some(&0x03));
    assert!(hdmi.len() > 3, "{hdmi:x?}");
    assert!(r.mac.all_hidden() && r.pc.all_hidden());
    // Cada código sin efecto esperó su plazo completo.
    let unseen = (hdmi.len() - 1) as u32;
    assert!(start.elapsed() >= Duration::from_secs(8) * unseen);
}

/// Con la Mac apagada no hay quién muestre la sonda de HDMI: la PC pregunta en su
/// propia ventana «¿el monitor cambió?».
#[tokio::test(start_paused = true)]
async fn without_the_other_machine_the_pc_asks_in_its_own_window() {
    let d = Desk::new().await.unwrap();
    // La PC no está encendida en la simulación de red: ningún enlace llega a la Mac.
    assert_eq!(d.active_link(Machine::Pc), None);
    let r = run(&d, samsung_32()).await;

    assert_eq!(
        r.result.unwrap().ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    assert!(r.mac.shown().is_empty());
    let places: Vec<_> =
        r.pc.shown()
            .into_iter()
            .map(|(_, ask, input, place)| (ask, input, place))
            .collect();
    assert_eq!(
        places,
        [
            (ProbeAsk::UserSwitch, Input::Hdmi, ProbePlace::Question),
            (
                ProbeAsk::Candidate { code: 0x0F },
                Input::DisplayPort,
                ProbePlace::Monitor { screen: None }
            ),
            (
                ProbeAsk::Candidate { code: 0x05 },
                Input::Hdmi,
                ProbePlace::Question
            ),
        ]
    );
    assert!(r.pc.all_hidden());
}
