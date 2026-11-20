//! Two `tandemd`s built with [`daemon::start`], as in the binary: paired identities
//! in the data directory, real QUIC over loopback and the config's fixed links.
//! Only each machine's steps are simulated.

mod common;

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::DeskPatch;
use tandem_core::{Machine, Side, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Link, PeerStatus, Reply, TxnEvent, UiCommand};
use tandemd::daemon::{self, Backends, Daemon, ShutdownReason};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::{Agent, AgentConfig, StoreError};
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

/// Cap so a broken test fails instead of hanging.
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

/// Simulated steps: they record what they are asked.
#[derive(Default)]
struct Fake {
    log: Mutex<Vec<Step>>,
}

impl Fake {
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
            Ok(())
        })
    }
}

/// A free loopback UDP port. It is released right away for the agent to take it:
/// each one needs to know the other's before starting.
fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

struct Running {
    agent: Arc<Agent>,
    steps: Arc<Fake>,
    ui: IpcClient<tokio::io::DuplexStream>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<ShutdownReason, StoreError>>,
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

/// An agent of `machine` built as in the binary. `peer` is the other agent's key and port;
/// without it, the agent has no paired peer.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: Option<(PublicKey, u16)>,
) -> Result<Running, PortTaken> {
    start_with(machine, identity, port, peer, "").await
}

/// Like [`start`], with `extra` right after `machine` in `tandem.toml` (top-level keys
/// and tables of its own, like a file from an earlier version).
async fn start_with(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: Option<(PublicKey, u16)>,
    extra: &str,
) -> Result<Running, PortTaken> {
    start_opts(machine, identity, port, peer, extra, true, true).await
}

/// Like [`start_with`]; with `dial` off the agent never reaches the other one, as when the
/// other machine's firewall drops inbound UDP (T-237). With `list_peer` off, the peer is
/// trusted but its address is not in `[network] peers`.
async fn start_opts(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: Option<(PublicKey, u16)>,
    extra: &str,
    dial: bool,
    list_peer: bool,
) -> Result<Running, PortTaken> {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let mut network = format!("[network]\nport = {port}\n");
    if let Some((key, peer_port)) = peer {
        let mut trust = TrustStore::open(paths.trust_file()).unwrap();
        trust
            .add_and_save(TrustedPeer {
                public_key: key,
                name: machine.other().to_string(),
            })
            .unwrap();
        if list_peer {
            network.push_str(&format!(
                "peers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n",
                loopback(peer_port)
            ));
        }
    }
    // Both with `[power.wol]` to wake the other (the steps are simulated).
    let text = format!("machine = \"{machine}\"\n{extra}\n{network}\n{WOL}");
    // On disk, as in the binary: what is changed from the UI is saved there.
    std::fs::write(&paths.config, &text).unwrap();
    let mut cfg = AgentConfig::from_toml(&text).unwrap();
    // A is a Mac and B a PC in these tests: what each can do comes from its system, not
    // from the machine running the test (on macOS neither would shut itself down).
    cfg.os = match machine {
        Machine::A => tandem_core::Os::MacOs,
        Machine::B => tandem_core::Os::Windows,
    };
    let steps = Arc::new(Fake::default());
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: steps.clone(),
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
    assert_eq!(network.local_addr(), loopback(port));
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
        steps,
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

fn paired(name: &str, link: Option<Link>) -> PeerStatus {
    PeerStatus::Paired {
        name: name.into(),
        link,
    }
}

async fn until_peer(agent: &Agent, want: PeerStatus) {
    let mut rx = agent.subscribe_peer();
    within(rx.wait_for(|s| *s == want)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_agents_find_each_other_and_shut_down_the_pc() {
    let (mac_id, pc_id) = sides(Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mut mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
    );

    // Each sees the other over the only fixed link, and the UI receives it.
    until_peer(&mac.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("A", Some(Link::Usb4))).await;
    assert_eq!(
        mac.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(paired("B", Some(Link::Usb4)))
    );

    // The Mac sees the PC appear and considers it on (peer_watch), and vice versa.
    let mut mac_state = mac.agent.subscribe();
    within(mac_state.wait_for(|s| s.is_on(Machine::B)))
        .await
        .unwrap();
    let mut pc_state = pc.agent.subscribe();
    within(pc_state.wait_for(|s| s.is_on(Machine::A)))
        .await
        .unwrap();

    // Power off the PC: the Mac asks it for the step over the link. Considering it on takes the
    // orchestrator for an instant: if it coincides, it is retried.
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
    let committed = within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, state }) if t == txn => break state,
                AgentToUi::Txn(e @ TxnEvent::Failed { .. }) => panic!("the shutdown failed: {e:?}"),
                _ => {}
            }
        }
    })
    .await;
    assert!(!committed.on.b);
    assert!(mac.steps.steps().is_empty());
    assert_eq!(
        pc.steps.steps(),
        [Step::PowerOff {
            machine: Machine::B
        }]
    );

    // When the PC shuts down, the Mac stops seeing it right away and has nobody to ask for steps.
    let mac_agent = mac.agent.clone();
    pc.stop().await;
    until_peer(&mac_agent, paired("B", None)).await;
    assert!(!*mac_agent.orchestrator().peer_connected().borrow());
    mac.stop().await;
}

