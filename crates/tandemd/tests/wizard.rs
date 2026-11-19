//! The initial setup wizard (DESIGN §5, without monitors) with two real `tandemd`s
//! over loopback, built with the first-start configuration: (1) this machine
//! and its permissions, (2) pair and see the link ping, (3) "press a key" and (4)
//! the summary. Only each machine's steps are simulated.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{PeerStatus, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::{Agent, AgentConfig, StoreError, autoconfig};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The two identities sorted by key: the first is side A's (DECISIONS A1), so
/// each test knows which side each agent gets.
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

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
    ui: IpcClient<tokio::io::DuplexStream>,
    config: PathBuf,
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
}

/// An agent built as in the binary, with the configuration the first start generates
/// and the loopback link toward the other.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
) -> Result<Running, PortTaken> {
    start_with(machine, identity, port, peer, true).await
}

/// Like [`start`]; with `dial` off the agent only has the connection the other opens.
async fn start_with(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    dial: bool,
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
            "port = {port}\npeers = [{{ kind = \"usb4\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::load(&paths.config).unwrap();
    assert_eq!(cfg.desk, tandem_core::Config::default(), "clean install");
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
            // Without reservation: a taken port fails right away and others are chosen.
            fallback_ports: Vec::new(),
            health: common::health(),
            dial,
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
        config: paths.config.clone(),
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reduced_wizard_end_to_end() {
    let (mac_id, pc_id) = sides(Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mut mac, mut pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(Machine::A, &mac_id, mac_port, (pc_id.public_key(), pc_port)),
        start(Machine::B, &pc_id, pc_port, (mac_id.public_key(), mac_port))
    );

    // (1) This machine and its permissions: what it can do and the process to grant permissions to.
    let Reply::Capabilities(caps) = mac.ui.request(UiCommand::GetCapabilities).await.unwrap()
    else {
        panic!("capabilities were expected");
    };
    assert_eq!(caps.here.machine, Machine::A);
    assert_eq!(caps.here.input.process.pid, std::process::id());
    assert_eq!(
        mac.ui.request(UiCommand::RequestPermissions).await.unwrap(),
        Reply::Done
    );
    let Reply::Input(input) = mac.ui.request(UiCommand::GetInput).await.unwrap() else {
        panic!("keyboard and mouse were expected");
    };
    assert!(
        !input.defined,
        "without a hub or answer, the keyboard is not set"
    );

    // (2) Paired (trust is already there): they see each other and the link ping is shown.
    let reply = within(async {
        loop {
            match mac.ui.request(UiCommand::PingPeer).await.unwrap() {
                Reply::Pong { link, millis } => break (link, millis),
                Reply::Rejected(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                other => panic!("{other:?}"),
            }
        }
    })
    .await;
    assert_eq!(reply.0, tandem_proto::Link::Usb4);
    assert!(reply.1 >= 1);
    assert!(matches!(
        mac.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Paired { link: Some(_), .. })
    ));

    // (3) "Press a key": the Mac's wizard arms the test and the PC finds out.
    assert_eq!(
        mac.ui
            .request(UiCommand::KeyProbe { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let mut probe = pc.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    // The key reaches the PC.
    assert_eq!(
        pc.ui.request(UiCommand::HeardKey).await.unwrap(),
        Reply::Done
    );
    let mut probe = mac.agent.subscribe_key_probe();
    let heard = within(probe.wait_for(|s| s.heard.is_some()))
        .await
        .unwrap()
        .heard;
    assert_eq!(heard, Some(Machine::B));
    // A key on the other one does not change what was heard first, but it is recorded (T-220).
    mac.ui.request(UiCommand::HeardKey).await.unwrap();
    assert_eq!(mac.agent.key_probe_status().heard, Some(Machine::B));
    assert_eq!(mac.agent.key_probe_status().second, Some(Machine::A));

    // "Is this right?": yes. It is saved on both and stops being "not set".
    assert_eq!(
        mac.ui
            .request(UiCommand::SetKeyboard(Machine::B))
            .await
            .unwrap(),
        Reply::Done
    );
    let mut kb = pc.agent.subscribe_keyboard();
    within(kb.wait_for(|k| *k == Some(Machine::B)))
        .await
        .unwrap();
    for r in [&mac, &pc] {
        let mut st = r.agent.subscribe_input();
        within(st.wait_for(|i| i.defined && i.server == Machine::B))
            .await
            .unwrap();
        let saved = AgentConfig::load(&r.config).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::B));
    }

    // (4) Summary: where the configuration came from and the desk (without declared screens;
    // only which machine each side is, which is noted when getting to know each other).
    let Reply::Setup(setup) = mac.ui.request(UiCommand::GetSetup).await.unwrap() else {
        panic!("the configuration was expected");
    };
    assert!(setup.config_file.is_some());
    let Reply::Desk(desk) = mac.ui.request(UiCommand::GetDesk).await.unwrap() else {
        panic!("the desk was expected");
    };
    assert!(desk.screens.is_empty());
    assert_eq!(desk.arrangement, None);
    // Turn listening off at the end.
    mac.ui
        .request(UiCommand::KeyProbe { on: false })
        .await
        .unwrap();
    let mut probe = pc.agent.subscribe_key_probe();
    within(probe.wait_for(|s| !s.armed)).await.unwrap();

    mac.stop().await;
    pc.stop().await;
}

/// T-166: the key heard on B reaches A's UI through its IPC socket, not only the agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_other_ones_activity_reaches_the_ui_over_ipc() {
    let (a_id, b_id) = sides(Identity::from_seed([3; 32]), Identity::from_seed([4; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    within(async {
        loop {
            match a.ui.request(UiCommand::PingPeer).await.unwrap() {
                Reply::Pong { .. } => break,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    assert_eq!(
        a.ui.request(UiCommand::KeyProbe { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let mut probe = b.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    b.ui.request(UiCommand::HeardKey).await.unwrap();
    // A never received a key: only the IPC push can notify its UI.
    let heard = within(async {
        loop {
            match a.ui.next_event().await.unwrap() {
                Some(tandem_proto::AgentToUi::KeyProbe(s)) if s.heard.is_some() => {
                    break s.heard;
                }
                Some(_) => {}
                None => panic!("the connection closed"),
            }
        }
    })
    .await;
    assert_eq!(heard, Some(Machine::B));
    a.stop().await;
    b.stop().await;
}

/// T-220: activity on both machines, in either order, reaches both UIs over IPC.
async fn both_machines_reach_both_uis(first_is_a: bool, seed: u8) {
    let (a_id, b_id) = sides(
        Identity::from_seed([seed; 32]),
        Identity::from_seed([seed + 1; 32]),
    );
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    within(async {
        loop {
            match a.ui.request(UiCommand::PingPeer).await.unwrap() {
                Reply::Pong { .. } => break,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    a.ui.request(UiCommand::KeyProbe { on: true })
        .await
        .unwrap();
    let mut probe = b.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    if first_is_a {
        a.ui.request(UiCommand::HeardKey).await.unwrap();
        within(probe.wait_for(|s| s.heard.is_some())).await.unwrap();
        b.ui.request(UiCommand::HeardKey).await.unwrap();
    } else {
        b.ui.request(UiCommand::HeardKey).await.unwrap();
        let mut pa = a.agent.subscribe_key_probe();
        within(pa.wait_for(|s| s.heard.is_some())).await.unwrap();
        a.ui.request(UiCommand::HeardKey).await.unwrap();
    }
    for ui in [&mut a.ui, &mut b.ui] {
        let machines = within(async {
            loop {
                match ui.next_event().await.unwrap() {
                    Some(tandem_proto::AgentToUi::KeyProbe(s))
                        if s.heard.is_some() && s.second.is_some() =>
                    {
                        break [s.heard, s.second];
                    }
                    Some(_) => {}
                    None => panic!("the connection closed"),
                }
            }
        })
        .await;
        let first = if first_is_a { Machine::A } else { Machine::B };
        assert_eq!(machines, [Some(first), Some(first.other())]);
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_on_a_then_b_reaches_both_uis() {
    both_machines_reach_both_uis(true, 5).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_on_b_then_a_reaches_both_uis() {
    both_machines_reach_both_uis(false, 7).await;
}

/// T-240: each agent installs its own link toward the other on its own time, so the key
/// can be heard on a machine whose link is not up yet even though the other's already is (slow CI).
/// B cannot dial: its link only appears when its manager retries and adopts A's connection, a
/// few seconds after A's is up, which forces that order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_heard_before_its_own_link_is_up_still_reaches_the_other() {
    let (a_id, b_id) = sides(Identity::from_seed([9; 32]), Identity::from_seed([10; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start_with(Machine::A, &a_id, a_port, (b_id.public_key(), b_port), true),
        start_with(
            Machine::B,
            &b_id,
            b_port,
            (a_id.public_key(), a_port),
            false
        )
    );
    within(async {
        loop {
            match a.ui.request(UiCommand::PingPeer).await.unwrap() {
                Reply::Pong { .. } => break,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    a.ui.request(UiCommand::KeyProbe { on: true })
        .await
        .unwrap();
    let mut probe = b.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    assert!(
        b.agent.orchestrator().peer().rtt().is_none(),
        "the premise: B has no link of its own yet"
    );
    b.ui.request(UiCommand::HeardKey).await.unwrap();
    let mut probe = a.agent.subscribe_key_probe();
    let heard = within(probe.wait_for(|s| s.heard.is_some()))
        .await
        .unwrap()
        .heard;
    assert_eq!(heard, Some(Machine::B));
    a.stop().await;
    b.stop().await;
}
