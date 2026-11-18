//! La máquina origen: abre y cierra túneles en la otra y sigue su salida.
//!
//! Por cada túnel abierto corre un seguidor que mantiene el canal de datos
//! ([`TunnelUp::Attach`]) por el enlace activo. Si el stream se corta, o el enlace activo
//! pasa a otro camino, vuelve a pedir la salida «desde el byte N» de cada flujo, con N lo
//! que ya tiene en su espejo: no se pierde ni se repite nada (lo que llega repetido se
//! descarta en [`OutputBuffer::accept`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tandem_core::Machine;
use tandem_proto::{
    Client, ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput, Project, Tunnel,
    TunnelDown, TunnelId, TunnelProcess, TunnelState, TunnelUp, TunnelView,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::Changes;
use crate::buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
use crate::process::output::{self, FlowRead};
use crate::wire::{Dialer, WireError};

/// El enlace por el que se habla con la otra máquina; `None` si ninguno la ve.
pub type Link = Option<Arc<dyn Dialer>>;

#[derive(Debug, Clone, Copy)]
pub struct OriginOptions {
    /// Lo que guarda el espejo de cada flujo.
    pub capacity: usize,
    /// Tope para abrir, cerrar o listar.
    pub request_timeout: Duration,
    /// Espera antes de reintentar el canal de datos por el mismo enlace.
    pub retry: Duration,
    /// Cuánto se recuerda un túnel cerrado (y su salida, para leerla). También es lo que se
    /// insiste en cerrar uno cuyo `Open` quedó sin respuesta.
    pub linger: Duration,
}

impl Default for OriginOptions {
    fn default() -> Self {
        OriginOptions {
            capacity: DEFAULT_CAPACITY,
            request_timeout: Duration::from_secs(10),
            retry: Duration::from_millis(500),
            linger: Duration::from_secs(10 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TunnelError {
    #[error("la otra máquina no está conectada")]
    NoLink,
    #[error("la otra máquina no contestó a tiempo")]
    Timeout,
    #[error("se cortó el enlace con la otra máquina: {0}")]
    Wire(#[from] WireError),
    #[error("la otra máquina no lo aceptó: {0}")]
    Refused(String),
    #[error("respuesta inesperada de la otra máquina")]
    Unexpected,
    #[error("el destino de un túnel tiene que ser la otra máquina ({0})")]
    SameMachine(Machine),
    #[error("no hay ningún túnel {0} abierto desde esta máquina")]
    Unknown(TunnelId),
    #[error("el túnel {0} no tiene el proceso {1}")]
    UnknownProcess(TunnelId, ProcessId),
    #[error("no se pudo elegir un id al azar: {0}")]
    Entropy(String),
}

/// Lo que pide el cliente al abrir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpec {
    pub title: String,
    pub project: Option<Project>,
    /// Esta máquina.
    pub origin: Machine,
    pub target: Machine,
    pub opened_by: Client,
}

/// Lo que se sabe de un flujo en el origen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowStatus {
    pub info: FlowInfo,
    /// Bytes recibidos (la posición del próximo).
    pub received: u64,
    /// Bytes que se perdieron porque el búfer de la destino se llenó sin leerse.
    pub lost: u64,
    /// El total, si el flujo ya terminó y llegó todo.
    pub ended: Option<u64>,
}

/// Los túneles abiertos desde esta máquina. Se clona barato; al soltar el último, sus
/// seguidores se detienen (los túneles siguen vivos en la destino).
#[derive(Clone)]
pub struct Origin {
    inner: Arc<OriginInner>,
}

struct OriginInner {
    opts: OriginOptions,
    link: watch::Sender<Link>,
    tunnels: Mutex<BTreeMap<TunnelId, Entry>>,
    /// Cambios de los túneles (abrir, cerrar), para la UI.
    changes: Changes,
    /// Sube con cada byte recibido, para quien lee la salida en vivo.
    output: watch::Sender<u64>,
}

struct Entry {
    info: Tunnel,
    flows: BTreeMap<FlowId, Mirror>,
    /// Los procesos que avisó la destino, también los terminados hace menos de `linger`.
    processes: BTreeMap<ProcessId, Proc>,
    follower: Option<JoinHandle<()>>,
    /// Desde cuándo está cerrado; pasado `linger` se olvida.
    closed_at: Option<Instant>,
}

struct Proc {
    info: TunnelProcess,
    /// Desde cuándo se sabe que terminó; pasado `linger` se olvida con sus flujos.
    ended_at: Option<Instant>,
}

struct Mirror {
    info: FlowInfo,
    buf: OutputBuffer,
    /// Lo que la destino avisó como perdido (no incluye lo que el espejo soltó por lugar).
    lost: u64,
    ended: Option<u64>,
}

impl Drop for OriginInner {
    fn drop(&mut self) {
        for e in lock(&self.tunnels).values() {
            if let Some(f) = &e.follower {
                f.abort();
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Origin {
    pub fn new(opts: OriginOptions, changes: Changes) -> Self {
        Origin {
            inner: Arc::new(OriginInner {
                opts,
                link: watch::Sender::new(None),
                tunnels: Mutex::new(BTreeMap::new()),
                changes,
                output: watch::Sender::new(0),
            }),
        }
    }

    /// El enlace activo cambió. El mismo `Arc` no cuenta como cambio.
    pub fn set_link(&self, link: Link) {
        self.inner.link.send_if_modified(|old| {
            let same = match (&*old, &link) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                *old = link;
            }
            !same
        });
    }

    /// Abre un túnel en `spec.target` y empieza a seguir su salida.
    pub async fn open(&self, spec: OpenSpec) -> Result<Tunnel, TunnelError> {
        if spec.target != spec.origin.other() {
            return Err(TunnelError::SameMachine(spec.target));
        }
        let id = getrandom::u64().map_err(|e| TunnelError::Entropy(e.to_string()))?;
        let tunnel = Tunnel {
            id: TunnelId(id),
            title: spec.title,
            project: spec.project,
            origin: spec.origin,
            target: spec.target,
            opened_by: spec.opened_by,
            state: TunnelState::Open,
            created: now_millis(),
        };
        let id = tunnel.id;
        let opened = match self.inner.request(TunnelUp::Open(tunnel)).await {
            Ok(TunnelDown::Opened(t)) => t,
            Ok(_) => return Err(TunnelError::Unexpected),
            // El pedido pudo llegar y la respuesta perderse: la destino tendría un túnel que
            // nadie sigue ni cierra. Se cierra en segundo plano cuando haya enlace (cerrar
            // uno que no existe no hace nada).
            Err(e @ (TunnelError::Timeout | TunnelError::Wire(_))) => {
                tokio::spawn(discard(Arc::downgrade(&self.inner), id));
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        // Primero el registro: el seguidor empieza leyendo lo que ya se tiene.
        self.inner.tunnels().insert(
            opened.id,
            Entry {
                info: opened.clone(),
                flows: BTreeMap::new(),
                processes: BTreeMap::new(),
                follower: None,
                closed_at: None,
            },
        );
        let follower = tokio::spawn(follow(Arc::downgrade(&self.inner), opened.id));
        if let Some(e) = lock(&self.inner.tunnels).get_mut(&opened.id) {
            e.follower = Some(follower);
        }
        self.inner.changes.bump();
        Ok(opened)
    }

    /// Cierra el túnel en la destino (con sus recursos). La salida que faltaba sigue
    /// llegando hasta el cierre.
    pub async fn close(&self, id: TunnelId) -> Result<(), TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self.inner.request(TunnelUp::Close(id)).await? {
            TunnelDown::Closed(_) => {
                // El aviso de cierre del seguidor llega por otro flujo y puede ir después de
                // esta respuesta: se marca aquí para que `tunnels()` ya lo vea cerrado.
                self.inner.mark_closed(id);
                Ok(())
            }
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Abre un stream de túnel por el enlace activo, para operaciones que llevan su propio
    /// diálogo (archivos y puertos, T-106).
    pub async fn dial(&self) -> Result<Box<dyn crate::OriginWire>, TunnelError> {
        let dialer = self
            .inner
            .link
            .borrow()
            .clone()
            .ok_or(TunnelError::NoLink)?;
        let wire = tokio::time::timeout(self.inner.opts.request_timeout, dialer.dial())
            .await
            .map_err(|_| TunnelError::Timeout)??;
        Ok(wire)
    }

    /// `true` si el túnel está abierto y lo abrió esta máquina.
    pub fn is_open(&self, id: TunnelId) -> bool {
        self.inner
            .tunnels()
            .get(&id)
            .is_some_and(|e| e.info.state == TunnelState::Open)
    }

    /// Los túneles de la destino, según ella.
    pub async fn remote_list(&self) -> Result<Vec<Tunnel>, TunnelError> {
        match self.inner.request(TunnelUp::List).await? {
            TunnelDown::List(list) => Ok(list),
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Los abiertos desde aquí (también los ya cerrados).
    pub fn tunnels(&self) -> Vec<Tunnel> {
        self.inner
            .tunnels()
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Lanza un proceso en el túnel `id` de la otra máquina (T-104). Los procesos son de
    /// ese túnel: cerrarlo los mata.
    pub async fn exec(&self, id: TunnelId, spec: ExecSpec) -> Result<TunnelProcess, TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self
            .inner
            .request(TunnelUp::Exec { tunnel: id, spec })
            .await?
        {
            TunnelDown::Started(p) => {
                self.inner.note_process(id, &p);
                Ok(p)
            }
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Mata un proceso del túnel con todo su árbol. Su estado nuevo llega por el canal de
    /// datos.
    pub async fn kill(&self, id: TunnelId, process: ProcessId) -> Result<(), TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self
            .inner
            .request(TunnelUp::Kill {
                tunnel: id,
                process,
            })
            .await?
        {
            TunnelDown::Killed(_) => Ok(()),
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Los procesos del túnel que se conocen aquí (corriendo y terminados hace poco), por
    /// id. `None` si el túnel no se abrió desde esta máquina.
    pub fn processes(&self, id: TunnelId) -> Option<Vec<TunnelProcess>> {
        self.inner
            .tunnels()
            .get(&id)
            .map(|e| e.processes.values().map(|p| p.info.clone()).collect())
    }

    /// La salida de un proceso desde `from`, con hasta `max` bytes de cada flujo (lo que
    /// T-104 expone como `output`). Sin esperar: lo que ya llegó.
    pub fn output(
        &self,
        id: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, TunnelError> {
        let tunnels = self.inner.tunnels();
        let e = tunnels.get(&id).ok_or(TunnelError::Unknown(id))?;
        let p = e
            .processes
            .get(&process)
            .ok_or(TunnelError::UnknownProcess(id, process))?;
        Ok(output::collect(
            p.info.clone(),
            from,
            max,
            |flow, at, max| {
                let m = e.flows.get(&flow)?;
                Some(FlowRead {
                    read: m.buf.read(at, max),
                    end: m.buf.end(),
                    ended: m.ended.is_some(),
                })
            },
        ))
    }

    /// Los túneles abiertos desde aquí, con sus procesos corriendo, para la UI.
    pub fn views(&self) -> Vec<TunnelView> {
        self.inner
            .tunnels()
            .values()
            .map(|e| {
                let mut running: Vec<TunnelProcess> = e
                    .processes
                    .values()
                    .filter(|p| p.info.is_running())
                    .map(|p| p.info.clone())
                    .collect();
                running.sort_by_key(|p| (p.started, p.id));
                TunnelView {
                    tunnel: e.info.clone(),
                    running,
                }
            })
            .collect()
    }

    pub fn flows(&self, id: TunnelId) -> Vec<FlowStatus> {
        self.inner
            .tunnels()
            .get(&id)
            .map(|e| {
                e.flows
                    .values()
                    .map(|m| FlowStatus {
                        info: m.info.clone(),
                        received: m.buf.end(),
                        lost: m.lost,
                        ended: m.ended,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// La salida recibida de un flujo desde `from` (lo que T-104 expone como `output`).
    pub fn read(&self, id: TunnelId, flow: FlowId, from: u64, max: usize) -> Option<Read> {
        self.inner
            .tunnels()
            .get(&id)?
            .flows
            .get(&flow)
            .map(|m| m.buf.read(from, max))
    }

    /// Sube con cada byte recibido de cualquier túnel.
    pub fn subscribe_output(&self) -> watch::Receiver<u64> {
        self.inner.output.subscribe()
    }
}

impl OriginInner {
    /// Los túneles, ya sin los cerrados hace más de `linger`.
    /// Marca el túnel como cerrado localmente (idempotente).
    fn mark_closed(&self, id: TunnelId) {
        let mut tunnels = lock(&self.tunnels);
        let Some(e) = tunnels.get_mut(&id) else {
            return;
        };
        if e.info.state == TunnelState::Closed {
            return;
        }
        e.info.state = TunnelState::Closed;
        e.closed_at = Some(Instant::now());
        drop(tunnels);
        self.changes.bump();
        self.output.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn tunnels(&self) -> MutexGuard<'_, BTreeMap<TunnelId, Entry>> {
        let mut tunnels = lock(&self.tunnels);
        let linger = self.opts.linger;
        tunnels.retain(|_, e| e.closed_at.is_none_or(|at| at.elapsed() < linger));
        for e in tunnels.values_mut() {
            forget_old_processes(e, linger);
        }
        tunnels
    }

    /// Anota un proceso que avisó la destino (la respuesta de `exec` o el canal de datos).
    fn note_process(&self, id: TunnelId, p: &TunnelProcess) {
        let changed = lock(&self.tunnels)
            .get_mut(&id)
            .is_some_and(|e| upsert_process(e, p));
        if changed {
            self.changes.bump();
        }
    }

    async fn request(&self, up: TunnelUp) -> Result<TunnelDown, TunnelError> {
        let dialer = self.link.borrow().clone().ok_or(TunnelError::NoLink)?;
        let ask = async {
            let mut wire = dialer.dial().await?;
            wire.send(up).await?;
            wire.recv()
                .await?
                .ok_or_else(|| WireError::new("la otra máquina cerró el stream sin contestar"))
        };
        match tokio::time::timeout(self.opts.request_timeout, ask).await {
            Err(_) => Err(TunnelError::Timeout),
            Ok(Err(e)) => Err(TunnelError::Wire(e)),
            Ok(Ok(TunnelDown::Refused(why))) => Err(TunnelError::Refused(why)),
            Ok(Ok(reply)) => Ok(reply),
        }
    }

    /// Lo que ya se tiene de cada flujo; `None` si el túnel ya no se sigue.
    fn cursors(&self, id: TunnelId) -> Option<Vec<(FlowId, u64)>> {
        let tunnels = lock(&self.tunnels);
        let e = tunnels.get(&id)?;
        (e.info.state == TunnelState::Open)
            .then(|| e.flows.iter().map(|(f, m)| (*f, m.buf.end())).collect())
    }

    /// Aplica un mensaje del canal de datos. `true` si el túnel terminó.
    fn apply(&self, id: TunnelId, msg: TunnelDown) -> bool {
        let mut tunnels = lock(&self.tunnels);
        let Some(e) = tunnels.get_mut(&id) else {
            return true;
        };
        let capacity = self.opts.capacity;
        match msg {
            TunnelDown::Flow(info) => {
                let id = info.id;
                mirror(e, id, capacity).info = info;
            }
            TunnelDown::Data {
                flow,
                offset,
                bytes,
            } => {
                let m = mirror(e, flow, capacity);
                if let Err(gap) = m.buf.accept(offset, &bytes) {
                    // No debería pasar: se pide siempre desde lo que se tiene.
                    tracing::warn!(tunnel = %id, "{gap}");
                    m.lost += gap.offset - gap.at;
                    m.buf.skip_to(gap.offset);
                    let _ = m.buf.accept(offset, &bytes);
                }
                drop(tunnels);
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return false;
            }
            TunnelDown::Lost { flow, from, to } => {
                let m = mirror(e, flow, capacity);
                let missing = to.saturating_sub(m.buf.end().max(from));
                if missing > 0 {
                    tracing::warn!(
                        tunnel = %id,
                        flow = %m.info.label,
                        "se perdieron {missing} bytes de salida: el búfer de la otra máquina se llenó"
                    );
                    m.lost += missing;
                    m.buf.skip_to(to);
                }
            }
            TunnelDown::End { flow, total } => mirror(e, flow, capacity).ended = Some(total),
            TunnelDown::Process(p) => {
                let changed = upsert_process(e, &p);
                drop(tunnels);
                if changed {
                    self.changes.bump();
                }
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return false;
            }
            TunnelDown::TunnelClosed | TunnelDown::Refused(_) => {
                if let TunnelDown::Refused(why) = &msg {
                    tracing::warn!(tunnel = %id, "la otra máquina ya no tiene el túnel: {why}");
                }
                e.info.state = TunnelState::Closed;
                e.follower = None;
                e.closed_at = Some(Instant::now());
                drop(tunnels);
                self.changes.bump();
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return true;
            }
            TunnelDown::Opened(_)
            | TunnelDown::Closed(_)
            | TunnelDown::List(_)
            | TunnelDown::Files(_)
            | TunnelDown::Port(_)
            | TunnelDown::Workspace(_)
            | TunnelDown::Started(_)
            | TunnelDown::Killed(_) => {}
        }
        drop(tunnels);
        self.output.send_modify(|n| *n = n.wrapping_add(1));
        false
    }
}

/// Guarda lo que avisó la destino de un proceso. Un aviso viejo (corriendo) no pisa uno
/// nuevo (terminado): la respuesta de `exec` y el canal de datos pueden llegar en cualquier
/// orden. `true` si algo cambió.
fn upsert_process(e: &mut Entry, p: &TunnelProcess) -> bool {
    if let Some(old) = e.processes.get_mut(&p.id) {
        if old.info == *p || (!old.info.is_running() && p.is_running()) {
            return false;
        }
        old.info = p.clone();
        if !p.is_running() && old.ended_at.is_none() {
            old.ended_at = Some(Instant::now());
        }
        return true;
    }
    e.processes.insert(
        p.id,
        Proc {
            info: p.clone(),
            ended_at: (!p.is_running()).then(Instant::now),
        },
    );
    true
}

/// Olvida los procesos terminados hace más de `linger`, con sus flujos.
fn forget_old_processes(e: &mut Entry, linger: Duration) {
    let gone: Vec<TunnelProcess> = e
        .processes
        .values()
        .filter(|p| p.ended_at.is_some_and(|at| at.elapsed() >= linger))
        .map(|p| p.info.clone())
        .collect();
    for p in gone {
        e.processes.remove(&p.id);
        e.flows.remove(&p.stdout);
        e.flows.remove(&p.stderr);
    }
}

fn mirror(e: &mut Entry, flow: FlowId, capacity: usize) -> &mut Mirror {
    e.flows.entry(flow).or_insert_with(|| Mirror {
        info: FlowInfo {
            id: flow,
            label: String::new(),
        },
        buf: OutputBuffer::new(capacity),
        lost: 0,
        ended: None,
    })
}

enum Attach {
    /// El túnel terminó (o ya no hay a quién avisarle).
    Done,
    /// El enlace activo cambió: se vuelve a pedir por el nuevo, sin esperar.
    LinkChanged,
    Failed(WireError),
}

/// Mantiene el canal de datos de `id` por el enlace activo hasta que el túnel se cierre.
async fn follow(origin: Weak<OriginInner>, id: TunnelId) {
    let (mut links, retry) = match origin.upgrade() {
        Some(o) => (o.link.subscribe(), o.opts.retry),
        None => return,
    };
    loop {
        let link = links.borrow_and_update().clone();
        let Some(dialer) = link else {
            if links.changed().await.is_err() {
                return;
            }
            continue;
        };
        match attach(&origin, id, dialer.as_ref(), &mut links).await {
            Attach::Done => return,
            Attach::LinkChanged => continue,
            Attach::Failed(e) => {
                tracing::debug!(tunnel = %id, "canal de datos del túnel cortado: {e}");
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    r = links.changed() => if r.is_err() { return },
                }
            }
        }
    }
}

async fn attach(
    origin: &Weak<OriginInner>,
    id: TunnelId,
    dialer: &dyn Dialer,
    links: &mut watch::Receiver<Link>,
) -> Attach {
    let Some(from) = origin.upgrade().and_then(|o| o.cursors(id)) else {
        return Attach::Done;
    };
    let session = async {
        let mut wire = dialer.dial().await?;
        wire.send(TunnelUp::Attach { tunnel: id, from }).await?;
        loop {
            let Some(msg) = wire.recv().await? else {
                return Err(WireError::new("la otra máquina cerró el canal de datos"));
            };
            let Some(o) = origin.upgrade() else {
                return Ok(());
            };
            if o.apply(id, msg) {
                return Ok(());
            }
        }
    };
    tokio::select! {
        r = session => match r {
            Ok(()) => Attach::Done,
            Err(e) => Attach::Failed(e),
        },
        r = links.changed() => match r {
            Ok(()) => Attach::LinkChanged,
            Err(_) => Attach::Done,
        },
    }
}

/// Cierra en la destino un túnel cuyo `Open` quedó sin respuesta: lo intenta por cada
/// enlace que aparezca hasta que la destino conteste, o hasta `linger`.
async fn discard(origin: Weak<OriginInner>, id: TunnelId) {
    let Some((mut links, retry, linger)) = origin
        .upgrade()
        .map(|o| (o.link.subscribe(), o.opts.retry, o.opts.linger))
    else {
        return;
    };
    let give_up = Instant::now() + linger;
    while Instant::now() < give_up {
        links.borrow_and_update();
        let Some(o) = origin.upgrade() else {
            return;
        };
        match o.request(TunnelUp::Close(id)).await {
            // Cerrado, o nunca llegó a abrirse.
            Ok(_) | Err(TunnelError::Refused(_)) => {
                tracing::debug!(tunnel = %id, "túnel sin respuesta al abrir: cerrado en la otra máquina");
                return;
            }
            Err(e) => {
                tracing::debug!(tunnel = %id, "no se pudo cerrar el túnel sin respuesta: {e}")
            }
        }
        drop(o);
        tokio::select! {
            () = tokio::time::sleep(retry) => {}
            r = links.changed() => if r.is_err() { return },
        }
    }
    tracing::warn!(tunnel = %id, "no se pudo cerrar en la otra máquina un túnel que quedó sin respuesta al abrir");
}
