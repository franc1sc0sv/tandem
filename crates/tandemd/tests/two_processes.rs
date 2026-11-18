//! Two real `tandemd` processes (the compiled binary, with `--sim-backends`), each
//! with its own config and temporary data directory, paired by preprovisioned
//! trust files and joined by QUIC over loopback. The test is the UI: it speaks
//! to each agent over its IPC socket, like the app.
//!
//! Linux only (T-075 requires it): the Unix socket lives in the data directory,
//! and on macOS the temporary directory's path can exceed the 104-byte limit.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs::File;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, PeerStatus, Reply, TxnEvent, UiCommand};
use tandemd::agent::BUSY;
use tandemd::ipc::{self, IpcClient};
use tandemd::sim::{self, HOLD_FILE};
use tandemd::store::StateStore;
use tokio::net::UnixStream;

/// Short times so a hung step or a lost PC resolve soon.
const TIMING: &str = "
[orchestrator]
power_on_timeout_secs = 5
step_timeout_secs = 3

[power]
pc_lost_after_secs = 1
";

/// Cap so a broken test fails instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

type Ui = IpcClient<UnixStream>;

async fn within<F: Future>(what: &str, f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .unwrap_or_else(|_| panic!("exceeded the maximum time: {what}"))
}

/// The development sandbox (WSL) does not allow creating Unix sockets; in CI it is never skipped.
fn unix_sockets_allowed() -> bool {
    let dir = tempfile::tempdir().unwrap();
    match std::os::unix::net::UnixListener::bind(dir.path().join("probe.sock")) {
        Ok(_) => true,
        Err(e)
            if e.kind() == std::io::ErrorKind::PermissionDenied
                && std::env::var_os("CI").is_none() =>
        {
            eprintln!("skipped: this environment does not allow Unix sockets");
            false
        }
        Err(e) => panic!("{e}"),
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

/// The identity of each side: from two fixed seeds, the one with the lower key is A's
/// (DECISIONS A1).
fn identity(machine: Machine) -> Identity {
    let (x, y) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (low, high) = if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    };
    match machine {
        Machine::A => low,
        Machine::B => high,
    }
}

/// What an agent needs to start: config, identity and trust in the other.
#[derive(Clone)]
struct Spec {
    machine: Machine,
    config: PathBuf,
    data: PathBuf,
}

impl Spec {
    fn new(root: &Path, machine: Machine, port: u16, peer_port: u16, peer: PublicKey) -> Self {
        let data = root.join(machine.to_string());
        std::fs::create_dir_all(&data).unwrap();
        identity(machine).save(&data.join("identity.key")).unwrap();
        TrustStore::open(data.join("trust.toml"))
            .unwrap()
            .add_and_save(TrustedPeer {
                public_key: peer,
                name: machine.other().to_string(),
            })
            .unwrap();
        let config = root.join(format!("{machine}.toml"));
        std::fs::write(
            &config,
            format!(
                "machine = \"{machine}\"\n\
                 [network]\nport = {port}\n\
                 peers = [{{ kind = \"usb4\", addr = \"127.0.0.1:{peer_port}\" }}]\n\
                 {TIMING}"
            ),
        )
        .unwrap();
        Spec {
            machine,
            config,
            data,
        }
    }

    fn socket(&self) -> PathBuf {
        ipc::default_endpoint(&self.data).unwrap()
    }

    fn steps(&self) -> Vec<Step> {
        sim::recorded(&self.data).unwrap()
    }

    fn saved_state(&self) -> Option<State> {
        StateStore::new(self.data.join("state.json"))
            .load()
            .unwrap()
    }

    fn hold(&self, on: bool) {
        let path = self.data.join(HOLD_FILE);
        if on {
            std::fs::write(path, "").unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
    }
}

/// A running `tandemd`. If the test fails, it shows its output.
struct Proc {
    spec: Spec,
    child: Child,
    output: PathBuf,
}

impl Proc {
    fn spawn(spec: Spec) -> Self {
        let output = spec.data.with_extension("out");
        let out = File::options()
            .create(true)
            .append(true)
            .open(&output)
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_tandemd"))
            .arg("--config")
            .arg(&spec.config)
            .arg("--data-dir")
            .arg(&spec.data)
            .arg("--sim-backends")
            .env("TANDEM_LOG", "tandemd=debug,tandem_link=debug")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap();
        Proc {
            spec,
            child,
            output,
        }
    }

    /// Connects a UI when the socket is ready; if the process died before, it fails.
    async fn ui(&mut self) -> Ui {
        let socket = self.spec.socket();
        let ui = within("connect the UI", async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    panic!("{} ended at startup: {status}", self.spec.machine);
                }
                if let Ok(ui) = ipc::connect(&socket).await {
                    return ui;
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await;
        assert_eq!(ui.agent(), self.spec.machine);
        ui
    }

    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// SIGKILL: without orderly shutdown, like a power cut or a hang.
    fn kill(mut self) -> Spec {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.spec.clone()
    }

    /// SIGTERM: the service's orderly shutdown.
    async fn stop(mut self) -> (Spec, ExitStatus) {
        let pid = self.child.id().to_string();
        assert!(Command::new("kill").arg(&pid).status().unwrap().success());
        let status = within("orderly shutdown", async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await;
        (self.spec.clone(), status)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let out = std::fs::read_to_string(&self.output).unwrap_or_default();
            eprintln!("--- output of {} ---\n{out}", self.spec.machine);
        }
    }
}

/// The two agents, with a UI connected to each.
struct Pair {
    root: tempfile::TempDir,
    ports: BTreeMap<Machine, u16>,
}

impl Pair {
    fn new() -> Self {
        Pair {
            root: tempfile::tempdir().unwrap(),
            ports: BTreeMap::from([(Machine::A, free_port()), (Machine::B, free_port())]),
        }
    }

    fn spec(&self, machine: Machine) -> Spec {
        Spec::new(
            self.root.path(),
            machine,
            self.ports[&machine],
            self.ports[&machine.other()],
            identity(machine.other()).public_key(),
        )
    }

    async fn start(&self, machine: Machine) -> (Proc, Ui) {
        restart(self.spec(machine)).await
    }
}

/// Starts an agent (again, if `spec` was already used) and connects a UI to it.
async fn restart(spec: Spec) -> (Proc, Ui) {
    let mut p = Proc::spawn(spec);
    let ui = p.ui().await;
    (p, ui)
}

/// Waits for the agent to see the other over some link.
async fn linked(ui: &mut Ui) {
    within("link with the other agent", async {
        loop {
            match ui.request(UiCommand::GetPeer).await.unwrap() {
                Reply::Peer(PeerStatus::Paired { link: Some(_), .. }) => return,
                Reply::Peer(_) => tokio::time::sleep(POLL).await,
                other => panic!("unexpected reply: {other:?}"),
            }
        }
    })
    .await;
}

/// Waits for the agent's state to satisfy `pred` and returns it.
async fn state_until(ui: &mut Ui, what: &str, pred: impl Fn(&State) -> bool) -> State {
    within(what, async {
        loop {
            let s = ui.get_state().await.unwrap();
            if pred(&s) {
                return s;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
}

/// The next notice that satisfies `pred`; the others are discarded.
async fn event_until(ui: &mut Ui, what: &str, pred: impl Fn(&AgentToUi) -> bool) -> AgentToUi {
    within(what, async {
        loop {
            let ev = ui
                .next_event()
                .await
                .unwrap()
                .expect("the agent closed the IPC");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
}

/// Requests `cmd` (retries if the orchestrator is busy, for example with `pc_watch`).
async fn begin(ui: &mut Ui, cmd: UiCommand) -> u64 {
    within("request accepted", async {
        loop {
            match ui.request(cmd.clone()).await.unwrap() {
                Reply::Accepted { txn } => return txn,
                Reply::Rejected(why) if why == BUSY => tokio::time::sleep(POLL).await,
                other => panic!("{cmd:?}: unexpected reply: {other:?}"),
            }
        }
    })
    .await
}

/// The notices of transaction `txn` up to its outcome.
async fn outcome(ui: &mut Ui, txn: u64) -> Vec<TxnEvent> {
    let mut events = Vec::new();
    loop {
        let AgentToUi::Txn(ev) = event_until(
            ui,
            "transaction outcome",
            |e| matches!(e, AgentToUi::Txn(ev) if txn_of(ev) == txn),
        )
        .await
        else {
            unreachable!()
        };
        let end = matches!(ev, TxnEvent::Committed { .. } | TxnEvent::Failed { .. });
        events.push(ev);
        if end {
            return events;
        }
    }
}

fn txn_of(ev: &TxnEvent) -> u64 {
    match ev {
        TxnEvent::Started { txn, .. }
        | TxnEvent::StepStarted { txn, .. }
        | TxnEvent::StepDone { txn, .. }
        | TxnEvent::StepFailed { txn, .. }
        | TxnEvent::StepUndone { txn, .. }
        | TxnEvent::Committed { txn, .. }
        | TxnEvent::Failed { txn, .. } => *txn,
    }
}

/// Requests `cmd` on the Mac, checks it was applied step by step and that the PC ended with
/// the same state (it receives it pushed over its IPC). Returns the steps and the state.
async fn commit(mac: &mut Ui, pc: &mut Ui, cmd: UiCommand) -> (Vec<Step>, State) {
    let txn = begin(mac, cmd.clone()).await;
    let events = outcome(mac, txn).await;
    let Some(TxnEvent::Started { steps, .. }) = events.first() else {
        panic!("{cmd:?}: Started was expected: {events:?}");
    };
    let steps = steps.clone();
    let Some(TxnEvent::Committed { state, .. }) = events.last() else {
        panic!("{cmd:?}: was not applied: {events:?}");
    };
    let state = state.clone();
    // Each step started and ended, in order.
    let mut expected = vec![TxnEvent::Started {
        txn,
        steps: steps.clone(),
    }];
    for index in 0..steps.len() as u32 {
        expected.push(TxnEvent::StepStarted { txn, index });
        expected.push(TxnEvent::StepDone { txn, index });
    }
    expected.push(TxnEvent::Committed {
        txn,
        state: state.clone(),
    });
    assert_eq!(events, expected, "{cmd:?}");
    assert_eq!(mac.get_state().await.unwrap(), state);
    pc_agrees(pc, &state).await;
    (steps, state)
}

/// `state` as `machine` saves it: each agent sees itself as on
/// (`Agent::set_state`), even if the other machine considers it off.
fn as_seen_by(state: &State, machine: Machine) -> State {
    let mut seen = state.clone();
    seen.on.set(machine, true);
    seen
}

/// The PC has `expected` (seen by it): it already had it or it arrives pushed over the IPC.
async fn pc_agrees(pc: &mut Ui, expected: &State) {
    let expected = &as_seen_by(expected, Machine::B);
    if pc.get_state().await.unwrap() == *expected {
        return;
    }
    event_until(
        pc,
        "the PC receives the final state",
        |e| matches!(e, AgentToUi::State(s) if s == expected),
    )
    .await;
    assert_eq!(pc.get_state().await.unwrap(), *expected);
}

/// The reason the agent rejects `cmd` (retries if busy).
async fn rejected(ui: &mut Ui, cmd: UiCommand) -> String {
    within("request rejected", async {
        loop {
            match ui.request(cmd.clone()).await.unwrap() {
                Reply::Rejected(why) if why == BUSY => tokio::time::sleep(POLL).await,
                Reply::Rejected(why) => return why,
                other => panic!("{cmd:?}: a rejection was expected: {other:?}"),
            }
        }
    })
    .await
}

/// What was added to `spec`'s trace since it had `before` steps.
fn new_steps(spec: &Spec, before: usize) -> Vec<Step> {
    spec.steps().split_off(before)
}

/// Brings up both agents and waits for them to see each other and the Mac to consider the PC on.
async fn running_pair(pair: &Pair) -> (Proc, Ui, Proc, Ui) {
    let (mac, mut ui_mac) = pair.start(Machine::A).await;
    let (pc, mut ui_pc) = pair.start(Machine::B).await;
    linked(&mut ui_mac).await;
    linked(&mut ui_pc).await;
    // `pc_watch`: the PC appears over a link, so it is on.
    state_until(&mut ui_mac, "the Mac considers the PC on", |s| s.on.b).await;
    (mac, ui_mac, pc, ui_pc)
}

fn off(machine: Machine) -> UiCommand {
    UiCommand::SetPower { machine, on: false }
}

#[tokio::test(flavor = "multi_thread")]
async fn power_across_two_real_processes() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mac, mut ui_mac, pc, mut ui_pc) = running_pair(&pair).await;

    // Powering on what is already on is an empty transaction; the PC stays the same.
    let (steps, s) = commit(
        &mut ui_mac,
        &mut ui_pc,
        UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        },
    )
    .await;
    assert!(steps.is_empty(), "{steps:?}");
    assert!(s.on.b);

    // Powered off: the step runs on the PC.
    let (mac_before, pc_before) = (mac.spec.steps().len(), pc.spec.steps().len());
    let (steps, s) = commit(&mut ui_mac, &mut ui_pc, off(Machine::B)).await;
    let want = [Step::PowerOff {
        machine: Machine::B,
    }];
    assert_eq!(steps, want);
    assert!(!s.on.b);
    assert_eq!(new_steps(&pc.spec, pc_before), want);
    assert!(new_steps(&mac.spec, mac_before).is_empty());

    // The orderly shutdown of both saves the state, and each sees itself as on
    // itself (the PC saves `on.b = true` even though the Mac considers it off).
    drop((ui_mac, ui_pc));
    for p in [mac, pc] {
        let (spec, status) = p.stop().await;
        assert!(status.success(), "{}: {status}", spec.machine);
        let saved = spec.saved_state().unwrap();
        assert!(saved.on.is_on(spec.machine), "{}: {saved:?}", spec.machine);
    }
}

/// The PC dies with the shutdown midway: the Mac does not receive the reply, takes it as done and
/// keeps serving; without links it keeps seeing it off.
#[tokio::test(flavor = "multi_thread")]
async fn killing_the_pc_mid_shutdown_leaves_the_mac_coherent() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mut mac, mut ui_mac, pc, _ui_pc) = running_pair(&pair).await;
    pc.spec.hold(true);
    let txn = begin(&mut ui_mac, off(Machine::B)).await;
    within("the PC starts shutting down", async {
        while pc.spec.steps().is_empty() {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    let _pc = pc.kill();

    let events = outcome(&mut ui_mac, txn).await;
    assert!(
        matches!(events.last(), Some(TxnEvent::Committed { state, .. }) if !state.on.b),
        "{events:?}"
    );
    let s = state_until(&mut ui_mac, "the PC considered off", |s| !s.on.b).await;
    assert!(matches!(
        ui_mac.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Paired { link: None, .. })
    ));
    // It keeps serving: the orchestrator was not left taken.
    assert_eq!(
        rejected(
            &mut ui_mac,
            UiCommand::SetPower {
                machine: Machine::B,
                on: true
            }
        )
        .await,
        "machine B is off and there is no way to power it on: the MAC of its network card (Wake-on-LAN) is missing; it is learned when pairing or written on the machine's page"
    );
    assert!(mac.running());
    let (spec, status) = mac.stop().await;
    assert!(status.success());
    assert_eq!(spec.saved_state(), Some(s));
}

/// The Mac dies with the PC midway through the step it asked for: the PC reports it, keeps its
/// state and when the Mac returns it can continue.
#[tokio::test(flavor = "multi_thread")]
async fn killing_the_mac_mid_change_leaves_the_pc_coherent() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mac, mut ui_mac, mut pc, mut ui_pc) = running_pair(&pair).await;
    pc.spec.hold(true);
    let pc_before = pc.spec.steps().len();
    begin(&mut ui_mac, off(Machine::B)).await;
    within("the PC starts the Mac's step", async {
        while pc.spec.steps().len() == pc_before {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    drop(ui_mac);
    let mac_spec = mac.kill();
    pc.spec.hold(false);

    event_until(&mut ui_pc, "the PC stops seeing the Mac", |e| {
        matches!(e, AgentToUi::Peer(PeerStatus::Paired { link: None, .. }))
    })
    .await;
    assert!(ui_pc.get_state().await.unwrap().on.b);
    assert!(pc.running());

    // The Mac returns and they see each other again.
    let (_mac, mut ui_mac) = restart(mac_spec).await;
    linked(&mut ui_mac).await;
    linked(&mut ui_pc).await;
    state_until(&mut ui_mac, "the Mac considers the PC on", |s| s.on.b).await;
}
