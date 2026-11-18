//! Arrastrar archivos entre las máquinas por el canal de control (D14, T-121).
//!
//! La lógica del flujo vive en `tandem_files::drag`; aquí se la conecta al enlace:
//! - origen: [`ControlDragPeer`] implementa `DragPeer` con los [`DragMsg`] de control y
//!   [`run_origin`] corre `DragOut`, publicando el avance para la UI;
//! - destino: [`serve`] atiende `Begin`/`Ready`/`Cancel` con un `DragIn`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tandem_files::FilesError;
use tandem_files::drag::{
    DragDetector, DragEnd, DragIn, DragOut, DragPeer, DragProgress, DragStarter, Upload,
};
use tandem_proto::{ControlMsg, DragMsg, DragOutcome, DragStatus};
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::agent::Agent;

type Result<T> = std::result::Result<T, FilesError>;

/// Avisos del arrastre que se guardan para una UI lenta.
const STATUS_BUFFER: usize = 16;

/// Reparte los mensajes de arrastre que llegan del otro agente y el avance para la UI.
#[derive(Debug)]
pub struct DragHub {
    status: broadcast::Sender<DragStatus>,
    /// Pedidos del origen, para quien atiende de destino ([`serve`]).
    requests: Mutex<Option<mpsc::UnboundedSender<DragMsg>>>,
    /// Respuestas del destino, para el arrastre de origen en curso.
    replies: Mutex<Option<mpsc::UnboundedSender<DragMsg>>>,
    next_id: AtomicU64,
}

impl Default for DragHub {
    fn default() -> Self {
        DragHub {
            status: broadcast::channel(STATUS_BUFFER).0,
            requests: Mutex::new(None),
            replies: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }
}

impl DragHub {
    /// Avance del arrastre, para la UI.
    pub fn subscribe(&self) -> broadcast::Receiver<DragStatus> {
        self.status.subscribe()
    }

    pub fn publish(&self, status: DragStatus) {
        // Sin UI mirando no pasa nada.
        let _ = self.status.send(status);
    }

    /// Entrega un mensaje del otro agente a quien lo espera; sin nadie, se descarta.
    pub fn deliver(&self, msg: DragMsg) {
        let slot = match msg {
            DragMsg::Begin { .. } | DragMsg::Ready { .. } | DragMsg::Cancel { .. } => {
                &self.requests
            }
            DragMsg::Staged { .. } | DragMsg::Finished { .. } => &self.replies,
        };
        let tx = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tx) = tx.as_ref()
            && tx.send(msg).is_err()
        {
            tracing::debug!("mensaje de arrastre sin quien lo atienda");
        }
    }
}

/// `DragPeer` por el canal de control: cada paso es un mensaje y su respuesta.
pub struct ControlDragPeer<'a> {
    agent: &'a Agent,
    id: u64,
    replies: AsyncMutex<mpsc::UnboundedReceiver<DragMsg>>,
}

impl<'a> ControlDragPeer<'a> {
    /// Un arrastre nuevo; reemplaza al anterior como destino de las respuestas.
    pub fn new(agent: &'a Agent) -> Self {
        let hub = agent.drag();
        let (tx, rx) = mpsc::unbounded_channel();
        *hub.replies.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        ControlDragPeer {
            agent,
            id: hub.next_id.fetch_add(1, Ordering::Relaxed),
            replies: AsyncMutex::new(rx),
        }
    }

    fn send(&self, msg: DragMsg) {
        self.agent.orchestrator().peer().send(ControlMsg::Drag(msg));
    }

    /// La próxima respuesta de este arrastre (las de otros se ignoran).
    async fn reply(&self) -> Result<DragMsg> {
        let mut rx = self.replies.lock().await;
        loop {
            match rx.recv().await {
                Some(msg @ (DragMsg::Staged { id, .. } | DragMsg::Finished { id, .. }))
                    if id == self.id =>
                {
                    return Ok(msg);
                }
                Some(_) => {}
                None => return Err(FilesError::Unexpected),
            }
        }
    }
}

