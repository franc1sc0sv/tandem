use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicUsize;

use proptest::prelude::*;
use tokio::time::Instant;

use super::sim::{Effect, Reports, Showing, SimMonitor, SimSpec, samsung_32, samsung_34};
use super::*;
use crate::ddc::switch::other_input;

/// La base de perfiles verificados de hoy (`known_profile`, T-050).
fn samsung_base() -> Vec<BrandProfile> {
    let p = |hdmi: u8, display_port: u8| BrandProfile {
        manufacturer: "SAM".into(),
        codes: InputCodes {
            hdmi: vec![hdmi],
            display_port: vec![display_port],
        },
    };
    vec![p(0x06, 0x09), p(0x05, 0x0F)]
}

fn request(start: Input, known: Vec<BrandProfile>) -> CalibrationRequest {
    CalibrationRequest {
        monitor: 0,
        start,
        known,
        options: CalibrationOptions::default(),
    }
}

struct Outcome {
    result: Result<CalibratedProfile, CalibrationError>,
    events: Vec<CalibrationEvent>,
}

impl Outcome {
    fn tried(&self, input: Input) -> Vec<u8> {
        self.events
            .iter()
            .filter_map(|e| match e {
                CalibrationEvent::Trying {
                    input: i,
                    candidate,
                    ..
                } if *i == input => Some(candidate.code),
                _ => None,
            })
            .collect()
    }

    fn states(&self) -> Vec<State> {
        self.events
            .iter()
            .filter_map(|e| match e {
                CalibrationEvent::State(s) => Some(*s),
                _ => None,
            })
            .collect()
    }
}

async fn run_with(
    sim: &SimMonitor,
    req: &CalibrationRequest,
    confirmer: &dyn Confirmer,
    cancel: &CancelToken,
    mut hook: impl FnMut(&CalibrationEvent) + Send,
) -> Outcome {
    let mut bus = sim.clone();
    let mut events = Vec::new();
    let result = calibrate(&mut bus, req, confirmer, cancel, &mut |e| {
        events.push(e.clone());
        hook(e);
    })
    .await;
    Outcome { result, events }
}

async fn run(sim: &SimMonitor, req: &CalibrationRequest) -> Outcome {
    run_with(sim, req, &sim.clone(), &CancelToken::new(), |_| {}).await
}

fn codes(hdmi: u8, display_port: u8) -> InputCodes {
    InputCodes {
        hdmi: vec![hdmi],
        display_port: vec![display_port],
    }
}

#[tokio::test(start_paused = true)]
async fn finds_the_real_profile_of_the_34() {
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    let profile = out.result.as_ref().unwrap();
    assert_eq!(profile.codes, codes(0x06, 0x09));
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    // La vuelta sale de la lectura (0x09) al primer intento. En HDMI la lectura también
    // dice 0x09 (miente) y queda fuera por ser la vuelta; MCCS y lo declarado no hacen
    // nada, y el primero de la marca es el bueno.
    assert_eq!(out.tried(Input::DisplayPort), [0x09]);
    assert_eq!(out.tried(Input::Hdmi), [0x11, 0x12, 0x0F, 0x10, 0x06]);
    // Después de cada candidato, la vuelta con el código verificado; al final, la ida y
    // vuelta de verificación.
    assert_eq!(
        sim.writes(),
        [
            0x09, 0x11, 0x09, 0x12, 0x09, 0x0F, 0x09, 0x10, 0x09, 0x06, 0x09, 0x06, 0x09
        ]
    );
    assert_eq!(
        profile.reported,
        Reported {
            hdmi: Some(0x09),
            display_port: Some(0x09)
        }
    );
    assert_eq!(profile.declared, [0x12, 0x0F, 0x10]);
    assert_eq!(profile.pnp_id.as_deref(), Some("SAM711A"));
    assert_eq!(profile.mccs_ver.as_deref(), Some("2.1"));
    assert_eq!(profile.attempts, 6);
}