/// Starts the Mac and the PC paired over one fixed USB4 link; `mac_dials` and `pc_dials`
/// say which of the two can reach the other (T-237).
async fn one_way_pair(seed: u8, mac_dials: bool, pc_dials: bool) -> (Running, Running) {
    pair_with(seed, mac_dials, pc_dials, true).await
}

/// Like [`one_way_pair`]; with `pc_lists_mac` off the PC trusts the Mac but has no address
/// for it.
async fn pair_with(
    seed: u8,
    mac_dials: bool,
    pc_dials: bool,
    pc_lists_mac: bool,
) -> (Running, Running) {
    let (mac_id, pc_id) = sides(
        Identity::from_seed([seed; 32]),
        Identity::from_seed([seed + 1; 32]),
    );
    tandemd::start_pair!(
        |mac_port, pc_port| start_opts(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
            "",
            mac_dials,
            true,
        ),
        start_opts(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
            "",
            pc_dials,
            pc_lists_mac,
        )
    )
}

/// `from` sees `to` as a working peer: the UI says the link is up, the wizard's ping
/// answers and a step reaches `to` over the link.
async fn works_towards(from: &mut Running, to: &Running, to_machine: Machine) {
    works_over(from, to, to_machine, Link::Usb4).await;
}

async fn works_over(from: &mut Running, to: &Running, to_machine: Machine, link: Link) {
    let name = to_machine.to_string();
    until_peer(&from.agent, paired(&name, Some(link))).await;
    assert_eq!(
        from.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(paired(&name, Some(link)))
    );
    let pong = from.ui.request(UiCommand::PingPeer).await.unwrap();
    assert!(
        matches!(pong, Reply::Pong { link: l, .. } if l == link),
        "the ping did not answer: {pong:?}"
    );
    let mut connected = from.agent.orchestrator().peer_connected();
    within(connected.wait_for(|c| *c)).await.unwrap();
    let step = Step::PowerOff {
        machine: to_machine,
    };
    within(async {
        loop {
            match from.agent.orchestrator().peer().execute(&step).await {
                Ok(()) => break,
                // The other side may be busy noting this one as on: retried.
                Err(why) if why.contains("busy") => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(why) => panic!("the step did not reach the other machine: {why}"),
            }
        }
    })
    .await;
    assert!(to.steps.steps().contains(&step), "{:?}", to.steps.steps());
}

/// The cable run of 2026-11-18: the Mac's firewall drops what the PC sends, so only
/// the Mac's connection exists. The PC must adopt it as its path too (T-237).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pc_that_cannot_dial_the_mac_links_over_the_macs_connection() {
    let (mut mac, mut pc) = one_way_pair(40, true, false).await;
    works_towards(&mut pc, &mac, Machine::A).await;
    works_towards(&mut mac, &pc, Machine::B).await;
    pc.stop().await;
    mac.stop().await;
}

/// The same the other way round: only the PC can dial.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mac_that_cannot_dial_the_pc_links_over_the_pcs_connection() {
    let (mut mac, mut pc) = one_way_pair(42, false, true).await;
    works_towards(&mut mac, &pc, Machine::B).await;
    works_towards(&mut pc, &mac, Machine::A).await;
    pc.stop().await;
    mac.stop().await;
}

