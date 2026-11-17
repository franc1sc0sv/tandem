//! Dobles de prueba: programas con salidas guardadas y contadores fijos. Públicos para que
//! el agente (`tandemd`) arme un [`crate::Host`] simulado en sus pruebas.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::meter::{Meter, MeterInfo, Reading};
use crate::run::{Output, RunError, Runner};

/// Respuesta a una llamada.
#[derive(Debug, Clone)]
pub enum Answer {
    Output(Output),
    Timeout,
}

#[derive(Debug, Default)]
struct Inner {
    /// Dónde está instalado cada programa.
    installed: BTreeMap<String, PathBuf>,
    /// Por programa: prefijo de argumentos → respuesta. Gana el prefijo más largo; a igual largo, el último cargado.
    answers: BTreeMap<String, Vec<(Vec<String>, Answer)>>,
    vars: BTreeMap<String, String>,
    calls: Vec<(String, Vec<String>)>,
}

/// Programas simulados: cada uno contesta lo que se le cargó (la salida real de un
/// fixture). Un programa sin respuesta para esos argumentos falla como si no existiera.
#[derive(Debug, Clone, Default)]
pub struct FakeRunner {
    inner: Arc<Mutex<Inner>>,
}

/// El nombre del programa sin carpeta ni extensión, con rutas de Windows o Unix en cualquier
/// sistema (las pruebas simulan uno desde el otro).
fn stem(path: &Path) -> String {
    let full = path.to_string_lossy();
    let name = full.rsplit(['/', '\\']).next().unwrap_or_default();
    name.strip_suffix(".exe").unwrap_or(name).to_owned()
}

impl FakeRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `program` está en `path`.
    pub fn install(&self, program: &str, path: impl Into<PathBuf>) -> &Self {
        self.lock().installed.insert(program.into(), path.into());
        self
    }

    /// `program` con argumentos que empiezan por `args` contesta `answer`. Lo instala en
    /// `/fake/bin` si no estaba.
    pub fn answer(&self, program: &str, args: &[&str], answer: Answer) -> &Self {
        let mut inner = self.lock();
        inner
            .installed
            .entry(program.into())
            .or_insert_with(|| PathBuf::from(format!("/fake/bin/{program}")));
        inner
            .answers
            .entry(program.into())
            .or_default()
            .push((args.iter().map(|a| (*a).to_owned()).collect(), answer));
        drop(inner);
        self
    }

    /// Salida exitosa con `stdout`.
    pub fn on(&self, program: &str, args: &[&str], stdout: impl Into<Vec<u8>>) -> &Self {
        self.answer(program, args, Answer::Output(Output::ok(stdout)))
    }

    pub fn set_var(&self, name: &str, value: &str) -> &Self {
        self.lock().vars.insert(name.into(), value.into());
        self
    }

    /// Las llamadas hechas, con el nombre del programa sin ruta.
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.lock().calls.clone()
    }
}

impl Runner for FakeRunner {
    fn find(&self, program: &str, _extra: Option<&str>) -> Option<PathBuf> {
        self.lock().installed.get(program).cloned()
    }

    fn run(&self, program: &Path, args: &[&str], timeout: Duration) -> Result<Output, RunError> {
        let name = stem(program);
        let mut inner = self.lock();
        inner
            .calls
            .push((name.clone(), args.iter().map(|a| (*a).to_owned()).collect()));
        let answer = inner
            .answers
            .get(&name)
            .and_then(|list| {
                list.iter()
                    .filter(|(prefix, _)| {
                        prefix.len() <= args.len() && prefix.iter().zip(args).all(|(p, a)| p == a)
                    })
                    .max_by_key(|(prefix, _)| prefix.len())
            })
            .map(|(_, a)| a.clone());
        match answer {
            Some(Answer::Output(out)) => Ok(out),
            Some(Answer::Timeout) => Err(RunError::Timeout {
                program: name,
                secs: timeout.as_secs(),
            }),
            None => Err(RunError::Spawn {
                program: name,
                reason: "sin respuesta cargada".into(),
            }),
        }
    }

    fn var(&self, name: &str) -> Option<String> {
        self.lock().vars.get(name).cloned()
    }
}

/// Contadores fijos; cada [`Meter::read`] da la siguiente lectura de la cola y, cuando se
/// acaba, repite la última.
#[derive(Debug, Clone, Default)]
pub struct FakeMeter {
    pub info: MeterInfo,
    readings: Arc<Mutex<VecDeque<Reading>>>,
    last: Arc<Mutex<Reading>>,
}

impl FakeMeter {
    pub fn new(info: MeterInfo) -> Self {
        FakeMeter {
            info,
            ..Self::default()
        }
    }

    pub fn push(&self, reading: Reading) -> &Self {
        self.readings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(reading);
        self
    }
}

impl Meter for FakeMeter {
    fn info(&mut self) -> MeterInfo {
        self.info.clone()
    }

    fn read(&mut self) -> Reading {
        let next = self
            .readings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = next {
            *last = r;
        }
        last.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_prefix_wins_and_unknown_calls_fail() {
        let r = FakeRunner::new();
        r.on("wsl", &["-l"], "lista").on("wsl", &[], "otra");
        let wsl = r.find("wsl", None).unwrap();
        let t = Duration::from_secs(1);
        assert_eq!(r.run(&wsl, &["-l", "-v"], t).unwrap().text(), "lista");
        assert_eq!(r.run(&wsl, &["-d", "x"], t).unwrap().text(), "otra");
        assert!(r.run(Path::new("/fake/bin/git"), &[], t).is_err());
        assert_eq!(r.calls().len(), 3);
        assert_eq!(r.find("git", None), None);
    }

    #[test]
    fn the_meter_repeats_the_last_reading() {
        let mut m = FakeMeter::default();
        m.push(Reading {
            cpu: 10.0,
            ..Reading::default()
        });
        assert_eq!(m.read().cpu, 10.0);
        assert_eq!(m.read().cpu, 10.0);
    }
}
