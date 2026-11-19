//! Two agents in the same process, joined by real QUIC over loopback, with simulated
//! steps: power on and off requested through the UI are split between the two machines, and a
//! failure on the other leaves the state as it was.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{AgentToUi, Reply, TxnEvent, UiCommand};
use tandemd::ipc::{IpcClient, mem};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::{Agent, AgentConfig, PeerClient, StateStore, daemon, peer};
use tokio::sync::oneshot;

/// The two identities sorted by key: the first is side A's (DECISIONS A1), so
/// each test knows which side each agent gets.
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

/// Cap so a broken test fails instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(20);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("the test exceeded the maximum time")
}

/// Simulated steps of a machine: they record what they are asked and fail where told.
#[derive(Default)]
struct Fake {
    log: Mutex<Vec<Step>>,
    fail_on: Vec<Step>,
}

impl Fake {
    fn failing(step: Step) -> Self {
        Fake {
            fail_on: vec![step],
            ..Default::default()
        }
    }

    fn steps(&self) -> Vec<Step> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl LocalSteps for Fake {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(step.clone());
            if self.fail_on.contains(step) {
                Err("access denied".to_owned())
            } else {
                Ok(())
            }
        })
    }
}

struct Keys(BTreeSet<PublicKey>);

impl PeerTrust for Keys {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.0.contains(key)
    }
}

struct Desk {
    mac: Arc<Agent>,
    pc: Arc<Agent>,
    mac_steps: Arc<Fake>,
    pc_steps: Arc<Fake>,
    ui: Ui,
    _dir: tempfile::TempDir,
    _link: Link,
}

type Ui = IpcClient<tokio::io::DuplexStream>;

/// Keeps the QUIC endpoints and the IPC daemons alive.
struct Link {
    _endpoints: (Endpoint, Endpoint),
    _stops: Vec<oneshot::Sender<()>>,
}

/// Configuration of `machine` and its store in `dir`, with `initial` saved.
fn setup(
    dir: &std::path::Path,
    machine: Machine,
    initial: &impl Fn(&mut State),
) -> (AgentConfig, StateStore) {
    // With `[power.wol]`: either wakes the other (the steps are simulated).
    let cfg = AgentConfig::from_toml(&format!(
        "machine = \"{machine}\"\n[power.wol]\nmac = \"aa:bb:cc:dd:ee:ff\"\n"
    ))
    .unwrap();
    let store = StateStore::new(dir.join(format!("{machine}.json")));
    let mut s = State::initial(machine);
    initial(&mut s);
    store.save(&s).unwrap();
    (cfg, store)
}

/// Brings up both agents with `initial` saved, joins them by QUIC and connects the UI to the Mac.
async fn desk(initial: impl Fn(&mut State), mac_steps: Fake, pc_steps: Fake) -> Desk {
    let dir = tempfile::tempdir().unwrap();
    let agent = |machine: Machine, steps: Arc<Fake>| {
        let (cfg, store) = setup(dir.path(), machine, &initial);
        Arc::new(Agent::with_local_steps(cfg, store, steps))
    };
    let (mac_steps, pc_steps) = (Arc::new(mac_steps), Arc::new(pc_steps));
    let mac = agent(Machine::A, mac_steps.clone());
    let pc = agent(Machine::B, pc_steps.clone());
    let (ui, _, link) = link(&mac, &pc).await;
    Desk {
        mac,
        pc,
        mac_steps,
        pc_steps,
        ui,
        _dir: dir,
        _link: link,
    }
}