/// A PC that knows the Mac only by its key (no address configured, no discovery) still
/// links when the Mac reaches it: the path the Mac came over enters its link manager.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pc_without_the_macs_address_links_over_the_macs_connection() {
    let (mut mac, mut pc) = pair_with(46, true, true, false).await;
    let kind = within(async {
        loop {
            if let PeerStatus::Paired {
                link: Some(link), ..
            } = pc.agent.peer_status()
            {
                break link;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    works_over(&mut pc, &mac, Machine::A, kind).await;
    works_towards(&mut mac, &pc, Machine::B).await;
    pc.stop().await;
    mac.stop().await;
}

/// With both directions open, both dial: neither closes the other's connection and the
/// link does not flap once up (T-237).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_both_dial_the_link_stays_up_without_flapping() {
    let (mac, pc) = one_way_pair(44, true, true).await;
    until_peer(&mac.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("A", Some(Link::Usb4))).await;
    let mut seen = [mac.agent.subscribe_peer(), pc.agent.subscribe_peer()];
    for rx in &mut seen {
        rx.borrow_and_update();
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    for (rx, name) in seen.iter_mut().zip(["B", "A"]) {
        assert!(
            !rx.has_changed().unwrap(),
            "the peer status changed: {:?}",
            *rx.borrow()
        );
        assert_eq!(*rx.borrow(), paired(name, Some(Link::Usb4)));
    }
    pc.stop().await;
    mac.stop().await;
}

#[tokio::test]
async fn unpaired_agent_works_alone_and_says_so() {
    let identity = Identity::from_seed([3; 32]);
    let mut a = tandemd::start_solo!(|port, _peer| start(Machine::A, &identity, port, None));
    assert_eq!(a.agent.peer_status(), PeerStatus::Unpaired);
    assert_eq!(
        a.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Unpaired)
    );
    assert_eq!(a.ui.get_state().await.unwrap(), a.agent.state());
    assert!(!*a.agent.orchestrator().peer_connected().borrow());
    // What only concerns this machine is done all the same: here, nothing to change.
    let reply =
        a.ui.request(UiCommand::SetPower {
            machine: Machine::A,
            on: true,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
    a.stop().await;
}

/// The first time the agent creates its identity and saves it; the next one reuses it.
#[tokio::test]
async fn identity_is_created_once_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    let cfg = AgentConfig::from_toml("machine = \"b\"").unwrap();
    let opts = NetOptions {
        bind: Some(loopback(0)),
        mdns: false,
        health: common::health(),
        ..NetOptions::default()
    };
    let backends = || Backends::Custom {
        local: Arc::new(Fake::default()),
        input: None,
        env: None,
    };
    let first = daemon::start(cfg.clone(), &paths, backends(), opts.clone()).unwrap();
    let key = Identity::load(&paths.identity_file()).unwrap().public_key();
    first.network.shutdown().await;
    let second = daemon::start(cfg, &paths, backends(), opts).unwrap();
    assert_eq!(
        Identity::load(&paths.identity_file()).unwrap().public_key(),
        key
    );
    assert_eq!(second.agent.peer_status(), PeerStatus::Unpaired);
}

/// What is needed for one machine to wake the other.
const WOL: &str = "[power.wol]\nmac = \"aa:bb:cc:dd:ee:ff\"\n";

/// Powering on the PC ends when its agent appears over a link, not before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_on_waits_for_the_pc_agent() {
    let (mac_id, pc_id) = sides(Identity::from_seed([4; 32]), Identity::from_seed([5; 32]));
    let (mut mac, mac_port, pc_port) = tandemd::start_solo!(|mac_port, pc_port| async {
        let r = start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        )
        .await;
        r.map(|r| (r, mac_port, pc_port))
    });
    assert_eq!(mac.agent.peer_status(), paired("B", None));
    let reply = mac
        .ui
        .request(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
    // The Wake-on-LAN went out and the transaction waits.
    within(async {
        while mac.steps.steps().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(!mac.agent.state().on.b);

    // The PC starts afterwards, on purpose: if another suite took its port meanwhile,
    // it fails right away (without reserve ports) instead of hanging.
    let pc = start(
        Machine::B,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await
    .expect("the PC's port was taken meanwhile");
    let state = within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { state, .. }) => break state,
                AgentToUi::Txn(e @ TxnEvent::Failed { .. }) => panic!("it did not power on: {e:?}"),
                _ => {}
            }
        }
    })
    .await;
    assert!(state.on.b);
    assert_eq!(
        mac.steps.steps()[0],
        Step::PowerOn {
            machine: Machine::B
        }
    );
    pc.stop().await;
    mac.stop().await;
}

