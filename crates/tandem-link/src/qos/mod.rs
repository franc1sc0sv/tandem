//! Prioridades y tope de los datos (DECISIONS A2, DESIGN §6).
//!
//! Los datos (túneles, archivos) van por cualquier camino, también por Wi-Fi. Para que no le
//! metan latencia al teclado y al mouse:
//!
//! - **Prioridad de streams QUIC** ([`StreamClass`]): entrada > control > portapapeles >
//!   datos. QUIC manda primero lo pendiente del stream de más prioridad, y los datagramas
//!   del movimiento van antes que cualquier stream en cada paquete.
//! - **Tope en Wi-Fi** ([`data_cap`]): la prioridad solo ordena lo que todavía no salió;
//!   lo que ya está en la cola del punto de acceso sale en orden de llegada. Con los datos
//!   por debajo de lo que da el Wi-Fi (por omisión, el 50 % de lo medido) esa cola no
//!   crece y la tecla siguiente no espera detrás de un archivo.
//! - **Pausa por latencia** ([`LatencyGuard`]): si aun así el p99 de la entrada pasa su
//!   objetivo durante 2 s (el Wi-Fi empeoró), los datos se pausan hasta que baje.
//!
//! Todo eso vive en un [`Qos`] por camino ([`LinkQos`]): entre las dos máquinas hay más de
//! una conexión (la que abre cada agente y la que acepta), y todas las de un mismo camino
//! comparten el mismo aire, así que comparten el tope y la pausa
//! ([`Connection::with_qos`](crate::transport::Connection::with_qos)). Los módulos de datos
//! escriben por un [`DataStream`], que espera la pausa y el tope, y siguen la pausa con
//! [`Qos::gate`] o [`Qos::events`].
//!
//! **Quien mide la entrada (T-093)** pasa cada latencia a
//! `conn.qos().record_input_latency(..)` de la conexión por la que llegó, **en las dos
//! máquinas**: los datos de las dos compiten por el mismo aire, así que el lado que inyecta
//! la mide y el que captura la recibe de vuelta (el histograma de T-093) y también la
//! anota en su `Qos`.

mod cap;
mod guard;
mod limiter;
mod stream;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::DataPrefs;
use tandem_proto::Channel;
use tokio::sync::{broadcast, watch};
use tokio::time::{self, Instant};

pub use cap::{UNMEASURED_WIFI, data_cap, input_p99_target};
pub use guard::{GuardConfig, LatencyGuard, Transition};
pub use limiter::{MAX_BURST, MIN_CAP, RateLimiter};
pub use stream::DataStream;

use crate::manager::LinkKind;

/// Clase de un stream QUIC, de más a menos prioridad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamClass {
    /// Teclas y clics (el movimiento va en datagramas).
    Input,
    /// Pedidos entre agentes, pings, estado.
    Control,
    Clipboard,
    /// Túneles y archivos.
    Data,
}

impl StreamClass {
    /// Prioridad para quinn (más alta, antes). Control queda en 0, la de omisión: un
    /// stream sin clase es de control.
    pub fn priority(self) -> i32 {
        match self {
            StreamClass::Input => 10,
            StreamClass::Control => 0,
            StreamClass::Clipboard => -10,
            StreamClass::Data => -20,
        }
    }

    /// La clase del stream propio de un canal ([`tandem_proto::ControlMsg::Open`]).
    pub fn of(channel: Channel) -> Self {
        match channel {
            Channel::Input => StreamClass::Input,
            Channel::Clipboard => StreamClass::Clipboard,
        }
    }
}

/// Si los datos pueden salir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataGate {
    Open,
    /// La entrada pasó su objetivo: `p99` medido contra `target`.
    Paused {
        p99: Duration,
        target: Duration,
    },
}

impl DataGate {
    pub fn is_open(&self) -> bool {
        matches!(self, DataGate::Open)
    }
}

/// Lo que pasa con los datos de una conexión, para los módulos y el log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosEvent {
    Paused {
        link: Option<LinkKind>,
        p99: Duration,
        target: Duration,
    },
    Resumed {
        link: Option<LinkKind>,
        /// Cuánto duró la pausa.
        after: Duration,
    },
    /// Cambió el tope (bytes por segundo; `None` = sin tope).
    Cap {
        link: Option<LinkKind>,
        cap: Option<u64>,
    },
}

