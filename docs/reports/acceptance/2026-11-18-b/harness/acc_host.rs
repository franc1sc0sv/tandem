//! Anfitrión de aceptación: un `tandemd` real (red QUIC, orquestador, IPC) con los dobles de
//! prueba que `--sim-backends` no trae: motor de entrada con backends falsos, monitores con el bus
//! falso de `monitor-ddc` y el sistema operativo declarado. Un puerto de control (TCP, JSON por
//! línea) hace de «mundo físico»: mover el mouse, conectar el hub, agregar o apagar monitores.
//!
//! Uso: `acc_host --os mac|windows --control PORT [--screen WxH]...` (con `--config`/`--data-dir`
//! o los directorios estándar, como `tandemd`). Se compila con `--features sim-backends`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use monitor_ddc::fake::{FakeBus, FakeClock, FakeMonitorConfig, Personality};
use monitor_ddc::{Input as DdcInput, MonitorHandle};
use serde_json::{Value, json};
use tandem_core::Os;
use tandem_input::geometry::{Rect, Screen};
use tandem_input::session::{FollowConfig, SessionConfig};
use tandem_input::{HidUsage, InputEvent, KeyState};
use tandem_monitor::{LocalConfig, LocalMonitors};
use tandem_peripherals::{UsbId, WatchConfig};
use tandemd::config::AgentConfig;
use tandemd::daemon::{self, Backends};
use tandemd::input::fake::FakeInputBackends;
use tandemd::input::{InputOptions, InputSetup};
use tandemd::network::NetOptions;
use tandemd::paths::{self, Paths};
use tandemd::{ipc, logging};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

struct World {
    fake: FakeInputBackends,
    bus: FakeBus,
    monitors: Mutex<Vec<MonitorHandle>>,
}

fn key(usage: u16, state: KeyState) -> InputEvent {
    InputEvent::Key { usage: HidUsage::key(usage), state }
}

fn personality(s: &str) -> Personality {
    match s {
        "factory" => Personality::Factory,
        "disconnected" => Personality::Disconnected,
        "garbage" => Personality::Garbage,
        _ => Personality::Standard,
    }
}

fn input_of(s: &str) -> DdcInput {
    if s == "hdmi" { DdcInput::Hdmi } else { DdcInput::DisplayPort }
}

