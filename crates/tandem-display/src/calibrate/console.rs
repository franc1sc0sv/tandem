//! La calibración desde la terminal (`tandem-ddc calibrate`): el usuario dice con una letra
//! qué ve en el monitor (m = la Mac, p = la PC, n = negro) y el avance se imprime línea a
//! línea.
//!
//! Las líneas llegan por un canal desde un hilo que lee stdin: así una sonda que vence
//! (el motor suelta el futuro) no deja una lectura bloqueada a medias, y lo que se tecleó
//! antes de la pregunta se descarta en vez de contestar la siguiente.
//!
//! La pregunta nombra las máquinas, no las entradas: con «¿se ve la máquina de HDMI?» el
//! usuario de la primera prueba real vio Windows y dijo que sí (T-081).

use std::io::Write;
use std::sync::{Mutex, PoisonError};

use tandem_core::{Input, Machine};
use tokio::sync::mpsc;

use super::{
    Answer, CalibrationEvent, ConfirmError, ConfirmFuture, Confirmer, Probe, ProbeKind, Sight,
    State,
};
use crate::ddc::switch::other_input;

/// La entrada como la nombra el menú del monitor.
pub fn name(input: Input) -> &'static str {
    match input {
        Input::Hdmi => "HDMI",
        Input::DisplayPort => "DisplayPort",
    }
}

/// Qué máquina va cableada a cada entrada del monitor que se calibra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wiring {
    /// La entrada de la PC; la Mac está en la otra.
    pub pc: Input,
}

impl Default for Wiring {
    /// El puesto de Tandem: la PC por DisplayPort y la Mac por HDMI.
    fn default() -> Self {
        Wiring {
            pc: Input::DisplayPort,
        }
    }
}

impl Wiring {
    pub fn machine(self, input: Input) -> Machine {
        if input == self.pc {
            Machine::Pc
        } else {
            Machine::Mac
        }
    }

    pub fn input(self, machine: Machine) -> Input {
        match machine {
            Machine::Pc => self.pc,
            Machine::Mac => other_input(self.pc),
        }
    }

    /// «la Mac (HDMI)».
    pub fn describe(self, input: Input) -> String {
        format!("{} ({})", machine_name(self.machine(input)), name(input))
    }
}

pub fn machine_name(machine: Machine) -> &'static str {
    match machine {
        Machine::Mac => "la Mac",
        Machine::Pc => "la PC",
    }
}

/// Las respuestas posibles, para repetirlas en cada pregunta.
pub const KEYS: &str = "m = la Mac · p = la PC · n = negro o sin señal · c = cancelar";

/// Lo que se contesta según lo tecleado; `None` si no se entiende. Enter solo no contesta:
/// «lo veo» no dice qué se ve.
pub fn answer(line: &str, wiring: Wiring) -> Option<Answer> {
    let sees = |m| Some(Answer::Saw(Sight::Input(wiring.input(m))));
    match line.trim().to_lowercase().as_str() {
        "m" | "mac" => sees(Machine::Mac),
        "p" | "pc" | "windows" => sees(Machine::Pc),
        "n" | "negro" | "nada" => Some(Answer::Saw(Sight::Black)),
        "c" | "cancelar" => Some(Answer::Stop),
        _ => None,
    }
}