/// Cada cuánto se recalcula el p99 como mucho, y cada cuánto revisa la pausa quien espera.
const EVALUATE_EVERY: Duration = Duration::from_millis(20);
const RECHECK: Duration = Duration::from_millis(100);

/// Fracción del tope configurado que se usa, en milésimas.
const FULL: u32 = 1000;
/// Lo menos que se baja (el tope real nunca queda bajo [`MIN_CAP`]).
const MIN_SCALE: u32 = 10;
/// Cada cuánto puede subir el tope como mucho.
const RAMP_EVERY: Duration = Duration::from_secs(1);

/// Prioridad, tope y pausa de los datos de una conexión. Se clona barato: todas las copias
/// comparten el mismo estado.
#[derive(Clone)]
pub struct Qos {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Qos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.lock();
        f.debug_struct("Qos")
            .field("link", &s.link)
            .field("cap", &s.limiter.as_ref().map(RateLimiter::cap))
            .field("paused", &s.guard.is_paused())
            .finish_non_exhaustive()
    }
}

struct Shared {
    inner: Mutex<Inner>,
    gate: watch::Sender<DataGate>,
    events: broadcast::Sender<QosEvent>,
}

struct Inner {
    link: Option<LinkKind>,
    prefs: DataPrefs,
    measured: Option<u64>,
    limiter: Option<RateLimiter>,
    guard: LatencyGuard,
    last_eval: Option<Instant>,
    paused_at: Option<Instant>,
    /// Qué parte del tope configurado se usa ahora (milésimas). Baja cuando la entrada se
    /// atrasa y sube de a poco cuando no: el tope sale de lo último medido, y si el Wi-Fi
    /// empeora después, el configurado ya no le cabe.
    scale: u32,
    last_decrease: Option<Instant>,
    last_change: Option<Instant>,
}

impl Default for Qos {
    fn default() -> Self {
        Self::new(GuardConfig::default())
    }
}

impl Qos {
    /// Sin camino conocido: sin tope y con el objetivo del Wi-Fi.
    pub fn new(guard: GuardConfig) -> Self {
        let (gate, _) = watch::channel(DataGate::Open);
        let (events, _) = broadcast::channel(32);
        Qos {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    link: None,
                    prefs: DataPrefs::default(),
                    measured: None,
                    limiter: None,
                    guard: LatencyGuard::new(guard, input_p99_target(None)),
                    last_eval: None,
                    paused_at: None,
                    scale: FULL,
                    last_decrease: None,
                    last_change: None,
                }),
                gate,
                events,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Por qué camino va la conexión. Cambiar de camino cambia el tope y el objetivo, y
    /// levanta una pausa: lo medido por el otro camino ya no vale.
    pub fn set_link(&self, link: Option<LinkKind>) {
        let mut s = self.lock();
        if s.link == link {
            return;
        }
        s.link = link;
        s.scale = FULL;
        let target = input_p99_target(link);
        if let Some(Transition::Resume { .. }) = s.guard.reset(target) {
            self.resumed(&mut s, Instant::now());
        }
        self.update_cap(&mut s);
    }

    pub fn link(&self) -> Option<LinkKind> {
        self.lock().link
    }

    /// Las preferencias del puesto (`[data]`).
    pub fn set_prefs(&self, prefs: DataPrefs) {
        let mut s = self.lock();
        if s.prefs != prefs {
            s.prefs = prefs;
            self.update_cap(&mut s);
        }
    }

    pub fn prefs(&self) -> DataPrefs {
        self.lock().prefs
    }

    /// El último ancho de banda medido del camino actual, en bytes por segundo.
    pub fn set_measured(&self, bytes_per_sec: u64) {
        let mut s = self.lock();
        if s.measured != Some(bytes_per_sec) {
            // Una medición nueva dice cuánto da el camino ahora: se vuelve a usar entero.
            s.measured = Some(bytes_per_sec);
            s.scale = FULL;
            self.update_cap(&mut s);
        }
    }

