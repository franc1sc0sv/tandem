//! File drag between two real `tandemd`s over loopback (T-121): the source asks for the
//! temporary folder over the control channel, copies, and the destination starts the synthetic
//! drag (here a double) and says how it ended. If canceled, the destination deletes everything.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_files::FilesError;
use tandem_files::drag::{DragDetector, DragEnd, DragIn, DragStarter, Upload};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{DragStatus, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::drag;
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::{Agent, AgentConfig, StoreError, autoconfig};
use tokio::io::DuplexStream;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Says where it waited, to know which step hung (T-230).
#[track_caller]
fn within<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let at = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(TIMEOUT, f).await {
            Ok(v) => v,
            Err(_) => panic!("the test exceeded the maximum time waiting at {at}"),
        }
    }
}

/// The first is side A's (DECISIONS A1).
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

struct Ok_;

impl LocalSteps for Ok_ {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

struct Running {
    agent: Arc<Agent>,
    ui: IpcClient<DuplexStream>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), StoreError>>,
    _dir: tempfile::TempDir,
}

impl Running {
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        within(self.task).await.unwrap().unwrap();
    }

    /// Waits for there to be a link with the other machine.
    async fn linked(&mut self) {
        within(async {
            loop {
                if let Reply::Capabilities(c) =
                    self.ui.request(UiCommand::GetCapabilities).await.unwrap()
                    && c.peer.is_some()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }
}

async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
) -> Result<Running, PortTaken> {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let mut trust = TrustStore::open(paths.trust_file()).unwrap();
    trust
        .add_and_save(TrustedPeer {
            public_key: peer.0,
            name: machine.other().to_string(),
        })
        .unwrap();
    let text = autoconfig::generate(machine).replace(
        &format!("port = {}", tandemd::config::DEFAULT_PORT),
        &format!(
            "port = {port}\npeers = [{{ kind = \"ethernet\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::load(&paths.config).unwrap();
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            input: None,
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            fallback_ports: Vec::new(),
            health: common::health(),
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = port_taken(started).unwrap()?;
    let shared = agent.clone();
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(
        Daemon {
            agent,
            network,
            input,
        }
        .serve(listener, async {
            let _ = stopped.await;
        }),
    );
    let ui = IpcClient::handshake(connector.connect().unwrap())
        .await
        .unwrap();
    Ok(Running {
        agent: shared,
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

async fn pair() -> (Running, Running) {
    let (a_id, b_id) = sides(Identity::from_seed([61; 32]), Identity::from_seed([62; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    a.linked().await;
    b.linked().await;
    (a, b)
}

struct NoDetector;

impl DragDetector for NoDetector {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        None
    }
}

/// Copies through the local disk (both "machines" share a file system); if
/// `gate` is there, it waits for it to open before copying the `slow` file.
struct CopyUpload {
    slow: Option<(String, Arc<Notify>)>,
}

impl Upload for CopyUpload {
    async fn upload(&self, local: &Path, remote: &str) -> Result<(), FilesError> {
        if let Some((name, gate)) = &self.slow
            && local.file_name().is_some_and(|n| n.to_str() == Some(name))
        {
            gate.notified().await;
        }
        std::fs::copy(local, remote)
            .map(|_| ())
            .map_err(|e| FilesError::io(local, e))
    }
}

type Seen = Arc<Mutex<Vec<(PathBuf, Vec<u8>)>>>;

/// Double of the synthetic drag: reads what the destination left and "drops".
#[derive(Clone)]
struct DropStarter {
    seen: Seen,
    end: DragEnd,
}

impl DragStarter for DropStarter {
    async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd, FilesError> {
        let mut seen = self.seen.lock().unwrap();
        for p in staged {
            let bytes = std::fs::read(&p).map_err(|e| FilesError::io(&p, e))?;
            seen.push((p, bytes));
        }
        Ok(self.end)
    }
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    ["one.txt", "two.bin"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            std::fs::write(&p, format!("content of {n}")).unwrap();
            p
        })
        .collect()
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<DragStatus>) -> Vec<DragStatus> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_drag_copies_to_the_destination_and_returns_how_it_ended() {
    let (a, b) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    let files = files_in(src.path());
    let starter = DropStarter {
        seen: Arc::default(),
        end: DragEnd::Dropped,
    };
    let seen = starter.seen.clone();
    let dest_agent = b.agent.clone();
    let base = staging.path().to_owned();
    let server = tokio::spawn(async move {
        drag::serve(&dest_agent, DragIn::new(starter, base)).await;
    });
    let mut status = a.agent.drag().subscribe();

    let end = within(drag::run_origin(
        &a.agent,
        NoDetector,
        CopyUpload { slow: None },
        &files,
        std::future::pending(),
    ))
    .await
    .unwrap();

    assert_eq!(end, DragEnd::Dropped);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].1, b"content of one.txt");
    assert_eq!(seen[1].1, b"content of two.bin");
    // When finished the destination leaves nothing in its temporary folder.
    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
    let name = |i: u32, n: &str| DragStatus::Copying {
        name: n.into(),
        index: i,
        total: 2,
    };
    assert_eq!(
        drain(&mut status),
        vec![
            name(0, "one.txt"),
            name(1, "two.bin"),
            DragStatus::Copied { total: 2 },
            DragStatus::Dropped,
        ]
    );
    server.abort();
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_midway_deletes_what_was_copied_on_the_destination() {
    let (a, b) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    let files = files_in(src.path());
    let starter = DropStarter {
        seen: Arc::default(),
        end: DragEnd::Dropped,
    };
    let seen = starter.seen.clone();
    let dest_agent = b.agent.clone();
    let base = staging.path().to_owned();
    let server = tokio::spawn(async move {
        drag::serve(&dest_agent, DragIn::new(starter, base)).await;
    });
    let mut status = a.agent.drag().subscribe();

    // The second file is left waiting; meanwhile the user presses Esc.
    let gate = Arc::new(Notify::new());
    let cancel = Arc::new(Notify::new());
    let esc = cancel.clone();
    let origin_agent = a.agent.clone();
    let slow = CopyUpload {
        slow: Some(("two.bin".into(), gate)),
    };
    let run = tokio::spawn(async move {
        drag::run_origin(&origin_agent, NoDetector, slow, &files, async move {
            cancel.notified().await;
        })
        .await
    });
    // With the first already copied in the destination's folder.
    within(async {
        loop {
            let copied = std::fs::read_dir(staging.path())
                .into_iter()
                .flatten()
                .flatten()
                .any(|d| d.path().join("one.txt").exists());
            if copied {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    esc.notify_one();

    assert_eq!(within(run).await.unwrap().unwrap(), DragEnd::Cancelled);
    within(async {
        while std::fs::read_dir(staging.path()).unwrap().count() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(seen.lock().unwrap().is_empty(), "it never got to drag");
    assert_eq!(drain(&mut status).last(), Some(&DragStatus::Cancelled));
    server.abort();
    a.stop().await;
    b.stop().await;
}

/// T-230: `serve` runs in a spawned task, so a `Begin` sent right after the link comes up
/// may arrive before it registers. Before the fix it was discarded and the source waited
/// for `Staged` forever; here the destination starts serving only after the request arrived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_begin_that_arrives_before_the_destination_serves_is_not_lost() {
    let (a, b) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    let files = files_in(src.path());
    let starter = DropStarter {
        seen: Arc::default(),
        end: DragEnd::Dropped,
    };
    let seen = starter.seen.clone();
    let origin_agent = a.agent.clone();
    let run = tokio::spawn(async move {
        drag::run_origin(
            &origin_agent,
            NoDetector,
            CopyUpload { slow: None },
            &files,
            std::future::pending(),
        )
        .await
    });
    // The request crosses the loopback link in milliseconds; this leaves it waiting at
    // the destination well before anyone serves there.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dest_agent = b.agent.clone();
    let base = staging.path().to_owned();
    let server = tokio::spawn(async move {
        drag::serve(&dest_agent, DragIn::new(starter, base)).await;
    });

    assert_eq!(within(run).await.unwrap().unwrap(), DragEnd::Dropped);
    assert_eq!(seen.lock().unwrap().len(), 2);
    server.abort();
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_destination_rejects_names_that_escape_its_folder() {
    let (a, b) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    let starter = DropStarter {
        seen: Arc::default(),
        end: DragEnd::Dropped,
    };
    let dest_agent = b.agent.clone();
    let base = staging.path().to_owned();
    let server = tokio::spawn(async move {
        drag::serve(&dest_agent, DragIn::new(starter, base)).await;
    });
    let mut status = a.agent.drag().subscribe();

    // The source already filters the names; here the raw request is sent to test the destination.
    let peer = drag::ControlDragPeer::new(&a.agent);
    let err = tandem_files::drag::DragPeer::begin(&peer, vec!["..".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, FilesError::Remote(_)), "{err}");
    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
    assert!(drain(&mut status).is_empty());
    let _ = src;
    server.abort();
    a.stop().await;
    b.stop().await;
}