fn control(w: &World, v: &Value) -> Value {
    let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
    let id = v["id"].as_u64().unwrap_or(0) as usize;
    let handle = |w: &World| w.monitors.lock().unwrap_or_else(PoisonError::into_inner).get(id).copied();
    match v["cmd"].as_str().unwrap_or("") {
        "abs" => json!({ "sent": w.fake.capture(InputEvent::MotionAbsolute { x: f("x"), y: f("y") }) }),
        "rel" => json!({ "sent": w.fake.capture(InputEvent::MotionRelative { dx: f("dx"), dy: f("dy") }) }),
        "key" => {
            let state = if v["down"].as_bool().unwrap_or(true) { KeyState::Pressed } else { KeyState::Released };
            json!({ "sent": w.fake.capture(key(v["usage"].as_u64().unwrap_or(4) as u16, state)) })
        }
        "capturing" => json!({ "capturing": w.fake.capturing() }),
        "injected" => {
            let all = w.fake.injected().borrow().clone();
            let tail: Vec<String> = all.iter().rev().take(20).map(|e| format!("{e:?}")).collect();
            json!({ "count": all.len(), "tail": tail })
        }
        "devices" => {
            let list: Vec<UsbId> = v["list"]
                .as_array()
                .map(|a| a.iter().filter_map(|d| Some(UsbId::new(d[0].as_u64()? as u16, d[1].as_u64()? as u16))).collect())
                .unwrap_or_default();
            w.fake.devices().set(list);
            json!({ "ok": true })
        }
        "missing" => {
            let list: Vec<tandem_proto::InputPermission> = v["list"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| match x.as_str()? { "accessibility" => Some(tandem_proto::InputPermission::Accessibility), "input_monitoring" => Some(tandem_proto::InputPermission::InputMonitoring), _ => None }).collect())
                .unwrap_or_default();
            w.fake.set_missing(list);
            w.fake.grant_on_request(v["grant_on_request"].as_bool().unwrap_or(false));
            json!({ "ok": true })
        }
        "clip_copy" => {
            let img = v["image"].as_array().map(|a| tandem_clipboard::Image { width: a[0].as_u64().unwrap_or(1) as u32, height: a[1].as_u64().unwrap_or(1) as u32, rgba: vec![255; (a[0].as_u64().unwrap_or(1) * a[1].as_u64().unwrap_or(1) * 4) as usize] });
            w.fake.clipboard().copy(Some(tandem_clipboard::ClipContent { text: v["text"].as_str().map(String::from), html: None, image: img }));
            json!({ "ok": true })
        }
        "clip_get" => {
            let st = w.fake.clipboard().state();
            json!({ "text": st.content.as_ref().and_then(|c| c.text.clone()), "image": st.content.as_ref().and_then(|c| c.image.as_ref()).map(|i| vec![i.width, i.height]), "writes": st.writes.len() })
        }
        "requested" => json!({ "count": w.fake.permission_requests() }),
        "screens" => {
            let list: Vec<Screen> = v["list"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .enumerate()
                        .map(|(i, d)| Screen::new(format!("s{i}").as_str().into(), Rect::new(0, 0, d[0].as_u64().unwrap_or(1920) as u32, d[1].as_u64().unwrap_or(1080) as u32), 1.0))
                        .collect()
                })
                .unwrap_or_default();
            w.fake.set_screens(list);
            json!({ "ok": true })
        }
        "monitor_add" => {
            let mut cfg = FakeMonitorConfig::new(personality(v["personality"].as_str().unwrap_or("standard")));
            let n = w.monitors.lock().unwrap_or_else(PoisonError::into_inner).len();
            let name = v["name"].as_str().unwrap_or("LC34G55T").to_owned();
            let serial = v["serial"].as_u64().map_or(n, |x| x as usize);
            cfg.edid = monitor_ddc::fake::test_edid("SAM", 0x7155 + serial as u16, &name, &format!("FAKE{serial:07}"));
            if let Some(i) = v["via"].as_str() {
                cfg.connected_via = input_of(i);
                let wire = if i == "hdmi" { tandem_core::MonitorInput::Hdmi } else { tandem_core::MonitorInput::DisplayPort };
                tandem_monitor::local::set_edid_input(&mut cfg.edid, Some(wire));
            }
            cfg.description = name;
            if let Some(i) = v["input"].as_str() {
                cfg.input = input_of(i);
            }
            if v["switch"].as_str() == Some("fail") {
                cfg.switch = monitor_ddc::fake::SwitchBehavior::Fail;
            }
            let h = w.bus.add(cfg);
            let mut m = w.monitors.lock().unwrap_or_else(PoisonError::into_inner);
            m.push(h);
            json!({ "id": m.len() - 1 })
        }
        "monitor_remove" => {
            if let Some(h) = handle(w) {
                w.bus.remove(h);
            }
            json!({ "ok": true })
        }
        "monitor_osd" => {
            if let Some(h) = handle(w) {
                w.bus.osd_switch(h, input_of(v["input"].as_str().unwrap_or("dp")));
            }
            json!({ "ok": true })
        }
        "monitor_screen" => json!({ "input": handle(w).and_then(|h| w.bus.screen_input(h)).map(|i| format!("{i:?}")) }),
        "monitor_ops" => {
            let ops: Vec<String> = w.bus.ops().iter().map(|o| format!("{o:?}")).collect();
            json!({ "ops": ops })
        }
        "monitor_clear_ops" => {
            w.bus.clear_ops();
            json!({ "ok": true })
        }
        other => json!({ "error": format!("comando desconocido {other}") }),
    }
}

