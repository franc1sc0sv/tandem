//! Dos procesos `tandemd` de verdad (el binario compilado, con `--sim-backends`), cada
//! uno con su config y su directorio de datos temporales, emparejados por archivos de
//! confianza preprovisionados y unidos por QUIC en loopback. La prueba es la UI: habla
//! con cada agente por su socket de IPC, como la app.
//!
//! Solo en Linux (lo pide la tarea T-075): el socket Unix vive en el directorio de
//! datos, y en macOS la ruta del directorio temporal puede pasar el límite de 104 bytes.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs::File;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use tandem_core::{Machine, Mode, MonitorId, State, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, PeerStatus, PowerEvent, Reply, TxnEvent, UiCommand};
use tandemd::agent::BUSY;
use tandemd::ipc::{self, IpcClient};
use tandemd::sim::{self, HOLD_FILE};
use tandemd::store::StateStore;
use tokio::net::UnixStream;

const DESK: &str = r#"
[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

/// Tiempos cortos para que un paso colgado o una PC perdida se resuelvan pronto.
const TIMING: &str = "
[orchestrator]
power_on_timeout_secs = 5
step_timeout_secs = 3

[power]
pc_lost_after_secs = 1
";

/// Tope para que una prueba rota falle en vez de colgarse.
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

type Ui = IpcClient<UnixStream>;

async fn within<F: Future>(what: &str, f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .unwrap_or_else(|_| panic!("superó el tiempo máximo: {what}"))
}

/// El sandbox de desarrollo (WSL) no deja crear sockets Unix; en CI nunca se omite.
fn unix_sockets_allowed() -> bool {
    let dir = tempfile::tempdir().unwrap();
    match std::os::unix::net::UnixListener::bind(dir.path().join("probe.sock")) {
        Ok(_) => true,
        Err(e)
            if e.kind() == std::io::ErrorKind::PermissionDenied
                && std::env::var_os("CI").is_none() =>
        {
            eprintln!("omitida: este entorno no permite sockets Unix");
            false
        }
        Err(e) => panic!("{e}"),
    }
}

/// Un puerto UDP libre de loopback. Se suelta enseguida para que lo tome el agente:
/// cada uno necesita conocer el del otro antes de arrancar.
fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn identity(machine: Machine) -> Identity {
    Identity::from_seed(match machine {
        Machine::Mac => [1; 32],
        Machine::Pc => [2; 32],
    })
}

/// Lo que necesita un agente para arrancar: config, identidad y confianza en el otro.
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
                 {TIMING}{DESK}"
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

/// Un `tandemd` corriendo. Si la prueba falla, muestra su salida.
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

    /// Conecta una UI cuando el socket esté listo; si el proceso murió antes, falla.
    async fn ui(&mut self) -> Ui {
        let socket = self.spec.socket();
        let ui = within("conectar la UI", async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    panic!("{} terminó al arrancar: {status}", self.spec.machine);
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

    /// SIGKILL: sin apagado ordenado, como un corte de corriente o un cuelgue.
    fn kill(mut self) -> Spec {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.spec.clone()
    }

    /// SIGTERM: el apagado ordenado del servicio.
    async fn stop(mut self) -> (Spec, ExitStatus) {
        let pid = self.child.id().to_string();
        assert!(Command::new("kill").arg(&pid).status().unwrap().success());
        let status = within("apagado ordenado", async {
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
            eprintln!("--- salida de {} ---\n{out}", self.spec.machine);
        }
    }
}

/// Los dos agentes, con una UI conectada a cada uno.
struct Pair {
    root: tempfile::TempDir,
    ports: BTreeMap<Machine, u16>,
}

impl Pair {
    fn new() -> Self {
        Pair {
            root: tempfile::tempdir().unwrap(),
            ports: BTreeMap::from([(Machine::Mac, free_port()), (Machine::Pc, free_port())]),
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

/// Arranca un agente (otra vez, si `spec` ya se usó) y le conecta una UI.
async fn restart(spec: Spec) -> (Proc, Ui) {
    let mut p = Proc::spawn(spec);
    let ui = p.ui().await;
    (p, ui)
}

/// Espera a que el agente vea al otro por algún enlace.
async fn linked(ui: &mut Ui) {
    within("enlace con el otro agente", async {
        loop {
            match ui.request(UiCommand::GetPeer).await.unwrap() {
                Reply::Peer(PeerStatus::Paired { link: Some(_), .. }) => return,
                Reply::Peer(_) => tokio::time::sleep(POLL).await,
                other => panic!("respuesta inesperada: {other:?}"),
            }
        }
    })
    .await;
}

/// Espera a que el estado del agente cumpla `pred` y lo devuelve.
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

/// El siguiente aviso que cumpla `pred`; los demás se descartan.
async fn event_until(ui: &mut Ui, what: &str, pred: impl Fn(&AgentToUi) -> bool) -> AgentToUi {
    within(what, async {
        loop {
            let ev = ui
                .next_event()
                .await
                .unwrap()
                .expect("el agente cerró la IPC");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
}

/// Pide `cmd` (reintenta si el orquestador está ocupado, por ejemplo con `pc_watch`).
async fn begin(ui: &mut Ui, cmd: UiCommand) -> u64 {
    within("pedido aceptado", async {
        loop {
            match ui.request(cmd.clone()).await.unwrap() {
                Reply::Accepted { txn } => return txn,
                Reply::Rejected(why) if why == BUSY => tokio::time::sleep(POLL).await,
                other => panic!("{cmd:?}: respuesta inesperada: {other:?}"),
            }
        }
    })
    .await
}

/// Los avisos de la transacción `txn` hasta su desenlace.
async fn outcome(ui: &mut Ui, txn: u64) -> Vec<TxnEvent> {
    let mut events = Vec::new();
    loop {
        let AgentToUi::Txn(ev) = event_until(
            ui,
            "desenlace de la transacción",
            |e| matches!(e, AgentToUi::Txn(ev) if txn_of(ev) == txn),
        )
        .await
        else {
            unreachable!()
        };
        let end = matches!(
            ev,
            TxnEvent::Committed { .. }
                | TxnEvent::RolledBack { .. }
                | TxnEvent::Inconsistent { .. }
        );
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
        | TxnEvent::RollingBack { txn }
        | TxnEvent::Committed { txn, .. }
        | TxnEvent::RolledBack { txn, .. }
        | TxnEvent::Inconsistent { txn, .. } => *txn,
    }
}

/// Pide `cmd` en la Mac, comprueba que se aplicó paso por paso y que la PC terminó con
/// el mismo estado (lo recibe empujado por su IPC). Devuelve los pasos y el estado.
async fn commit(mac: &mut Ui, pc: &mut Ui, cmd: UiCommand) -> (Vec<Step>, State) {
    let txn = begin(mac, cmd.clone()).await;
    let events = outcome(mac, txn).await;
    let Some(TxnEvent::Started { steps, .. }) = events.first() else {
        panic!("{cmd:?}: se esperaba Started: {events:?}");
    };
    let steps = steps.clone();
    let Some(TxnEvent::Committed { state, .. }) = events.last() else {
        panic!("{cmd:?}: no se aplicó: {events:?}");
    };
    let state = state.clone();
    // Cada paso empezó y terminó, en orden.
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

/// La PC tiene `expected`: ya lo tenía o le llega empujado por la IPC.
async fn pc_agrees(pc: &mut Ui, expected: &State) {
    if pc.get_state().await.unwrap() == *expected {
        return;
    }
    event_until(
        pc,
        "la PC recibe el estado final",
        |e| matches!(e, AgentToUi::State(s) if s == expected),
    )
    .await;
    assert_eq!(pc.get_state().await.unwrap(), *expected);
}

fn all_on(state: &State, machine: Machine) -> bool {
    state.owner.values().all(|m| *m == machine)
}

/// Lo que se agregó al rastro de `spec` desde que tenía `before` pasos.
fn new_steps(spec: &Spec, before: usize) -> Vec<Step> {
    spec.steps().split_off(before)
}

/// Levanta los dos agentes y espera a que se vean y la Mac dé la PC por prendida.
async fn running_pair(pair: &Pair) -> (Proc, Ui, Proc, Ui) {
    let (mac, mut ui_mac) = pair.start(Machine::Mac).await;
    let (pc, mut ui_pc) = pair.start(Machine::Pc).await;
    linked(&mut ui_mac).await;
    linked(&mut ui_pc).await;
    // `pc_watch`: la PC aparece por un enlace, así que está prendida.
    state_until(&mut ui_mac, "la Mac da la PC por prendida", |s| s.pc_on).await;
    (mac, ui_mac, pc, ui_pc)
}

#[tokio::test(flavor = "multi_thread")]
async fn modes_across_two_real_processes() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mac, mut ui_mac, pc, mut ui_pc) = running_pair(&pair).await;

    // Solo Mac: ya está así, transacción vacía; la PC queda con el mismo estado.
    let (steps, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetMode(Mode::SoloMac)).await;
    assert!(steps.is_empty());
    assert!(s.pc_on && all_on(&s, Machine::Mac));

    // Compartido: el 34" a la PC. La PC cambia la entrada y lo agrega; la Mac lo quita.
    let (mac_before, pc_before) = (mac.spec.steps().len(), pc.spec.steps().len());
    let shared = Mode::Shared([("m34".into(), Machine::Pc)].into());
    let (steps, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetMode(shared)).await;
    assert_eq!(s.owner[&MonitorId::from("m34")], Machine::Pc);
    assert_eq!(s.owner[&MonitorId::from("m32")], Machine::Mac);
    let switch = steps
        .iter()
        .find(|st| matches!(st, Step::SwitchInput { .. }))
        .unwrap()
        .clone();
    let pc_steps = new_steps(&pc.spec, pc_before);
    let mac_steps = new_steps(&mac.spec, mac_before);
    assert!(pc_steps.contains(&switch), "{pc_steps:?}");
    assert!(pc_steps.contains(&Step::Attach {
        machine: Machine::Pc,
        monitor: "m34".into()
    }));
    assert!(mac_steps.contains(&Step::Detach {
        machine: Machine::Mac,
        monitor: "m34".into()
    }));
    // La geometría se recalcula en las dos máquinas.
    assert!(pc_steps.contains(&Step::UpdateInputGeometry));
    assert!(mac_steps.contains(&Step::UpdateInputGeometry));
    assert!(
        !mac_steps.contains(&switch),
        "las entradas las cambia la PC"
    );

    // Solo Windows: todo a la PC.
    let (_, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetMode(Mode::SoloPc)).await;
    assert!(s.pc_on && all_on(&s, Machine::Pc));

    // Apagado: primero todo vuelve a la Mac, al final se apaga la PC (en la PC).
    let pc_before = pc.spec.steps().len();
    let (steps, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetPcPower(false)).await;
    assert_eq!(steps.last(), Some(&Step::PowerOffPc));
    assert!(!s.pc_on && all_on(&s, Machine::Mac));
    assert_eq!(
        new_steps(&pc.spec, pc_before).last(),
        Some(&Step::PowerOffPc)
    );

    // El apagado ordenado de los dos guarda el mismo estado.
    drop((ui_mac, ui_pc));
    for p in [mac, pc] {
        let (spec, status) = p.stop().await;
        assert!(status.success(), "{}: {status}", spec.machine);
        assert_eq!(spec.saved_state(), Some(s.clone()), "{}", spec.machine);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn killing_the_pc_mid_change_leaves_the_mac_coherent() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mut mac, mut ui_mac, pc, _ui_pc) = running_pair(&pair).await;
    let before = ui_mac.get_state().await.unwrap();
    assert!(all_on(&before, Machine::Mac));

    // La PC se queda en su primer paso (cambiar una entrada) y ahí se la mata.
    pc.spec.hold(true);
    let txn = begin(&mut ui_mac, UiCommand::SetMode(Mode::SoloPc)).await;
    within("la PC empieza a cambiar una entrada", async {
        while !pc
            .spec
            .steps()
            .iter()
            .any(|s| matches!(s, Step::SwitchInput { .. }))
        {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    let _pc = pc.kill();

    // La Mac no sabe si la entrada llegó a cambiar y no la puede devolver: lo dice.
    let events = outcome(&mut ui_mac, txn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, TxnEvent::StepFailed { index: 0, .. })),
        "{events:?}"
    );
    let Some(TxnEvent::Inconsistent { undo_failures, .. }) = events.last() else {
        panic!("se esperaba Inconsistent: {events:?}");
    };
    assert!(
        undo_failures.iter().any(|f| matches!(
            f.step,
            Step::SwitchInput {
                to: Machine::Mac,
                ..
            }
        )),
        "{undo_failures:?}"
    );
    // Ningún monitor se dio por movido.
    assert_eq!(ui_mac.get_state().await.unwrap().owner, before.owner);

    // Sin enlaces, la Mac da la PC por apagada y lo avisa; todo sigue en la Mac.
    let ev = event_until(&mut ui_mac, "aviso de PC perdida", |e| {
        matches!(e, AgentToUi::Power(PowerEvent::PcLost { .. }))
    })
    .await;
    let AgentToUi::Power(PowerEvent::PcLost { stranded, .. }) = ev else {
        unreachable!()
    };
    assert!(stranded.is_empty());
    let s = state_until(&mut ui_mac, "la PC dada por apagada", |s| !s.pc_on).await;
    assert_eq!(s.owner, before.owner);
    assert!(matches!(
        ui_mac.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Paired { link: None, .. })
    ));

    // Y sigue atendiendo: el orquestador no quedó tomado.
    let txn = begin(&mut ui_mac, UiCommand::SetMode(Mode::SoloMac)).await;
    assert!(matches!(
        outcome(&mut ui_mac, txn).await.last(),
        Some(TxnEvent::Committed { .. })
    ));
    assert!(mac.running());
    let (spec, status) = mac.stop().await;
    assert!(status.success());
    assert_eq!(spec.saved_state(), Some(s));
}

#[tokio::test(flavor = "multi_thread")]
async fn killing_the_mac_mid_change_leaves_the_pc_coherent() {
    if !unix_sockets_allowed() {
        return;
    }
    let pair = Pair::new();
    let (mac, mut ui_mac, mut pc, mut ui_pc) = running_pair(&pair).await;
    let shared = Mode::Shared([("m34".into(), Machine::Pc)].into());
    let (_, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetMode(shared)).await;

    // La Mac pide Solo Windows y muere con la PC a mitad del primer paso.
    pc.spec.hold(true);
    let pc_before = pc.spec.steps().len();
    begin(&mut ui_mac, UiCommand::SetMode(Mode::SoloPc)).await;
    within("la PC empieza el paso de la Mac", async {
        while pc.spec.steps().len() == pc_before {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    drop(ui_mac);
    let mac_spec = mac.kill();
    pc.spec.hold(false);

    // La PC lo reporta (sin enlace) y se queda con el último estado confirmado.
    event_until(&mut ui_pc, "la PC deja de ver a la Mac", |e| {
        matches!(e, AgentToUi::Peer(PeerStatus::Paired { link: None, .. }))
    })
    .await;
    assert_eq!(ui_pc.get_state().await.unwrap(), s);
    assert!(pc.running());

    // La Mac vuelve con lo que había guardado: coincide con la PC y se puede seguir.
    let (_mac, mut ui_mac) = restart(mac_spec).await;
    assert_eq!(ui_mac.get_state().await.unwrap(), s);
    linked(&mut ui_mac).await;
    linked(&mut ui_pc).await;
    let (_, s) = commit(&mut ui_mac, &mut ui_pc, UiCommand::SetMode(Mode::SoloPc)).await;
    assert!(all_on(&s, Machine::Pc));
}
