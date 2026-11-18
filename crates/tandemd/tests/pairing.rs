//! Pairing from the UI between two `tandemd`s built with [`daemon::start`], without
//! prior trust: real QUIC over loopback, each looks for the other over the config's fixed link (and
//! waits on its own) and each UI speaks over the in-memory IPC.
//!
//! The machines are called "Mac" and "PC" (their `[machines]`, as the UI would set them); the code
//! does not assume it: the sides come from the keys.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Machine;
use tandem_input::geometry::{Rect, Screen};
use tandem_input::session::SessionConfig;
use tandem_link::identity::{Identity, TrustStore};
use tandem_proto::{
    AgentToUi, InputStatus, Link, PairingStatus, PeerStatus, Reply, TxnEvent, UiCommand,
};
use tandemd::agent::NO_NETWORK;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::input::fake::FakeInputBackends;
use tandemd::input::{InputOptions, InputSetup};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::{NOT_PAIRED, NOTHING_TO_CONFIRM, NetOptions, PAIRING_IN_PROGRESS};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::pairing::PairingTimeouts;
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, free_pair, port_taken};
use tandemd::{Agent, AgentConfig, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(30);

async fn within<F: Future>(f: F) -> F::Output {
    within_for("an unlabeled wait", f).await
}

/// Same as [`within`], but the timeout message says what was being waited for.
async fn within_for<F: Future>(what: &str, f: F) -> F::Output {
    match tokio::time::timeout(TIMEOUT, f).await {
        Ok(out) => out,
        Err(_) => panic!("the test exceeded the maximum time waiting for: {what}"),
    }
}

/// Simulated steps that always succeed.
struct Ok_;

impl LocalSteps for Ok_ {
    fn execute<'a>(&'a self, _step: &'a tandem_core::Step) -> StepFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// The two identities sorted by key: the first is side A's (DECISIONS A1).
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

/// The MAC each machine announces in these tests (T-159).
fn mac_of(name: &str) -> &'static str {
    if name == "Mac" {
        "02:00:00:00:00:01"
    } else {
        "02:00:00:00:00:02"
    }
}

/// The name of each side in these tests.
fn name_of(machine: Machine) -> &'static str {
    match machine {
        Machine::A => "Mac",
        Machine::B => "PC",
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

struct Running {
    agent: Arc<Agent>,
    ui: IpcClient<tokio::io::DuplexStream>,
    paths: Paths,
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

    async fn ask(&mut self, cmd: UiCommand) -> Reply {
        within(self.ui.request(cmd)).await.unwrap()
    }

    /// Waits for the pairing notice that satisfies `done` (the others are skipped).
    async fn until_pairing(&mut self, done: impl Fn(&PairingStatus) -> bool) -> PairingStatus {
        within(async {
            loop {
                if let AgentToUi::Pairing(s) = self.ui.next_event().await.unwrap().unwrap() {
                    // A failure is not fixed by waiting: it is reported with its reason instead of
                    // letting the test time out without saying what happened.
                    if let PairingStatus::Failed { reason } = &s
                        && !done(&s)
                    {
                        panic!("the pairing failed: {reason}");
                    }
                    if done(&s) {
                        break s;
                    }
                }
            }
        })
        .await
    }

    fn trusted(&self, other: &Identity) -> bool {
        TrustStore::open(self.paths.trust_file())
            .unwrap()
            .is_trusted(&other.public_key())
    }
}

/// An agent without a peer: it knows the other's address over the fixed link, nothing more.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer_port: u16,
    timeouts: PairingTimeouts,
) -> Result<Running, PortTaken> {
    start_with_input(
        (machine, name_of(machine)),
        identity,
        (port, peer_port),
        timeouts,
        None,
    )
    .await
}

/// Like [`start`], but the agent starts already trusting `trusts` (a pairing that
/// the other machine no longer remembers).
async fn start_trusting(
    machine: Machine,
    identity: &Identity,
    trusts: &Identity,
    (port, peer_port): (u16, u16),
) -> Result<Running, PortTaken> {
    start_inner(
        (machine, name_of(machine)),
        identity,
        (port, peer_port),
        PairingTimeouts::default(),
        None,
        Some(trusts),
    )
    .await
}

/// Like [`start`], with the input engine running if given backends. `machine` is the
/// side its `tandem.toml` says (the real one is decided by the keys when pairing) and the
/// name it introduces itself with.
async fn start_with_input(
    who: (Machine, &str),
    identity: &Identity,
    ports: (u16, u16),
    timeouts: PairingTimeouts,
    input: Option<InputSetup>,
) -> Result<Running, PortTaken> {
    start_inner(who, identity, ports, timeouts, input, None).await
}

async fn start_inner(
    (machine, name): (Machine, &str),
    identity: &Identity,
    (port, peer_port): (u16, u16),
    timeouts: PairingTimeouts,
    input: Option<InputSetup>,
    trusts: Option<&Identity>,
) -> Result<Running, PortTaken> {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    if let Some(other) = trusts {
        let mut store = TrustStore::open(paths.trust_file()).unwrap();
        store.add(tandem_link::identity::TrustedPeer {
            public_key: other.public_key(),
            name: "other".into(),
        });
        store.save().unwrap();
    }
    let side = machine.as_str();
    let text = format!(
        "machine = \"{side}\"\n[network]\nport = {port}\n\
         peers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n\
         [machines.{side}]\nos = \"windows\"\nname = \"{name}\"\ncolor = \"#7aa7e6\"\n\
         [power]\nmac = \"{}\"\n",
        loopback(peer_port),
        mac_of(name)
    );
    // On disk, as in the binary: the side that results when pairing is saved there.
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::from_toml(&text).unwrap();
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            input: input.map(Box::new),
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            pairing: timeouts,
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = port_taken(started).unwrap()?;
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(
        Daemon {
            agent: agent.clone(),
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
        agent,
        ui,
        paths,
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

struct Desk {
    mac: Running,
    pc: Running,
    mac_id: Identity,
    pc_id: Identity,
}

async fn desk(seed: u8, timeouts: PairingTimeouts) -> Desk {
    let (mac_id, pc_id) = sides(
        Identity::from_seed([seed; 32]),
        Identity::from_seed([seed + 1; 32]),
    );
    let (mac, pc) = tandemd::start_pair!(
        ports: free_pair; |mac_port, pc_port| start(Machine::A, &mac_id, mac_port, pc_port, timeouts),
        start(Machine::B, &pc_id, pc_port, mac_port, timeouts)
    );
    Desk {
        mac,
        pc,
        mac_id,
        pc_id,
    }
}

fn comparing(s: &PairingStatus) -> bool {
    matches!(s, PairingStatus::Comparing { .. })
}

fn code_of(s: &PairingStatus) -> String {
    match s {
        PairingStatus::Comparing { code, .. } => code.clone(),
        other => panic!("a code was expected: {other:?}"),
    }
}

async fn until_input(what: &str, agent: &Agent, f: impl Fn(&InputStatus) -> bool) {
    let mut rx = agent.subscribe_input();
    let seen = within_for(what, rx.wait_for(|s| f(s))).await.unwrap();
    drop(seen);
}

async fn until_peer(agent: &Agent, want: PeerStatus) {
    let mut rx = agent.subscribe_peer();
    within(rx.wait_for(|s| *s == want)).await.unwrap();
}

/// Start on both -> the same code on both UIs -> confirm on both ->
/// paired, saved to disk and connected over the fixed link without restarting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_compare_confirm_and_both_are_paired() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(40, PairingTimeouts::default()).await;
    assert_eq!(mac.agent.peer_status(), PeerStatus::Unpaired);
    // Confirming without a code does nothing.
    assert_eq!(
        mac.ask(UiCommand::ConfirmPairing).await,
        Reply::Rejected(NOTHING_TO_CONFIRM.into())
    );

    // The PC first: the order does not matter, both look for each other until they find each other.
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(
        pc.ask(UiCommand::GetPairing).await,
        Reply::Pairing(PairingStatus::Searching)
    );
    assert_eq!(
        pc.ask(UiCommand::StartPairing).await,
        Reply::Rejected(PAIRING_IN_PROGRESS.into())
    );
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);

    let on_mac = mac.until_pairing(comparing).await;
    let on_pc = pc.until_pairing(comparing).await;
    assert_eq!(code_of(&on_mac), code_of(&on_pc));
    assert_eq!(code_of(&on_mac).len(), 6);
    assert!(
        matches!(&on_mac, PairingStatus::Comparing { peer, confirmed: false, .. } if peer == "PC")
    );
    assert!(
        matches!(&on_pc, PairingStatus::Comparing { peer, confirmed: false, .. } if peer == "Mac")
    );

    // Confirming on just one does not pair anyone yet.
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let waiting = mac
        .until_pairing(|s| {
            matches!(
                s,
                PairingStatus::Comparing {
                    confirmed: true,
                    ..
                }
            )
        })
        .await;
    assert_eq!(code_of(&waiting), code_of(&on_mac));
    assert!(!mac.trusted(&pc_id) && !pc.trusted(&mac_id));

    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let done = |s: &PairingStatus| matches!(s, PairingStatus::Done { .. });
    assert_eq!(
        pc.until_pairing(done).await,
        PairingStatus::Done { peer: "Mac".into() }
    );
    assert_eq!(
        mac.until_pairing(done).await,
        PairingStatus::Done { peer: "PC".into() }
    );
    assert!(mac.trusted(&pc_id));
    assert!(pc.trusted(&mac_id));

    // Without restarting, each follows the other over the fixed link and the state changes.
    until_peer(
        &mac.agent,
        PeerStatus::Paired {
            name: "PC".into(),
            link: Some(Link::Usb4),
        },
    )
    .await;
    until_peer(
        &pc.agent,
        PeerStatus::Paired {
            name: "Mac".into(),
            link: Some(Link::Usb4),
        },
    )
    .await;
    let mut st = mac.agent.subscribe();
    within(st.wait_for(|s| s.on.b)).await.unwrap();
    let txn = within(async {
        loop {
            match mac
                .ui
                .request(UiCommand::SetPower {
                    machine: Machine::B,
                    on: false,
                })
                .await
                .unwrap()
            {
                Reply::Accepted { txn } => break txn,
                Reply::Rejected(why) if why == tandemd::agent::BUSY => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("Accepted was expected: {other:?}"),
            }
        }
    })
    .await;
    within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, .. }) if t == txn => break,
                AgentToUi::Txn(e @ TxnEvent::Failed { .. }) => panic!("the shutdown failed: {e:?}"),
                _ => {}
            }
        }
    })
    .await;

    // Already paired, it is not paired again without unpairing.
    assert!(matches!(
        mac.ask(UiCommand::StartPairing).await,
        Reply::Rejected(why) if why.contains("unpair it")
    ));
    pc.stop().await;
    mac.stop().await;
}

