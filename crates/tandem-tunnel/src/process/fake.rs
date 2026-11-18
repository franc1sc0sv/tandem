//! El doble de prueba de [`Spawner`]: los «procesos» los maneja la prueba (escribe su
//! salida, los hace terminar) y ve qué se le pidió lanzar y si lo mataron.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{mpsc, watch};

use super::{Child, Launch, SpawnError, Spawned, Spawner};
use crate::wire::BoxFuture;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lanza procesos simulados; cada uno queda en [`FakeSpawner::processes`].
#[derive(Clone, Default)]
pub struct FakeSpawner {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    spawned: Vec<Arc<FakeProcess>>,
    fail: Option<String>,
}

impl FakeSpawner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Desde ahora, lanzar falla con este mensaje (`None`: vuelve a funcionar).
    pub fn fail_with(&self, why: Option<&str>) {
        lock(&self.state).fail = why.map(str::to_owned);
    }

    /// Los procesos lanzados, en orden.
    pub fn processes(&self) -> Vec<Arc<FakeProcess>> {
        lock(&self.state).spawned.clone()
    }
}

impl Spawner for FakeSpawner {
    fn spawn(&self, launch: &Launch) -> Result<Spawned, SpawnError> {
        let mut state = lock(&self.state);
        if let Some(why) = &state.fail {
            return Err(SpawnError(why.clone()));
        }
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (err_tx, err_rx) = mpsc::unbounded_channel();
        let pid = 1000 + u32::try_from(state.spawned.len()).unwrap_or(0);
        let process = Arc::new(FakeProcess {
            launch: launch.clone(),
            pid,
            stdout: Mutex::new(Some(out_tx)),
            stderr: Mutex::new(Some(err_tx)),
            exit: watch::Sender::new(None),
            killed: AtomicBool::new(false),
            released: AtomicBool::new(false),
        });
        state.spawned.push(process.clone());
        Ok(Spawned {
            pid: Some(pid),
            stdout: Box::new(Pipe::new(out_rx)),
            stderr: Box::new(Pipe::new(err_rx)),
            child: Arc::new(FakeChild(process)),
        })
    }
}

/// Un proceso simulado.
pub struct FakeProcess {
    /// Lo que se pidió lanzar.
    pub launch: Launch,
    pub pid: u32,
    stdout: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    stderr: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    exit: watch::Sender<Option<Option<i32>>>,
    killed: AtomicBool,
    released: AtomicBool,
}

impl FakeProcess {
    pub fn stdout(&self, bytes: &[u8]) {
        if let Some(tx) = &*lock(&self.stdout) {
            let _ = tx.send(bytes.to_vec());
        }
    }

    pub fn stderr(&self, bytes: &[u8]) {
        if let Some(tx) = &*lock(&self.stderr) {
            let _ = tx.send(bytes.to_vec());
        }
    }

    /// Cierra las salidas sin terminar el proceso.
    pub fn close_output(&self) {
        lock(&self.stdout).take();
        lock(&self.stderr).take();
    }

    /// Termina con ese código y cierra sus salidas.
    pub fn exit(&self, code: i32) {
        self.exit_keeping_output(code);
        self.close_output();
    }

    /// Termina pero deja las salidas abiertas, como un proceso que dejó un hijo vivo.
    pub fn exit_keeping_output(&self, code: i32) {
        self.exit.send_if_modified(|e| {
            let first = e.is_none();
            if first {
                *e = Some(Some(code));
            }
            first
        });
    }

    /// Lo mataron con `kill_tree` (por `kill` o por el cierre del túnel).
    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// El túnel ya lo dio por terminado del todo (`release`).
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    pub fn has_exited(&self) -> bool {
        self.exit.borrow().is_some()
    }
}

struct FakeChild(Arc<FakeProcess>);

impl Child for FakeChild {
    fn wait(&self) -> BoxFuture<'_, Option<i32>> {
        let mut rx = self.0.exit.subscribe();
        Box::pin(async move {
            match rx.wait_for(Option::is_some).await {
                Ok(v) => (*v).flatten(),
                Err(_) => None,
            }
        })
    }

    fn kill_tree(&self) {
        self.0.killed.store(true, Ordering::SeqCst);
        self.0.close_output();
        self.0.exit.send_if_modified(|e| {
            let first = e.is_none();
            if first {
                *e = Some(None);
            }
            first
        });
    }

    fn release(&self) {
        self.0.released.store(true, Ordering::SeqCst);
    }
}

/// Una salida que se lee como un archivo: termina cuando se cierra el extremo que escribe.
struct Pipe {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pending: Vec<u8>,
    at: usize,
}

impl Pipe {
    fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Pipe {
            rx,
            pending: Vec::new(),
            at: 0,
        }
    }
}

impl AsyncRead for Pipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.at < self.pending.len() {
                let n = buf.remaining().min(self.pending.len() - self.at);
                let at = self.at;
                buf.put_slice(&self.pending[at..at + n]);
                self.at += n;
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.pending = chunk;
                    self.at = 0;
                }
                // Cerrado: fin del archivo.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
