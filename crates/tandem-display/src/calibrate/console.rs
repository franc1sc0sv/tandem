//! La calibración desde la terminal (`tandem-ddc calibrate`): el usuario confirma con el
//! teclado (Enter = lo veo) y el avance se imprime línea a línea.
//!
//! Las líneas llegan por un canal desde un hilo que lee stdin: así una sonda que vence
//! (el motor suelta el futuro) no deja una lectura bloqueada a medias, y lo que se tecleó
//! antes de la pregunta se descarta en vez de contestar la siguiente.

use std::io::Write;
use std::sync::{Mutex, PoisonError};

use tandem_core::Input;
use tokio::sync::mpsc;

use super::{CalibrationEvent, ConfirmError, ConfirmFuture, Confirmer, Probe, ProbeKind, State};

/// La entrada como la nombra el menú del monitor.
pub fn name(input: Input) -> &'static str {
    match input {
        Input::Hdmi => "HDMI",
        Input::DisplayPort => "DisplayPort",
    }
}

/// Lo que se contesta según lo tecleado; `None` si no se entiende.
pub fn answer(line: &str) -> Option<bool> {
    match line.trim().to_lowercase().as_str() {
        "" | "s" | "si" | "sí" | "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

/// La pregunta de cada sonda.
pub fn question(probe: &Probe) -> String {
    let input = name(probe.input);
    let monitor = probe.monitor.model();
    let secs = probe.timeout.as_secs();
    match probe.kind {
        ProbeKind::Candidate { code } => format!(
            "¿Se ve la máquina de {input} en {monitor}? (mandé {code:#04X}) Enter = lo veo, n = no [{secs} s]"
        ),
        ProbeKind::UserSwitch => format!(
            "Pasa {monitor} a {input} desde el menú del monitor y pulsa Enter cuando veas esa máquina (n = no puedo) [{secs} s]"
        ),
    }
}

/// [`Confirmer`] de teclado: imprime la pregunta y espera la siguiente línea.
pub struct LineConfirmer {
    lines: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    out: Mutex<Box<dyn Write + Send>>,
}

impl LineConfirmer {
    pub fn new(lines: mpsc::UnboundedReceiver<String>, out: Box<dyn Write + Send>) -> Self {
        LineConfirmer {
            lines: tokio::sync::Mutex::new(lines),
            out: Mutex::new(out),
        }
    }

    /// Lee stdin en un hilo propio, que termina con el proceso (o al cerrarse stdin).
    pub fn stdin(out: Box<dyn Write + Send>) -> Self {
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
        Self::new(rx, out)
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
            self.say(&question(probe));
            loop {
                let Some(line) = lines.recv().await else {
                    return Err(ConfirmError(
                        "la entrada estándar se cerró: no hay quién confirme".into(),
                    ));
                };
                match answer(&line) {
                    Some(seen) => return Ok(seen),
                    None => self.say("Enter = lo veo, n = no"),
                }
            }
        })
    }
}

/// El avance como una línea para la terminal; `None` para lo que no hace falta mostrar.
pub fn describe(event: &CalibrationEvent) -> Option<String> {
    Some(match event {
        CalibrationEvent::State(State::Reading) => "leyendo capabilities y 0x60…".into(),
        CalibrationEvent::State(State::UserSwitch { .. }) => return None,
        CalibrationEvent::State(State::Testing { input }) => {
            format!("probando códigos para {}", name(*input))
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
            Some(v) => format!("en {} el monitor lee 0x60 = {v:#04X}", name(*input)),
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
        CalibrationEvent::Seen { seen: false, .. } => "no se vio".into(),
        CalibrationEvent::Seen { seen: true, .. } => return None,
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
            timeout: Duration::from_secs(8),
        }
    }

    #[test]
    fn enter_means_seen() {
        for (line, want) in [
            ("\n", Some(true)),
            ("  \r\n", Some(true)),
            ("Sí\n", Some(true)),
            ("n\n", Some(false)),
            ("NO", Some(false)),
            ("quizás", None),
        ] {
            assert_eq!(answer(line), want, "{line:?}");
        }
    }

    #[test]
    fn questions_name_the_monitor_the_input_and_the_wait() {
        let q = question(&probe(ProbeKind::Candidate { code: 0x05 }));
        assert!(q.contains("Odyssey G5") && q.contains("HDMI") && q.contains("0x05"));
        assert!(q.contains("Enter = lo veo") && q.contains("[8 s]"), "{q}");
        let q = question(&probe(ProbeKind::UserSwitch));
        assert!(q.contains("desde el menú del monitor"), "{q}");
    }

    #[tokio::test]
    async fn the_next_line_answers_and_earlier_ones_are_dropped() {
        let (tx, rx) = mpsc::unbounded_channel();
        let out = Shared::default();
        let c = LineConfirmer::new(rx, Box::new(out.clone()));
        // Tecleado antes de preguntar: no cuenta.
        tx.send("\n".into()).unwrap();
        let p = probe(ProbeKind::Candidate { code: 0x05 });
        let answer = tokio::spawn({
            let tx = tx.clone();
            async move {
                tokio::task::yield_now().await;
                tx.send("¿eh?\n".into()).unwrap();
                tx.send("n\n".into()).unwrap();
            }
        });
        assert_eq!(c.confirm(&p).await, Ok(false));
        answer.await.unwrap();
        let text = out.text();
        assert!(text.contains("¿Se ve la máquina de HDMI"), "{text}");
        assert!(text.contains("Enter = lo veo, n = no\n"), "{text}");

        tx.send("\n".into()).unwrap();
        // Una línea que llegó antes de la pregunta se descarta: sin otra, se espera.
        let waiting = tokio::time::timeout(Duration::from_millis(50), c.confirm(&p)).await;
        assert!(waiting.is_err());
        tx.send("\n".into()).unwrap();
        drop(tx);
        // El canal se cerró con la línea ya leída: la siguiente pregunta falla claro.
        assert!(c.confirm(&p).await.is_err());
    }

    #[test]
    fn progress_lines() {
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
            describe(&trying).unwrap(),
            "DisplayPort: pruebo 0x0F (lectura del monitor, 1 de 31)"
        );
        assert_eq!(
            describe(&CalibrationEvent::Found {
                input: Input::Hdmi,
                code: 0x05
            })
            .unwrap(),
            "HDMI: 0x05 ✓"
        );
        assert_eq!(
            describe(&CalibrationEvent::Declared(Some(vec![0x0F, 0x10]))).unwrap(),
            "el monitor declara 60 = 0x0F 0x10"
        );
        assert!(describe(&CalibrationEvent::State(State::Done)).is_none());
    }
}