#[tokio::test(start_paused = true)]
async fn without_the_brand_base_the_34_falls_back_to_the_sweep() {
    // 0x03 también lleva el 34" a HDMI (docs/research): el barrido lo encuentra antes.
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    let out = run(&sim, &request(Input::DisplayPort, Vec::new())).await;
    assert_eq!(out.result.unwrap().codes, codes(0x03, 0x09));
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

/// Lo que ve el usuario cada vez que el motor manda un candidato: tiene que ser la
/// máquina de la entrada contraria (T-081). Si no, un código que el monitor ignora
/// parece funcionar.
fn on_the_opposite_input(sim: &SimMonitor) -> impl FnMut(&CalibrationEvent) + Send + use<> {
    let sim = sim.clone();
    move |e| {
        if let CalibrationEvent::Trying {
            input, candidate, ..
        } = e
        {
            assert_eq!(
                sim.showing(),
                Showing::Input(other_input(*input)),
                "{:#04X} para {input:?} se probó sin el monitor en la otra entrada",
                candidate.code
            );
        }
    }
}

fn verified(out: &Outcome) -> Vec<(Input, u8)> {
    out.events
        .iter()
        .filter_map(|e| match e {
            CalibrationEvent::Verified { input, code } => Some((*input, *code)),
            _ => None,
        })
        .collect()
}

/// T-081, la primera prueba real (`calibrate 1 --from hdmi` en el 32"): el monitor sigue
/// leyendo 0x05 tras pasarlo a mano a DisplayPort, ignora 0x10 y el usuario tarda 10 s en
/// contestar. Con el motor de T-077 (8 s y «¿se ve la máquina?») salió HDMI 0x0F /
/// DP 0x10; ahora sale el perfil verificado y cada entrada se resuelve al primer intento.
#[tokio::test(start_paused = true)]
async fn the_first_real_calibration_of_the_32_finds_its_verified_profile() {
    let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
    sim.lock().answer_delay = Duration::from_secs(10);
    let out = run_with(
        &sim,
        &request(Input::Hdmi, Vec::new()),
        &sim.clone(),
        &CancelToken::new(),
        on_the_opposite_input(&sim),
    )
    .await;
    let profile = out.result.as_ref().unwrap();
    assert_eq!(profile.codes, codes(0x05, 0x0F));
    assert_eq!(
        profile.reported,
        Reported {
            hdmi: Some(0x05),
            display_port: Some(0x05)
        }
    );
    assert_eq!(out.tried(Input::Hdmi), [0x05]);
    // La lectura vieja de DisplayPort (0x05) es la vuelta verificada: no se prueba.
    assert_eq!(out.tried(Input::DisplayPort), [0x0F]);
    assert_eq!(
        verified(&out),
        [
            (Input::Hdmi, 0x05),
            (Input::DisplayPort, 0x0F),
            (Input::Hdmi, 0x05)
        ]
    );
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, CalibrationEvent::NoAnswer { .. }))
    );
    assert_eq!(sim.writes(), [0x05, 0x0F, 0x05, 0x0F, 0x05]);
    assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));
    assert_eq!(
        out.states(),
        [
            State::Reading,
            State::UserSwitch {
                input: Input::DisplayPort
            },
            State::Testing { input: Input::Hdmi },
            State::Testing {
                input: Input::DisplayPort
            },
            State::Verifying,
            State::Done,
        ]
    );
}

