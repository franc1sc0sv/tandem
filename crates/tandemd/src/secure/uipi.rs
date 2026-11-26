//! A window of higher integrity in the foreground (T-363).
//!
//! UIPI keeps a medium-integrity process from moving the cursor or injecting input while an
//! elevated window (Task Manager, an installer) has the foreground. Without the secure
//! desktop service every event fails with the same error; logging each one buried the log
//! (9,700 lines in 13 minutes) and told the user nothing. This module groups the failures
//! into *episodes*: one log line when it starts, a counter while it lasts, one line when it
//! ends, and a state the UI turns into "<process> runs as administrator...".
//!
//! The decisions are portable; what asks Windows who has the foreground goes behind
//! [`ForegroundProbe`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Who has the foreground and whether it outranks this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// Executable name, without the path.
    pub process: String,
    /// Higher integrity than the agent: UIPI applies.
    pub elevated: bool,
}

pub trait ForegroundProbe: Send {
    /// `None`: no foreground window, or it cannot be inspected.
    fn foreground(&self) -> Option<Foreground>;
}

/// How often an ongoing episode asks again who has the foreground (the user may switch
/// windows without a single injection succeeding in between).
pub const REPROBE_EVERY: Duration = Duration::from_secs(1);

/// What a failure means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not UIPI: the caller reports the error as usual (and any episode is over).
    Other,
    /// A new episode (or another process took over): log it once.
    Started { process: String },
    /// Already counted; nothing to log.
    Ongoing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Active {
    process: String,
    count: u64,
    next_probe: Instant,
}

#[derive(Debug, Default)]
pub struct Episodes {
    active: Option<Active>,
}

impl Episodes {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Events lost in the current episode.
    pub fn count(&self) -> u64 {
        self.active.as_ref().map_or(0, |a| a.count)
    }

    /// An injection failed at `now`.
    pub fn failure(&mut self, now: Instant, probe: &dyn ForegroundProbe) -> Verdict {
        if let Some(a) = self.active.as_mut()
            && now < a.next_probe
        {
            a.count += 1;
            publish(Some((&a.process, a.count)));
            return Verdict::Ongoing;
        }
        match probe.foreground() {
            Some(fg) if fg.elevated => {
                let same = self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.process == fg.process);
                let count = self.active.as_ref().map_or(0, |a| a.count) + 1;
                publish(Some((&fg.process, count)));
                self.active = Some(Active {
                    process: fg.process.clone(),
                    count,
                    next_probe: now + REPROBE_EVERY,
                });
                if same {
                    Verdict::Ongoing
                } else {
                    Verdict::Started {
                        process: fg.process,
                    }
                }
            }
            _ => {
                self.active = None;
                publish(None);
                Verdict::Other
            }
        }
    }

    /// An injection worked: the episode is over. Returns what it was, to log it.
    pub fn recovered(&mut self) -> Option<(String, u64)> {
        let a = self.active.take()?;
        publish(None);
        Some((a.process, a.count))
    }
}

/// What the UI is told while an episode lasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub process: String,
    pub count: u64,
}

static BLOCKED: Mutex<Option<Blocked>> = Mutex::new(None);

fn publish(state: Option<(&str, u64)>) {
    let mut guard = BLOCKED.lock().unwrap_or_else(|p| p.into_inner());
    *guard = state.map(|(process, count)| Blocked {
        process: process.to_owned(),
        count,
    });
}

/// The ongoing episode, if any.
pub fn blocked() -> Option<Blocked> {
    BLOCKED.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// What the module says during an episode.
pub fn blocked_reason(process: &str) -> String {
    format!(
        "{process} runs as administrator; Tandem cannot control it without the protected screens service"
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Serializes the tests that read or write the process-wide state.
    pub static GLOBAL: Mutex<()> = Mutex::new(());

    pub struct Fake(pub Mutex<Option<Foreground>>);

    impl Fake {
        pub fn elevated(name: &str) -> Self {
            Fake(Mutex::new(Some(Foreground {
                process: name.into(),
                elevated: true,
            })))
        }
        fn set(&self, fg: Option<Foreground>) {
            *self.0.lock().unwrap() = fg;
        }
    }

    impl ForegroundProbe for Fake {
        fn foreground(&self) -> Option<Foreground> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn an_episode_logs_once_and_counts_the_rest() {
        let _g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
        let probe = Fake::elevated("Taskmgr.exe");
        let mut eps = Episodes::default();
        let t0 = Instant::now();
        assert_eq!(
            eps.failure(t0, &probe),
            Verdict::Started {
                process: "Taskmgr.exe".into()
            }
        );
        for i in 1..1000u64 {
            let t = t0 + Duration::from_micros(i);
            assert_eq!(eps.failure(t, &probe), Verdict::Ongoing);
        }
        assert_eq!(eps.count(), 1000);
        assert_eq!(
            blocked(),
            Some(Blocked {
                process: "Taskmgr.exe".into(),
                count: 1000
            })
        );
        assert_eq!(eps.recovered(), Some(("Taskmgr.exe".into(), 1000)));
        assert_eq!(blocked(), None);
        assert_eq!(eps.recovered(), None);
    }

    #[test]
    fn a_failure_with_a_normal_foreground_is_not_an_episode() {
        let _g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
        let probe = Fake(Mutex::new(Some(Foreground {
            process: "notepad.exe".into(),
            elevated: false,
        })));
        let mut eps = Episodes::default();
        assert_eq!(eps.failure(Instant::now(), &probe), Verdict::Other);
        assert!(!eps.is_active());
        probe.set(None);
        assert_eq!(eps.failure(Instant::now(), &probe), Verdict::Other);
    }

    #[test]
    fn a_new_foreground_process_starts_a_new_log_line_after_the_reprobe() {
        let _g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
        let probe = Fake::elevated("Taskmgr.exe");
        let mut eps = Episodes::default();
        let t0 = Instant::now();
        eps.failure(t0, &probe);
        probe.set(Some(Foreground {
            process: "regedit.exe".into(),
            elevated: true,
        }));
        // Within the reprobe window it is still counted for the first one.
        assert_eq!(
            eps.failure(t0 + Duration::from_millis(10), &probe),
            Verdict::Ongoing
        );
        assert_eq!(
            eps.failure(t0 + REPROBE_EVERY, &probe),
            Verdict::Started {
                process: "regedit.exe".into()
            }
        );
        // The same process after another reprobe is not a new line.
        assert_eq!(
            eps.failure(t0 + REPROBE_EVERY * 3, &probe),
            Verdict::Ongoing
        );
    }

    #[test]
    fn the_episode_ends_when_the_foreground_stops_being_elevated() {
        let _g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
        let probe = Fake::elevated("Taskmgr.exe");
        let mut eps = Episodes::default();
        let t0 = Instant::now();
        eps.failure(t0, &probe);
        probe.set(None);
        assert_eq!(eps.failure(t0 + REPROBE_EVERY, &probe), Verdict::Other);
        assert!(!eps.is_active());
        assert_eq!(blocked(), None);
    }

    #[test]
    fn the_reason_names_the_process() {
        assert!(blocked_reason("Taskmgr.exe").starts_with("Taskmgr.exe runs as administrator;"));
    }
}
