//! This machine's power steps: Wake-on-LAN toward the other (if it has its
//! `[power.wol]`) and clean shutdown of itself (if it can, DESIGN §5). Either of the
//! two can wake the other (DESIGN §3.7).

use std::sync::{Arc, PoisonError, RwLock};

use tandem_core::Step;
use tandem_power::{Shutdown, UdpWake, Wake};

use crate::config::AgentConfig;
use crate::orchestrator::{LocalSteps, STEP_NOT_IMPLEMENTED, StepFuture, describe};

/// Without `[power.wol]` there is nothing to wake the other with.
pub const WOL_NOT_CONFIGURED: &str = "the other machine's MAC is missing to power it on (it is learned when pairing or written on its page)";

/// A machine's power steps.
pub struct PowerSteps {
    wake: RwLock<Option<Arc<dyn Wake>>>,
    shutdown: Option<Arc<dyn Shutdown>>,
}

impl std::fmt::Debug for PowerSteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowerSteps")
            .field("wake", &self.has_wake())
            .field("shutdown", &self.shutdown.is_some())
            .finish_non_exhaustive()
    }
}

impl PowerSteps {
    pub fn new(wake: Option<Arc<dyn Wake>>, shutdown: Option<Arc<dyn Shutdown>>) -> Self {
        PowerSteps {
            wake: RwLock::new(wake),
            shutdown,
        }
    }

    fn has_wake(&self) -> bool {
        self.wake
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// What corresponds to `cfg`'s machine: it wakes the other with `[power.wol]` and
    /// shuts itself down if its system can (today, Windows).
    pub fn for_machine(cfg: &AgentConfig) -> Self {
        let wake = cfg
            .power
            .wol
            .clone()
            .map(|t| Arc::new(UdpWake::new(t)) as Arc<dyn Wake>);
        let shutdown = if crate::capabilities::powers_off(cfg.os) {
            system_shutdown()
        } else {
            None
        };
        Self::new(wake, shutdown)
    }
}

#[cfg(windows)]
fn system_shutdown() -> Option<Arc<dyn Shutdown>> {
    Some(Arc::new(tandem_power::WindowsShutdown))
}

#[cfg(not(windows))]
fn system_shutdown() -> Option<Arc<dyn Shutdown>> {
    None
}

impl LocalSteps for PowerSteps {
    fn set_wol(&self, wol: Option<tandem_power::WolTarget>) {
        *self.wake.write().unwrap_or_else(PoisonError::into_inner) =
            wol.map(|t| Arc::new(UdpWake::new(t)) as Arc<dyn Wake>);
    }

    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        match step {
            // Power on the other (the orchestrator only sends it to the one that does not power itself on).
            Step::PowerOn { .. } => {
                // A UDP send does not block: it need not be taken out of the runtime.
                let wake = self
                    .wake
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let r = match &wake {
                    Some(w) => w.wake().map_err(|e| e.to_string()),
                    None => Err(format!("{}: {WOL_NOT_CONFIGURED}", describe(step))),
                };
                Box::pin(async move { r })
            }
            // Shutting itself down.
            Step::PowerOff { .. } => {
                let r = match &self.shutdown {
                    Some(s) => {
                        tracing::warn!("shutting this machine down at the orchestrator's request");
                        s.power_off().map_err(|e| e.to_string())
                    }
                    None => Err(format!("{}: {STEP_NOT_IMPLEMENTED}", describe(step))),
                };
                Box::pin(async move { r })
            }
            // Monitors are changed by the orchestrator with their owner, and scene steps are
            // done by whoever applies it: none of this is power.
            Step::SwitchMonitor { .. }
            | Step::ShowMachine { .. }
            | Step::Arrange { .. }
            | Step::Geometry => {
                Box::pin(async move { Err(format!("{}: {STEP_NOT_IMPLEMENTED}", describe(step))) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tandem_power::PowerError;

    use tandem_core::Machine;

    use super::*;
    use crate::config::fixtures;

    #[derive(Default)]
    struct Count(AtomicUsize);

    impl Wake for Count {
        fn wake(&self) -> Result<(), PowerError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    impl Shutdown for Count {
        fn power_off(&self) -> Result<(), PowerError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct Refuse;

    impl Shutdown for Refuse {
        fn power_off(&self) -> Result<(), PowerError> {
            Err(PowerError::Shutdown("access denied".into()))
        }
    }

    #[tokio::test]
    async fn power_steps_go_to_their_backends() {
        let wake = Arc::new(Count::default());
        let off = Arc::new(Count::default());
        let p = PowerSteps::new(Some(wake.clone()), Some(off.clone()));
        p.execute(&Step::PowerOn {
            machine: Machine::B,
        })
        .await
        .unwrap();
        p.execute(&Step::PowerOff {
            machine: Machine::B,
        })
        .await
        .unwrap();
        assert_eq!(wake.0.load(Ordering::Relaxed), 1);
        assert_eq!(off.0.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn missing_backends_fail_clearly() {
        let p = PowerSteps::new(None, None);
        assert_eq!(
            p.execute(&Step::PowerOn {
                machine: Machine::B
            })
            .await
            .unwrap_err(),
            format!("power on machine B: {WOL_NOT_CONFIGURED}")
        );
        assert_eq!(
            p.execute(&Step::PowerOff {
                machine: Machine::B
            })
            .await
            .unwrap_err(),
            format!("power off machine B: {STEP_NOT_IMPLEMENTED}")
        );
    }

    #[tokio::test]
    async fn shutdown_errors_reach_the_orchestrator() {
        let p = PowerSteps::new(None, Some(Arc::new(Refuse)));
        let e = p
            .execute(&Step::PowerOff {
                machine: Machine::B,
            })
            .await
            .unwrap_err();
        assert!(e.contains("access denied"), "{e}");
    }

    #[test]
    fn each_machine_gets_its_own_backends() {
        let mut cfg = fixtures::desk();
        cfg.os = tandem_core::Os::MacOs;
        let p = PowerSteps::for_machine(&cfg);
        assert!(
            !p.has_wake(),
            "without [power.wol] there is nothing to wake with"
        );
        assert!(p.shutdown.is_none(), "macOS does not shut down from Tandem");

        cfg.power.wol = Some(tandem_power::WolTarget::new(
            "aa:bb:cc:dd:ee:ff".parse().unwrap(),
        ));
        let p = PowerSteps::for_machine(&cfg);
        assert!(p.has_wake());

        // One with Windows also wakes the other if it has its [power.wol], whichever the side
        // is (DESIGN §3.7), and shuts itself down only where the backend exists.
        cfg.machine = Machine::B;
        cfg.os = tandem_core::Os::Windows;
        let p = PowerSteps::for_machine(&cfg);
        assert!(p.has_wake(), "B wakes A");
        assert_eq!(p.shutdown.is_some(), cfg!(windows));
        cfg.power.wol = None;
        let p = PowerSteps::for_machine(&cfg);
        assert!(!p.has_wake());
    }

    #[test]
    fn the_wake_target_can_be_learned_after_starting() {
        let p = PowerSteps::new(None, None);
        assert!(!p.has_wake());
        p.set_wol(Some(tandem_power::WolTarget::new(
            "aa:bb:cc:dd:ee:ff".parse().unwrap(),
        )));
        assert!(p.has_wake());
        p.set_wol(None);
        assert!(!p.has_wake());
    }
}