/// Desde DisplayPort la lectura vieja (0x0F en HDMI) no ayuda: HDMI se busca por MCCS, lo
/// declarado y la marca, con el negro de 0x06 en el camino.
#[tokio::test(start_paused = true)]
async fn the_32_from_displayport_with_a_stale_reading() {
    let sim = SimMonitor::new(samsung_32(), Input::DisplayPort);
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &sim.clone(),
        &CancelToken::new(),
        on_the_opposite_input(&sim),
    )
    .await;
    let profile = out.result.as_ref().unwrap();
    assert_eq!(
        profile.ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    assert_eq!(out.tried(Input::DisplayPort), [0x0F]);
    assert_eq!(
        out.tried(Input::Hdmi),
        [0x11, 0x12, 0x01, 0x03, 0x04, 0x10, 0x06, 0x05]
    );
    assert!(matches!(
        out.events.iter().find(|e| matches!(
            e,
            CalibrationEvent::Trying {
                input: Input::DisplayPort,
                ..
            }
        )),
        Some(CalibrationEvent::Trying {
            candidate: Candidate {
                source: Source::Reported,
                ..
            },
            attempt: 1,
            ..
        })
    ));
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    assert_eq!(profile.pnp_id.as_deref(), Some("SAM7489"));
}

/// Si el usuario ve la máquina de la entrada contraria, el código no sirve para la
/// entrada probada (T-081: con «¿se ve la máquina de HDMI?» el usuario vio Windows y
/// dijo que sí, y 0x0F quedó como código de HDMI).
#[tokio::test(start_paused = true)]
async fn seeing_the_opposite_machine_rejects_the_code() {
    let mut spec = samsung_32();
    spec.reports = Reports::Fails;
    let sim = SimMonitor::new(spec, Input::Hdmi);
    let out = run_with(
        &sim,
        &request(Input::Hdmi, Vec::new()),
        &sim.clone(),
        &CancelToken::new(),
        on_the_opposite_input(&sim),
    )
    .await;
    assert_eq!(out.result.as_ref().unwrap().codes, codes(0x05, 0x0F));
    // 0x0F se probó para HDMI: el usuario vio la PC (DisplayPort) y no contó.
    assert!(out.events.contains(&CalibrationEvent::Seen {
        input: Input::Hdmi,
        code: 0x0F,
        sight: Sight::Input(Input::DisplayPort),
    }));
    assert_eq!(out.tried(Input::Hdmi).last(), Some(&0x05));
    assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));
}

#[tokio::test(start_paused = true)]
async fn black_code_is_recovered_with_the_verified_return() {
    // Sin la lectura del 32" se prueba 0x06, que lo deja en negro.
    let mut spec = samsung_32();
    spec.reports = Reports::Fails;
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    // El 0x06 del 34" es el primer código de la marca para HDMI.
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &sim.clone(),
        &CancelToken::new(),
        on_the_opposite_input(&sim),
    )
    .await;
    assert_eq!(out.result.unwrap().codes, codes(0x05, 0x0F));
    let writes = sim.writes();
    let black = writes.iter().position(|c| *c == 0x06).unwrap();
    assert_eq!(writes[black + 1], 0x0F);
    // Tras el negro, la vuelta se confirma con el usuario antes del siguiente.
    assert!(
        sim.probes()
            .iter()
            .any(|p| p.kind == ProbeKind::Back { code: 0x0F })
    );
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

/// Si al buscar la vuelta el monitor queda en negro, no hay código verificado con qué
/// volver a la otra entrada: se pide hacerlo desde el menú antes del siguiente.
#[tokio::test(start_paused = true)]
async fn black_while_looking_for_the_return_asks_for_the_menu() {
    let mut spec = samsung_32();
    spec.reports = Reports::Fails;
    let sim = SimMonitor::new(spec, Input::Hdmi);
    let out = run_with(
        &sim,
        &request(Input::Hdmi, samsung_base()),
        &sim.clone(),
        &CancelToken::new(),
        on_the_opposite_input(&sim),
    )
    .await;
    assert_eq!(out.result.unwrap().codes, codes(0x05, 0x0F));
    let switches = sim
        .probes()
        .iter()
        .filter(|p| p.kind == ProbeKind::UserSwitch && p.input == Input::DisplayPort)
        .count();
    assert_eq!(
        switches, 2,
        "el cambio inicial y el que sigue al negro de 0x06"
    );
}