impl DragPeer for ControlDragPeer<'_> {
    async fn begin(&self, names: Vec<String>) -> Result<String> {
        self.send(DragMsg::Begin { id: self.id, names });
        match self.reply().await? {
            DragMsg::Staged { dir, .. } => dir.map_err(FilesError::Remote),
            _ => Err(FilesError::Unexpected),
        }
    }

    async fn ready(&self) -> Result<DragEnd> {
        self.send(DragMsg::Ready { id: self.id });
        match self.reply().await? {
            DragMsg::Finished { end, .. } => end.map(from_outcome).map_err(FilesError::Remote),
            _ => Err(FilesError::Unexpected),
        }
    }

    async fn cancel(&self) -> Result<()> {
        self.send(DragMsg::Cancel { id: self.id });
        Ok(())
    }
}

fn from_outcome(o: DragOutcome) -> DragEnd {
    match o {
        DragOutcome::Dropped => DragEnd::Dropped,
        DragOutcome::Cancelled => DragEnd::Cancelled,
    }
}

fn to_outcome(e: DragEnd) -> DragOutcome {
    match e {
        DragEnd::Dropped => DragOutcome::Dropped,
        DragEnd::Cancelled => DragOutcome::Cancelled,
    }
}

/// Corre el arrastre desde este lado (origen): copia `files` al destino y espera cómo
/// termina allí. El avance y el final van a la UI con [`DragHub::publish`].
pub async fn run_origin<D: DragDetector, U: Upload>(
    agent: &Agent,
    detector: D,
    uploader: U,
    files: &[PathBuf],
    cancelled: impl Future<Output = ()>,
) -> Result<DragEnd> {
    let hub = agent.drag();
    let out = DragOut {
        detector,
        uploader,
        peer: ControlDragPeer::new(agent),
    };
    let total = u32::try_from(files.len()).unwrap_or(u32::MAX);
    let progress = |p: DragProgress| {
        hub.publish(if p.index >= p.total {
            DragStatus::Copied { total }
        } else {
            DragStatus::Copying {
                name: p.name,
                index: u32::try_from(p.index).unwrap_or(u32::MAX),
                total,
            }
        });
    };
    let end = out.run(files, cancelled, &progress).await;
    hub.publish(match &end {
        Ok(DragEnd::Dropped) => DragStatus::Dropped,
        Ok(DragEnd::Cancelled) => DragStatus::Cancelled,
        Err(e) => DragStatus::Failed(e.to_string()),
    });
    end
}

/// Atiende los pedidos del origen (destino): prepara la carpeta, inicia el arrastre
/// sintético y devuelve cómo terminó. Corre hasta que se descarta el agente; se lanza
/// con `tokio::spawn` y el `DragStarter` de la plataforma.
pub async fn serve<S: DragStarter>(agent: &Agent, drag: DragIn<S>) {
    let hub = agent.drag();
    let (tx, mut rx) = mpsc::unbounded_channel();
    *hub.requests.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    let send = |msg| agent.orchestrator().peer().send(ControlMsg::Drag(msg));
    // El arrastre en curso: un `Ready` o `Cancel` de otro id es de uno viejo.
    let mut current: Option<u64> = None;
    while let Some(msg) = rx.recv().await {
        match msg {
            DragMsg::Begin { id, names } => {
                let dir = drag
                    .begin(&names)
                    .map(|d| d.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string());
                current = dir.is_ok().then_some(id);
                send(DragMsg::Staged { id, dir });
            }
            DragMsg::Ready { id } if current == Some(id) => {
                current = None;
                let end = drag
                    .ready()
                    .await
                    .map(to_outcome)
                    .map_err(|e| e.to_string());
                send(DragMsg::Finished { id, end });
            }
            DragMsg::Cancel { id } if current == Some(id) => {
                current = None;
                drag.cancel();
            }
            _ => tracing::debug!("mensaje de arrastre de uno que ya terminó"),
        }
    }
}
