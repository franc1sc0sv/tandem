//! El asistente de configuración inicial (T-087) con dos `tandemd` reales por loopback y el
//! puesto real simulado: cada monitor tiene un id PnP distinto por entrada (DisplayPort en
//! la PC, HDMI en la Mac). Solo los pasos de cada máquina y los detectores de monitores
//! son simulados.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{DdcProfile, Input, Machine, Mode, Step};
use tandem_display::switcher::ManualSwitcher;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, LinkBasis, MonitorScan, NewMonitor, Reply, TxnEvent, UiCommand};
use tandemd::capabilities::Seen;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::wizard::MonitorScanner;
use tandemd::{Agent, AgentConfig, StoreError, autoconfig};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

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

/// Lo que detecta una máquina; se puede cambiar para simular que se desconecta un monitor.
#[derive(Debug, Clone, Default)]
struct World(Arc<Mutex<Vec<Seen>>>);

impl World {
    fn set(&self, seen: Vec<Seen>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = seen;
    }
}

impl MonitorScanner for World {
    fn scan(&self) -> Result<Vec<Seen>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }
}

fn seen(
    pnp: &str,
    model: &str,
    name: &str,
    serial: u32,
    inches: u32,
    output: Option<Input>,
) -> Seen {
    Seen {
        pnp_id: Some(pnp.into()),
        model: Some(model.into()),
        name: name.into(),
        serial: Some(serial),
        inches: Some(inches),
        output,
    }
}

/// El 34" y el 32" como los ve la PC (DisplayPort).
fn pc_view() -> Vec<Seen> {
    vec![
        seen(
            "SAM711A",
            "LC34G55T",
            "34\"",
            0x4D30_4E30,
            34,
            Some(Input::DisplayPort),
        ),
        seen(
            "SAM7489",
            "Odyssey G5",
            "32\"",
            0x4D30_4E31,
            32,
            Some(Input::DisplayPort),
        ),
    ]
}

/// Los mismos, como los ve la Mac (HDMI): otro id PnP, el mismo número de serie, y
/// CoreGraphics no dice por qué cable.
fn mac_view() -> Vec<Seen> {
    vec![
        seen("SAM7119", "LC34G55T", "34\"", 0x4D30_4E30, 34, None),
        seen("SAM748A", "Odyssey G5", "32\"", 0x4D30_4E31, 32, None),
    ]
}

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
    world: World,
    ui: IpcClient<tokio::io::DuplexStream>,
    config: PathBuf,
    data: PathBuf,
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

    /// Un perfil del usuario para `pnp`, como el que deja una calibración.
    fn write_user_profile(&self, pnp: &str) -> PathBuf {
        let dir = self.data.join("profiles").join("ddc");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{pnp}.toml"));
        std::fs::write(
            &path,
            format!(
                "pnp_id = \"{pnp}\"\nname = \"Mío\"\n[codes]\nhdmi = 0x03\ndisplay_port = 0x04\n"
            ),
        )
        .unwrap();
        path
    }
}

/// Un agente armado como en el binario, con la configuración que genera el primer arranque
/// (sin monitores) y los enlaces de loopback.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    world: World,
) -> Running {
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
    let generated = autoconfig::generate(machine).replace(
        &format!("port = {}", tandemd::config::DEFAULT_PORT),
        &format!(
            "port = {port}\npeers = [{{ kind = \"usb4\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    let text = format!("{generated}\n[power.wol]\nmac = \"9c:6b:00:1a:2b:ff\"\n");
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::from_toml(&text).unwrap();
    assert!(cfg.desk.monitors.is_empty(), "instalación limpia");
    let steps = Arc::new(Fake::default());
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: steps.clone(),
            manual: Arc::new(ManualSwitcher::new()),
            input: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            ..NetOptions::default()
        },
    )
    .unwrap();
    agent.set_scanner(Arc::new(world.clone()));
    // Como en el binario: lo que esta máquina ve al arrancar.
    agent.set_seen_monitors(world.scan().unwrap());
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
    Running {
        agent,
        steps,
        world,
        ui,
        config: paths.config.clone(),
        data: dir.path().to_owned(),
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

struct Pair {
    mac: Running,
    pc: Running,
}

async fn pair() -> Pair {
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let world = |v| {
        let w = World::default();
        w.set(v);
        w
    };
    let mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        world(mac_view()),
    )
    .await;
    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        world(pc_view()),
    )
    .await;
    // Cada una ya anunció sus capacidades a la otra.
    for a in [&mac.agent, &pc.agent] {
        let mut caps = a.subscribe_capabilities();
        within(caps.wait_for(|c| c.peer.is_some())).await.unwrap();
    }
    Pair { mac, pc }
}

