//! La máquina destino: los túneles viven aquí, con sus flujos y recursos, aunque el enlace
//! con el origen se caiga.
//!
//! Lo que corre dentro de un túnel (procesos en T-104, puertos en T-106, workspaces en
//! T-107) se registra en su [`HostTunnel`]: la salida como un flujo ([`FlowWriter`]) y lo
//! que hay que soltar al cerrar como un [`Resource`]. Los procesos ([`HostTunnel::exec`])
//! llevan además su propia tabla, que cierra el túnel matando cada árbol.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_proto::{
    ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput, Tunnel, TunnelDown,
    TunnelId, TunnelProcess, TunnelState, TunnelUp, TunnelView,
};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::Changes;
use crate::buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
use crate::process::output::{self, FlowRead};
use crate::process::system::SystemSpawner;
use crate::process::table::Table;
use crate::process::{ExecError, Launch, Os, Spawner, launch};
use crate::wire::{TargetWire, WireError};

/// Lo más que va en un mensaje de datos: muy por debajo del máximo de una trama, y
/// suficientemente chico para alternar entre flujos.
pub const CHUNK: usize = 64 * 1024;

/// Algo del túnel que hay que soltar al cerrarlo (matar un árbol de procesos, cerrar un
/// puerto). `close` tiene que volver enseguida: lo lento, en su propia tarea.
pub trait Resource: Send + Sync {
    fn close(&self);
}

#[derive(Debug, Clone, Copy)]
pub struct HostOptions {
    /// Lo que guarda cada flujo mientras nadie lo lee.
    pub capacity: usize,
    /// Cuánto se recuerda un túnel cerrado, para que el origen que se reconecta reciba
    /// lo que faltaba y el cierre.
    pub linger: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        HostOptions {
            capacity: DEFAULT_CAPACITY,
            linger: Duration::from_secs(10 * 60),
        }
    }
}

/// Los túneles de los que esta máquina es destino. Se clona barato.
#[derive(Clone)]
pub struct Host {
    inner: Arc<HostInner>,
}

struct HostInner {
    opts: HostOptions,
    tunnels: Mutex<BTreeMap<TunnelId, HostTunnel>>,
    changes: Changes,
    /// Con el que se lanzan los procesos: el real, salvo en las pruebas.
    spawner: Mutex<Arc<dyn Spawner>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    pub fn new(opts: HostOptions, changes: Changes) -> Self {
        Host {
            inner: Arc::new(HostInner {
                opts,
                tunnels: Mutex::new(BTreeMap::new()),
                changes,
                spawner: Mutex::new(Arc::new(SystemSpawner)),
            }),
        }
    }

    /// Cambia cómo se lanzan los procesos (las pruebas usan un doble).
    pub fn set_spawner(&self, spawner: Arc<dyn Spawner>) {
        *lock(&self.inner.spawner) = spawner;
    }

    fn tunnel(&self, id: TunnelId) -> Result<HostTunnel, ExecError> {
        self.get(id).ok_or(ExecError::UnknownTunnel(id))
    }

    /// Lanza un proceso en el túnel `id` ([`HostTunnel::exec`]).
    pub fn exec(&self, id: TunnelId, spec: ExecSpec) -> Result<TunnelProcess, ExecError> {
        let spawner = lock(&self.inner.spawner).clone();
        self.tunnel(id)?.exec(spawner.as_ref(), spec)
    }

    /// Mata un proceso del túnel con todo su árbol.
    pub fn kill(&self, id: TunnelId, process: ProcessId) -> Result<(), ExecError> {
        self.tunnel(id)?.kill(process)
    }

    /// Los túneles abiertos con sus procesos corriendo, para la UI.
    pub fn views(&self) -> Vec<TunnelView> {
        self.list()
            .into_iter()
            .filter(|t| t.state == TunnelState::Open)
            .map(|tunnel| {
                let running = self.get(tunnel.id).map(|t| t.running()).unwrap_or_default();
                TunnelView { tunnel, running }
            })
            .collect()
    }

