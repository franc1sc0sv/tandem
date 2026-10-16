//! Los pasos de energía de esta máquina: Wake-on-LAN hacia la otra (si tiene su
//! `[power.wol]`) y apagado limpio de sí misma (si sabe, DESIGN §5). Cualquiera de las
//! dos puede despertar a la otra (DESIGN §3.7). El resto de los pasos pasa a otro
//! [`LocalSteps`].

use std::sync::Arc;

use tandem_core::Step;
use tandem_power::{Shutdown, UdpWake, Wake};

use crate::config::AgentConfig;
use crate::orchestrator::{LocalSteps, STEP_NOT_IMPLEMENTED, StepFuture, describe};

/// Falta `[power.wol]` en `tandem.toml`.
pub const WOL_NOT_CONFIGURED: &str = "falta [power.wol] en tandem.toml";

/// Envuelve los pasos de una máquina y agrega los de energía.
pub struct PowerSteps {
    inner: Arc<dyn LocalSteps>,
    wake: Option<Arc<dyn Wake>>,
    shutdown: Option<Arc<dyn Shutdown>>,
}

impl std::fmt::Debug for PowerSteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowerSteps")
            .field("wake", &self.wake.is_some())
            .field("shutdown", &self.shutdown.is_some())
            .finish_non_exhaustive()
    }
}

impl PowerSteps {
    pub fn new(
        inner: Arc<dyn LocalSteps>,
        wake: Option<Arc<dyn Wake>>,
        shutdown: Option<Arc<dyn Shutdown>>,
    ) -> Self {
        PowerSteps {
            inner,
            wake,
            shutdown,
        }
    }

    /// Lo que corresponde a la máquina de `cfg`: despierta a la otra con `[power.wol]` y
    /// se apaga a sí misma si sabe (hoy, Windows).
    pub fn for_machine(cfg: &AgentConfig, inner: Arc<dyn LocalSteps>) -> Self {
        let wake = cfg
            .power
            .wol
            .clone()
            .map(|t| Arc::new(UdpWake::new(t)) as Arc<dyn Wake>);
        let shutdown = if crate::capabilities::powers_off(cfg.machine) {
            system_shutdown()
        } else {
            None
        };
        Self::new(inner, wake, shutdown)
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
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        match step {
            // Encender a la otra (el orquestador solo lo manda a la que no se enciende).
            Step::PowerOn { .. } => {
                // Un envío UDP no bloquea: no hace falta sacarlo del runtime.
                let r = match &self.wake {
                    Some(w) => w.wake().map_err(|e| e.to_string()),
                    None => Err(format!("{}: {WOL_NOT_CONFIGURED}", describe(step))),
                };
                Box::pin(async move { r })
            }
            // Apagarse a sí misma.
            Step::PowerOff { .. } => {
                let r = match &self.shutdown {
                    Some(s) => {
                        tracing::warn!("apagando esta máquina a pedido del orquestador");
                        s.power_off().map_err(|e| e.to_string())
                    }
                    None => Err(format!("{}: {STEP_NOT_IMPLEMENTED}", describe(step))),
                };
                Box::pin(async move { r })
            }
            _ => self.inner.execute(step),
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
    use crate::orchestrator::Unimplemented;
    use crate::orchestrator::tests::FakeSteps;

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
            Err(PowerError::Shutdown("acceso denegado".into()))
        }
    }

    #[tokio::test]
    async fn power_steps_go_to_their_backends_and_the_rest_to_inner() {
        let inner = Arc::new(FakeSteps::default());
        let wake = Arc::new(Count::default());
        let off = Arc::new(Count::default());
        let p = PowerSteps::new(inner.clone(), Some(wake.clone()), Some(off.clone()));
        p.execute(&Step::PowerOn {
            machine: Machine::Pc,
        })
        .await
        .unwrap();
        p.execute(&Step::PowerOff {
            machine: Machine::Pc,
        })
        .await
        .unwrap();
        p.execute(&Step::UpdateInputGeometry).await.unwrap();
        assert_eq!(wake.0.load(Ordering::Relaxed), 1);
        assert_eq!(off.0.load(Ordering::Relaxed), 1);
        assert_eq!(inner.steps(), [Step::UpdateInputGeometry]);
    }

    #[tokio::test]
    async fn missing_backends_fail_clearly() {
        let p = PowerSteps::new(Arc::new(Unimplemented), None, None);
        assert_eq!(
            p.execute(&Step::PowerOn {
                machine: Machine::Pc
            })
            .await
            .unwrap_err(),
            format!("encender la PC: {WOL_NOT_CONFIGURED}")
        );
        assert_eq!(
            p.execute(&Step::PowerOff {
                machine: Machine::Pc
            })
            .await
            .unwrap_err(),
            format!("apagar la PC: {STEP_NOT_IMPLEMENTED}")
        );
    }

    #[tokio::test]
    async fn shutdown_errors_reach_the_orchestrator() {
        let p = PowerSteps::new(Arc::new(Unimplemented), None, Some(Arc::new(Refuse)));
        let e = p
            .execute(&Step::PowerOff {
                machine: Machine::Pc,
            })
            .await
            .unwrap_err();
        assert!(e.contains("acceso denegado"), "{e}");
    }

    #[test]
    fn each_machine_gets_its_own_backends() {
        let mut cfg = fixtures::desk();
        let p = PowerSteps::for_machine(&cfg, Arc::new(Unimplemented));
        assert!(p.wake.is_none(), "sin [power.wol] no hay con qué despertar");
        assert!(p.shutdown.is_none(), "la Mac no se apaga desde Tandem");

        cfg.power.wol = Some(tandem_power::WolTarget::new(
            "9c:6b:00:1a:2b:ff".parse().unwrap(),
        ));
        let p = PowerSteps::for_machine(&cfg, Arc::new(Unimplemented));
        assert!(p.wake.is_some());

        // La PC también puede despertar a la Mac si tiene su [power.wol] (DESIGN §3.7).
        cfg.machine = Machine::Pc;
        let p = PowerSteps::for_machine(&cfg, Arc::new(Unimplemented));
        assert!(p.wake.is_some(), "la PC despierta a la Mac");
        assert_eq!(p.shutdown.is_some(), cfg!(windows));
        cfg.power.wol = None;
        let p = PowerSteps::for_machine(&cfg, Arc::new(Unimplemented));
        assert!(p.wake.is_none());
    }
}
