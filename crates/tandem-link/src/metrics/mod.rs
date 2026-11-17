//! Métricas por camino para la pantalla Red y el Puesto (T-102, DESIGN §6).
//!
//! - [`NetMetrics`]: lógica pura. Recibe la foto del gestor de enlaces, la latencia de la
//!   entrada de cada [`Qos`](crate::qos::Qos), los cambios de camino y las mediciones, y
//!   arma el [`NetworkStatus`] que ve la UI: estado de cada camino, RTT, p50/p99 de la
//!   entrada, último ancho de banda, 60 s de historial y los últimos 50 cambios.
//! - [`Metrics`]: la misma, compartida (el agente la guarda aunque el gestor se rearme).
//! - [`follow`]: la tarea que la alimenta desde un [`LinkManager`](crate::manager::LinkManager)
//!   en marcha, una muestra por segundo.
//! - [`probe`]: la prueba de ancho de banda.

pub mod probe;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tandem_proto::net::{HISTORY_SECS, MAX_SWITCHES};
use tandem_proto::{Bandwidth, LatencySample, NetworkStatus, PathMetrics, PathState, PathSwitch};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, MissedTickBehavior};

use crate::manager::{LinkEvent, LinkKind, LinkStatus, Snapshot};
use crate::qos::{LinkQos, input_p99_target};

/// Cada cuánto se toma una muestra.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// Lo que se sabe de un camino.
#[derive(Debug, Clone, Default)]
struct PathRecord {
    rtt: Option<Duration>,
    ever_up: bool,
    input: Option<(Duration, Duration)>,
    bandwidth: Option<Bandwidth>,
    history: VecDeque<LatencySample>,
}

/// Las métricas de los tres caminos. Sin E/S ni reloj: el tiempo llega como argumento.
#[derive(Debug, Clone)]
pub struct NetMetrics {
    paths: BTreeMap<LinkKind, PathRecord>,
    active: Option<LinkKind>,
    /// Del más nuevo al más viejo.
    switches: VecDeque<PathSwitch>,
    measuring: Option<LinkKind>,
}

impl Default for NetMetrics {
    fn default() -> Self {
        NetMetrics {
            paths: LinkKind::ALL
                .into_iter()
                .map(|k| (k, PathRecord::default()))
                .collect(),
            active: None,
            switches: VecDeque::new(),
            measuring: None,
        }
    }
}

fn micros(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

impl NetMetrics {
    /// La foto del gestor: qué responde y cuál lleva el tráfico. Un camino que no está en
    /// `links` (no configurado) cuenta como que no responde.
    pub fn observe(&mut self, links: &[LinkStatus], active: Option<LinkKind>) {
        for (kind, p) in &mut self.paths {
            p.rtt = links.iter().find(|l| l.kind == *kind).and_then(|l| l.rtt);
            p.ever_up |= p.rtt.is_some();
        }
        self.active = active;
    }

    /// El p50 y el p99 de la entrada por `kind` en el último segundo.
    pub fn set_input(&mut self, kind: LinkKind, input: Option<(Duration, Duration)>) {
        if let Some(p) = self.paths.get_mut(&kind) {
            p.input = input;
        }
    }

    /// Pasó un segundo: una muestra más en el historial de cada camino.
    pub fn tick(&mut self) {
        for p in self.paths.values_mut() {
            p.history.push_back(LatencySample {
                rtt_us: p.rtt.map(micros),
                input_p99_us: p.input.map(|(_, p99)| micros(p99)),
            });
            while p.history.len() > HISTORY_SECS {
                p.history.pop_front();
            }
        }
    }

    /// Un cambio del camino activo, `at_ms` en milisegundos Unix.
    pub fn switched(&mut self, event: &LinkEvent, at_ms: u64) {
        self.active = event.to;
        self.switches.push_front(PathSwitch {
            at_ms,
            from: event.from.map(Into::into),
            to: event.to.map(Into::into),
            took_ms: event
                .took
                .map(|d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX)),
        });
        self.switches.truncate(MAX_SWITCHES);
    }

    /// Una medición de ancho de banda de `kind`.
    pub fn measured(&mut self, kind: LinkKind, bytes_per_sec: u64, at_ms: u64) {
        if let Some(p) = self.paths.get_mut(&kind) {
            p.bandwidth = Some(Bandwidth {
                bytes_per_sec,
                at_ms,
            });
        }
    }

    pub fn state(&self, kind: LinkKind) -> PathState {
        let Some(p) = self.paths.get(&kind) else {
            return PathState::Unplugged;
        };
        match (p.rtt.is_some(), p.ever_up) {
            (true, _) if self.active == Some(kind) => PathState::Primary,
            (true, _) => PathState::Backup,
            (false, true) => PathState::Down,
            (false, false) => PathState::Unplugged,
        }
    }

    pub fn status(&self) -> NetworkStatus {
        NetworkStatus {
            paths: self
                .paths
                .iter()
                .map(|(kind, p)| PathMetrics {
                    link: (*kind).into(),
                    state: self.state(*kind),
                    rtt_us: p.rtt.map(micros),
                    input_p50_us: p.input.map(|(p50, _)| micros(p50)),
                    input_p99_us: p.input.map(|(_, p99)| micros(p99)),
                    input_target_us: micros(input_p99_target(Some(*kind))),
                    bandwidth: p.bandwidth,
                    history: p.history.iter().copied().collect(),
                })
                .collect(),
            switches: self.switches.iter().copied().collect(),
            measuring: self.measuring.map(Into::into),
        }
    }
}