    /// El tope actual en bytes por segundo; `None` es sin tope.
    pub fn cap(&self) -> Option<u64> {
        self.lock().limiter.as_ref().map(RateLimiter::cap)
    }

    /// El tope configurado (sin la parte que se bajó por latencia).
    pub fn configured_cap(&self) -> Option<u64> {
        let s = self.lock();
        data_cap(s.link, &s.prefs, s.measured)
    }

    fn update_cap(&self, s: &mut Inner) {
        let cap = data_cap(s.link, &s.prefs, s.measured).map(|c| {
            let scaled = u128::from(c) * u128::from(s.scale) / u128::from(FULL);
            u64::try_from(scaled).unwrap_or(c)
        });
        let now = Instant::now();
        let before = s.limiter.as_ref().map(RateLimiter::cap);
        s.limiter = match (s.limiter.take(), cap) {
            (Some(mut l), Some(cap)) => {
                l.set_cap(cap, now);
                Some(l)
            }
            (None, Some(cap)) => Some(RateLimiter::new(cap, now)),
            (_, None) => None,
        };
        // El tope real puede ser el mínimo y no el pedido.
        let cap = s.limiter.as_ref().map(RateLimiter::cap);
        if cap == before {
            return;
        }
        if s.scale == FULL {
            tracing::info!(link = ?s.link, ?cap, "tope de los datos");
        } else {
            tracing::debug!(link = ?s.link, ?cap, scale = s.scale, "tope de los datos ajustado por latencia");
        }
        let _ = self.shared.events.send(QosEvent::Cap { link: s.link, cap });
    }

    /// Ajusta la parte del tope que se usa según el p99 de la entrada: baja enseguida si se
    /// acerca al objetivo (a lo sumo una vez por ventana, que es lo que tarda el p99 en ver
    /// el efecto) y sube de a poco si sobra margen. Así un Wi-Fi que empeora y se queda
    /// malo no hace que los datos se pausen y reanuden en ciclo.
    fn adapt(&self, s: &mut Inner, now: Instant, paused: bool) {
        if s.limiter.is_none() {
            return;
        }
        let target = s.guard.target();
        let window = s.guard.window();
        let since =
            |t: Option<Instant>| t.map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        let scale = match s.guard.last_p99() {
            _ if paused => return,
            Some(p) if p > target * 3 / 4 => {
                if since(s.last_decrease) < window {
                    return;
                }
                s.last_decrease = Some(now);
                // Pasado el objetivo, a la mitad; cerca, un poco menos.
                let factor = if p > target { 500 } else { 700 };
                (s.scale * factor / FULL).max(MIN_SCALE)
            }
            Some(p) if p > target / 2 => return,
            _ => {
                if s.scale >= FULL || since(s.last_change) < RAMP_EVERY {
                    return;
                }
                (s.scale + s.scale / 4 + 5).min(FULL)
            }
        };
        s.last_change = Some(now);
        s.scale = scale;
        self.update_cap(s);
    }

    /// Una latencia medida de la entrada (de quien captura a quien inyecta).
    pub fn record_input_latency(&self, latency: Duration) {
        let now = Instant::now();
        let mut s = self.lock();
        s.guard.record(now, latency);
        if s.last_eval
            .is_some_and(|t| now.duration_since(t) < EVALUATE_EVERY)
        {
            return;
        }
        self.evaluate(&mut s, now);
    }

    /// Revisa la pausa sin una muestra nueva (sin entrada también hay que reanudar).
    pub fn tick(&self) {
        let mut s = self.lock();
        self.evaluate(&mut s, Instant::now());
    }

    fn evaluate(&self, s: &mut Inner, now: Instant) {
        s.last_eval = Some(now);
        let transition = s.guard.evaluate(now);
        self.adapt(s, now, s.guard.is_paused());
        match transition {
            Some(Transition::Pause { p99 }) => {
                let target = s.guard.target();
                s.paused_at = Some(now);
                // Lo que había no alcanzó: al volver, la mitad.
                s.scale = (s.scale / 2).max(MIN_SCALE);
                s.last_decrease = Some(now);
                s.last_change = Some(now);
                self.update_cap(s);
                tracing::warn!(
                    link = ?s.link,
                    p99_ms = p99.as_secs_f64() * 1e3,
                    target_ms = target.as_secs_f64() * 1e3,
                    "datos en pausa: la entrada pasó su objetivo"
                );
                self.shared
                    .gate
                    .send_replace(DataGate::Paused { p99, target });
                let _ = self.shared.events.send(QosEvent::Paused {
                    link: s.link,
                    p99,
                    target,
                });
            }
            Some(Transition::Resume { .. }) => self.resumed(s, now),
            None => {}
        }
    }

