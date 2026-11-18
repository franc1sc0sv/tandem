//! In-memory paths between an [`Origin`](tandem_tunnel::Origin) and a [`Host`] with
//! [`Files`], to test files and ports with two agents in the same process.
//!
//! Like [`tandem_tunnel::mem::MemPath`], but the destination side also serves files and
//! ports, and it can be cut after letting a certain number of file chunks through
//! (in either direction), like a cable unplugged midway through a copy.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tandem_proto::{FilesDown, FilesUp, TunnelDown, TunnelUp};
use tandem_tunnel::{BoxFuture, Dialer, Host, OriginWire, TargetWire, WireError};
use tokio::sync::{mpsc, watch};

use crate::Files;

/// An in-memory path to the destination `host`. Cheap to clone.
#[derive(Clone)]
pub struct MemPath {
    inner: Arc<PathInner>,
}

/// Whoever serves the streams on the destination: by default, [`Files::serve`].
type Server =
    Arc<dyn Fn(Host, MemTarget) -> BoxFuture<'static, Result<(), WireError>> + Send + Sync>;

struct PathInner {
    host: Host,
    server: Server,
    cut: watch::Sender<bool>,
    /// File pieces that can still pass before it cuts itself.
    budget: Mutex<Option<u64>>,
    /// File pieces that passed in total, for the tests.
    passed: AtomicU64,
}

impl MemPath {
    pub fn new(host: &Host, files: &Files) -> Self {
        let files = files.clone();
        Self::with_server(host, move |host, wire| {
            let files = files.clone();
            Box::pin(async move { files.serve(&host, wire).await })
        })
    }

    /// Like [`MemPath::new`], with another stream server (other modules hanging off
    /// `Files`, like the git workspaces).
    pub fn with_server(
        host: &Host,
        server: impl Fn(Host, MemTarget) -> BoxFuture<'static, Result<(), WireError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        MemPath {
            inner: Arc::new(PathInner {
                host: host.clone(),
                server: Arc::new(server),
                cut: watch::Sender::new(false),
                budget: Mutex::new(None),
                passed: AtomicU64::new(0),
            }),
        }
    }

    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::new(self.clone())
    }

    pub fn cut(&self) {
        self.inner.cut.send_replace(true);
    }

    /// It is cut when the file piece number `pieces + 1` would pass.
    pub fn cut_after_pieces(&self, pieces: u64) {
        *self
            .inner
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(pieces);
    }

    pub fn pieces_passed(&self) -> u64 {
        self.inner.passed.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<(), WireError> {
        if *self.inner.cut.borrow() {
            Err(WireError::new("cut path"))
        } else {
            Ok(())
        }
    }

    fn spend(&self) -> Result<(), WireError> {
        let mut budget = self
            .inner
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match budget.as_mut() {
            Some(0) => {
                drop(budget);
                self.cut();
                Err(WireError::new("cut path"))
            }
            Some(left) => {
                *left -= 1;
                self.inner.passed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            None => {
                self.inner.passed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    }
}

impl Dialer for MemPath {
    fn dial(&self) -> BoxFuture<'_, Result<Box<dyn OriginWire>, WireError>> {
        Box::pin(async move {
            self.check()?;
            let (up_tx, up_rx) = mpsc::unbounded_channel();
            let (down_tx, down_rx) = mpsc::unbounded_channel();
            let target = MemTarget {
                path: self.clone(),
                rx: up_rx,
                tx: down_tx,
            };
            let (host, server) = (self.inner.host.clone(), self.inner.server.clone());
            tokio::spawn(async move {
                if let Err(e) = server(host, target).await {
                    tracing::debug!("stream en memoria terminado: {e}");
                }
            });
            let wire: Box<dyn OriginWire> = Box::new(MemOrigin {
                path: self.clone(),
                tx: up_tx,
                rx: down_rx,
            });
            Ok(wire)
        })
    }
}

/// Like the real cable: a message that does not fit in a frame does not pass.
fn check_frame<T: serde::Serialize>(msg: &T) -> Result<(), WireError> {
    let len = tandem_proto::encode(msg)
        .map_err(|e| WireError::new(e.to_string()))?
        .len();
    if len > tandem_proto::MAX_FRAME_LEN {
        return Err(WireError::new(format!(
            "mensaje de {len} B: no cabe en un marco"
        )));
    }
    Ok(())
}

async fn recv_or_cut<T>(
    path: &MemPath,
    rx: &mut mpsc::UnboundedReceiver<T>,
) -> Result<Option<T>, WireError> {
    let mut cut = path.inner.cut.subscribe();
    tokio::select! {
        biased;
        _ = cut.wait_for(|c| *c) => Err(WireError::new("cut path")),
        msg = rx.recv() => {
            path.check()?;
            Ok(msg)
        }
    }
}

struct MemOrigin {
    path: MemPath,
    tx: mpsc::UnboundedSender<TunnelUp>,
    rx: mpsc::UnboundedReceiver<TunnelDown>,
}

impl OriginWire for MemOrigin {
    fn send(&mut self, msg: TunnelUp) -> BoxFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            self.path.check()?;
            if matches!(msg, TunnelUp::Files(FilesUp::Chunk { .. })) {
                self.path.spend()?;
            }
            check_frame(&msg)?;
            self.tx
                .send(msg)
                .map_err(|_| WireError::new("the destination closed the stream"))
        })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Option<TunnelDown>, WireError>> {
        Box::pin(recv_or_cut(&self.path, &mut self.rx))
    }
}

/// The destination side of an in-memory stream.
pub struct MemTarget {
    path: MemPath,
    rx: mpsc::UnboundedReceiver<TunnelUp>,
    tx: mpsc::UnboundedSender<TunnelDown>,
}

impl TargetWire for MemTarget {
    async fn recv(&mut self) -> Result<Option<TunnelUp>, WireError> {
        recv_or_cut(&self.path, &mut self.rx).await
    }

    async fn send(&mut self, msg: &TunnelDown) -> Result<(), WireError> {
        self.path.check()?;
        if matches!(msg, TunnelDown::Files(FilesDown::Chunk { .. })) {
            self.path.spend()?;
        }
        check_frame(msg)?;
        self.tx
            .send(msg.clone())
            .map_err(|_| WireError::new("the origin closed the stream"))
    }
}