/// Starts a PC agent without a peer at `bind`, with those reserve ports.
fn start_on(
    dir: &std::path::Path,
    bind: SocketAddr,
    fallback_ports: Vec<u16>,
) -> Result<Daemon, String> {
    let paths = Paths {
        config: dir.join("tandem.toml"),
        data_dir: dir.to_owned(),
    };
    let cfg = AgentConfig::from_toml("machine = \"b\"").unwrap();
    daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Fake::default()),
            input: None,
            env: None,
        },
        NetOptions {
            bind: Some(bind),
            fallback_ports,
            mdns: false,
            health: common::health(),
            ..NetOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// T-082: the agent's port being busy (on the real PC, reserved by Windows) does not prevent
/// starting: the first free one from the reserve list is used, and it is the one that would be announced.
#[tokio::test]
async fn an_occupied_port_falls_back_to_a_free_one() {
    let dir = tempfile::tempdir().unwrap();
    let busy = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = busy.local_addr().unwrap().port();
    let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let also_taken = other.local_addr().unwrap().port();
    let free = free_port();
    let d = start_on(dir.path(), loopback(taken), vec![also_taken, free]).unwrap();
    assert_eq!(d.network.local_addr(), loopback(free));
    assert_eq!(d.agent.peer_status(), PeerStatus::Unpaired);
    d.network.shutdown().await;
}

/// If no port opens, the error says which were tried, the cause and how to fix it.
#[tokio::test]
async fn no_free_port_explains_how_to_fix_it() {
    let dir = tempfile::tempdir().unwrap();
    let busy = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = busy.local_addr().unwrap().port();
    let busy2 = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken2 = busy2.local_addr().unwrap().port();
    let Err(text) = start_on(dir.path(), loopback(taken), vec![taken2]) else {
        panic!("it started with all ports busy");
    };
    for want in [
        taken.to_string(),
        taken2.to_string(),
        "excludedportrange".into(),
        "[network] port".into(),
        "tandem.toml".into(),
    ] {
        assert!(text.contains(&want), "missing \"{want}\" in: {text}");
    }
}

/// What the desk has in `r`'s `tandem.toml` (what it would read on starting again).
fn saved(r: &Running) -> AgentConfig {
    AgentConfig::load(&r._dir.path().join("tandem.toml")).unwrap()
}

async fn until_desk(r: &Running, want: &tandem_core::Config) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| d == want)).await.unwrap();
}

/// The same, looking only at the arrangement (the screens and the sides): which machine each side is
/// is noted separately, when both announce themselves.
async fn until_layout(r: &Running, want: &tandem_core::Config) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| d.screens == want.screens && d.arrangement == want.arrangement))
        .await
        .unwrap();
}