    /// Abre el túnel; si ya existe uno con ese id (un reintento) devuelve ese.
    pub fn open(&self, tunnel: Tunnel) -> Tunnel {
        let mut tunnels = lock(&self.inner.tunnels);
        self.purge(&mut tunnels);
        if let Some(t) = tunnels.get(&tunnel.id) {
            return t.info();
        }
        let mut info = tunnel;
        info.state = TunnelState::Open;
        tracing::info!(id = %info.id, title = %info.title, "túnel abierto desde {}", info.origin);
        tunnels.insert(
            info.id,
            HostTunnel::new(info.clone(), self.inner.opts, self.inner.changes.clone()),
        );
        drop(tunnels);
        self.inner.changes.bump();
        info
    }

    /// Cierra el túnel: suelta sus recursos y termina sus flujos. Cerrar uno ya cerrado no
    /// hace nada; uno desconocido es un error.
    pub fn close(&self, id: TunnelId) -> Result<(), String> {
        let t = self.get(id).ok_or_else(|| unknown(id))?;
        if t.close() {
            tracing::info!(%id, "túnel cerrado");
            self.inner.changes.bump();
        }
        Ok(())
    }

    /// Todos, también los cerrados que todavía se recuerdan.
    pub fn list(&self) -> Vec<Tunnel> {
        let mut tunnels = lock(&self.inner.tunnels);
        self.purge(&mut tunnels);
        tunnels.values().map(HostTunnel::info).collect()
    }

    pub fn get(&self, id: TunnelId) -> Option<HostTunnel> {
        lock(&self.inner.tunnels).get(&id).cloned()
    }

    fn purge(&self, tunnels: &mut BTreeMap<TunnelId, HostTunnel>) {
        let linger = self.inner.opts.linger;
        tunnels.retain(|_, t| t.closed_at().is_none_or(|at| at.elapsed() < linger));
    }
}

fn unknown(id: TunnelId) -> String {
    format!("no hay ningún túnel {id} en esta máquina")
}

/// Un túnel en la destino. Se clona barato.
#[derive(Clone)]
pub struct HostTunnel {
    inner: Arc<TunnelInner>,
}

struct TunnelInner {
    info: Mutex<Tunnel>,
    closed_at: Mutex<Option<Instant>>,
    flows: Mutex<Vec<Arc<Flow>>>,
    resources: Mutex<Vec<Box<dyn Resource>>>,
    next_flow: AtomicU32,
    opts: HostOptions,
    processes: Table,
    /// El cierre ya terminó (flujos, recursos y procesos): lo que se manda después ya es lo
    /// último.
    settled: AtomicBool,
    /// Sube con cada byte escrito, flujo nuevo o cierre: quien manda la salida lo sigue.
    changes: watch::Sender<u64>,
    /// Los cambios de todos los túneles (la UI), que también suben con cada proceso.
    agent_changes: Changes,
}

struct Flow {
    info: FlowInfo,
    state: Mutex<FlowState>,
}

struct FlowState {
    buf: OutputBuffer,
    ended: bool,
    /// Ya se avisó en el log que el búfer descarta (una vez por flujo).
    warned: bool,
}

/// El túnel ya está cerrado: no admite flujos ni recursos nuevos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("el túnel {0} está cerrado")]
pub struct TunnelClosed(pub TunnelId);

impl HostTunnel {
    fn new(info: Tunnel, opts: HostOptions, agent_changes: Changes) -> Self {
        HostTunnel {
            inner: Arc::new(TunnelInner {
                info: Mutex::new(info),
                closed_at: Mutex::new(None),
                flows: Mutex::new(Vec::new()),
                resources: Mutex::new(Vec::new()),
                next_flow: AtomicU32::new(1),
                opts,
                processes: Table::new(),
                settled: AtomicBool::new(false),
                changes: watch::Sender::new(0),
                agent_changes,
            }),
        }
    }

    pub fn info(&self) -> Tunnel {
        lock(&self.inner.info).clone()
    }

    pub fn id(&self) -> TunnelId {
        lock(&self.inner.info).id
    }

    pub fn is_open(&self) -> bool {
        self.closed_at().is_none()
    }

    fn closed_at(&self) -> Option<Instant> {
        *lock(&self.inner.closed_at)
    }

