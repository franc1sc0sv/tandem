//! In-memory paths between an [`Origin`](crate::Origin) and a [`Host`], to test
//! tunnels with two agents in the same process without a network.
//!
//! Each [`MemPath`] is a path (USB-C, Ethernet, Wi-Fi) that can be cut now or after
//! letting a certain number of output bytes through: a cut leaves all its
//! streams dead and what was in flight is lost, like an unplugged cable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tandem_proto::{TunnelDown, TunnelUp};
use tokio::sync::{mpsc, watch};

use crate::host::{Host, serve};
use crate::wire::{BoxFuture, Dialer, OriginWire, TargetWire, WireError};

/// An in-memory path to the destination `host`. Cheap to clone.
#[derive(Clone)]
pub struct MemPath {
    inner: Arc<PathInner>,
}

struct PathInner {
    host: Host,
    cut: watch::Sender<bool>,
    /// Output bytes that can still pass before it cuts itself.
    budget: Mutex<Option<u64>>,
    /// Cut when the destination answers a request: the request was served, the response
    /// is lost.
    cut_reply: AtomicBool,
}

impl MemPath {
    pub fn new(host: &Host) -> Self {
        MemPath {
            inner: Arc::new(PathInner {
                host: host.clone(),
                cut: watch::Sender::new(false),
                budget: Mutex::new(None),
                cut_reply: AtomicBool::new(false),
            }),
        }
    }

    /// For [`crate::Origin::set_link`]: each call is a different link.
    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::new(self.clone())
    }

    pub fn cut(&self) {
        self.inner.cut.send_replace(true);
    }

    /// It cuts itself when a data message would exceed `bytes` of output in total.
    pub fn cut_after(&self, bytes: u64) {
        *self
            .inner
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(bytes);
    }

    /// It is cut when the destination is about to answer a request (open, close, list): it
    /// did it, but the origin does not find out.
    pub fn cut_before_reply(&self) {
        self.inner.cut_reply.store(true, Ordering::SeqCst);
    }

    pub fn is_cut(&self) -> bool {
        *self.inner.cut.borrow()
    }

    /// Ends when the path is cut.
    pub async fn wait_cut(&self) {
        let mut rx = self.inner.cut.subscribe();
        let _ = rx.wait_for(|cut| *cut).await;
    }

    fn check(&self) -> Result<(), WireError> {
        if self.is_cut() {
            Err(WireError::new("cut path"))
        } else {
            Ok(())
        }
    }

    /// Subtracts `n` output bytes; if they are not enough, cuts.
    fn spend(&self, n: u64) -> Result<(), WireError> {
        let mut budget = self
            .inner
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match budget.as_mut() {
            Some(left) if *left < n => {
                drop(budget);
                self.cut();
                Err(WireError::new("cut path"))
            }
            Some(left) => {
                *left -= n;
                Ok(())
            }
            None => Ok(()),
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
            let host = self.inner.host.clone();
            tokio::spawn(async move {
                if let Err(e) = serve(&host, target).await {
                    tracing::debug!("in-memory tunnel stream ended: {e}");
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

/// Waits for the next message or the cut; with the path cut it does not deliver what was in
/// flight.
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
            self.tx
                .send(msg)
                .map_err(|_| WireError::new("the destination closed the stream"))
        })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Option<TunnelDown>, WireError>> {
        Box::pin(recv_or_cut(&self.path, &mut self.rx))
    }
}

struct MemTarget {
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
        match msg {
            TunnelDown::Data { bytes, .. } => self.path.spend(bytes.len() as u64)?,
            TunnelDown::Opened(_)
            | TunnelDown::Closed(_)
            | TunnelDown::List(_)
            | TunnelDown::Refused(_)
                if self.path.inner.cut_reply.load(Ordering::SeqCst) =>
            {
                self.path.cut();
                return Err(WireError::new("cut path"));
            }
            _ => {}
        }
        self.tx
            .send(msg.clone())
            .map_err(|_| WireError::new("the origin closed the stream"))
    }
}
