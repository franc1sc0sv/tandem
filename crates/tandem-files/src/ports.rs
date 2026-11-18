//! TCP port forwarding over a tunnel.
//!
//! Each forwarded connection uses **one stream per direction** instead of multiplexing: the `Send`
//! stream only carries bytes from the origin to the destination and `Recv` only from the destination to the origin.
//! This way each task uses its stream in a single direction and there is no need to wait on two things at
//! once on the same one (which with the current transport cannot be canceled safely).
//!
//! - `LocalToRemote` (like `ssh -L`): the origin listens; for each connection it asks `Connect`
//!   of the destination and opens `Send` and `Recv` with the same `conn`.
//! - `RemoteToLocal` (like `ssh -R`): the origin opens a `Listen` stream; the destination listens and
//!   announces each accepted connection with `Incoming { conn }`; the origin connects to its local port
//!   and opens `Send` and `Recv` with that `conn`.
//!
//! Ports are released with the tunnel: the destination registers a [`Resource`] that cuts what
//! it listens on and what is in progress, and the origin stops listening when the tunnel closes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_proto::{Direction, ForwardInfo, PortDown, PortUp, TunnelDown, TunnelId, TunnelUp};
use tandem_tunnel::{Host, Origin, OriginWire, Resource, TargetWire, WireError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::error::{FilesError, Result};

/// Simultaneous connections per forward: each uses two streams and QUIC allows 100.
const MAX_CONNS: usize = 32;
/// What is read from a socket per turn.
const BUF: usize = 64 * 1024;
/// How long an already open connection waits for the other side to open its streams.
const PENDING_TTL: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the destination sends a heartbeat over a `Listen`.
const BEAT: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn io_err(e: std::io::Error) -> FilesError {
    FilesError::io(std::path::Path::new("TCP connection"), e)
}

// ---------------------------------------------------------------------------------------
// The destination
// ---------------------------------------------------------------------------------------

struct Pending {
    read: Option<OwnedReadHalf>,
    write: Option<OwnedWriteHalf>,
}

struct TunnelPorts {
    stop: watch::Sender<bool>,
    pending: Mutex<HashMap<u64, Pending>>,
}

impl TunnelPorts {
    /// `false` (and it is discarded) if there are already [`MAX_CONNS`] waiting: without a cap, anyone who
    /// reaches an open `Listen` would fill the memory with sockets.
    fn insert(self: &Arc<Self>, conn: u64, tcp: TcpStream) -> bool {
        let (read, write) = tcp.into_split();
        let mut pending = lock(&self.pending);
        if pending.len() >= MAX_CONNS {
            return false;
        }
        pending.insert(
            conn,
            Pending {
                read: Some(read),
                write: Some(write),
            },
        );
        // If the other side never opens its streams, it does not stay forever.
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(PENDING_TTL).await;
            if let Some(me) = me.upgrade() {
                lock(&me.pending).remove(&conn);
            }
        });
        true
    }

    fn take_read(&self, conn: u64) -> Option<OwnedReadHalf> {
        let mut p = lock(&self.pending);
        let e = p.get_mut(&conn)?;
        let r = e.read.take();
        if e.read.is_none() && e.write.is_none() {
            p.remove(&conn);
        }
        r
    }

    fn take_write(&self, conn: u64) -> Option<OwnedWriteHalf> {
        let mut p = lock(&self.pending);
        let e = p.get_mut(&conn)?;
        let w = e.write.take();
        if e.read.is_none() && e.write.is_none() {
            p.remove(&conn);
        }
        w
    }
}

/// What each destination tunnel has open in ports.
#[derive(Clone, Default)]
pub struct PortHost {
    tunnels: Arc<Mutex<HashMap<TunnelId, Arc<TunnelPorts>>>>,
}

struct Closer {
    id: TunnelId,
    owner: PortHost,
    ports: Arc<TunnelPorts>,
}

impl Resource for Closer {
    fn close(&self) {
        self.ports.stop.send_replace(true);
        lock(&self.ports.pending).clear();
        lock(&self.owner.tunnels).remove(&self.id);
    }
}

impl PortHost {
    /// The tunnel's ports; `None` if it is not open on this machine. The first time it
    /// registers the [`Resource`] that releases them on close.
    fn ports_for(&self, host: &Host, tunnel: TunnelId) -> Option<Arc<TunnelPorts>> {
        let t = host.get(tunnel).filter(|t| t.is_open())?;
        let ports = {
            let mut map = lock(&self.tunnels);
            if let Some(p) = map.get(&tunnel) {
                return Some(p.clone());
            }
            let p = Arc::new(TunnelPorts {
                stop: watch::Sender::new(false),
                pending: Mutex::new(HashMap::new()),
            });
            map.insert(tunnel, p.clone());
            p
        };
        // Outside the lock: if the tunnel already closed, `add_resource` calls `close` right there.
        let closer = Closer {
            id: tunnel,
            owner: self.clone(),
            ports: ports.clone(),
        };
        t.add_resource(Box::new(closer)).ok()?;
        Some(ports)
    }
}