/// Waits for both to have both machines noted in the desk, with the same
/// version: from then on what changes is the user's.
async fn until_both_known(a: &Running, b: &Running) {
    within(async {
        loop {
            let (da, db) = (a.agent.desk_sync(), b.agent.desk_sync());
            let known =
                |d: &tandem_core::Config| Machine::ALL.iter().all(|m| d.machines.get(*m).is_some());
            if known(&da.desk) && da.stamp.rev == db.stamp.rev && da.desk == db.desk {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// T-084: the desk belongs to both. Changing the arrangement on the Mac reaches the PC (live
/// and in its `tandem.toml`), and vice versa; what is local-only does not travel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desk_changed_on_either_machine_reaches_the_other() {
    let (mac_id, pc_id) = sides(Identity::from_seed([5; 32]), Identity::from_seed([6; 32]));
    let (mut mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
    );
    until_peer(&mac.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("A", Some(Link::Usb4))).await;

    // Each sees what the other can do: the PC shuts itself down.
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| c.peer.as_ref().is_some_and(|p| p.machine == Machine::B)))
        .await
        .unwrap();
    assert!(mac.agent.capabilities().peer.unwrap().power.power_off);
    // On getting to know each other, both are noted in the desk (on both machines).
    until_both_known(&mac, &pc).await;
    let base = mac.agent.desk_sync().stamp.rev;
    assert_eq!(
        mac.agent.machine_info(Machine::B),
        Some(mac.agent.capabilities().peer.unwrap().info)
    );

    // On the Mac: the Mac moves to the right of the PC.
    let mut desk = mac.agent.desk();
    desk.arrangement = Some(tandem_core::Arrangement {
        a_side: Side::Right,
        align: tandem_core::Align::Center,
    });
    mac.agent.set_desk(desk.clone()).unwrap();
    until_desk(&pc, &desk).await;
    for r in [&mac, &pc] {
        let cfg = saved(r);
        assert_eq!(cfg.desk, desk, "{}", cfg.machine);
        assert_eq!(cfg.desk_rev, base + 1);
        assert_eq!(cfg.desk_origin, Some(Machine::A));
    }
    // What is local-only is still each one's.
    assert_eq!(saved(&pc).machine, Machine::B);
    assert_eq!(saved(&pc).network.port, pc.agent.config().network.port);

    // The other way, from the PC: the usual arrangement returns.
    let mut back = desk.clone();
    back.arrangement = None;
    pc.agent.set_desk(back.clone()).unwrap();
    until_desk(&mac, &back).await;
    let cfg = saved(&mac);
    assert_eq!(cfg.desk, back);
    assert_eq!(
        (cfg.desk_rev, cfg.desk_origin),
        (base + 2, Some(Machine::B))
    );
    // The Mac's UI receives the new desk pushed.
    within(async {
        loop {
            if let Some(AgentToUi::Desk(d)) = mac.ui.next_event().await.unwrap()
                && d == back
            {
                break;
            }
        }
    })
    .await;

    mac.stop().await;
    pc.stop().await;
}

/// T-084: the Mac reconnects after missing a change (it was off): on returning, it
/// catches up with the PC's newest version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_made_while_the_other_was_away_arrives_on_reconnect() {
    let (mac_id, pc_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (pc, mac_port, pc_port) = tandemd::start_solo!(|mac_port, pc_port| async {
        let r = start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
        .await;
        r.map(|r| (r, mac_port, pc_port))
    });
    // The Mac is not there yet: the change stays only on the PC.
    let mut desk = pc.agent.desk();
    desk.arrangement = Some(tandem_core::Arrangement {
        a_side: Side::Right,
        align: tandem_core::Align::Center,
    });
    pc.agent.set_desk(desk.clone()).unwrap();

    // The Mac starts afterwards, on purpose: if another suite took its port meanwhile,
    // it fails right away (without reserve ports) instead of hanging.
    let mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await
    .expect("the Mac's port was taken meanwhile");
    until_layout(&mac, &desk).await;
    // Besides the change, on getting to know each other both machines are noted: one more version.
    until_both_known(&mac, &pc).await;
    let got = saved(&mac);
    assert_eq!(got.desk.arrangement, desk.arrangement);
    assert!(got.desk_rev >= 1);
    assert_eq!(got.desk, pc.agent.desk());

    mac.stop().await;
    pc.stop().await;
}

/// The next "Changed from X" notice that `r`'s UI receives: who, version, what and which
/// version to go back to.
async fn next_desk_changed(r: &mut Running) -> (Machine, u64, String, u64) {
    within(async {
        loop {
            if let Some(AgentToUi::DeskChanged {
                by,
                rev,
                summary,
                undo_to,
            }) = r.ui.next_event().await.unwrap()
            {
                return (by, rev, summary, undo_to);
            }
        }
    })
    .await
}

async fn until_same_desk(a: &Running, b: &Running) {
    within(async {
        while a.agent.desk_sync() != b.agent.desk_sync() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// T-159: the desk's name is edited from the UI and reaches the other machine and its file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desk_name_edited_in_one_machine_reaches_the_other() {
    let (a_id, b_id) = sides(Identity::from_seed([43; 32]), Identity::from_seed([44; 32]));
    let (mut a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, Some((b_id.public_key(), b_port))),
        start(Machine::B, &b_id, b_port, Some((a_id.public_key(), a_port)))
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_both_known(&a, &b).await;
    let Reply::Desk(edited) =
        a.ui.request(UiCommand::EditDesk {
            patch: DeskPatch::Name("Office".into()),
        })
        .await
        .unwrap()
    else {
        panic!("the edit did not return the desk");
    };
    assert_eq!(edited.name.as_deref(), Some("Office"));
    until_desk(&b, &edited).await;
    assert_eq!(saved(&b).desk.name.as_deref(), Some("Office"));
    assert_eq!(saved(&a).desk.name.as_deref(), Some("Office"));
    a.stop().await;
    b.stop().await;
}

/// T-099 (DESIGN §4.4, D5): the desk is edited from the UI of either machine. What
/// is edited on A is seen on B; if edited on both at once, the last wins and both
/// end up the same, B receives the notice with "Undo", and undoing on B goes back exactly to
/// its version, also on A.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desk_edits_from_either_ui_converge_and_the_other_gets_the_notice() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, Some((b_id.public_key(), b_port))),
        start(Machine::B, &b_id, b_port, Some((a_id.public_key(), a_port)))
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_both_known(&a, &b).await;

    // Editing on A (from its UI) is seen on B, live and in its file, with the notice.
    let arr = tandem_core::Arrangement {
        a_side: Side::Up,
        align: tandem_core::Align::End,
    };
    let Reply::Desk(edited) =
        a.ui.request(UiCommand::EditDesk {
            patch: DeskPatch::Arrangement(Some(arr)),
        })
        .await
        .unwrap()
    else {
        panic!("the edit did not return the desk");
    };
    assert_eq!(edited.arrangement, Some(arr));
    until_desk(&b, &edited).await;
    assert_eq!(saved(&b).desk.arrangement, Some(arr));
    let (by, rev, summary, _) = next_desk_changed(&mut b).await;
    assert_eq!((by, summary.as_str()), (Machine::A, "Arrangement"));
    assert_eq!(rev, a.agent.desk_sync().stamp.rev);

    // Both at once: first B and right after A, without waiting for them to be seen. A wins: either
    // they tie on version and A has the same or newer date (and the tie is A's), or A
    // had already received B's and its own is the next one.
    let prefs = tandem_core::InputPrefs {
        drag_files: false,
        ..tandem_core::InputPrefs::default()
    };
    b.agent.edit_desk(DeskPatch::Prefs(prefs)).unwrap();
    let mine_on_b = b.agent.desk_sync();
    a.agent
        .edit_desk(DeskPatch::Shortcuts(tandem_core::Shortcuts {
            lock: vec!["KeyL".into()],
        }))
        .unwrap();
    until_same_desk(&a, &b).await;
    let last = a.agent.desk_sync();
    assert_eq!(last.stamp.by, Machine::A);
    assert_eq!(last.desk.shortcuts.lock, ["KeyL"]);
    assert_eq!(saved(&b).desk, last.desk);
    // B receives the notice, and undoing takes it to its own version.
    let (by, rev, _, undo_to) = next_desk_changed(&mut b).await;
    assert_eq!((by, rev), (Machine::A, last.stamp.rev));
    assert_eq!(undo_to, mine_on_b.stamp.rev);
    let Reply::Desk(back) =
        b.ui.request(UiCommand::UndoDesk { to_rev: undo_to })
            .await
            .unwrap()
    else {
        panic!("it did not undo");
    };
    assert_eq!(back, mine_on_b.desk);
    until_same_desk(&a, &b).await;
    assert_eq!(a.agent.desk(), mine_on_b.desk);
    assert!(!saved(&a).desk.prefs.drag_files);
    // Now the notice is for A.
    let (by, _, summary, _) = next_desk_changed(&mut a).await;
    assert_eq!(by, Machine::B);
    assert!(!summary.is_empty());
    let Reply::DeskHistory(h) = a.ui.request(UiCommand::GetDeskHistory).await.unwrap() else {
        panic!("no history");
    };
    assert_eq!(h[0].stamp, a.agent.desk_sync().stamp);

    a.stop().await;
    b.stop().await;
}

/// A pre-T-099 `tandem.toml`, with both machines already known and the same desk
/// version. `input` goes in its `[input]`.
fn before_t099(input: &str) -> String {
    format!(
        "desk_rev = 3\ndesk_origin = \"a\"\ndesk_sum = \"0123456789abcdef\"\n\n\
         [machines.a]\nos = \"macos\"\nname = \"Mac\"\ncolor = \"#7aa7e6\"\n\n\
         [machines.b]\nos = \"windows\"\nname = \"PC\"\ncolor = \"#a99be8\"\n\n\
         [arrangement]\na_side = \"left\"\noffset = 0\n\n\
         [input]\nlock_shortcut = [\"ScrollLock\"]\n{input}\n"
    )
}

/// Review of #74: before T-099 the clipboard belonged to each machine (`[input]
/// clipboard`); now it is a module of each, in the desk (`[modules.b] clipboard`). If one had it off and the other did not, both
/// started with the same stamp and a different desk, and never converged. On connecting
/// they have to end up equal, with the clipboard as left by whoever had changed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_setting_moved_to_the_desk_does_not_leave_the_machines_apart() {
    let (a_id, b_id) = sides(Identity::from_seed([43; 32]), Identity::from_seed([44; 32]));
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start_with(
            Machine::A,
            &a_id,
            a_port,
            Some((b_id.public_key(), b_port)),
            &before_t099(""),
        ),
        start_with(
            Machine::B,
            &b_id,
            b_port,
            Some((a_id.public_key(), a_port)),
            &before_t099("clipboard = false"),
        )
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    // T-235: both start with the same `desk_rev` and sum, so "the same desk" can already be
    // true before the migrated clipboard is merged; wait for the state being asserted.
    let off = |d: &tandem_core::Config| d.modules.get(Machine::B, "clipboard") == Some(false);
    let mut on_a = a.agent.subscribe_desk();
    let mut on_b = b.agent.subscribe_desk();
    within(on_a.wait_for(off)).await.unwrap();
    within(on_b.wait_for(off)).await.unwrap();
    until_same_desk(&a, &b).await;
    assert!(off(&a.agent.desk()));
    assert!(off(&b.agent.desk()));
    assert_eq!(
        saved(&a).desk.modules.get(Machine::B, "clipboard"),
        Some(false)
    );
    // Each applies what is on its side: B has it off, A does not.
    assert!(!b.agent.module_on(tandem_proto::ModuleId::Clipboard));
    assert!(a.agent.module_on(tandem_proto::ModuleId::Clipboard));
    a.stop().await;
    b.stop().await;
}

/// T-101: a `[data]` change reaches the `Qos` of all connections, also the one
/// each agent accepted from the other (they share their path's `Qos`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wifi_data_cap_reaches_the_connections_already_open() {
    use tandem_link::manager::LinkKind;

    let (a_id, b_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, Some((b_id.public_key(), b_port))),
        start(Machine::B, &b_id, b_port, Some((a_id.public_key(), a_port)))
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_both_known(&a, &b).await;

    // The connections are already open (each its own and the one it accepted from the other) and use
    // their path's `Qos`, the same one requested here.
    let b_usb4 = b.agent.link_qos().get(LinkKind::Usb4);
    let b_wifi = b.agent.link_qos().get(LinkKind::Wifi);
    let before = b_wifi.cap().unwrap();

    let data = tandem_core::DataPrefs {
        wifi_cap_percent: 10,
        wifi_cap_mbps: None,
    };
    let desk = a
        .agent
        .edit_desk(tandem_core::DeskPatch::Data(data))
        .unwrap();
    assert_eq!(desk.data, data);
    until_desk(&b, &desk).await;
    within(async {
        while b_usb4.prefs() != desk.data || a.agent.link_qos().prefs() != desk.data {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(b_wifi.prefs(), desk.data);
    assert_eq!(b_wifi.cap(), Some(before / 5));
    a.stop().await;
    b.stop().await;
}

/// What the desk shows in memory is already in `tandem.toml`: on getting to know each other, both
/// machines are noted in the desk, and before it was published in memory (seen by the UI, the
/// other machine and the tests) an instant before saving it. A test that compared the
/// file with memory failed now and then on macOS (T-093, review of #78).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_desk_in_memory_is_never_ahead_of_tandem_toml() {
    for seed in 0..6u8 {
        let (mac_id, pc_id) = sides(
            Identity::from_seed([90 + seed; 32]),
            Identity::from_seed([100 + seed; 32]),
        );
        let (mac, pc) = tandemd::start_pair!(
            |mac_port, pc_port| start(
                Machine::A,
                &mac_id,
                mac_port,
                Some((pc_id.public_key(), pc_port)),
            ),
            start(
                Machine::B,
                &pc_id,
                pc_port,
                Some((mac_id.public_key(), mac_port)),
            )
        );
        let known =
            |d: &tandem_core::Config| Machine::ALL.iter().all(|m| d.machines.get(*m).is_some());
        let mut rx = mac.agent.subscribe_desk();
        within(async {
            loop {
                rx.changed().await.unwrap();
                let shown = rx.borrow_and_update().clone();
                if known(&shown) {
                    // At the instant it is seen, the file already has it.
                    assert!(
                        known(&saved(&mac).desk),
                        "the desk was shown before being saved"
                    );
                    return;
                }
            }
        })
        .await;
        mac.stop().await;
        pc.stop().await;
    }
}