#[tokio::test(start_paused = true)]
async fn unanswered_probes_are_repeated_once_and_then_asked_whether_to_go_on() {
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    sim.lock().silent_when_unseen = true;
    let t0 = Instant::now();
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    assert_eq!(out.result.unwrap().codes, codes(0x06, 0x09));
    // Cuatro candidatos de HDMI sin respuesta, cada uno dos veces.
    assert!(t0.elapsed() >= 8 * CONFIRM_TIMEOUT);
    let probes = sim.probes();
    assert!(probes.iter().all(|p| match p.kind {
        ProbeKind::Candidate { .. } | ProbeKind::Back { .. } | ProbeKind::Verify { .. } =>
            p.timeout == CONFIRM_TIMEOUT,
        ProbeKind::UserSwitch | ProbeKind::Stalled { .. } => p.timeout == USER_SWITCH_TIMEOUT,
    }));
    assert_eq!(probes[0].kind, ProbeKind::UserSwitch);
    assert_eq!(probes[0].input, Input::Hdmi);
    assert_eq!(probes[0].monitor.model(), "LC34G55T");
    // El mismo código dos veces y después «¿sigo?».
    let for_11: Vec<ProbeKind> = probes
        .iter()
        .map(|p| p.kind)
        .filter(|k| k.code() == Some(0x11))
        .collect();
    assert_eq!(
        for_11,
        [
            ProbeKind::Candidate { code: 0x11 },
            ProbeKind::Candidate { code: 0x11 },
            ProbeKind::Stalled { code: 0x11 }
        ]
    );
    assert_eq!(&sim.writes()[1..4], [0x11, 0x11, 0x09]);
    assert!(out.events.contains(&CalibrationEvent::NoAnswer {
        input: Input::Hdmi,
        code: 0x11,
        again: true
    }));
}

/// Nadie contesta ni el «¿sigo?»: la calibración para en vez de descartar el código.
#[tokio::test(start_paused = true)]
async fn nobody_answering_stops_instead_of_discarding_the_code() {
    let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
    let out = run_with(
        &sim,
        &request(Input::Hdmi, Vec::new()),
        &Mute(sim.clone()),
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(out.result, Err(CalibrationError::NoAnswer { code: 0x05 }));
    // Solo se probó el primer código; el usuario regresó el monitor a mano.
    assert_eq!(out.tried(Input::Hdmi), [0x05]);
    assert_eq!(sim.writes(), [0x05, 0x05]);
    assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));
}

/// Hace los cambios a mano y no contesta nada más.
struct Mute(SimMonitor);

impl Confirmer for Mute {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        if probe.kind == ProbeKind::UserSwitch {
            return self.0.confirm(probe);
        }
        self.0.lock().probes.push(probe.clone());
        Box::pin(async { Ok(None) })
    }
}

/// Contesta lo que ve, salvo `lie` para las sondas de `kind`.
struct Lies {
    sim: SimMonitor,
    kind: fn(ProbeKind) -> bool,
    lie: Answer,
}

impl Confirmer for Lies {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        if (self.kind)(probe.kind) {
            let lie = self.lie;
            return Box::pin(async move { Ok(Some(lie)) });
        }
        self.sim.confirm(probe)
    }
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_and_returns() {
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    let user = Lies {
        sim: sim.clone(),
        kind: |k| k == ProbeKind::Candidate { code: 0x12 },
        lie: Answer::Stop,
    };
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &user,
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(out.result, Err(CalibrationError::Cancelled));
    assert_eq!(sim.writes().last(), Some(&0x09));
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    assert_eq!(out.states().last(), Some(&State::Cancelled));
}