/// La pregunta de cada sonda.
pub fn question(probe: &Probe, wiring: Wiring) -> String {
    let want = wiring.describe(probe.input);
    let monitor = probe.monitor.model();
    let secs = probe.timeout.as_secs();
    let what = format!("¿Qué ves en {monitor}? {KEYS} [{secs} s]");
    match probe.kind {
        ProbeKind::Candidate { code } => {
            format!("Mandé {code:#04X}, candidato para {want}. {what}")
        }
        ProbeKind::UserSwitch => format!(
            "Pasa {monitor} a {} desde el menú del monitor para ver {}. Cuando cambie, {}",
            name(probe.input),
            machine_name(wiring.machine(probe.input)),
            lower_first(&what)
        ),
        ProbeKind::Back { code } => {
            format!("Volví con {code:#04X}: debería verse {want}. {what}")
        }
        ProbeKind::Verify { code } => {
            format!("Verificación: mandé {code:#04X}, debería verse {want}. {what}")
        }
        ProbeKind::Stalled { code } => format!(
            "Nadie contestó tras mandar {code:#04X} dos veces. ¿Sigo? Dime qué ves en {monitor} \
             (m = la Mac · p = la PC · n = negro o sin señal) o c para cancelar [{secs} s]"
        ),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some('¿') => format!("¿{}", lower_first(c.as_str())),
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Lo que se explica antes de empezar: cómo se contesta y qué pasa sin respuesta.
/// `source` dice de dónde salió el cableado.
pub fn instructions(wiring: Wiring, source: &str, wait: std::time::Duration) -> String {
    format!(
        "en este monitor la Mac va por {} y la PC por {} ({source})\n\
         contesta cada pregunta con una letra y Enter: {KEYS}\n\
         si esta terminal está en el monitor que calibras, no la verás mientras muestre la otra \
         máquina: escribe la letra igual\n\
         tienes {} s por pregunta; sin respuesta repito el mismo código y después pregunto si sigo",
        name(wiring.input(Machine::Mac)),
        name(wiring.pc),
        wait.as_secs()
    )
}

/// [`Confirmer`] de teclado: imprime la pregunta y espera la siguiente línea.
pub struct LineConfirmer {
    lines: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    out: Mutex<Box<dyn Write + Send>>,
    wiring: Wiring,
}

impl LineConfirmer {
    pub fn new(
        lines: mpsc::UnboundedReceiver<String>,
        out: Box<dyn Write + Send>,
        wiring: Wiring,
    ) -> Self {
        LineConfirmer {
            lines: tokio::sync::Mutex::new(lines),
            out: Mutex::new(out),
            wiring,
        }
    }

    /// Lee stdin en un hilo propio, que termina con el proceso (o al cerrarse stdin).
    pub fn stdin(out: Box<dyn Write + Send>, wiring: Wiring) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            while matches!(stdin.read_line(&mut line), Ok(n) if n > 0) {
                if tx.send(std::mem::take(&mut line)).is_err() {
                    return;
                }
            }
        });
        Self::new(rx, out, wiring)
    }

    fn say(&self, text: &str) {
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        // Sin terminal donde escribir la pregunta igual se espera la respuesta.
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

impl Confirmer for LineConfirmer {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        Box::pin(async move {
            let mut lines = self.lines.lock().await;
            // Lo tecleado antes de la pregunta no la contesta.
            while lines.try_recv().is_ok() {}
            self.say(&question(probe, self.wiring));
            loop {
                let Some(line) = lines.recv().await else {
                    return Err(ConfirmError(
                        "la entrada estándar se cerró: no hay quién confirme".into(),
                    ));
                };
                match answer(&line, self.wiring) {
                    Some(a) => return Ok(Some(a)),
                    None => self.say(&format!("contesta con una letra: {KEYS}")),
                }
            }
        })
    }
}