/// Las métricas compartidas. Se clona barato.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    inner: Arc<Mutex<NetMetrics>>,
}

/// Mientras exista, hay una medición en curso por su camino.
#[must_use = "la medición termina al soltarla"]
#[derive(Debug)]
pub struct Measuring {
    metrics: Metrics,
}

impl Drop for Measuring {
    fn drop(&mut self) {
        self.metrics.lock().measuring = None;
    }
}

/// La hora Unix en milisegundos, para las fechas que ve la UI.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

impl Metrics {
    fn lock(&self) -> MutexGuard<'_, NetMetrics> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn status(&self) -> NetworkStatus {
        self.lock().status()
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut NetMetrics) -> R) -> R {
        f(&mut self.lock())
    }

    /// Marca que se está midiendo `kind`. `None` si ya hay otra medición: dos a la vez se
    /// reparten el camino y las dos darían la mitad.
    pub fn begin_measure(&self, kind: LinkKind) -> Option<Measuring> {
        let mut m = self.lock();
        if m.measuring.is_some() {
            return None;
        }
        m.measuring = Some(kind);
        Some(Measuring {
            metrics: self.clone(),
        })
    }

    /// Guarda una medición de `kind` y se la da a su `Qos`: el tope de los datos por Wi-Fi
    /// sale de lo último medido (T-101).
    pub fn record_bandwidth(&self, kind: LinkKind, bytes_per_sec: u64, qos: &LinkQos) {
        self.lock().measured(kind, bytes_per_sec, now_ms());
        qos.get(kind).set_measured(bytes_per_sec);
    }
}

/// Alimenta `metrics` desde un gestor de enlaces en marcha (su foto y su cola de cambios)
/// y la latencia de la entrada de cada camino (`qos`), con una muestra cada `every`.
/// Termina cuando el gestor se suelta.
pub async fn follow<H: Clone>(
    metrics: Metrics,
    mut links: watch::Receiver<Snapshot<H>>,
    mut events: mpsc::UnboundedReceiver<LinkEvent>,
    qos: LinkQos,
    every: Duration,
) {
    let observe = |links: &watch::Receiver<Snapshot<H>>| {
        let s = links.borrow();
        let active = s.active.as_ref().map(|a| a.kind);
        metrics.update(|m| m.observe(&s.links, active));
    };
    let mut tick = time::interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            // Primero los cambios: la muestra del mismo instante ya los ve.
            biased;
            event = events.recv() => {
                let Some(event) = event else { return };
                metrics.update(|m| m.switched(&event, now_ms()));
                observe(&links);
            }
            changed = links.changed() => {
                if changed.is_err() {
                    return;
                }
                observe(&links);
            }
            _ = tick.tick() => {
                observe(&links);
                let input: Vec<_> = LinkKind::ALL
                    .into_iter()
                    .map(|k| (k, qos.get(k).input_latency()))
                    .collect();
                metrics.update(|m| {
                    for (k, i) in input {
                        m.set_input(k, i);
                    }
                    m.tick();
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