fn fast() -> InputOptions {
    InputOptions {
        session: SessionConfig {
            heartbeat_interval: Duration::from_millis(100),
            heartbeat_timeout: Duration::from_millis(1500),
            resync_idle: Duration::from_millis(20),
            resync_every: 8,
            latency_every: Duration::from_millis(500),
        },
        hub: WatchConfig { poll: Duration::from_millis(50), debounce: Duration::from_millis(200) },
        follow: FollowConfig::default(),
        report_every: Duration::from_millis(250),
        screens_every: Duration::from_millis(250),
        permissions_every: Duration::from_millis(100),
        retry: Duration::from_millis(200),
        ..InputOptions::default()
    }
}

fn main() -> anyhow::Result<()> {
    let (mut os, mut ctl, mut screens) = (Os::Windows, 0u16, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--os" => os = if args.next().as_deref() == Some("mac") { Os::MacOs } else { Os::Windows },
            "--control" => ctl = args.next().context("falta --control")?.parse()?,
            "--screen" => {
                let s = args.next().context("falta --screen")?;
                let (w, h) = s.split_once('x').context("WxH")?;
                screens.push((w.parse::<u32>()?, h.parse::<u32>()?));
            }
            other => anyhow::bail!("opción desconocida {other}"),
        }
    }
    if screens.is_empty() {
        screens.push((1920, 1080));
    }
    let paths = Paths::resolve(None, None)?;
    paths::create_private_dir(&paths.data_dir)?;
    let _log = logging::init(&paths.log_dir())?;
    let loaded = tandemd::autoconfig::load_or_generate(&paths.config, None)?;
    let mut cfg: AgentConfig = loaded.config;
    cfg.os = os;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async {
        let endpoint = ipc::default_endpoint(&paths.data_dir)?;
        let listener = ipc::bind(&endpoint).await?;
        let fake = FakeInputBackends::new(screens.iter().enumerate().map(|(i, (w, h))| Screen::new(format!("s{i}").as_str().into(), Rect::new(0, 0, *w, *h), 1.0)).collect());
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let ticker = clock.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ticker.advance(Duration::from_millis(10));
            }
        });
        let backends = Backends::Custom {
            local: Arc::new(tandemd::sim::SimSteps::new(&paths.data_dir)),
            input: Some(Box::new(InputSetup { backends: Arc::new(fake.clone()), opts: fast(), file_drag: None })),
            env: Some(Arc::new(tandem_env::Host::system())),
        };
        let net = NetOptions { mdns: false, fallback_ports: Vec::new(), ..NetOptions::default() };
        let daemon = daemon::start(cfg, &paths, backends, net)?;
        daemon.agent.set_generated(loaded.generated);
        let core = LocalMonitors::new(
            Arc::new(bus.clone()),
            Arc::new(clock),
            tandem_monitor::local::system_wall(),
            LocalConfig::default(),
        );
        drop(daemon.agent.start_monitors(core, tandemd::monitors::MonitorTimes::default(), true));
        let world = Arc::new(World { fake, bus, monitors: Mutex::new(Vec::new()) });
        if ctl != 0 {
            let l = TcpListener::bind(("127.0.0.1", ctl)).await?;
            tokio::spawn(async move {
                loop {
                    let Ok((s, _)) = l.accept().await else { return };
                    let world = world.clone();
                    tokio::spawn(async move {
                        let (r, mut wr) = s.into_split();
                        let mut lines = BufReader::new(r).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let out = match serde_json::from_str::<Value>(&line) {
                                Ok(v) => control(&world, &v),
                                Err(e) => json!({ "error": e.to_string() }),
                            };
                            if wr.write_all(format!("{out}\n").as_bytes()).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
        }
        daemon.serve(listener, daemon::shutdown_signal()).await?;
        anyhow::Ok(())
    })
}
