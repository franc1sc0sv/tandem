//! Muestreo de recursos a 1 Hz, solo mientras alguien mira (T-105).
//!
//! Cada interesado toma un [`Watch`]; con el primero arranca la tarea de muestreo y con el
//! último que se suelta se detiene. Las muestras van por un `broadcast` a quien se suscriba.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_proto::Resources;
use tokio::sync::broadcast;

/// Cada cuánto se muestrea.
pub const PERIOD: Duration = Duration::from_secs(1);

type SampleFn = dyn Fn() -> Resources + Send + Sync;

struct Inner {
    sample: Arc<SampleFn>,
    tx: broadcast::Sender<Resources>,
    period: Duration,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    watchers: usize,
    running: bool,
}

/// El muestreador. Clonarlo comparte el mismo.
#[derive(Clone)]
pub struct ResourceMonitor {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ResourceMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceMonitor")
            .field("watchers", &self.watchers())
            .finish_non_exhaustive()
    }
}

/// Mientras exista, se muestrea.
#[must_use = "se deja de muestrear al soltarlo"]
pub struct Watch {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Watch")
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut s = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        s.watchers = s.watchers.saturating_sub(1);
    }
}

impl ResourceMonitor {
    /// `sample` lee los recursos; puede bloquear (corre en un hilo aparte).
    pub fn new(sample: impl Fn() -> Resources + Send + Sync + 'static) -> Self {
        Self::with_period(sample, PERIOD)
    }

    pub fn with_period(
        sample: impl Fn() -> Resources + Send + Sync + 'static,
        period: Duration,
    ) -> Self {
        ResourceMonitor {
            inner: Arc::new(Inner {
                sample: Arc::new(sample),
                tx: broadcast::channel(8).0,
                period,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Las muestras que salgan desde ahora.
    pub fn subscribe(&self) -> broadcast::Receiver<Resources> {
        self.inner.tx.subscribe()
    }

    /// Cuántos están mirando.
    pub fn watchers(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .watchers
    }

    /// Empezar a mirar. Fuera de un runtime de tokio no se muestrea (y se avisa).
    pub fn watch(&self) -> Watch {
        let mut s = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        s.watchers += 1;
        if !s.running {
            match tokio::runtime::Handle::try_current() {
                Ok(rt) => {
                    s.running = true;
                    rt.spawn(run(self.inner.clone()));
                }
                Err(_) => tracing::warn!("sin runtime: no se muestrean los recursos"),
            }
        }
        drop(s);
        Watch {
            inner: self.inner.clone(),
        }
    }
}

/// Sigue mientras haya alguien mirando; la decisión de parar y la de arrancar otra tarea se
/// toman con el mismo candado, así nunca quedan dos tareas ni ninguna con alguien mirando.
async fn run(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(inner.period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if stop_if_unwatched(&inner) {
            return;
        }
        let sample = inner.sample.clone();
        let Ok(resources) = tokio::task::spawn_blocking(move || sample()).await else {
            tracing::warn!("la lectura de recursos falló");
            continue;
        };
        if stop_if_unwatched(&inner) {
            return;
        }
        // Sin suscriptores no es un error: quien mira puede no estar escuchando todavía.
        let _ = inner.tx.send(resources);
    }
}

fn stop_if_unwatched(inner: &Inner) -> bool {
    let mut s = inner.state.lock().unwrap_or_else(PoisonError::into_inner);
    if s.watchers == 0 {
        s.running = false;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU16, Ordering};

    use tandem_proto::CpuLoad;

    use super::*;

    fn counting() -> (ResourceMonitor, Arc<AtomicU16>) {
        let n = Arc::new(AtomicU16::new(0));
        let c = n.clone();
        let m = ResourceMonitor::new(move || Resources {
            cpu: CpuLoad {
                load: c.fetch_add(1, Ordering::SeqCst) + 1,
                ..CpuLoad::default()
            },
            ..Resources::default()
        });
        (m, n)
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_sampled_without_watchers() {
        let (m, n) = counting();
        let _rx = m.subscribe();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn samples_once_per_second_while_watched_and_stops_after() {
        let (m, n) = counting();
        let mut rx = m.subscribe();
        let w = m.watch();
        let w2 = m.watch();
        assert_eq!(m.watchers(), 2);
        for want in 1..=3 {
            assert_eq!(rx.recv().await.unwrap().cpu.load, want);
        }
        drop(w);
        assert_eq!(rx.recv().await.unwrap().cpu.load, 4);
        drop(w2);
        assert_eq!(m.watchers(), 0);
        let before = n.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(n.load(Ordering::SeqCst) <= before + 1, "siguió muestreando");
        assert!(rx.try_recv().is_err());
        // Volver a mirar arranca otra vez, con una sola tarea.
        let _w = m.watch();
        let next = rx.recv().await.unwrap().cpu.load;
        let after = rx.recv().await.unwrap().cpu.load;
        assert_eq!(after, next + 1);
    }

    #[test]
    fn without_a_runtime_watching_does_not_panic() {
        let (m, n) = counting();
        let w = m.watch();
        assert_eq!(m.watchers(), 1);
        drop(w);
        assert_eq!(m.watchers(), 0);
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }
}
