//! Test doubles: programs with saved outputs and fixed counters. Public so that
//! the agent (`tandemd`) can build a simulated [`crate::Host`] in its tests.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::meter::{Meter, MeterInfo, Reading};
use crate::run::{Output, RunError, Runner};

/// The answer to a call.
#[derive(Debug, Clone)]
pub enum Answer {
    Output(Output),
    Timeout,
}

#[derive(Debug, Default)]
struct Inner {
    /// Where each program is installed.
    installed: BTreeMap<String, Vec<PathBuf>>,
    /// Per program: argument prefix → answer. The longest prefix wins; at equal length, the last one loaded.
    answers: BTreeMap<String, Vec<(Vec<String>, Answer)>>,
    vars: BTreeMap<String, String>,
    calls: Vec<(String, Vec<String>)>,
}

/// Simulated programs: each one answers what was loaded into it (the real output of a
/// fixture). A program with no answer for those arguments fails as if it did not exist.
#[derive(Debug, Clone, Default)]
pub struct FakeRunner {
    inner: Arc<Mutex<Inner>>,
}

/// The program name without folder or extension, with Windows or Unix paths on any
/// system (tests simulate one from the other).
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

    /// `program` is at `path` (and only there).
    pub fn install(&self, program: &str, path: impl Into<PathBuf>) -> &Self {
        self.lock()
            .installed
            .insert(program.into(), vec![path.into()]);
        self
    }

    /// Another copy of `program`, further along in the `PATH`.
    pub fn install_also(&self, program: &str, path: impl Into<PathBuf>) -> &Self {
        self.lock()
            .installed
            .entry(program.into())
            .or_default()
            .push(path.into());
        self
    }

    /// `program` with arguments that start with `args` answers `answer`. It installs it in
    /// `/fake/bin` if it was not there.
    pub fn answer(&self, program: &str, args: &[&str], answer: Answer) -> &Self {
        let mut inner = self.lock();
        inner
            .installed
            .entry(program.into())
            .or_insert_with(|| vec![PathBuf::from(format!("/fake/bin/{program}"))]);
        inner
            .answers
            .entry(program.into())
            .or_default()
            .push((args.iter().map(|a| (*a).to_owned()).collect(), answer));
        drop(inner);
        self
    }

    /// Successful output with `stdout`.
    pub fn on(&self, program: &str, args: &[&str], stdout: impl Into<Vec<u8>>) -> &Self {
        self.answer(program, args, Answer::Output(Output::ok(stdout)))
    }

    pub fn set_var(&self, name: &str, value: &str) -> &Self {
        self.lock().vars.insert(name.into(), value.into());
        self
    }

    /// The calls made, with the program name without a path.
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.lock().calls.clone()
    }
}

impl Runner for FakeRunner {
    fn find(&self, program: &str, _extra: Option<&str>) -> Option<PathBuf> {
        self.lock()
            .installed
            .get(program)
            .and_then(|p| p.first().cloned())
    }

    fn find_all(&self, program: &str) -> Vec<PathBuf> {
        self.lock()
            .installed
            .get(program)
            .cloned()
            .unwrap_or_default()
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
                reason: "no answer loaded".into(),
            }),
        }
    }

    fn var(&self, name: &str) -> Option<String> {
        self.lock().vars.get(name).cloned()
    }
}

/// Fixed counters; each [`Meter::read`] gives the next reading in the queue and, when it
/// runs out, repeats the last one.
#[derive(Debug, Clone, Default)]
pub struct FakeMeter {
    pub info: MeterInfo,
    readings: Arc<Mutex<VecDeque<Reading>>>,
    last: Arc<Mutex<Reading>>,
    /// How many readings were made (across all clones): says whether someone is still sampling.
    reads: Arc<AtomicUsize>,
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

    /// The readings made so far.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl Meter for FakeMeter {
    fn info(&mut self) -> MeterInfo {
        self.info.clone()
    }

    fn read(&mut self) -> Reading {
        self.reads.fetch_add(1, Ordering::SeqCst);
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
        r.on("wsl", &["-l"], "list").on("wsl", &[], "other");
        let wsl = r.find("wsl", None).unwrap();
        let t = Duration::from_secs(1);
        assert_eq!(r.run(&wsl, &["-l", "-v"], t).unwrap().text(), "list");
        assert_eq!(r.run(&wsl, &["-d", "x"], t).unwrap().text(), "other");
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
        assert_eq!(m.clone().read().cpu, 10.0);
        assert_eq!(m.reads(), 2);
    }
}