/// Rejecting on one machine (even if the other already confirmed) leaves both without a peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejecting_on_one_side_leaves_both_unpaired() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(50, PairingTimeouts::default()).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;

    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(mac.ask(UiCommand::CancelPairing).await, Reply::Done);

    // Whoever rejected goes back to rest; the other finds out why.
    assert_eq!(
        mac.until_pairing(|s| !comparing(s)).await,
        PairingStatus::Idle
    );
    let failed = pc
        .until_pairing(|s| matches!(s, PairingStatus::Failed { .. }))
        .await;
    assert!(
        matches!(&failed, PairingStatus::Failed { reason } if reason.contains("rejected at Mac")),
        "{failed:?}"
    );
    assert!(!mac.trusted(&pc_id));
    assert!(!pc.trusted(&mac_id));
    assert_eq!(mac.agent.peer_status(), PeerStatus::Unpaired);
    assert_eq!(pc.agent.peer_status(), PeerStatus::Unpaired);

    // Closing the failure notice leaves it at rest, and it can be tried again.
    assert_eq!(pc.ask(UiCommand::CancelPairing).await, Reply::Done);
    assert_eq!(pc.agent.pairing_status(), PairingStatus::Idle);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    pc.stop().await;
    mac.stop().await;
}

/// If the other machine never opens pairing, it is reported and nothing is left half done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn searching_alone_gives_up() {
    let timeouts = PairingTimeouts {
        search: Duration::from_millis(1500),
        ..PairingTimeouts::default()
    };
    let Desk {
        mut mac, mut pc, ..
    } = desk(60, timeouts).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    let failed = mac
        .until_pairing(|s| matches!(s, PairingStatus::Failed { .. }))
        .await;
    assert!(
        matches!(&failed, PairingStatus::Failed { reason } if reason.contains("the other machine did not appear")),
        "{failed:?}"
    );
    // The PC never found out.
    assert_eq!(
        pc.ask(UiCommand::GetPairing).await,
        Reply::Pairing(PairingStatus::Idle)
    );
    pc.stop().await;
    mac.stop().await;
}