    fn resumed(&self, s: &mut Inner, now: Instant) {
        let after = s
            .paused_at
            .take()
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        tracing::info!(link = ?s.link, after_ms = after.as_millis(), "datos reanudados");
        self.shared.gate.send_replace(DataGate::Open);
        let _ = self.shared.events.send(QosEvent::Resumed {
            link: s.link,
            after,
        });
    }

    /// Si los datos pueden salir ahora.
    pub fn gate(&self) -> DataGate {
        *self.shared.gate.borrow()
    }

    /// Para seguir la pausa.
    pub fn watch_gate(&self) -> watch::Receiver<DataGate> {
        self.shared.gate.subscribe()
    }

    /// Pausas, reanudaciones y cambios de tope, en orden. Quien se atrase pierde los más
    /// viejos (`RecvError::Lagged`); el estado actual siempre está en [`Self::gate`].
    pub fn events(&self) -> broadcast::Receiver<QosEvent> {
        self.shared.events.subscribe()
    }

    /// Espera a que puedan salir datos y devuelve cuántos de los `want` pueden salir ya
    /// (al menos 1 si `want > 0`).
    pub async fn acquire(&self, want: usize) -> usize {
        if want == 0 {
            return 0;
        }
        let mut gate = self.watch_gate();
        loop {
            if !gate.borrow_and_update().is_open() {
                // Sin entrada no llegan muestras: la pausa se revisa sola cada tanto.
                let _ = time::timeout(RECHECK, gate.changed()).await;
                self.tick();
                continue;
            }
            let taken = {
                let mut s = self.lock();
                match s.limiter.as_mut() {
                    Some(l) => l.take(Instant::now(), want),
                    None => Ok(want),
                }
            };
            match taken {
                Ok(n) => return n,
                // Corto: una pausa o un tope nuevo se notan enseguida.
                Err(wait) => time::sleep(wait.min(RECHECK)).await,
            }
        }
    }

    /// Envuelve `writer` para que todo lo que se escriba respete la pausa y el tope.
    pub fn data_stream<W>(&self, writer: W) -> DataStream<W> {
        DataStream::new(self.clone(), writer)
    }
}

/// Un [`Qos`] por camino, compartido por todas las conexiones con el otro agente por ese
/// camino. Se clona barato.
#[derive(Clone, Default)]
pub struct LinkQos {
    inner: Arc<Mutex<LinkQosInner>>,
}

#[derive(Default)]
struct LinkQosInner {
    prefs: DataPrefs,
    by_link: BTreeMap<LinkKind, Qos>,
}

impl std::fmt::Debug for LinkQos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkQos").finish_non_exhaustive()
    }
}

impl LinkQos {
    fn lock(&self) -> MutexGuard<'_, LinkQosInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// El de `kind`; el primero que lo pide lo crea con las preferencias actuales.
    pub fn get(&self, kind: LinkKind) -> Qos {
        let mut s = self.lock();
        let prefs = s.prefs;
        s.by_link
            .entry(kind)
            .or_insert_with(|| {
                let qos = Qos::default();
                qos.set_link(Some(kind));
                qos.set_prefs(prefs);
                qos
            })
            .clone()
    }

    /// El `[data]` del puesto, para todos los caminos (los que ya existen y los que vengan).
    pub fn set_prefs(&self, prefs: DataPrefs) {
        let mut s = self.lock();
        s.prefs = prefs;
        for qos in s.by_link.values() {
            qos.set_prefs(prefs);
        }
    }

    pub fn prefs(&self) -> DataPrefs {
        self.lock().prefs
    }
}

impl Qos {
    /// Si `self` y `other` son el mismo (comparten estado).
    pub fn same(&self, other: &Qos) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

#[cfg(test)]
mod tests;
