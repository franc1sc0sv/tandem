//! File drag between the machines over the control channel (D14, T-121).
//!
//! The flow's logic lives in `tandem_files::drag`; here it is connected to the link:
//! - source: [`ControlDragPeer`] implements `DragPeer` with the control [`DragMsg`]s and
//!   [`run_origin`] runs `DragOut`, publishing progress for the UI;
//! - destination: [`serve`] handles `Begin`/`Ready`/`Cancel` with a `DragIn`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tandem_files::FilesError;
use tandem_files::drag::{
    DragDetector, DragEnd, DragIn, DragOut, DragPeer, DragProgress, DragStarter, Upload,
};
use tandem_files::drag_os::{PxRect, StripEdge};
use tandem_proto::{Client, ClientKind, ControlMsg, DragMsg, DragOutcome, DragStatus, TunnelId};
use tandem_tunnel::{BoxFuture, OpenSpec};
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::agent::Agent;

type Result<T> = std::result::Result<T, FilesError>;

/// Drag notices kept for a slow UI.
const STATUS_BUFFER: usize = 16;

/// Requests kept while nobody serves as destination yet ([`serve`] is spawned and the other
/// agent may already be sending); a drag needs three at most.
const EARLY_REQUESTS: usize = 8;

/// Where the source's requests go.
#[derive(Debug)]
enum Requests {
    /// Nobody serves yet: they wait here instead of being lost, so a source that
    /// sends `Begin` right after the link comes up does not wait forever for `Staged` (T-230).
    Early(VecDeque<DragMsg>),
    Serving(mpsc::UnboundedSender<DragMsg>),
}

/// Distributes the drag messages that arrive from the other agent and the progress for the UI.
#[derive(Debug)]
pub struct DragHub {
    status: broadcast::Sender<DragStatus>,
    /// Requests from the source, for whoever serves as destination ([`serve`]).
    requests: Mutex<Requests>,
    /// Replies from the destination, for the source drag in progress.
    replies: Mutex<Option<mpsc::UnboundedSender<DragMsg>>>,
    next_id: AtomicU64,
}

impl Default for DragHub {
    fn default() -> Self {
        DragHub {
            status: broadcast::channel(STATUS_BUFFER).0,
            requests: Mutex::new(Requests::Early(VecDeque::new())),
            replies: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }
}

impl DragHub {
    /// Drag progress, for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<DragStatus> {
        self.status.subscribe()
    }

    pub fn publish(&self, status: DragStatus) {
        // With no UI watching nothing happens.
        let _ = self.status.send(status);
    }

    /// Delivers a message from the other agent to whoever waits for it. A reply with no
    /// drag waiting is discarded; a request with no destination yet waits for [`serve`].
    pub fn deliver(&self, msg: DragMsg) {
        match msg {
            DragMsg::Begin { .. } | DragMsg::Ready { .. } | DragMsg::Cancel { .. } => {
                let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
                match &mut *requests {
                    Requests::Serving(tx) => {
                        if tx.send(msg).is_err() {
                            tracing::debug!("drag request with nobody to handle it");
                        }
                    }
                    Requests::Early(queue) => {
                        if queue.len() == EARLY_REQUESTS {
                            queue.pop_front();
                        }
                        queue.push_back(msg);
                    }
                }
            }
            DragMsg::Staged { .. } | DragMsg::Finished { .. } => {
                let tx = self.replies.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(tx) = tx.as_ref()
                    && tx.send(msg).is_err()
                {
                    tracing::debug!("drag reply with nobody to handle it");
                }
            }
        }
    }

    /// Becomes the destination of the requests, with those that arrived before.
    fn serve(&self) -> mpsc::UnboundedReceiver<DragMsg> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        if let Requests::Early(queue) = &mut *requests {
            for msg in queue.drain(..) {
                let _ = tx.send(msg);
            }
        }
        *requests = Requests::Serving(tx);
        rx
    }
}

/// `DragPeer` over the control channel: each step is a message and its reply.
pub struct ControlDragPeer<'a> {
    agent: &'a Agent,
    id: u64,
    replies: AsyncMutex<mpsc::UnboundedReceiver<DragMsg>>,
}

impl<'a> ControlDragPeer<'a> {
    /// A new drag; it replaces the previous one as the destination of replies.
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

    /// The next reply of this drag (those of others are ignored).
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

/// Runs the drag from this side (source): copies `files` to the destination and waits for how
/// it ends there. Progress and the end go to the UI with [`DragHub::publish`].
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

/// Handles the source's requests (destination): prepares the folder, starts the synthetic
/// drag and returns how it ended. It runs until the agent is dropped; it is launched
/// with `tokio::spawn` and the platform's `DragStarter`.
pub async fn serve<S: DragStarter>(agent: &Agent, drag: DragIn<S>) {
    let mut rx = agent.drag().serve();
    let send = |msg| agent.orchestrator().peer().send(ControlMsg::Drag(msg));
    // The drag in progress: a `Ready` or `Cancel` of another id belongs to an old one.
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
            _ => tracing::debug!("drag message from one that already ended"),
        }
    }
}

/// What the input engine needs from the system for source file drags
/// (T-121): detecting the drag, knowing where the edge is and uploading the files.
/// It is an object trait so the engine receives it in [`crate::input::InputSetup`]; tests
/// give it with a double.
pub trait FileDragBackend: Send + Sync {
    /// The paths being dragged, or `None` if there is no file drag.
    fn file_drag(&self) -> Option<Vec<PathBuf>>;