/// Canceling while searching goes back to rest right away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_search() {
    let Desk { mut mac, pc, .. } = desk(70, PairingTimeouts::default()).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(mac.ask(UiCommand::CancelPairing).await, Reply::Done);
    assert_eq!(
        mac.until_pairing(|s| *s == PairingStatus::Idle).await,
        PairingStatus::Idle
    );
    pc.stop().await;
    mac.stop().await;
}

/// Unpairing from the UI: it is removed from trust, the link is cut and the other
/// machine, notified by it, also forgets the key (T-152).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpairing_from_the_ui() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(80, PairingTimeouts::default()).await;
    assert_eq!(
        mac.ask(UiCommand::Unpair).await,
        Reply::Rejected(NOT_PAIRED.into())
    );
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let usb4 = |name: &str| PeerStatus::Paired {
        name: name.into(),
        link: Some(Link::Usb4),
    };
    until_peer(&mac.agent, usb4("PC")).await;
    until_peer(&pc.agent, usb4("Mac")).await;

    assert_eq!(pc.ask(UiCommand::Unpair).await, Reply::Done);
    assert_eq!(pc.agent.peer_status(), PeerStatus::Unpaired);
    assert!(!pc.trusted(&mac_id));
    assert!(!*pc.agent.orchestrator().peer_connected().borrow());
    // The other machine finds out over the link (T-152): it also forgets the key and is left without a
    // peer, as the UI says.
    until_peer(&mac.agent, PeerStatus::Unpaired).await;
    assert!(!mac.trusted(&pc_id));
    assert!(!*mac.agent.orchestrator().peer_connected().borrow());
    pc.stop().await;
    mac.stop().await;
}