/// Si la ida y vuelta final no se ve, el perfil no se da por bueno y se pide regresar
/// el monitor a mano (la vuelta ya no es de fiar).
#[tokio::test(start_paused = true)]
async fn failed_verification_gives_no_profile() {
    let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
    let user = Lies {
        sim: sim.clone(),
        kind: |k| k == ProbeKind::Verify { code: 0x0F },
        lie: Answer::black(),
    };
    let out = run_with(
        &sim,
        &request(Input::Hdmi, Vec::new()),
        &user,
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(
        out.result,
        Err(CalibrationError::VerificationFailed {
            input: Input::DisplayPort,
            code: 0x0F
        })
    );
    let last = sim.probes().pop().unwrap();
    assert_eq!(
        (last.kind, last.input),
        (ProbeKind::UserSwitch, Input::Hdmi)
    );
    assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));
}

/// Tras un negro, la vuelta confirmada tiene que mostrar la máquina de partida.
#[tokio::test(start_paused = true)]
async fn a_return_that_is_not_seen_stops_the_calibration() {
    let mut spec = samsung_32();
    spec.reports = Reports::Fails;
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let user = Lies {
        sim: sim.clone(),
        kind: |k| matches!(k, ProbeKind::Back { .. }),
        lie: Answer::sees(Input::Hdmi),
    };
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &user,
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(
        out.result,
        Err(CalibrationError::ReturnNotSeen {
            input: Input::DisplayPort,
            code: 0x0F
        })
    );
    assert_eq!(sim.probes().pop().unwrap().kind, ProbeKind::UserSwitch);
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

#[tokio::test(start_paused = true)]
async fn no_working_code_leaves_the_monitor_at_the_start() {
    let mut spec = samsung_34();
    spec.effects
        .retain(|_, e| *e != Effect::Select(Input::Hdmi));
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    assert_eq!(
        out.result,
        Err(CalibrationError::NoWorkingCode { input: Input::Hdmi })
    );
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    assert_eq!(sim.writes().last(), Some(&0x09));
    assert_eq!(out.states().last(), Some(&State::Failed));
    // Se probó todo el barrido menos la vuelta.
    assert_eq!(out.tried(Input::Hdmi).len(), candidates::SWEEP.count() - 1);
}

#[tokio::test(start_paused = true)]
async fn no_working_return_asks_the_user_to_go_back() {
    let mut spec = samsung_34();
    spec.effects
        .retain(|_, e| *e != Effect::Select(Input::DisplayPort));
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    assert_eq!(
        out.result,
        Err(CalibrationError::NoWorkingCode {
            input: Input::DisplayPort
        })
    );
    // Sin vuelta verificada no se probó ningún código de HDMI.
    assert!(out.tried(Input::Hdmi).is_empty());
    let last = sim.probes().pop().unwrap();
    assert_eq!(
        (last.kind, last.input),
        (ProbeKind::UserSwitch, Input::DisplayPort)
    );
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

#[tokio::test(start_paused = true)]
async fn user_who_does_not_switch_stops_the_calibration_before_any_write() {
    let sim = SimMonitor::new(samsung_32(), Input::DisplayPort);
    sim.lock().user_switches = false;
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    assert_eq!(
        out.result,
        Err(CalibrationError::UserDidNotSwitch { input: Input::Hdmi })
    );
    assert!(sim.writes().is_empty());
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

#[tokio::test(start_paused = true)]
async fn user_who_does_not_return_is_reported() {
    let mut spec = samsung_34();
    spec.effects
        .retain(|_, e| *e != Effect::Select(Input::DisplayPort));
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    // Hace el primer cambio a mano, pero no el de regreso.
    let confirmer = Scripted {
        sim: sim.clone(),
        user_switches: AtomicUsize::new(1),
    };
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, Vec::new()),
        &confirmer,
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(
        out.result,
        Err(CalibrationError::NotReturned {
            start: Input::DisplayPort
        })
    );
}

/// Deja hacer `user_switches` cambios a mano y después deja de contestar.
struct Scripted {
    sim: SimMonitor,
    user_switches: AtomicUsize,
}

impl Confirmer for Scripted {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        if probe.kind == ProbeKind::UserSwitch {
            let left = self.user_switches.load(Ordering::SeqCst);
            if left == 0 {
                return Box::pin(std::future::pending());
            }
            self.user_switches.store(left - 1, Ordering::SeqCst);
        }
        self.sim.confirm(probe)
    }
}

#[tokio::test(start_paused = true)]
async fn cancel_while_probing_returns_with_the_verified_code() {
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    sim.lock().silent_when_unseen = true;
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &sim.clone(),
        &cancel,
        move |e| {
            // Cancelar desde fuera mientras se espera la confirmación de 0x12.
            if let CalibrationEvent::Trying { candidate, .. } = e
                && candidate.code == 0x12
            {
                let c = c.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    c.cancel();
                });
            }
        },
    )
    .await;
    assert_eq!(out.result, Err(CalibrationError::Cancelled));
    assert_eq!(out.tried(Input::Hdmi), [0x11, 0x12]);
    assert_eq!(sim.writes().last(), Some(&0x09));
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    assert_eq!(out.states().last(), Some(&State::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn cancel_away_from_the_start_and_without_a_return_code_asks_the_user() {
    let sim = SimMonitor::new(samsung_32(), Input::DisplayPort);
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &sim.clone(),
        &cancel,
        move |e| {
            // Recién pasado a HDMI a mano, antes de verificar la vuelta.
            if matches!(
                e,
                CalibrationEvent::Reported {
                    input: Input::Hdmi,
                    ..
                }
            ) {
                c.cancel();
            }
        },
    )
    .await;
    assert_eq!(out.result, Err(CalibrationError::Cancelled));
    assert!(sim.writes().is_empty());
    let last = sim.probes().pop().unwrap();
    assert_eq!(
        (last.kind, last.input),
        (ProbeKind::UserSwitch, Input::DisplayPort)
    );
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

#[tokio::test(start_paused = true)]
async fn flaky_writes_are_retried() {
    let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
    sim.lock().write_script = [false, false, true, false, true].into();
    let out = run(&sim, &request(Input::Hdmi, samsung_base())).await;
    assert_eq!(out.result.unwrap().codes, codes(0x05, 0x0F));
    assert_eq!(sim.writes(), [0x05, 0x0F, 0x05, 0x0F, 0x05]);
}

#[tokio::test(start_paused = true)]
async fn failed_return_after_an_ignored_code_needs_no_help() {
    let sim = SimMonitor::new(samsung_34(), Input::DisplayPort);
    // Vuelta verificada y el primer candidato de HDMI bien; la vuelta después falla siempre.
    sim.lock().write_script = [true, true, false, false, false].into();
    let out = run(&sim, &request(Input::DisplayPort, samsung_base())).await;
    assert!(matches!(
        out.result,
        Err(CalibrationError::ReturnFailed { code: 0x09, .. })
    ));
    // El usuario acababa de decir que veía la máquina de partida: no hay que pedirle nada.
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    let last = sim.probes().pop().unwrap();
    assert_eq!(last.kind, ProbeKind::Candidate { code: 0x11 });
}

#[tokio::test(start_paused = true)]
async fn failed_return_away_from_the_start_falls_back_to_the_user() {
    let mut spec = samsung_32();
    spec.reports = Reports::Fails;
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let s = sim.clone();
    // Cancelar con el monitor en negro (0x06) y la vuelta rota.
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &sim.clone(),
        &cancel,
        move |e| {
            if let CalibrationEvent::Seen {
                sight: Sight::Black,
                ..
            } = e
            {
                s.lock().write_script = [false; 9].into();
                c.cancel();
            }
        },
    )
    .await;
    assert_eq!(out.result, Err(CalibrationError::Cancelled));
    assert_eq!(sim.probes().pop().unwrap().kind, ProbeKind::UserSwitch);
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

struct Failing(SimMonitor);

impl Confirmer for Failing {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        if probe.kind == ProbeKind::UserSwitch {
            return self.0.confirm(probe);
        }
        Box::pin(async { Err(ConfirmError("se cayó el enlace".into())) })
    }
}

#[tokio::test(start_paused = true)]
async fn confirmer_error_aborts_at_the_start() {
    let sim = SimMonitor::new(samsung_32(), Input::DisplayPort);
    let out = run_with(
        &sim,
        &request(Input::DisplayPort, samsung_base()),
        &Failing(sim.clone()),
        &CancelToken::new(),
        |_| {},
    )
    .await;
    assert_eq!(
        out.result,
        Err(CalibrationError::Confirmer(ConfirmError(
            "se cayó el enlace".into()
        )))
    );
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
}

#[tokio::test(start_paused = true)]
async fn bad_monitor_index_touches_nothing() {
    let sim = SimMonitor::new(samsung_32(), Input::DisplayPort);
    let mut req = request(Input::DisplayPort, Vec::new());
    req.monitor = 2;
    let out = run(&sim, &req).await;
    assert_eq!(
        out.result,
        Err(CalibrationError::Ddc(DdcError::NoSuchMonitor(2)))
    );
    assert!(sim.probes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancel_token_wakes_waiters_and_stays_cancelled() {
    let t = CancelToken::new();
    assert!(!t.is_cancelled());
    let waiter = tokio::spawn({
        let t = t.clone();
        async move { t.cancelled().await }
    });
    tokio::task::yield_now().await;
    t.cancel();
    waiter.await.unwrap();
    // Ya cancelado: vuelve enseguida.
    t.cancelled().await;
    assert!(t.is_cancelled());
}

// --- Propiedades ---

/// Monitor arbitrario con un código válido por entrada en el rango del barrido.
#[derive(Debug, Clone)]
struct Arbitrary {
    start: Input,
    hdmi: u8,
    display_port: u8,
    black: Vec<u8>,
    reports: Reports,
    declared: Vec<u8>,
    brand: Vec<u8>,
    flaky: Vec<bool>,
}

fn input() -> impl Strategy<Value = Input> {
    prop_oneof![Just(Input::Hdmi), Just(Input::DisplayPort)]
}

fn arbitrary_monitor() -> impl Strategy<Value = Arbitrary> {
    (
        input(),
        candidates::SWEEP,
        candidates::SWEEP,
        proptest::collection::vec(candidates::SWEEP, 0..3),
        prop_oneof![
            any::<u8>().prop_map(Reports::Fixed),
            Just(Reports::CurrentCode),
            Just(Reports::LastSet),
            Just(Reports::Fails),
        ],
        proptest::collection::vec(0u8..0x30, 0..6),
        proptest::collection::vec(0u8..0x30, 0..4),
        proptest::collection::vec(any::<bool>(), 0..20),
    )
        .prop_filter("un código por entrada", |t| t.1 != t.2)
        .prop_map(
            |(start, hdmi, display_port, black, reports, declared, brand, flaky)| Arbitrary {
                start,
                hdmi,
                display_port,
                black: black
                    .into_iter()
                    .filter(|c| *c != hdmi && *c != display_port)
                    .collect(),
                reports,
                declared,
                brand,
                flaky,
            },
        )
}

impl Arbitrary {
    fn spec(&self, with_other: bool) -> SimSpec {
        let other = other_input(self.start);
        let mut effects = BTreeMap::new();
        for c in &self.black {
            effects.insert(*c, Effect::Black);
        }
        for (input, code) in [
            (Input::Hdmi, self.hdmi),
            (Input::DisplayPort, self.display_port),
        ] {
            if with_other || input != other {
                effects.insert(code, Effect::Select(input));
            }
        }
        let hex: Vec<String> = self.declared.iter().map(|c| format!("{c:02X}")).collect();
        let mut spec = samsung_32();
        spec.capabilities = format!("(prot(monitor)vcp(10 60({})))", hex.join(" "));
        spec.effects = effects;
        spec.reports = self.reports;
        spec.native = Reported {
            hdmi: Some(self.hdmi),
            display_port: Some(self.display_port),
        };
        spec
    }

    fn sim(&self, with_other: bool) -> SimMonitor {
        let sim = SimMonitor::new(self.spec(with_other), self.start);
        // Fallos sueltos: nunca dos seguidos, así los reintentos siempre alcanzan.
        let script: VecDeque<bool> = self
            .flaky
            .iter()
            .flat_map(|ok| if *ok { vec![true] } else { vec![false, true] })
            .collect();
        sim.lock().write_script = script;
        sim
    }

    fn request(&self) -> CalibrationRequest {
        request(
            self.start,
            vec![BrandProfile {
                manufacturer: "SAM".into(),
                codes: InputCodes {
                    hdmi: self.brand.clone(),
                    display_port: self.brand.iter().rev().copied().collect(),
                },
            }],
        )
    }
}

/// Entre dos candidatos de la otra entrada siempre hay una vuelta (o la verificación
/// vio la entrada de partida), y el último también vuelve.
fn always_returns_between_candidates(out: &Outcome, start: Input) -> bool {
    let mut pending = false;
    for e in &out.events {
        match e {
            CalibrationEvent::Trying { input, .. } if *input != start => {
                if pending {
                    return false;
                }
                pending = true;
            }
            CalibrationEvent::Returned { .. } => pending = false,
            CalibrationEvent::Verified { input, .. } => pending = *input != start,
            _ => {}
        }
    }
    !pending
}

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(f)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Con un código válido por entrada, el motor encuentra exactamente esos y deja el
    /// monitor en la entrada de partida.
    #[test]
    fn finds_any_valid_code_and_ends_at_the_start(m in arbitrary_monitor()) {
        let sim = m.sim(true);
        let out = block_on(run_with(
            &sim,
            &m.request(),
            &sim.clone(),
            &CancelToken::new(),
            on_the_opposite_input(&sim),
        ));
        prop_assert!(always_returns_between_candidates(&out, m.start));
        prop_assert_eq!(out.result.map(|p| p.codes), Ok(codes(m.hdmi, m.display_port)));
        prop_assert_eq!(sim.showing(), Showing::Input(m.start));
    }

    /// Sin código para la otra entrada: `NoWorkingCode` y en la entrada de partida.
    #[test]
    fn no_code_for_the_other_input_ends_at_the_start(m in arbitrary_monitor()) {
        let sim = m.sim(false);
        let out = block_on(run_with(
            &sim,
            &m.request(),
            &sim.clone(),
            &CancelToken::new(),
            on_the_opposite_input(&sim),
        ));
        prop_assert!(always_returns_between_candidates(&out, m.start));
        prop_assert_eq!(
            out.result,
            Err(CalibrationError::NoWorkingCode { input: other_input(m.start) })
        );
        prop_assert_eq!(sim.showing(), Showing::Input(m.start));
    }

    /// Cancelar después de cualquier evento deja el monitor en la entrada de partida.
    #[test]
    fn cancel_at_any_point_ends_at_the_start(
        m in arbitrary_monitor(),
        at in 0usize..80,
        silent in any::<bool>(),
    ) {
        let sim = m.sim(true);
        sim.lock().silent_when_unseen = silent;
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let mut seen = 0usize;
        let mut opposite = on_the_opposite_input(&sim);
        let out = block_on(run_with(&sim, &m.request(), &sim.clone(), &cancel, move |e| {
            opposite(e);
            if seen == at {
                c.cancel();
            }
            seen += 1;
        }));
        prop_assert!(always_returns_between_candidates(&out, m.start));
        match &out.result {
            Ok(p) => prop_assert_eq!(&p.codes, &codes(m.hdmi, m.display_port)),
            Err(e) => prop_assert_eq!(e, &CalibrationError::Cancelled),
        }
        prop_assert_eq!(sim.showing(), Showing::Input(m.start));
    }
}