    /// Where the crossing edge is (the detector's strip); `None` if it cannot be crossed.
    /// Detectors that do not need a window (macOS reads the drag pasteboard) leave it
    /// ignore it.
    fn set_edge(&self, _edge: Option<(PxRect, StripEdge)>) {}

    /// Copies `local` to `remote` on the other machine.
    fn upload<'a>(
        &'a self,
        agent: &'a Agent,
        local: &'a Path,
        remote: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// The drag ended (dropped, canceled or with an error): releases what `upload` opened.
    fn finish<'a>(&'a self, _agent: &'a Agent) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Uploads files through a tunnel the agent itself opens at the first file and closes at
/// the end of the drag.
#[derive(Debug, Default)]
pub struct TunnelUploader {
    tunnel: AsyncMutex<Option<TunnelId>>,
}

impl TunnelUploader {
    pub async fn upload(&self, agent: &Agent, local: &Path, remote: &str) -> Result<()> {
        let id = {
            let mut slot = self.tunnel.lock().await;
            match *slot {
                Some(id) => id,
                None => {
                    let spec = OpenSpec {
                        title: "File drag".into(),
                        project: None,
                        origin: agent.machine(),
                        target: agent.machine().other(),
                        opened_by: Client {
                            kind: ClientKind::Cli,
                            session: "drag".into(),
                        },
                    };
                    let t = agent
                        .tunnels()
                        .origin
                        .open(spec)
                        .await
                        .map_err(|e| FilesError::Remote(e.to_string()))?;
                    *slot = Some(t.id);
                    t.id
                }
            }
        };
        let up = tandem_files::drag::TunnelUpload {
            files: agent.files(),
            origin: &agent.tunnels().origin,
            tunnel: id,
        };
        up.upload(local, remote).await
    }

    pub async fn finish(&self, agent: &Agent) {
        if let Some(id) = self.tunnel.lock().await.take()
            && let Err(e) = agent.tunnels().origin.close(id).await
        {
            tracing::debug!("could not close the drag tunnel: {e}");
        }
    }
}

/// How to move the detector's strip.
type EdgeFn = Box<dyn Fn(Option<(PxRect, StripEdge)>) + Send + Sync>;

/// [`FileDragBackend`] with a system [`DragDetector`] and the agent's tunnel.
pub struct SystemFileDrag<D> {
    detector: D,
    uploads: TunnelUploader,
    edge: Option<EdgeFn>,
}

impl<D: DragDetector + Send + Sync> SystemFileDrag<D> {
    pub fn new(detector: D) -> Self {
        SystemFileDrag {
            detector,
            uploads: TunnelUploader::default(),
            edge: None,
        }
    }

    /// How to move the detector's strip when the crossing edge changes.
    pub fn with_edge(
        mut self,
        f: impl Fn(Option<(PxRect, StripEdge)>) + Send + Sync + 'static,
    ) -> Self {
        self.edge = Some(Box::new(f));
        self
    }
}

impl<D: DragDetector + Send + Sync> FileDragBackend for SystemFileDrag<D> {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        self.detector.file_drag()
    }

    fn set_edge(&self, edge: Option<(PxRect, StripEdge)>) {
        if let Some(f) = &self.edge {
            f(edge);
        }
    }

    fn upload<'a>(
        &'a self,
        agent: &'a Agent,
        local: &'a Path,
        remote: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.uploads.upload(agent, local, remote))
    }

    fn finish<'a>(&'a self, agent: &'a Agent) -> BoxFuture<'a, ()> {
        Box::pin(self.uploads.finish(agent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(id: u64) -> DragMsg {
        DragMsg::Begin {
            id,
            names: vec!["a.txt".into()],
        }
    }

    fn ids(rx: &mut mpsc::UnboundedReceiver<DragMsg>) -> Vec<u64> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| match m {
                DragMsg::Begin { id, .. } | DragMsg::Ready { id } | DragMsg::Cancel { id } => id,
                _ => u64::MAX,
            })
            .collect()
    }

    #[test]
    fn requests_before_serving_are_handed_over_in_order() {
        let hub = DragHub::default();
        hub.deliver(begin(1));
        hub.deliver(DragMsg::Ready { id: 1 });
        let mut rx = hub.serve();
        hub.deliver(DragMsg::Cancel { id: 2 });
        assert_eq!(ids(&mut rx), vec![1, 1, 2]);
    }

    #[test]
    fn the_early_queue_keeps_only_the_newest() {
        let hub = DragHub::default();
        let total = EARLY_REQUESTS as u64 + 3;
        for id in 0..total {
            hub.deliver(begin(id));
        }
        let mut rx = hub.serve();
        let got = ids(&mut rx);
        assert_eq!(got.len(), EARLY_REQUESTS);
        assert_eq!(got.first(), Some(&3));
        assert_eq!(got.last(), Some(&(total - 1)));
    }

    #[test]
    fn a_reply_without_a_drag_waiting_is_discarded() {
        let hub = DragHub::default();
        hub.deliver(DragMsg::Staged {
            id: 1,
            dir: Ok("x".into()),
        });
        let mut rx = hub.serve();
        assert!(ids(&mut rx).is_empty());
    }
}