/// T-152, without a link: the one that leaves unpairs all the same and without error; the notice is not saved
/// for later (DECISIONS), so the other keeps trusting until it leaves too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpairing_with_the_other_machine_offline() {
    let Desk {
        mut mac,
        mut pc,
        pc_id,
        mac_id,
    } = desk(100, PairingTimeouts::default()).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let usb4 = |name: &str| PeerStatus::Paired {
        name: name.into(),
        link: Some(Link::Usb4),
    };
    until_peer(&mac.agent, usb4("PC")).await;
    until_peer(&pc.agent, usb4("Mac")).await;
    assert!(mac.trusted(&pc_id));
    // The Mac shuts down: there is no link to notify over.
    let mac_trust = mac.paths.trust_file();
    let kept = tempfile::tempdir().unwrap();
    std::fs::copy(&mac_trust, kept.path().join("trust.toml")).unwrap();
    mac.stop().await;

    assert_eq!(pc.ask(UiCommand::Unpair).await, Reply::Done);
    assert_eq!(pc.agent.peer_status(), PeerStatus::Unpaired);
    assert!(!pc.trusted(&mac_id));
    // What the Mac saved does not change: it keeps trusting the PC until it leaves too.
    let theirs = TrustStore::open(kept.path().join("trust.toml")).unwrap();
    assert!(theirs.is_trusted(&pc_id.public_key()));
    pc.stop().await;
}

