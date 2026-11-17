//! Caminos en memoria entre un [`Origin`](crate::Origin) y un [`Host`], para probar los
//! túneles con dos agentes en el mismo proceso sin red.
//!
//! Cada [`MemPath`] es un camino (USB-C, Ethernet, Wi-Fi) que se puede cortar ya o después
//! de dejar pasar cierta cantidad de bytes de salida: un corte deja muertos todos sus
//! streams y lo que estaba en vuelo se pierde, como un cable desenchufado.

use std::sync::{Arc, Mutex, PoisonError};

use tandem_proto::{TunnelDown, TunnelUp};
use tokio::sync::{mpsc, watch};

use crate::host::{Host, serve};
use crate::wire::{BoxFuture, Dialer, OriginWire, TargetWire, WireError};

/// Un camino en memoria hacia la destino `host`. Se clona barato.
#[derive(Clone)]
pub struct MemPath {
    inner: Arc<PathInner>,
}

struct PathInner {
    host: Host,
    cut: watch::Sender<bool>,
    /// Bytes de salida que todavía pueden pasar antes de cortarse solo.
    budget: Mutex<Option<u64>>,
}

impl MemPath {
    pub fn new(host: &Host) -> Self {
        MemPath {
            inner: Arc::new(PathInner {
                host: host.clone(),
                cut: watch::Sender::new(false),
                budget: Mutex::new(None),
            }),
        }
    }

    /// Para [`crate::Origin::set_link`]: cada llamada es un enlace distinto.
    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::new(self.clone())
    }

    pub fn cut(&self) {
        self.inner.cut.send_replace(true);
    }

    /// Se corta solo cuando un mensaje de datos pasaría de `bytes` de salida en total.
    pub fn cut_after(&self, bytes: u64) {
        *self
            .inner
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(bytes);
    }

    pub fn is_cut(&self) -> bool {
        *self.inner.cut.borrow()
    }

    /// Termina cuando el camino se corta.
    pub async fn wait_cut(&self) {
        let mut rx = self.inner.cut.subscribe();
        let _ = rx.wait_for(|cut| *cut).await;
    }

    fn check(&self) -> Result<(), WireError> {
        if self.is_cut() {
            Err(WireError::new("camino cortado"))
        } else {
            Ok(())
        }
    }

    /// Descuenta `n` bytes de salida; si no alcanzan, corta.
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
                Err(WireError::new("camino cortado"))
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
                    tracing::debug!("stream de túnel en memoria terminado: {e}");
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

/// Espera el siguiente mensaje o el corte; con el camino cortado no entrega lo que quedó en
/// vuelo.
async fn recv_or_cut<T>(
    path: &MemPath,
    rx: &mut mpsc::UnboundedReceiver<T>,
) -> Result<Option<T>, WireError> {
    let mut cut = path.inner.cut.subscribe();
    tokio::select! {
        biased;
        _ = cut.wait_for(|c| *c) => Err(WireError::new("camino cortado")),
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
                .map_err(|_| WireError::new("la destino cerró el stream"))
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
        if let TunnelDown::Data { bytes, .. } = msg {
            self.path.spend(bytes.len() as u64)?;
        }
        self.tx
            .send(msg.clone())
            .map_err(|_| WireError::new("el origen cerró el stream"))
    }
}