    fn bump(&self) {
        self.inner.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Un flujo de salida nuevo (por ejemplo, la de un proceso), con el búfer circular del
    /// túnel.
    pub fn add_flow(&self, label: impl Into<String>) -> Result<FlowWriter, TunnelClosed> {
        let mut flows = lock(&self.inner.flows);
        if !self.is_open() {
            return Err(TunnelClosed(self.id()));
        }
        let id = FlowId(self.inner.next_flow.fetch_add(1, Ordering::Relaxed));
        let flow = Arc::new(Flow {
            info: FlowInfo {
                id,
                label: label.into(),
            },
            state: Mutex::new(FlowState {
                buf: OutputBuffer::new(self.inner.opts.capacity),
                ended: false,
                warned: false,
            }),
        });
        flows.push(flow.clone());
        drop(flows);
        self.bump();
        Ok(FlowWriter {
            tunnel: self.clone(),
            flow,
        })
    }

    /// Registra algo que se suelta al cerrar el túnel. Si ya está cerrado se suelta ya.
    pub fn add_resource(&self, resource: Box<dyn Resource>) -> Result<(), TunnelClosed> {
        let mut resources = lock(&self.inner.resources);
        if !self.is_open() {
            drop(resources);
            resource.close();
            return Err(TunnelClosed(self.id()));
        }
        resources.push(resource);
        Ok(())
    }

    /// `true` si lo cerró esta llamada.
    fn close(&self) -> bool {
        // Con el candado de los flujos (el mismo orden que `add_flow`): quien manda la salida
        // y ve el túnel cerrado ve también todos sus flujos terminados.
        {
            let flows = lock(&self.inner.flows);
            {
                let mut at = lock(&self.inner.closed_at);
                if at.is_some() {
                    return false;
                }
                *at = Some(Instant::now());
            }
            for flow in flows.iter() {
                lock(&flow.state).ended = true;
            }
        }
        lock(&self.inner.info).state = TunnelState::Closed;
        let resources = std::mem::take(&mut *lock(&self.inner.resources));
        for r in &resources {
            r.close();
        }
        // Con el túnel ya cerrado: ningún proceso nuevo entra (ver `Table::insert`).
        self.inner.processes.kill_all();
        self.inner.settled.store(true, Ordering::SeqCst);
        self.bump();
        true
    }

    fn is_settled(&self) -> bool {
        self.inner.settled.load(Ordering::SeqCst)
    }

    /// Lanza un proceso con `spawner` (T-104). Su stdout y su stderr son dos flujos nuevos
    /// del túnel; el proceso queda en la tabla hasta que pase `linger` de su fin.
    pub fn exec(&self, spawner: &dyn Spawner, spec: ExecSpec) -> Result<TunnelProcess, ExecError> {
        let tunnel = self.id();
        if !self.is_open() {
            return Err(ExecError::Closed(tunnel));
        }
        self.purge_processes();
        let launch: Launch = launch::plan(&spec, Os::current())?;
        let spawned = spawner.spawn(&launch)?;
        let id = self.inner.processes.next_id();
        let command = launch::display(&spec);
        let flows = self
            .add_flow(format!("{id} {command} · stdout"))
            .and_then(|out| Ok((out, self.add_flow(format!("{id} {command} · stderr"))?)));
        let Ok((out, err)) = flows else {
            spawned.child.kill_tree();
            return Err(ExecError::Closed(tunnel));
        };
        let info = TunnelProcess {
            id,
            tunnel,
            command,
            shell: spec.shell,
            cwd: spec.cwd,
            pid: spawned.pid,
            stdout: out.id(),
            stderr: err.id(),
            started: now_millis(),
            duration: None,
            state: tandem_proto::ProcessState::Running,
        };
        if !self
            .inner
            .processes
            .insert(info.clone(), spawned.child.clone(), || self.is_open())
        {
            spawned.child.kill_tree();
            return Err(ExecError::Closed(tunnel));
        }
        tracing::info!(%tunnel, process = %id, command = %info.command, "proceso lanzado");
        crate::process::supervise(self.clone(), id, spawned, out, err);
        self.process_changed();
        Ok(info)
    }

    /// Mata un proceso con todo su árbol. Matar uno que ya terminó no hace nada.
    pub fn kill(&self, process: ProcessId) -> Result<(), ExecError> {
        match self.inner.processes.kill(process) {
            None => Err(ExecError::UnknownProcess(self.id(), process)),
            Some(was_running) => {
                if was_running {
                    tracing::info!(tunnel = %self.id(), %process, "proceso matado");
                    self.process_changed();
                }
                Ok(())
            }
        }
    }

    /// El proceso terminó solo (lo llama quien lo vigila).
    pub(crate) fn process_exited(&self, process: ProcessId, code: Option<i32>) {
        if self.inner.processes.exited(process, code) {
            tracing::info!(tunnel = %self.id(), %process, ?code, "proceso terminado");
            self.process_changed();
        }
    }

    fn process_changed(&self) {
        self.bump();
        self.inner.agent_changes.bump();
    }

    /// Los procesos del túnel, también los que terminaron hace poco.
    pub fn processes(&self) -> Vec<TunnelProcess> {
        self.purge_processes();
        self.inner.processes.list()
    }

    /// Los que están corriendo, del más viejo al más nuevo.
    pub fn running(&self) -> Vec<TunnelProcess> {
        self.inner.processes.running()
    }

    /// La salida de un proceso desde `from`, con hasta `max` bytes de cada flujo.
    pub fn output(
        &self,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, ExecError> {
        let p = self
            .inner
            .processes
            .get(process)
            .ok_or(ExecError::UnknownProcess(self.id(), process))?;
        let flows = self.flows();
        Ok(output::collect(p, from, max, |flow, at, max| {
            let f = flows.iter().find(|f| f.info.id == flow)?;
            let s = lock(&f.state);
            Some(FlowRead {
                read: s.buf.read(at, max),
                end: s.buf.end(),
                ended: s.ended,
            })
        }))
    }

    /// Olvida los procesos terminados hace más de `linger` con sus flujos.
    fn purge_processes(&self) {
        let gone = self.inner.processes.purge(self.inner.opts.linger);
        if gone.is_empty() {
            return;
        }
        lock(&self.inner.flows).retain(|f| {
            !gone
                .iter()
                .any(|p| p.stdout == f.info.id || p.stderr == f.info.id)
        });
    }

    fn flows(&self) -> Vec<Arc<Flow>> {
        lock(&self.inner.flows).clone()
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Donde escribe quien produce la salida de un flujo. Al soltarlo, el flujo termina.
pub struct FlowWriter {
    tunnel: HostTunnel,
    flow: Arc<Flow>,
}

impl FlowWriter {
    pub fn id(&self) -> FlowId {
        self.flow.info.id
    }

    /// Agrega al flujo; si el búfer está lleno se descarta lo más viejo (y el origen lo
    /// sabrá con [`TunnelDown::Lost`]). Después de terminar, no hace nada.
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut s = lock(&self.flow.state);
            if s.ended {
                return;
            }
            s.buf.push(bytes);
            if s.buf.dropped() > 0 && !s.warned {
                s.warned = true;
                tracing::warn!(
                    tunnel = %self.tunnel.id(),
                    flow = %self.flow.info.label,
                    "el búfer de salida se llenó sin que nadie lo leyera: se descarta lo más viejo"
                );
            }
        }
        self.tunnel.bump();
    }

    /// Ya no hay más salida.
    pub fn finish(&self) {
        let changed = {
            let mut s = lock(&self.flow.state);
            !std::mem::replace(&mut s.ended, true)
        };
        if changed {
            self.tunnel.bump();
        }
    }
}

impl Drop for FlowWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Atiende un stream de túnel que abrió el origen: un pedido y su respuesta, o el canal
/// de datos de un túnel hasta que se cierre o se corte.
pub async fn serve<W: TargetWire>(host: &Host, mut wire: W) -> Result<(), WireError> {
    let Some(req) = wire.recv().await? else {
        return Ok(());
    };
    serve_request(host, wire, req).await
}

/// Como [`serve`] con el primer pedido ya leído: para quien reparte los pedidos de otros
/// módulos (archivos y puertos) y deja el resto a los túneles.
pub async fn serve_request<W: TargetWire>(
    host: &Host,
    mut wire: W,
    req: TunnelUp,
) -> Result<(), WireError> {
    let reply = match req {
        TunnelUp::Files(_) | TunnelUp::Port(_) => {
            return wire
                .send(&TunnelDown::Refused(
                    "esta máquina no atiende archivos ni puertos".into(),
                ))
                .await;
        }
        TunnelUp::Open(t) => TunnelDown::Opened(host.open(t)),
        TunnelUp::Close(id) => match host.close(id) {
            Ok(()) => TunnelDown::Closed(id),
            Err(e) => TunnelDown::Refused(e),
        },
        TunnelUp::List => TunnelDown::List(host.list()),
        TunnelUp::Exec { tunnel, spec } => match host.exec(tunnel, spec) {
            Ok(p) => TunnelDown::Started(p),
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Kill { tunnel, process } => match host.kill(tunnel, process) {
            Ok(()) => TunnelDown::Killed(process),
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Attach { tunnel, from } => {
            let Some(t) = host.get(tunnel) else {
                return wire.send(&TunnelDown::Refused(unknown(tunnel))).await;
            };
            wire.start_data()?;
            return stream(&t, from.into_iter().collect(), &mut wire).await;
        }
    };
    wire.send(&reply).await
}

/// Manda la salida de `t` desde las posiciones `from`, un pedazo por flujo por vuelta, hasta
/// que el túnel se cierre y no quede nada.
async fn stream<W: TargetWire>(
    t: &HostTunnel,
    mut pos: BTreeMap<FlowId, u64>,
    wire: &mut W,
) -> Result<(), WireError> {
    let mut known: BTreeSet<FlowId> = BTreeSet::new();
    let mut ended: BTreeSet<FlowId> = BTreeSet::new();
    let mut procs: BTreeMap<ProcessId, u64> = BTreeMap::new();
    let mut changes = t.inner.changes.subscribe();
    loop {
        // Antes de mirar: lo que se escriba desde aquí despierta la espera de abajo.
        changes.borrow_and_update();
        let closed = t.is_settled();
        let mut sent = false;
        // Los procesos primero: así el origen sabe de qué son los flujos que siguen.
        for p in t.inner.processes.updates(&mut procs) {
            wire.send(&TunnelDown::Process(p)).await?;
            sent = true;
        }
        for flow in t.flows() {
            let id = flow.info.id;
            if known.insert(id) {
                wire.send(&TunnelDown::Flow(flow.info.clone())).await?;
            }
            if ended.contains(&id) {
                continue;
            }
            let at = pos.entry(id).or_insert(0);
            let (read, done, total) = {
                let s = lock(&flow.state);
                (s.buf.read(*at, CHUNK), s.ended, s.buf.end())
            };
            let msg = match read {
                Read::Data { offset, bytes } => {
                    *at = offset + bytes.len() as u64;
                    TunnelDown::Data {
                        flow: id,
                        offset,
                        bytes,
                    }
                }
                Read::Lost { from, to } => {
                    *at = to;
                    TunnelDown::Lost { flow: id, from, to }
                }
                Read::Pending if done => {
                    ended.insert(id);
                    TunnelDown::End { flow: id, total }
                }
                Read::Pending => continue,
            };
            wire.send(&msg).await?;
            sent = true;
        }
        if sent {
            continue;
        }
        if closed {
            return wire.send(&TunnelDown::TunnelClosed).await;
        }
        if changes.changed().await.is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use tandem_core::Machine;
    use tandem_proto::{Client, ClientKind};

    use super::*;

    pub(crate) fn tunnel(id: u64) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: "pruebas".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
            state: TunnelState::Open,
            created: 0,
        }
    }

    struct Flag(Arc<AtomicBool>);

    impl Resource for Flag {
        fn close(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn abrir_dos_veces_devuelve_el_mismo_y_cerrar_suelta_los_recursos() {
        let host = Host::new(HostOptions::default(), Changes::default());
        host.open(tunnel(1));
        let mut again = tunnel(1);
        again.title = "otro".into();
        assert_eq!(host.open(again).title, "pruebas");
        let t = host.get(TunnelId(1)).unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        t.add_resource(Box::new(Flag(closed.clone()))).unwrap();
        let w = t.add_flow("out").unwrap();
        host.close(TunnelId(1)).unwrap();
        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(host.list()[0].state, TunnelState::Closed);
        // Lo que llegue después se suelta enseguida y no se acepta.
        let late = Arc::new(AtomicBool::new(false));
        assert!(t.add_resource(Box::new(Flag(late.clone()))).is_err());
        assert!(late.load(Ordering::SeqCst));
        assert!(t.add_flow("x").is_err());
        w.write(b"tarde");
        assert!(host.close(TunnelId(9)).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn los_cerrados_se_olvidan_despues_de_un_rato() {
        let opts = HostOptions {
            linger: Duration::from_secs(5),
            ..HostOptions::default()
        };
        let host = Host::new(opts, Changes::default());
        host.open(tunnel(1));
        host.close(TunnelId(1)).unwrap();
        assert_eq!(host.list().len(), 1);
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(host.list().is_empty());
    }
}
