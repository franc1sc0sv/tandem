//! Gestor de enlaces múltiples y política de planos (DESIGN §6).
//!
//! Cada enlace configurado (USB4, Ethernet, Wi-Fi) tiene su propia conexión
//! con el otro agente y un ping de salud cada segundo. El enlace activo es
//! el sano de mayor prioridad: si cae, se pasa al siguiente en cuanto falla un
//! solo ping (sin esperar al timeout de inactividad de QUIC). Para volver a un
//! enlace mejor que ya se cayó, este tiene que llevar
//! [`HealthConfig::settle`] respondiendo sin cortes (histéresis: un cable que
//! parpadea no arrastra el tráfico de un lado a otro), y no se sube mientras haya
//! una transacción en curso ([`LinkManager::hold`]). Cada cambio sale como
//! [`LinkEvent`], en orden.
//!
//! El plano de datos solo existe sobre cable: [`LinkManager::data_plane`]
//! devuelve [`PlaneError::RequiresCable`] si el activo es Wi-Fi, y no hay
//! opción para forzarlo.

mod kind;
mod probe;
mod quic;
mod table;

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{self, Instant, MissedTickBehavior};

pub use kind::{LinkCandidate, LinkKind, Plane, PlaneError, check_plane};
pub use probe::Prober;
pub use quic::{QuicProber, QuicSession, answer_pings};
pub use table::{LinkEvent, LinkStatus};

use table::LinkTable;

/// Tiempos de la revisión de salud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthConfig {
    /// Cada cuánto se hace ping por enlace (DESIGN §6: 1 s).
    pub ping_interval: Duration,
    /// Cuánto se espera el Pong. `ping_interval + ping_timeout` es lo máximo
    /// que tarda en detectarse una caída, y tiene que quedar bajo 1,5 s.
    pub ping_timeout: Duration,
    /// Tope para abrir la conexión de un enlace caído.
    pub connect_timeout: Duration,
    /// Cuánto tiene que responder sin cortes un enlace que ya se cayó antes de
    /// volver a él. Con 0 se vuelve en el primer ping bueno.
    pub settle: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(1),
            ping_timeout: Duration::from_millis(250),
            connect_timeout: Duration::from_secs(1),
            settle: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManagerError {
    #[error("no hay ningún enlace configurado")]
    NoCandidates,
    #[error("el enlace {0} está configurado dos veces")]
    DuplicateLink(LinkKind),
    #[error("los tiempos de la revisión de salud no pueden ser 0")]
    ZeroDuration,
}

/// Enlace activo y lo necesario para mandar tráfico por él.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLink<H> {
    pub kind: LinkKind,
    pub addr: SocketAddr,
    pub handle: H,
}

/// Foto del gestor; se actualiza con cada ping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<H> {
    pub active: Option<ActiveLink<H>>,
    /// Todos los enlaces configurados, en orden de prioridad.
    pub links: Vec<LinkStatus>,
}

impl<H> Snapshot<H> {
    pub fn data_plane_available(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.kind.allows(Plane::Data))
    }
}

/// Gestor en marcha. Al soltarlo se detienen los pings y se cierran las
/// conexiones de todos los enlaces.
pub struct LinkManager<H> {
    state: watch::Receiver<Snapshot<H>>,
    holds: Arc<watch::Sender<usize>>,
    task: JoinHandle<()>,
}

/// Mientras exista, el gestor no cambia de enlace salvo que el activo caiga.
/// Al soltar el último, si hay un enlace mejor ya asentado, se pasa a él enseguida.
#[must_use = "la retención termina al soltarla"]
pub struct LinkHold {
    holds: Arc<watch::Sender<usize>>,
}

impl fmt::Debug for LinkHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkHold").finish_non_exhaustive()
    }
}

impl Drop for LinkHold {
    fn drop(&mut self) {
        self.holds.send_modify(|n| *n = n.saturating_sub(1));
    }
}

type Report<H> = (LinkKind, Option<(H, Duration)>);

impl<H: Clone + Send + Sync + 'static> LinkManager<H> {
    /// Arranca un ping por enlace. Devuelve también la cola de
    /// [`LinkEvent`], que conserva el orden y no pierde eventos aunque el
    /// consumidor se atrase. Tiene que llamarse dentro de un runtime de tokio.
    pub fn spawn<P: Prober<Handle = H>>(
        candidates: &[LinkCandidate],
        prober: P,
        config: HealthConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<LinkEvent>), ManagerError> {
        if [
            config.ping_interval,
            config.ping_timeout,
            config.connect_timeout,
        ]
        .contains(&Duration::ZERO)
        {
            return Err(ManagerError::ZeroDuration);
        }
        let mut table = LinkTable::new(candidates, config.settle)?;
        let (state_tx, state) = watch::channel(Snapshot {
            active: None,
            links: table.statuses(),
        });
        let (event_tx, events) = mpsc::unbounded_channel();
        let (report_tx, mut reports) = mpsc::unbounded_channel::<Report<H>>();
        let holds = Arc::new(watch::Sender::new(0usize));
        let mut held = holds.subscribe();

        let prober = Arc::new(prober);
        let mut links = JoinSet::new();
        for c in table.candidates() {
            links.spawn(run_link(prober.clone(), c, config, report_tx.clone()));
        }
        drop(report_tx);

        // Una sola tarea aplica los resultados: así los eventos salen en el
        // orden en que cambió la tabla. Al abortarla, `links` se suelta y
        // aborta los pings.
        let task = tokio::spawn(async move {
            let _links = links;
            loop {
                let event = tokio::select! {
                    report = reports.recv() => {
                        let Some((kind, up)) = report else { return };
                        let held = *held.borrow_and_update() > 0;
                        table.report(kind, up, Instant::now(), held)
                    }
                    Ok(()) = held.changed() => {
                        let held = *held.borrow_and_update() > 0;
                        table.reevaluate(Instant::now(), held)
                    }
                };
                // Primero la foto: quien reciba el evento ya ve el estado nuevo.
                state_tx.send_replace(snapshot(&table));
                if let Some(e) = event {
                    tracing::info!(
                        from = ?e.from,
                        to = ?e.to,
                        data_plane = e.data_plane,
                        "cambio de enlace activo"
                    );
                    // Si nadie escucha los eventos, el gestor sigue igual.
                    let _ = event_tx.send(e);
                }
            }
        });
        Ok((Self { state, holds, task }, events))
    }
}