/// T-152, on reconnecting: A no longer trusts B (B was left without a link
/// when A left the desk). On calling, A rejects it for an untrusted key and B
/// goes to `Unpaired` on its own, without a new state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_still_trusts_us_unpairs_when_rejected_on_reconnect() {
    let (a_id, b_id) = sides(
        Identity::from_seed([110; 32]),
        Identity::from_seed([111; 32]),
    );
    let (a, b) = tandemd::start_pair!(
        ports: free_pair; |a_port, b_port| start(Machine::A, &a_id, a_port, b_port, PairingTimeouts::default()),
        start_trusting(Machine::B, &b_id, &a_id, (b_port, a_port))
    );
    // B starts believing itself paired with A, which does not know it.
    until_peer(&b.agent, PeerStatus::Unpaired).await;
    assert!(!b.trusted(&a_id));
    assert_eq!(a.agent.peer_status(), PeerStatus::Unpaired);
    b.stop().await;
    a.stop().await;
}

/// T-159: when pairing, each machine saves the other's MAC on its own (in its `tandem.toml`'s
/// `[power.wol]`) and is left with something to power it on with, without editing files; a MAC set from the
/// UI is kept in the file and an invalid one is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_teaches_each_machine_the_wake_on_lan_mac_of_the_other() {
    let Desk {
        mut mac, mut pc, ..
    } = desk(90, PairingTimeouts::default()).await;
    assert_eq!(mac.agent.wol(), None);
    assert!(!mac.agent.capabilities().here.power.wake_peer);
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let learned = |a: &Agent| {
        let mut rx = a.subscribe_capabilities();
        async move {
            within(rx.wait_for(|c| c.here.power.wake_peer))
                .await
                .unwrap()
                .clone()
        }
    };
    let (m, p) = (learned(&mac.agent).await, learned(&pc.agent).await);
    assert_eq!(m.here.power.wol_mac.as_deref(), Some("02:00:00:00:00:02"));
    assert_eq!(p.here.power.wol_mac.as_deref(), Some("02:00:00:00:00:01"));
    // It is in the file, for the next start.
    let saved = AgentConfig::load(&mac.paths.config).unwrap();
    assert_eq!(
        saved.power.wol.unwrap().mac.to_string(),
        "02:00:00:00:00:02"
    );
    // Editing it from the UI changes it; an invalid one is rejected without touching anything.
    let bad = mac
        .ask(UiCommand::SetWol {
            mac: Some("not-a-mac".into()),
        })
        .await;
    assert!(matches!(bad, Reply::Rejected(_)), "{bad:?}");
    assert_eq!(
        mac.ask(UiCommand::SetWol {
            mac: Some("aa-bb-cc-dd-ee-ff".into())
        })
        .await,
        Reply::Done
    );
    assert_eq!(
        mac.agent.wol().unwrap().mac.to_string(),
        "aa:bb:cc:dd:ee:ff"
    );
    let saved = AgentConfig::load(&mac.paths.config).unwrap();
    assert_eq!(
        saved.power.wol.unwrap().mac.to_string(),
        "aa:bb:cc:dd:ee:ff"
    );
    // Deleting it leaves this machine with nothing to power the other on with, and the other does not change.
    assert_eq!(mac.ask(UiCommand::SetWol { mac: None }).await, Reply::Done);
    assert!(!mac.agent.capabilities().here.power.wake_peer);
    assert!(pc.agent.capabilities().here.power.wake_peer);
    pc.stop().await;
    mac.stop().await;
}