/// Joins the two agents by QUIC over loopback and connects a UI to each (Mac, PC).
async fn link(mac: &Arc<Agent>, pc: &Arc<Agent>) -> (Ui, Ui, Link) {
    // Paired identities and QUIC over loopback.
    let (mac_id, pc_id) = sides(Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let trust = |id: &Identity| Arc::new(Keys(BTreeSet::from([id.public_key()])));
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mac_ep = Endpoint::bind(loopback, &mac_id, trust(&pc_id)).unwrap();
    let pc_ep = Endpoint::bind(loopback, &pc_id, trust(&mac_id)).unwrap();
    let (to_pc, to_mac) =
        within(async { tokio::join!(mac_ep.connect(pc_ep.local_addr().unwrap()), pc_ep.accept()) })
            .await;
    let (to_pc, to_mac) = (to_pc.unwrap(), to_mac.unwrap().unwrap());

    // Each agent serves the other's streams and opens its own to ask for steps.
    tokio::spawn(peer::accept_streams(to_pc.clone(), mac.clone()));
    tokio::spawn(peer::accept_streams(to_mac.clone(), pc.clone()));
    let (mac_client, pc_client) = within(async {
        tokio::join!(
            PeerClient::open(&to_pc, Machine::A),
            PeerClient::open(&to_mac, Machine::B)
        )
    })
    .await;
    mac.set_peer(Arc::new(mac_client.unwrap()));
    pc.set_peer(Arc::new(pc_client.unwrap()));

    // A UI on each machine, over the in-memory IPC.
    let mut stops = Vec::new();
    let mut ui = async |agent: &Arc<Agent>| {
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        stops.push(stop);
        tokio::spawn(daemon::run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap()
    };
    let ui_mac = ui(mac).await;
    let ui_pc = ui(pc).await;
    (
        ui_mac,
        ui_pc,
        Link {
            _endpoints: (mac_ep, pc_ep),
            _stops: stops,
        },
    )
}

/// Asks to power `machine` on or off through the UI and gathers the transaction's notices until its
/// outcome.
async fn power(ui: &mut Ui, machine: Machine, on: bool) -> Vec<TxnEvent> {
    let reply = within(ui.request(UiCommand::SetPower { machine, on }))
        .await
        .unwrap();
    let Reply::Accepted { txn } = reply else {
        panic!("Accepted was expected: {reply:?}");
    };
    let mut events = Vec::new();
    loop {
        match within(ui.next_event()).await.unwrap() {
            Some(AgentToUi::Txn(e)) => {
                let end = matches!(
                    e,
                    TxnEvent::Committed { txn: t, .. } | TxnEvent::Failed { txn: t, .. } if t == txn
                );
                events.push(e);
                if end {
                    return events;
                }
            }
            // The other machine's capabilities announcement arrives on connecting (T-084), and with
            // it both machines are noted in the desk (T-097). Also its version (T-122).
            Some(
                AgentToUi::State(_)
                | AgentToUi::Machines { .. }
                | AgentToUi::Desk(_)
                | AgentToUi::Update(_),
            ) => {}
            other => panic!("unexpected notice: {other:?}"),
        }
    }
}

/// Waits for the agent to reach `want` (the other agent receives the final state separately).
async fn reaches(agent: &Agent, want: &State) {
    let mut rx = agent.subscribe();
    within(rx.wait_for(|s| s == want)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_on_the_pc_end_to_end() {
    // The Mac believes it off: it sends the Wake-on-LAN and waits for its agent to answer (it
    // is already connected).
    let mut d = desk(|s| s.on.b = false, Fake::default(), Fake::default()).await;
    let events = power(&mut d.ui, Machine::B, true).await;
    let want = d.mac.state();
    assert!(want.on.b);
    assert!(matches!(events.last(), Some(TxnEvent::Committed { state, .. }) if *state == want));
    assert_eq!(
        d.mac_steps.steps(),
        [Step::PowerOn {
            machine: Machine::B
        }]
    );
    assert!(d.pc_steps.steps().is_empty());
    // The PC learns the final state.
    reaches(&d.pc, &want).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_off_the_pc_runs_on_the_pc() {
    let mut d = desk(|s| s.on.b = true, Fake::default(), Fake::default()).await;
    let events = power(&mut d.ui, Machine::B, false).await;
    assert!(matches!(events.last(), Some(TxnEvent::Committed { .. })));
    assert!(!d.mac.state().on.b);
    assert_eq!(
        d.pc_steps.steps(),
        [Step::PowerOff {
            machine: Machine::B
        }]
    );
    assert!(d.mac_steps.steps().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_failure_leaves_the_state_as_it_was() {
    let off = Step::PowerOff {
        machine: Machine::B,
    };
    let mut d = desk(
        |s| s.on.b = true,
        Fake::default(),
        Fake::failing(off.clone()),
    )
    .await;
    let initial = d.mac.state();
    let events = power(&mut d.ui, Machine::B, false).await;
    let n = events.len();
    assert_eq!(
        events[n - 2..],
        [
            TxnEvent::StepFailed {
                txn: 1,
                index: 0,
                error: "machine B: access denied".into()
            },
            TxnEvent::Failed {
                txn: 1,
                state: initial.clone()
            },
        ]
    );
    assert_eq!(d.mac.state(), initial);
    assert_eq!(d.pc_steps.steps(), [off]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pc_also_orchestrates() {
    // The same protocol in the other direction: the PC asks the Mac to shut down.
    let d = desk(|s| s.on.a = true, Fake::default(), Fake::default()).await;
    let mut rx = d.pc.subscribe_txns();
    assert!(matches!(
        d.pc.handle(UiCommand::SetPower {
            machine: Machine::A,
            on: false
        }),
        Reply::Accepted { .. }
    ));
    loop {
        match within(rx.recv()).await.unwrap() {
            TxnEvent::Committed { state, .. } => {
                assert!(!state.on.a);
                break;
            }
            TxnEvent::Failed { .. } => panic!("failed"),
            _ => {}
        }
    }
    assert_eq!(
        d.mac_steps.steps(),
        [Step::PowerOff {
            machine: Machine::A
        }]
    );
}