async fn refuse<W: TargetWire>(
    wire: &mut W,
    tunnel: TunnelId,
) -> std::result::Result<(), WireError> {
    wire.send(&TunnelDown::Refused(format!(
        "no tunnel {tunnel} is open on this machine"
    )))
    .await
}

async fn failed<W: TargetWire>(
    wire: &mut W,
    why: impl ToString,
) -> std::result::Result<(), WireError> {
    wire.send(&TunnelDown::Port(PortDown::Failed(why.to_string())))
        .await
}

async fn reply<W: TargetWire>(wire: &mut W, msg: PortDown) -> std::result::Result<(), WireError> {
    wire.send(&TunnelDown::Port(msg)).await
}

/// The destination side of a port request; `first` is the first message of the stream.
pub async fn serve<W: TargetWire>(
    host: &Host,
    ports: &PortHost,
    wire: &mut W,
    first: PortUp,
) -> std::result::Result<(), WireError> {
    let tunnel = match &first {
        PortUp::Connect { tunnel, .. }
        | PortUp::Listen { tunnel, .. }
        | PortUp::Send { tunnel, .. }
        | PortUp::Recv { tunnel, .. } => *tunnel,
        PortUp::Bytes(_) | PortUp::Eof => {
            return failed(wire, "the stream has to start with a request").await;
        }
    };
    let Some(tp) = ports.ports_for(host, tunnel) else {
        return refuse(wire, tunnel).await;
    };
    let mut stop = tp.stop.subscribe();
    match first {
        PortUp::Connect {
            conn, host, port, ..
        } => {
            let host = if host.is_empty() { "127.0.0.1" } else { &host };
            let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)));
            match tcp.await {
                Ok(Ok(tcp)) => {
                    if !tp.insert(conn, tcp) {
                        return failed(wire, "too many pending connections on this tunnel").await;
                    }
                    reply(wire, PortDown::Ready).await
                }
                Ok(Err(e)) => {
                    failed(wire, format!("could not connect to {host}:{port}: {e}")).await
                }
                Err(_) => failed(wire, format!("{host}:{port} did not answer in time")).await,
            }
        }
        PortUp::Listen { bind, port, .. } => {
            let bind = if bind.is_empty() { "127.0.0.1" } else { &bind };
            let listener = match TcpListener::bind((bind, port)).await {
                Ok(l) => l,
                Err(e) => {
                    return failed(wire, format!("could not listen on {bind}:{port}: {e}")).await;
                }
            };
            let actual = listener.local_addr().map_or(port, |a| a.port());
            reply(wire, PortDown::Listening { port: actual }).await?;
            loop {
                tokio::select! {
                    _ = stopped(&mut stop) => return Ok(()),
                    accepted = listener.accept() => {
                        let Ok((tcp, _)) = accepted else { continue };
                        let Ok(id) = getrandom::u64() else { continue };
                        if !tp.insert(id, tcp) {
                            continue;
                        }
                        reply(wire, PortDown::Incoming { conn: id }).await?;
                    }
                    () = tokio::time::sleep(BEAT) => reply(wire, PortDown::Beat).await?,
                }
            }
        }
        PortUp::Send { conn, .. } => {
            let Some(mut write) = tp.take_write(conn) else {
                return failed(
                    wire,
                    "that connection does not exist (or has already expired)",
                )
                .await;
            };
            reply(wire, PortDown::Ready).await?;
            let run = async {
                while let Ok(Some(TunnelUp::Port(PortUp::Bytes(b)))) = wire.recv().await {
                    if write.write_all(&b).await.is_err() {
                        break;
                    }
                }
                let _ = write.shutdown().await;
            };
            tokio::select! {
                _ = stopped(&mut stop) => {}
                () = run => {}
            }
            Ok(())
        }
        PortUp::Recv { conn, .. } => {
            let Some(mut read) = tp.take_read(conn) else {
                return failed(
                    wire,
                    "that connection does not exist (or has already expired)",
                )
                .await;
            };
            reply(wire, PortDown::Ready).await?;
            wire.start_data()?;
            let run = async {
                let mut buf = vec![0u8; BUF];
                loop {
                    match read.read(&mut buf).await {
                        Ok(0) | Err(_) => return reply(wire, PortDown::Eof).await,
                        Ok(n) => reply(wire, PortDown::Bytes(buf[..n].to_vec())).await?,
                    }
                }
            };
            tokio::select! {
                _ = stopped(&mut stop) => Ok(()),
                r = run => r,
            }
        }
        PortUp::Bytes(_) | PortUp::Eof => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------