/// El avance como una línea para la terminal; `None` para lo que no hace falta mostrar.
pub fn describe(event: &CalibrationEvent, wiring: Wiring) -> Option<String> {
    Some(match event {
        CalibrationEvent::State(State::Reading) => "leyendo capabilities y 0x60…".into(),
        CalibrationEvent::State(State::UserSwitch { .. }) => return None,
        CalibrationEvent::State(State::Testing { input }) => {
            format!("probando códigos para {}", wiring.describe(*input))
        }
        CalibrationEvent::State(State::Verifying) => {
            "verificación final: ida y vuelta con los dos códigos".into()
        }
        CalibrationEvent::State(State::Done) => return None,
        CalibrationEvent::State(State::Cancelled) => "cancelada: el monitor volvió".into(),
        CalibrationEvent::State(State::Failed) => return None,
        CalibrationEvent::Declared(Some(codes)) if !codes.is_empty() => format!(
            "el monitor declara 60 = {}",
            codes
                .iter()
                .map(|c| format!("{c:#04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        CalibrationEvent::Declared(_) => "el monitor no declara códigos de entrada".into(),
        CalibrationEvent::Reported { input, value } => match value {
            Some(v) => format!(
                "en {} el monitor lee 0x60 = {v:#04X} (solo una pista)",
                name(*input)
            ),
            None => format!("en {} no se pudo leer 0x60", name(*input)),
        },
        CalibrationEvent::Trying {
            input,
            candidate,
            attempt,
            of,
        } => format!(
            "{}: pruebo {:#04X} ({}, {attempt} de {of})",
            name(*input),
            candidate.code,
            candidate.source
        ),
        CalibrationEvent::Seen { input, sight, .. } if *sight == Sight::Input(*input) => {
            return None;
        }
        CalibrationEvent::Seen {
            input,
            code,
            sight: Sight::Input(seen),
        } => format!(
            "se ve {}: {code:#04X} no sirve para {}",
            machine_name(wiring.machine(*seen)),
            name(*input)
        ),
        CalibrationEvent::Seen {
            code,
            sight: Sight::Black,
            ..
        } => format!("negro: {code:#04X} lleva a una entrada sin señal"),
        CalibrationEvent::NoAnswer {
            code, again: true, ..
        } => format!("sin respuesta: repito {code:#04X}"),
        CalibrationEvent::NoAnswer {
            code, again: false, ..
        } => format!("sin respuesta otra vez con {code:#04X}"),
        CalibrationEvent::Verified { input, code } => {
            format!("✓ {code:#04X} → {}", wiring.describe(*input))
        }
        CalibrationEvent::Returned { code } => format!("vuelta con {code:#04X}"),
        CalibrationEvent::Found { input, code } => {
            format!("{}: {code:#04X} ✓", name(*input))
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::calibrate::sim::samsung_32;
    use crate::calibrate::{Candidate, Source};

    /// Salida compartida para mirar lo que se imprimió.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn probe(kind: ProbeKind) -> Probe {
        Probe {
            monitor: samsung_32().info,
            input: Input::Hdmi,
            kind,
            timeout: Duration::from_secs(20),
        }
    }

    #[test]
    fn letters_name_the_machine_not_the_input() {
        let w = Wiring::default();
        for (line, want) in [
            ("m\n", Some(Answer::sees(Input::Hdmi))),
            ("  Mac\r\n", Some(Answer::sees(Input::Hdmi))),
            ("p\n", Some(Answer::sees(Input::DisplayPort))),
            ("PC", Some(Answer::sees(Input::DisplayPort))),
            ("n\n", Some(Answer::black())),
            ("negro", Some(Answer::black())),
            ("c\n", Some(Answer::Stop)),
            // Enter solo ya no es «lo veo»: no dice qué se ve.
            ("\n", None),
            ("sí", None),
            ("quizás", None),
        ] {
            assert_eq!(answer(line, w), want, "{line:?}");
        }
        // Con la PC en HDMI, «p» es HDMI.
        let crossed = Wiring { pc: Input::Hdmi };
        assert_eq!(answer("p", crossed), Some(Answer::sees(Input::Hdmi)));
        assert_eq!(answer("m", crossed), Some(Answer::sees(Input::DisplayPort)));
    }

    #[test]
    fn questions_name_the_monitor_the_machine_and_the_wait() {
        let w = Wiring::default();
        let q = question(&probe(ProbeKind::Candidate { code: 0x05 }), w);
        assert!(q.contains("Odyssey G5") && q.contains("0x05"), "{q}");
        assert!(
            q.contains("la Mac (HDMI)") && q.contains("¿Qué ves en"),
            "{q}"
        );
        assert!(q.contains(KEYS) && q.contains("[20 s]"), "{q}");
        let q = question(&probe(ProbeKind::UserSwitch), w);
        assert!(
            q.contains("desde el menú del monitor para ver la Mac"),
            "{q}"
        );
        assert!(q.contains("¿qué ves en Odyssey G5?"), "{q}");
        let q = question(&probe(ProbeKind::Back { code: 0x0F }), w);
        assert!(q.starts_with("Volví con 0x0F: debería verse la Mac"), "{q}");
        let q = question(&probe(ProbeKind::Verify { code: 0x05 }), w);
        assert!(q.starts_with("Verificación: mandé 0x05"), "{q}");
        let q = question(&probe(ProbeKind::Stalled { code: 0x05 }), w);
        assert!(
            q.contains("dos veces. ¿Sigo?") && q.contains("c para cancelar"),
            "{q}"
        );
        let i = instructions(w, "por omisión", Duration::from_secs(20));
        assert!(
            i.contains("la Mac va por HDMI y la PC por DisplayPort (por omisión)\n"),
            "{i}"
        );
        assert!(
            i.contains("escribe la letra igual") && i.contains("20 s"),
            "{i}"
        );
    }

    #[tokio::test]
    async fn the_next_line_answers_and_earlier_ones_are_dropped() {
        let (tx, rx) = mpsc::unbounded_channel();
        let out = Shared::default();
        let c = LineConfirmer::new(rx, Box::new(out.clone()), Wiring::default());
        // Tecleado antes de preguntar: no cuenta.
        tx.send("m\n".into()).unwrap();
        let p = probe(ProbeKind::Candidate { code: 0x05 });
        let answer = tokio::spawn({
            let tx = tx.clone();
            async move {
                tokio::task::yield_now().await;
                tx.send("\n".into()).unwrap();
                tx.send("p\n".into()).unwrap();
            }
        });
        assert_eq!(
            c.confirm(&p).await,
            Ok(Some(Answer::sees(Input::DisplayPort)))
        );
        answer.await.unwrap();
        let text = out.text();
        assert!(text.contains("¿Qué ves en Odyssey G5?"), "{text}");
        assert!(
            text.contains(&format!("contesta con una letra: {KEYS}\n")),
            "{text}"
        );

        tx.send("m\n".into()).unwrap();
        // Una línea que llegó antes de la pregunta se descarta: sin otra, se espera.
        let waiting = tokio::time::timeout(Duration::from_millis(50), c.confirm(&p)).await;
        assert!(waiting.is_err());
        tx.send("m\n".into()).unwrap();
        drop(tx);
        // El canal se cerró con la línea ya leída: la siguiente pregunta falla claro.
        assert!(c.confirm(&p).await.is_err());
    }

    #[test]
    fn progress_lines() {
        let w = Wiring::default();
        let trying = CalibrationEvent::Trying {
            input: Input::DisplayPort,
            candidate: Candidate {
                code: 0x0F,
                source: Source::Reported,
            },
            attempt: 1,
            of: 31,
        };
        assert_eq!(
            describe(&trying, w).unwrap(),
            "DisplayPort: pruebo 0x0F (lectura del monitor, 1 de 31)"
        );
        assert_eq!(
            describe(
                &CalibrationEvent::Found {
                    input: Input::Hdmi,
                    code: 0x05
                },
                w
            )
            .unwrap(),
            "HDMI: 0x05 ✓"
        );
        // La máquina contraria: el código no hizo nada.
        assert_eq!(
            describe(
                &CalibrationEvent::Seen {
                    input: Input::DisplayPort,
                    code: 0x10,
                    sight: Sight::Input(Input::Hdmi)
                },
                w
            )
            .unwrap(),
            "se ve la Mac: 0x10 no sirve para DisplayPort"
        );
        assert_eq!(
            describe(
                &CalibrationEvent::Seen {
                    input: Input::Hdmi,
                    code: 0x05,
                    sight: Sight::Input(Input::Hdmi)
                },
                w
            ),
            None
        );
        assert_eq!(
            describe(
                &CalibrationEvent::NoAnswer {
                    input: Input::Hdmi,
                    code: 0x05,
                    again: true
                },
                w
            )
            .unwrap(),
            "sin respuesta: repito 0x05"
        );
        assert_eq!(
            describe(
                &CalibrationEvent::Verified {
                    input: Input::DisplayPort,
                    code: 0x0F
                },
                w
            )
            .unwrap(),
            "✓ 0x0F → la PC (DisplayPort)"
        );
        assert_eq!(
            describe(&CalibrationEvent::Declared(Some(vec![0x0F, 0x10])), w).unwrap(),
            "el monitor declara 60 = 0x0F 0x10"
        );
        assert!(describe(&CalibrationEvent::State(State::Done), w).is_none());
    }
}