impl<H> LinkManager<H> {
    /// Retiene el enlace activo durante una transacción: los pasos de un mismo
    /// cambio no saltan de enlace a medias salvo que el activo caiga.
    pub fn hold(&self) -> LinkHold {
        self.holds.send_modify(|n| *n += 1);
        LinkHold {
            holds: self.holds.clone(),
        }
    }
}

impl<H: Clone> LinkManager<H> {
    pub fn snapshot(&self) -> Snapshot<H> {
        self.state.borrow().clone()
    }

    /// Para seguir la foto (la UI dibuja los enlaces con esto).
    pub fn watch(&self) -> watch::Receiver<Snapshot<H>> {
        self.state.clone()
    }

    pub fn active(&self) -> Option<ActiveLink<H>> {
        self.state.borrow().active.clone()
    }

    pub fn links(&self) -> Vec<LinkStatus> {
        self.state.borrow().links.clone()
    }

    /// Si ahora hay un enlace de cable sano para el plano de datos.
    pub fn data_plane_available(&self) -> bool {
        self.state.borrow().data_plane_available()
    }

    /// Enlace para `plane`, o por qué no lo hay.
    pub fn plane(&self, plane: Plane) -> Result<ActiveLink<H>, PlaneError> {
        let state = self.state.borrow();
        check_plane(plane, state.active.as_ref().map(|a| a.kind))?;
        state.active.clone().ok_or(PlaneError::NoLink)
    }

    pub fn control_plane(&self) -> Result<ActiveLink<H>, PlaneError> {
        self.plane(Plane::Control)
    }

    /// El enlace para el plano de datos. Con solo Wi-Fi activo devuelve
    /// [`PlaneError::RequiresCable`].
    pub fn data_plane(&self) -> Result<ActiveLink<H>, PlaneError> {
        self.plane(Plane::Data)
    }
}

impl<H> Drop for LinkManager<H> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn snapshot<H: Clone>(table: &LinkTable<H>) -> Snapshot<H> {
    let links = table.statuses();
    let active = table.active().and_then(|(kind, handle)| {
        let addr = links.iter().find(|l| l.kind == kind)?.addr;
        Some(ActiveLink {
            kind,
            addr,
            handle: handle.clone(),
        })
    });
    Snapshot { active, links }
}

enum Failure<E> {
    Connect(E),
    ConnectTimeout,
    Ping(E),
    PingTimeout,
}

impl<E: fmt::Display> fmt::Display for Failure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Connect(e) => write!(f, "no conecta: {e}"),
            Failure::ConnectTimeout => f.write_str("la conexión no respondió a tiempo"),
            Failure::Ping(e) => write!(f, "ping fallido: {e}"),
            Failure::PingTimeout => f.write_str("el ping no respondió a tiempo"),
        }
    }
}

async fn run_link<P: Prober>(
    prober: Arc<P>,
    link: LinkCandidate,
    config: HealthConfig,
    reports: mpsc::UnboundedSender<Report<P::Handle>>,
) {
    let mut tick = time::interval(config.ping_interval);
    // Tras una conexión lenta no se encadenan pings atrasados.
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut session = None;
    let mut was_up = false;
    loop {
        tick.tick().await;
        let up = match probe_once(&*prober, link.addr, &mut session, &config).await {
            Ok(up) => {
                if !was_up {
                    tracing::info!(link = %link.kind, addr = %link.addr, "enlace sano");
                }
                Some(up)
            }
            Err(failure) => {
                if was_up {
                    tracing::warn!(link = %link.kind, addr = %link.addr, "enlace caído: {failure}");
                } else {
                    tracing::debug!(link = %link.kind, addr = %link.addr, "enlace caído: {failure}");
                }
                None
            }
        };
        was_up = up.is_some();
        if reports.send((link.kind, up)).is_err() {
            return;
        }
    }
}

async fn probe_once<P: Prober>(
    prober: &P,
    addr: SocketAddr,
    session: &mut Option<P::Session>,
    config: &HealthConfig,
) -> Result<(P::Handle, Duration), Failure<P::Error>> {
    let s = match session {
        Some(s) => s,
        None => match time::timeout(config.connect_timeout, prober.connect(addr)).await {
            Ok(Ok(s)) => session.insert(s),
            Ok(Err(e)) => return Err(Failure::Connect(e)),
            Err(_) => return Err(Failure::ConnectTimeout),
        },
    };
    let start = Instant::now();
    let result = match time::timeout(config.ping_timeout, prober.ping(s)).await {
        Ok(Ok(())) => return Ok((P::handle(s), start.elapsed())),
        Ok(Err(e)) => Failure::Ping(e),
        Err(_) => Failure::PingTimeout,
    };
    // Un ping cortado a medias puede dejar el stream desalineado: se
    // descarta la sesión y el próximo tick conecta de nuevo.
    *session = None;
    Err(result)
}

#[cfg(test)]
mod tests;