// The origin
// ---------------------------------------------------------------------------------------

/// What is requested when forwarding a port.
#[derive(Debug, Clone)]
pub struct ForwardSpec {
    pub tunnel: TunnelId,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub direction: Direction,
}

struct Active {
    info: ForwardInfo,
    stop: watch::Sender<bool>,
}

/// The forwards this machine has running. Cheap to clone.
#[derive(Clone, Default)]
pub struct Forwards {
    inner: Arc<ForwardsInner>,
}

#[derive(Default)]
struct ForwardsInner {
    next: AtomicU64,
    active: Mutex<HashMap<u64, Active>>,
}

impl Forwards {
    pub fn list(&self) -> Vec<ForwardInfo> {
        let mut v: Vec<_> = lock(&self.inner.active)
            .values()
            .map(|a| a.info.clone())
            .collect();
        v.sort_by_key(|i| i.id);
        v
    }

    /// Stops forwarding `id`. `false` if it does not exist.
    pub fn stop(&self, id: u64) -> bool {
        match lock(&self.inner.active).get(&id) {
            Some(a) => {
                a.stop.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// Starts forwarding. If `local_port` is 0 (toward the destination) the system picks one.
    pub async fn start(&self, origin: &Origin, spec: ForwardSpec) -> Result<ForwardInfo> {
        if !origin.is_open(spec.tunnel) {
            return Err(FilesError::NoTunnel(spec.tunnel));
        }
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (stop_tx, stop_rx) = watch::channel(false);
        let mut info = ForwardInfo {
            id,
            tunnel: spec.tunnel,
            direction: spec.direction,
            local_port: spec.local_port,
            remote_host: spec.remote_host.clone(),
            remote_port: spec.remote_port,
        };
        let me = self.clone();
        let origin = origin.clone();
        match spec.direction {
            Direction::LocalToRemote => {
                let listener = TcpListener::bind(("127.0.0.1", spec.local_port))
                    .await
                    .map_err(io_err)?;
                info.local_port = listener.local_addr().map_err(io_err)?.port();
                self.register(&info, stop_tx);
                tokio::spawn(async move {
                    listen_local(&origin, spec, listener, stop_rx).await;
                    lock(&me.inner.active).remove(&id);
                });
            }
            Direction::RemoteToLocal => {
                let mut wire = origin.dial().await?;
                wire.send(TunnelUp::Port(PortUp::Listen {
                    tunnel: spec.tunnel,
                    bind: spec.remote_host.clone(),
                    port: spec.remote_port,
                }))
                .await?;
                match recv_port(&mut wire).await? {
                    PortDown::Listening { port } => info.remote_port = port,
                    _ => return Err(FilesError::Unexpected),
                }
                self.register(&info, stop_tx);
                tokio::spawn(async move {
                    listen_remote(&origin, spec, wire, stop_rx).await;
                    lock(&me.inner.active).remove(&id);
                });
            }
        }
        Ok(info)
    }

    fn register(&self, info: &ForwardInfo, stop: watch::Sender<bool>) {
        lock(&self.inner.active).insert(
            info.id,
            Active {
                info: info.clone(),
                stop,
            },
        );
    }
}

async fn recv_port(wire: &mut Box<dyn OriginWire>) -> Result<PortDown> {
    match wire.recv().await? {
        None => Err(WireError::new("the other machine closed the stream").into()),
        Some(TunnelDown::Refused(why)) | Some(TunnelDown::Port(PortDown::Failed(why))) => {
            Err(FilesError::Remote(why))
        }
        Some(TunnelDown::Port(m)) => Ok(m),
        Some(_) => Err(FilesError::Unexpected),
    }
}

async fn expect_ready(wire: &mut Box<dyn OriginWire>) -> Result<()> {
    match recv_port(wire).await? {
        PortDown::Ready => Ok(()),
        _ => Err(FilesError::Unexpected),
    }
}

/// Listens here and for each connection asks for another from the destination.
async fn listen_local(
    origin: &Origin,
    spec: ForwardSpec,
    listener: TcpListener,
    mut stop: watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    let slots = Arc::new(Semaphore::new(MAX_CONNS));
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => break,
            _ = tick.tick() => {
                if !origin.is_open(spec.tunnel) {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((tcp, _)) = accepted else { continue };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    tracing::warn!("port forward: too many connections at once, one is rejected");
                    continue;
                };
                let (o, tunnel) = (origin.clone(), spec.tunnel);
                let (host, port) = (spec.remote_host.clone(), spec.remote_port);
                tasks.spawn(async move {
                    if let Err(e) = relay_out(&o, tunnel, host, port, tcp).await {
                        tracing::debug!("forwarded connection ended: {e}");
                    }
                    drop(permit);
                });
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    // Dropping `tasks` cuts the connections in progress.
}

/// The origin listens to the destination's `Listen` and connects to its local port for each notice.
async fn listen_remote(
    origin: &Origin,
    spec: ForwardSpec,
    mut wire: Box<dyn OriginWire>,
    mut stop: watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    let slots = Arc::new(Semaphore::new(MAX_CONNS));
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => break,
            msg = wire.recv() => match msg {
                Ok(Some(TunnelDown::Port(PortDown::Incoming { conn }))) => {
                    let Ok(permit) = slots.clone().try_acquire_owned() else {
                        tracing::warn!("port forward: too many connections at once, one is rejected");
                        continue;
                    };
                    let (o, tunnel, local) = (origin.clone(), spec.tunnel, spec.local_port);
                    tasks.spawn(async move {
                        if let Err(e) = relay_in(&o, tunnel, conn, local).await {
                            tracing::debug!("forwarded connection ended: {e}");
                        }
                        drop(permit);
                    });
                }
                Ok(Some(TunnelDown::Port(PortDown::Beat))) => {}
                // The tunnel closed, the link was cut or the destination no longer listens.
                _ => break,
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn relay_out(
    origin: &Origin,
    tunnel: TunnelId,
    host: String,
    port: u16,
    tcp: TcpStream,
) -> Result<()> {
    let conn = getrandom::u64().map_err(|e| FilesError::Remote(e.to_string()))?;
    let mut wire = origin.dial().await?;
    wire.send(TunnelUp::Port(PortUp::Connect {
        tunnel,
        conn,
        host,
        port,
    }))
    .await?;
    expect_ready(&mut wire).await?;
    drop(wire);
    pump(origin, tunnel, conn, tcp).await
}

async fn relay_in(origin: &Origin, tunnel: TunnelId, conn: u64, local_port: u16) -> Result<()> {
    let tcp = TcpStream::connect(("127.0.0.1", local_port))
        .await
        .map_err(io_err)?;
    pump(origin, tunnel, conn, tcp).await
}

/// Joins the local socket with the destination's connection `conn` through two streams.
async fn pump(origin: &Origin, tunnel: TunnelId, conn: u64, tcp: TcpStream) -> Result<()> {
    let (mut rd, mut wr) = tcp.into_split();
    let mut up = origin.dial().await?;
    up.send(TunnelUp::Port(PortUp::Send { tunnel, conn }))
        .await?;
    expect_ready(&mut up).await?;
    up.start_data()?;
    let mut down = origin.dial().await?;
    down.send(TunnelUp::Port(PortUp::Recv { tunnel, conn }))
        .await?;
    expect_ready(&mut down).await?;
    let uplink = async {
        let mut buf = vec![0u8; BUF];
        loop {
            let n = rd.read(&mut buf).await.map_err(io_err)?;
            if n == 0 {
                up.send(TunnelUp::Port(PortUp::Eof)).await?;
                return Ok::<(), FilesError>(());
            }
            up.send(TunnelUp::Port(PortUp::Bytes(buf[..n].to_vec())))
                .await?;
        }
    };
    let downlink = async {
        loop {
            match down.recv().await? {
                Some(TunnelDown::Port(PortDown::Bytes(b))) => {
                    wr.write_all(&b).await.map_err(io_err)?;
                }
                Some(TunnelDown::Port(PortDown::Eof)) | None => {
                    let _ = wr.shutdown().await;
                    return Ok::<(), FilesError>(());
                }
                Some(_) => return Err(FilesError::Unexpected),
            }
        }
    };
    tokio::try_join!(uplink, downlink).map(|_| ())
}

/// Ends when the stop signal is activated (or the sender is closed).
async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn socket(l: &TcpListener) -> TcpStream {
        let (c, _s) = tokio::join!(TcpStream::connect(l.local_addr().unwrap()), l.accept());
        c.unwrap()
    }

    #[tokio::test]
    async fn pending_has_a_cap_per_tunnel() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tp = Arc::new(TunnelPorts {
            stop: watch::channel(false).0,
            pending: Mutex::new(HashMap::new()),
        });
        for i in 0..MAX_CONNS as u64 {
            assert!(tp.insert(i, socket(&l).await));
        }
        assert!(!tp.insert(999, socket(&l).await));
        assert_eq!(lock(&tp.pending).len(), MAX_CONNS);
        // On taking one half and the other, the slot is freed.
        assert!(tp.take_read(0).is_some() && tp.take_write(0).is_some());
        assert!(tp.insert(1000, socket(&l).await));
    }
}