/// Pide el escaneo hasta que se ven los monitores de las dos máquinas: la otra detecta de
/// nuevo y anuncia lo suyo un momento después.
async fn scan_until_linked(ui: &mut IpcClient<tokio::io::DuplexStream>, n: usize) -> MonitorScan {
    within(async {
        loop {
            let Reply::Scan(scan) = ui.request(UiCommand::ScanMonitors).await.unwrap() else {
                panic!("se esperaba un escaneo")
            };
            if scan.peer_known
                && scan.candidates.len() == n
                && scan.candidates.iter().all(|c| c.basis != LinkBasis::Single)
            {
                return scan;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

/// Lo que el usuario confirma en el asistente: las dos identidades de cada monitor y su
/// cableado (la PC por DisplayPort, la Mac por HDMI).
fn chosen(scan: &MonitorScan) -> Vec<NewMonitor> {
    scan.candidates
        .iter()
        .map(|c| NewMonitor {
            label: c.label.clone(),
            pnp_ids: c.pnp_ids.clone(),
            wiring: [
                (
                    Machine::Pc,
                    c.wiring[&Machine::Pc].unwrap_or(Input::DisplayPort),
                ),
                (Machine::Mac, c.wiring[&Machine::Mac].unwrap_or(Input::Hdmi)),
            ]
            .into(),
        })
        .collect()
}

async fn until_desk(agent: &Agent, ok: impl Fn(&tandem_core::Config) -> bool) {
    let mut rx = agent.subscribe_desk();
    within(rx.wait_for(|d| ok(d))).await.unwrap();
}

async fn until_committed(ui: &mut IpcClient<tokio::io::DuplexStream>, txn: u64) {
    within(async {
        loop {
            match ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, .. }) if t == txn => return,
                AgentToUi::Txn(
                    e @ (TxnEvent::RolledBack { .. } | TxnEvent::Inconsistent { .. }),
                ) => panic!("el cambio falló: {e:?}"),
                _ => {}
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_asistente_completo_deja_exactamente_dos_monitores_vinculados_y_el_estado_confirmado() {
    let Pair { mut mac, pc } = pair().await;

    // Instalación limpia: ni monitores, ni estado, ni nada que planear. La escena no
    // tendría qué dibujar hasta pasar por el asistente.
    for a in [&mac.agent, &pc.agent] {
        assert!(a.desk().monitors.is_empty());
        assert!(a.plan_desk().monitors.is_empty());
        assert!(a.state().owner.is_empty());
        assert!(!a.input_status().defined, "nunca se supone el teclado");
    }

    // Paso 4: lo conectado ahora, juntado por monitor físico.
    let scan = scan_until_linked(&mut mac.ui, 2).await;
    assert!(scan.candidates.iter().all(|c| c.basis == LinkBasis::Serial));
    assert!(scan.candidates.iter().all(|c| c.in_desk.is_none()));
    assert!(scan.candidates.iter().all(|c| c.profile.is_some()));
    // Escanear no crea nada.
    assert!(mac.agent.desk().monitors.is_empty());
    assert!(pc.agent.desk().monitors.is_empty());

    // Lo elegido se agrega desde la Mac y llega a la PC.
    let reply = mac
        .ui
        .request(UiCommand::AddMonitors(chosen(&scan)))
        .await
        .unwrap();
    assert_eq!(reply, Reply::Done);
    until_desk(&pc.agent, |d| d.monitors.len() == 2).await;

    // Exactamente dos, iguales en las dos máquinas, con las dos identidades, el perfil de
    // la base y el cableado confirmado; y guardados en `tandem.toml` de cada una.
    let desk = mac.agent.desk();
    assert_eq!(desk, pc.agent.desk());
    assert_eq!(desk.monitors.len(), 2);
    let m34 = &desk.monitors[0];
    assert_eq!(m34.edid_model.as_deref(), Some("SAM711A"));
    assert_eq!(m34.edid_aliases, ["SAM7119"]);
    assert_eq!(m34.label, "34\"");
    assert_eq!(
        m34.ddc,
        Some(DdcProfile {
            hdmi: 0x06,
            display_port: 0x09
        })
    );
    let m32 = &desk.monitors[1];
    assert_eq!(m32.edid_model.as_deref(), Some("SAM7489"));
    assert_eq!(m32.edid_aliases, ["SAM748A"]);
    assert_eq!(
        m32.ddc,
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    for m in &desk.monitors {
        assert_eq!(m.wiring[&Machine::Pc], Input::DisplayPort);
        assert_eq!(m.wiring[&Machine::Mac], Input::Hdmi);
    }
    for r in [&mac, &pc] {
        assert_eq!(AgentConfig::load(&r.config).unwrap().desk, desk);
    }

    // Cada máquina reconoce a sus monitores por la identidad que ve, sin duplicarlos.
    for a in [&mac.agent, &pc.agent] {
        let want = [Some("sam711a".into()), Some("sam7489".into())];
        let mut caps = a.subscribe_capabilities();
        within(caps.wait_for(|c| {
            c.here
                .monitors
                .iter()
                .map(|m| m.monitor.clone())
                .collect::<Vec<_>>()
                == want
        }))
        .await
        .unwrap();
    }
    // Y un segundo escaneo ya los marca como agregados.
    let again = scan_until_linked(&mut mac.ui, 2).await;
    assert!(again.candidates.iter().all(|c| c.in_desk.is_some()));

    // Paso 6: «Aplicar modo» manda todas las entradas y deja el estado confirmado en las dos.
    // Antes, la Mac sabe que la PC puede cambiar la entrada de los dos (su anuncio ya los trae).
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| {
        c.peer.as_ref().is_some_and(|p| {
            p.monitors
                .iter()
                .filter(|m| m.switch && m.monitor.is_some())
                .count()
                == 2
        })
    }))
    .await
    .unwrap();
    let mut state = mac.agent.subscribe();
    within(state.wait_for(|s| s.is_on(Machine::Pc)))
        .await
        .unwrap();
    let txn = loop {
        match mac
            .ui
            .request(UiCommand::ApplyMode(Mode::SoloMac))
            .await
            .unwrap()
        {
            Reply::Accepted { txn } => break txn,
            Reply::Rejected(why) if why == tandemd::agent::BUSY => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => panic!("se esperaba Accepted: {other:?}"),
        }
    };
    until_committed(&mut mac.ui, txn).await;
    let committed = mac.agent.state();
    assert!(committed.is_confirmed(), "{committed:?}");
    assert_eq!(committed.owner.len(), 2);
    let mut pc_state = pc.agent.subscribe();
    within(pc_state.wait_for(|s| *s == committed))
        .await
        .unwrap();
    // Las entradas las cambió la PC, una por monitor.
    let switches = pc
        .steps
        .steps()
        .into_iter()
        .filter(|s| matches!(s, Step::SwitchInput { .. }))
        .count();
    assert_eq!(switches, 2);

    mac.stop().await;
    pc.stop().await;
}

/// T-089: la ida y vuelta del paso 5, con el estado sin confirmar, solo manda la entrada
/// del monitor probado (el 32"): ni el 34", ni ningún escritorio, ni cambia el estado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_ida_y_vuelta_del_asistente_solo_toca_la_entrada_del_monitor_probado() {
    let Pair { mut mac, pc } = pair().await;
    let scan = scan_until_linked(&mut mac.ui, 2).await;
    assert_eq!(
        mac.ui
            .request(UiCommand::AddMonitors(chosen(&scan)))
            .await
            .unwrap(),
        Reply::Done
    );
    until_desk(&pc.agent, |d| d.monitors.len() == 2).await;
    // La Mac sabe que la PC puede cambiar la entrada de los dos (su anuncio ya los trae).
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| {
        c.peer.as_ref().is_some_and(|p| {
            p.monitors
                .iter()
                .filter(|m| m.switch && m.monitor.is_some())
                .count()
                == 2
        })
    }))
    .await
    .unwrap();
    let before = mac.agent.state();
    assert!(!before.is_confirmed());

    let probe = |to, back| UiCommand::ProbeInput {
        monitor: "sam7489".into(),
        to,
        back,
    };
    for (to, back) in [(Machine::Pc, Machine::Mac), (Machine::Mac, Machine::Mac)] {
        let txn = loop {
            match mac.ui.request(probe(to, back)).await.unwrap() {
                Reply::Accepted { txn } => break txn,
                Reply::Rejected(why) if why == tandemd::agent::BUSY => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("se esperaba Accepted: {other:?}"),
            }
        };
        until_committed(&mut mac.ui, txn).await;
    }
    let steps = pc.steps.steps();
    let switched: Vec<_> = steps
        .iter()
        .map(|s| match s {
            Step::SwitchInput { monitor, to, .. } => (monitor.to_string(), *to),
            other => panic!("solo se manda entradas: {other:?}"),
        })
        .collect();
    assert_eq!(
        switched,
        [
            ("sam7489".to_owned(), Machine::Pc),
            ("sam7489".to_owned(), Machine::Mac)
        ]
    );
    assert!(mac.steps.steps().is_empty(), "la Mac no toca su escritorio");
    assert_eq!(mac.agent.state(), before);
    assert!(mac.agent.pending_returns().is_empty());
    mac.stop().await;
    pc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quitar_un_monitor_borra_todo_lo_suyo_en_las_dos_maquinas() {
    let Pair { mut mac, mut pc } = pair().await;
    let scan = scan_until_linked(&mut mac.ui, 2).await;
    mac.ui
        .request(UiCommand::AddMonitors(chosen(&scan)))
        .await
        .unwrap();
    until_desk(&pc.agent, |d| d.monitors.len() == 2).await;
    // Un perfil del usuario del 34" en cada máquina (una calibración previa), y otro del 32".
    let profiles: Vec<PathBuf> = [&mac, &pc]
        .iter()
        .flat_map(|r| {
            [
                r.write_user_profile("SAM711A"),
                r.write_user_profile("SAM7489"),
            ]
        })
        .collect();
    assert!(profiles.iter().all(|p| p.exists()));
    // Y algo de estado de cada uno.
    assert!(mac.agent.state().owner.contains_key(&"sam711a".into()));

    // Se quita el 34" desde la PC.
    let reply = pc
        .ui
        .request(UiCommand::RemoveMonitor {
            monitor: "sam711a".into(),
        })
        .await
        .unwrap();
    assert_eq!(reply, Reply::Done);
    until_desk(&mac.agent, |d| d.monitors.len() == 1).await;

    for r in [&mac, &pc] {
        let desk = r.agent.desk();
        assert_eq!(desk.monitors.len(), 1, "{desk:?}");
        assert_eq!(desk.monitors[0].id, "sam7489".into());
        // Ni su entrada en tandem.toml, ni su alias (el vínculo entre identidades)...
        let saved = std::fs::read_to_string(&r.config).unwrap();
        assert!(
            !saved.contains("SAM711A") && !saved.contains("SAM7119"),
            "{saved}"
        );
        assert_eq!(AgentConfig::load(&r.config).unwrap().desk, desk);
        // ...ni su estado...
        assert!(!r.agent.state().owner.contains_key(&"sam711a".into()));
        assert!(r.agent.state().owner.contains_key(&"sam7489".into()));
        // ...ni su perfil del usuario; el del otro monitor sigue.
        let dir = r.data.join("profiles").join("ddc");
        assert!(!dir.join("SAM711A.toml").exists());
        assert!(dir.join("SAM7489.toml").exists());
    }

    // Restablecer el puesto desde la Mac borra el resto, en las dos.
    assert_eq!(
        mac.ui.request(UiCommand::ResetDesk).await.unwrap(),
        Reply::Done
    );
    until_desk(&pc.agent, |d| d.monitors.is_empty()).await;
    for r in [&mac, &pc] {
        assert!(r.agent.desk().monitors.is_empty());
        assert!(r.agent.state().owner.is_empty());
        assert!(!r.data.join("profiles/ddc/SAM7489.toml").exists());
        let saved = AgentConfig::load(&r.config).unwrap();
        assert!(saved.desk.monitors.is_empty());
    }
    mac.stop().await;
    pc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_monitor_desconectado_no_aparece_en_la_escena_ni_en_los_planes() {
    let Pair { mut mac, pc } = pair().await;
    let scan = scan_until_linked(&mut mac.ui, 2).await;
    mac.ui
        .request(UiCommand::AddMonitors(chosen(&scan)))
        .await
        .unwrap();
    until_desk(&pc.agent, |d| d.monitors.len() == 2).await;
    for a in [&mac.agent, &pc.agent] {
        assert_eq!(a.plan_desk().monitors.len(), 2);
    }

    // Se desconecta el 32" de las dos máquinas.
    mac.world.set(mac_view()[..1].to_vec());
    pc.world.set(pc_view()[..1].to_vec());
    let scan = scan_until_linked(&mut mac.ui, 1).await;
    assert_eq!(scan.candidates.len(), 1);
    assert_eq!(scan.disconnected, 1);
    let mut caps = pc.agent.subscribe_capabilities();
    within(caps.wait_for(|c| {
        c.peer
            .as_ref()
            .is_some_and(|p| p.monitors.iter().filter(|m| m.seen).count() == 1)
    }))
    .await
    .unwrap();
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| {
        c.peer
            .as_ref()
            .is_some_and(|p| p.monitors.iter().filter(|m| m.seen).count() == 1)
    }))
    .await
    .unwrap();

    for a in [&mac.agent, &pc.agent] {
        let plan_desk = a.plan_desk();
        let ids: Vec<_> = plan_desk.monitors.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(
            ids,
            ["sam711a"],
            "la escena y los planes solo ven lo conectado"
        );
        // Sigue en el puesto: al volver a conectarlo no hay que agregarlo otra vez.
        assert_eq!(a.desk().monitors.len(), 2);
    }
    // Un cambio de modo no toca el monitor desconectado.
    let mut state = mac.agent.subscribe();
    within(state.wait_for(|s| s.is_on(Machine::Pc)))
        .await
        .unwrap();
    mac.agent.set_state(mac.agent.state().confirmed()).unwrap();
    let txn = loop {
        match mac
            .ui
            .request(UiCommand::SetMode(Mode::SoloPc))
            .await
            .unwrap()
        {
            Reply::Accepted { txn } => break txn,
            Reply::Rejected(why) if why == tandemd::agent::BUSY => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => panic!("se esperaba Accepted: {other:?}"),
        }
    };
    until_committed(&mut mac.ui, txn).await;
    let touched: Vec<_> = pc
        .steps
        .steps()
        .into_iter()
        .chain(mac.steps.steps())
        .filter_map(|s| match s {
            Step::SwitchInput { monitor, .. }
            | Step::Attach { monitor, .. }
            | Step::Detach { monitor, .. } => Some(monitor.0),
            _ => None,
        })
        .collect();
    assert!(!touched.is_empty());
    assert!(touched.iter().all(|m| m == "sam711a"), "{touched:?}");
    mac.stop().await;
    pc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presionar_una_tecla_dice_en_que_maquina_esta_el_teclado_y_se_recuerda() {
    let Pair { mut mac, mut pc } = pair().await;
    // Las dos escuchan: el asistente de la Mac arma la prueba y la PC se entera.
    assert_eq!(
        mac.ui
            .request(UiCommand::KeyProbe { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let mut probe = pc.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    // La tecla llega a la PC.
    assert_eq!(
        pc.ui.request(UiCommand::HeardKey).await.unwrap(),
        Reply::Done
    );
    let mut probe = mac.agent.subscribe_key_probe();
    let heard = within(probe.wait_for(|s| s.heard.is_some()))
        .await
        .unwrap()
        .heard;
    assert_eq!(heard, Some(Machine::Pc));
    // Una segunda tecla en la otra no cambia lo que se oyó primero.
    mac.ui.request(UiCommand::HeardKey).await.unwrap();
    assert_eq!(mac.agent.key_probe_status().heard, Some(Machine::Pc));

    // «¿Es correcto?»: sí. Se guarda en las dos y deja de ser «sin definir».
    assert_eq!(
        mac.ui
            .request(UiCommand::SetKeyboard(Machine::Pc))
            .await
            .unwrap(),
        Reply::Done
    );
    let mut kb = pc.agent.subscribe_keyboard();
    within(kb.wait_for(|k| *k == Some(Machine::Pc)))
        .await
        .unwrap();
    for r in [&mac, &pc] {
        // El estado se publica un instante después de la declaración: se espera.
        let mut st = r.agent.subscribe_input();
        within(st.wait_for(|i| i.defined && i.server == Machine::Pc))
            .await
            .unwrap();
        let input = r.agent.input_status();
        assert!(input.defined);
        assert_eq!(input.server, Machine::Pc);
        let saved = AgentConfig::load(&r.config).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::Pc));
    }
    mac.stop().await;
    pc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_ping_del_enlace_se_ve_y_cambiar_el_cableado_pide_confirmacion() {
    let Pair { mut mac, pc } = pair().await;
    // El ping sale del gestor de enlaces; con el enlace fijo ya hay un rtt.
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

    let scan = scan_until_linked(&mut mac.ui, 2).await;
    mac.ui
        .request(UiCommand::AddMonitors(chosen(&scan)))
        .await
        .unwrap();
    let swapped = [
        (Machine::Pc, Input::Hdmi),
        (Machine::Mac, Input::DisplayPort),
    ];
    let set = |confirmed| UiCommand::SetWiring {
        monitor: "sam711a".into(),
        wiring: swapped.into(),
        confirmed,
    };
    let Reply::Rejected(why) = mac.ui.request(set(false)).await.unwrap() else {
        panic!("sin confirmar debía rechazarse")
    };
    assert!(why.contains("confirmación"), "{why}");
    assert_eq!(
        mac.agent.desk().monitors[0].wiring[&Machine::Pc],
        Input::DisplayPort
    );
    assert_eq!(mac.ui.request(set(true)).await.unwrap(), Reply::Done);
    assert_eq!(
        mac.agent.desk().monitors[0].wiring[&Machine::Pc],
        Input::Hdmi
    );
    mac.stop().await;
    pc.stop().await;
}