/// An agent without the network built (IPC only) rejects pairing with a clear reason.
#[tokio::test]
async fn without_network_pairing_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AgentConfig::from_toml("machine = \"a\"").unwrap();
    let agent = Arc::new(Agent::new(
        cfg,
        tandemd::StateStore::new(dir.path().join("state.json")),
    ));
    assert_eq!(
        agent.handle(UiCommand::StartPairing),
        Reply::Rejected(NO_NETWORK.into())
    );
}

fn input_setup(id: &str) -> InputSetup {
    let screen = Screen::new(id.into(), Rect::new(0, 0, 1920, 1080), 1.0);
    InputSetup {
        backends: Arc::new(FakeInputBackends::new(vec![screen])),
        opts: InputOptions {
            session: SessionConfig {
                heartbeat_interval: Duration::from_millis(100),
                heartbeat_timeout: Duration::from_millis(1500),
                ..SessionConfig::default()
            },
            report_every: Duration::from_millis(200),
            screens_every: Duration::from_millis(200),
            permissions_every: Duration::from_millis(50),
            retry: Duration::from_millis(100),
            ..InputOptions::default()
        },
        file_drag: None,
    }
}

/// The agents start without a peer, freshly installed (both believe themselves `A`) and with the
/// input engine running. When pairing from the UI, each ends up with the side the
/// keys say (the one that was `B` changes live: desk, file and engine), the engine sees the
/// new link and opens the session without restarting; when unpairing, it closes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_hot_opens_the_input_session() {
    for (x, y) in [(90, 91), (91, 90)] {
        pair_two_fresh_agents(x, y).await;
    }
}

async fn pair_two_fresh_agents(x: u8, y: u8) {
    let (mac_id, pc_id) = (Identity::from_seed([x; 32]), Identity::from_seed([y; 32]));
    let timeouts = PairingTimeouts::default();
    let (mut mac, mut pc) = tandemd::start_pair!(
        ports: free_pair; |mac_port, pc_port| start_with_input(
        (Machine::A, "Mac"),
        &mac_id,
        (mac_port, pc_port),
        timeouts,
        Some(input_setup("m34")),
    ),
        start_with_input(
        (Machine::A, "PC"),
        &pc_id,
        (pc_port, mac_port),
        timeouts,
        Some(input_setup("m34")),
    )
    );
    assert!(!mac.agent.input_status().session);

    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);

    until_input("input session open on Mac", &mac.agent, |s| {
        s.session
    })
    .await;
    until_input("input session open on PC", &pc.agent, |s| s.session).await;
    // Each with the keys' side, and the two distinct.
    let mac_side = Machine::of_keys(
        mac_id.public_key().as_bytes(),
        pc_id.public_key().as_bytes(),
    )
    .unwrap();
    assert_eq!(mac.agent.machine(), mac_side);
    assert_eq!(pc.agent.machine(), mac_side.other());
    // The name stays with its machine, on both (when they finish exchanging the
    // desk and the announcements), and the file of the one that changed already says so.
    for r in [&mac, &pc] {
        let mut desk = r.agent.subscribe_desk();
        within_for(
            "the machine names in the desk",
            desk.wait_for(|d| {
                d.machines.name(mac_side) == "Mac" && d.machines.name(mac_side.other()) == "PC"
            }),
        )
        .await
        .unwrap();
    }
    for r in [&mac, &pc] {
        let saved = AgentConfig::load(&r.paths.config).unwrap();
        assert_eq!(saved.machine, r.agent.machine());
        // Review of PR #68: two new installations stay with A to the left of B
        // even if one switched side: without a declared arrangement (T-099), the default one.
        assert_eq!(r.agent.desk().arrangement, None);
        assert_eq!(saved.desk.arrangement, None);
    }

    assert_eq!(pc.ask(UiCommand::Unpair).await, Reply::Done);
    until_input("input session closed on PC", &pc.agent, |s| {
        !s.session
    })
    .await;
    until_input("input session closed on Mac", &mac.agent, |s| {
        !s.session
    })
    .await;
    pc.stop().await;
    mac.stop().await;
}
