//! Acceptance bridge: what Tauri does between the window and `tandemd`, but over HTTP, to
//! drive the real UI (the production build) with Playwright without WebKitGTK.
//!
//! It uses the app's real code (`tandem_ui::session` for the IPC, `shortcuts`, `setup`,
//! `files`, `admin_service`, `update`); only the Tauri glue is replaced:
//!
//! - `GET  /__bridge/events`        SSE stream `{event, payload}` (Tauri's events).
//! - `POST /__bridge/invoke/<cmd>`  JSON body = command arguments; JSON response, or 500 `{error}`.
//! - `GET  /__bridge/calls`         what the UI asked for and Tauri would have done in the window (quit, hide…).
//! - `GET  /<path>`                 static files from `--static` (default `index.html`).
//!
//! Usage: `acc_bridge --port N --static DIR [--latest-json URL]`, with `TANDEM_DATA_DIR`,
//! `XDG_CONFIG_HOME` and `XDG_DATA_HOME` as those of the agent it connects to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tandem_core::Machine;
use tandem_proto::UpdateStatus;
use tandem_ui::session::{self, Action, ActionResult, Handle, Notice, PeerView, Sink, Snapshot};
use tandem_ui::shortcuts::{self, Bindings, Shortcuts};
use tandem_ui::update::{self, Fleet, Peer, Settings, Step, UpdateView, Version};
use tandem_ui::{admin_service, files, setup, status};
use tandemd::ipc;
use tandemd::paths::Paths;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Hub {
    tx: broadcast::Sender<String>,
    update: watch::Sender<Option<UpdateStatus>>,
}

impl Hub {
    fn emit<T: serde::Serialize>(&self, event: &str, payload: &T) {
        if let Ok(line) = serde_json::to_string(&json!({ "event": event, "payload": payload })) {
            let _ = self.tx.send(line);
        }
    }
}

impl Sink for Hub {
    fn snapshot(&self, s: &Snapshot) {
        self.emit("agent-snapshot", s);
    }
    fn notice(&self, n: &Notice) {
        self.emit("agent-notice", n);
    }
    fn update(&self, s: &UpdateStatus) {
        self.update.send_replace(Some(s.clone()));
    }
    fn resources(&self, machine: Machine, r: &tandem_proto::Resources) {
        self.emit("agent-resources", &json!({ "machine": machine, "resources": r }));
    }
    fn network(&self, n: &session::NetworkView) {
        self.emit("agent-network", n);
    }
    fn tunnels(&self, t: &[tandem_ui::tunnels::ProjectItem]) {
        self.emit("agent-tunnels", &t);
    }
    fn transfer(&self, t: &tandem_ui::tunnels::TransferItem) {
        self.emit("agent-transfer", t);
    }
}

struct Keys {
    current: Shortcuts,
    file: Option<PathBuf>,
    warning: Option<String>,
}

impl Keys {
    fn view(&self) -> shortcuts::ShortcutsView {
        shortcuts::ShortcutsView {
            bindings: self.current.bindings(),
            failed: BTreeMap::new(),
            warning: self.warning.clone(),
            file: self.file.as_ref().map(|f| f.display().to_string()),
        }
    }
}

struct App {
    hub: Arc<Hub>,
    handle: Handle,
    keys: Mutex<Keys>,
    setup_file: Option<PathBuf>,
    calls: Mutex<Vec<Value>>,
    updater: Mutex<UpdaterState>,
    latest: Option<String>,
}

struct UpdaterState {
    view: UpdateView,
    found: Option<Version>,
    busy: bool,
    file: Option<PathBuf>,
}

const HERE: &str = env!("CARGO_PKG_VERSION");

fn parse_version(s: &str) -> Version {
    s.parse().unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
        pre: vec![],
    })
}

impl App {
    fn record(&self, what: &str, detail: Value) {
        lock(&self.calls).push(json!({ "call": what, "detail": detail }));
    }

    fn change(&self, f: impl FnOnce(&mut UpdaterState)) -> UpdateView {
        let view = {
            let mut s = lock(&self.updater);
            f(&mut s);
            s.view.clone()
        };
        self.hub.emit("ui-update", &view);
        view
    }

    fn peer(&self) -> Peer {
        let status = self.hub.update.borrow().clone();
        match self.handle.snapshot() {
            Snapshot::Connected { peer: PeerView::Unpaired, .. } => Peer::Alone,
            Snapshot::Connected { peer: PeerView::Paired { link, .. }, .. } => {
                update::peer_from(true, link.is_some(), status.as_ref())
            }
            _ => Peer::Unreachable,
        }
    }

    async fn check(self: &Arc<Self>) -> UpdateView {
        self.change(|s| {
            s.view.checking = true;
            s.view.error = None;
        });
        let found = match &self.latest {
            None => Err("could not check: no update server".to_owned()),
            Some(url) => fetch_latest(url).await,
        };
        let peer = self.peer();
        self.change(|s| {
            s.view.checking = false;
            match found {
                Ok(v) => {
                    let ok = update::fetch_release(&parse_version(HERE), &v, &peer);
                    s.view.available = ok.then(|| v.to_string());
                    s.found = ok.then_some(v);
                }
                Err(e) => s.view.error = Some(e),
            }
        })
    }

    fn update_all(self: &Arc<Self>) -> Result<UpdateView, String> {
        let target = lock(&self.updater)
            .found
            .clone()
            .ok_or("there is no new version: check first")?;
        let steps = update::plan(&parse_version(HERE), &self.peer(), &target).map_err(|e| e.to_string())?;
        let mut taken = false;
        let view = self.change(|s| {
            if !s.busy {
                s.busy = true;
                taken = true;
                s.view.error = None;
            }
        });
        if !taken {
            return Err("an update is already in progress".into());
        }
        let me = self.clone();
        tokio::spawn(async move {
            let mut fleet = BridgeFleet { app: me.clone(), agent: me.hub.update.subscribe() };
            let progress = {
                let me = me.clone();
                move |step| {
                    me.change(|s| s.view.step = Some(step));
                }
            };
            let result = update::run(&steps, &target, &mut fleet, Duration::from_secs(20), progress).await;
            me.change(|s| {
                s.busy = false;
                s.view.step = None;
                s.view.error = result.err().map(|e| e.to_string());
            });
        });
        Ok(view)
    }
}

struct BridgeFleet {
    app: Arc<App>,
    agent: watch::Receiver<Option<UpdateStatus>>,
}

impl Fleet for BridgeFleet {
    async fn ask_peer(&mut self, target: &Version) -> Result<(), String> {
        match self.app.handle.act(Action::UpdatePeer { version: target.to_string() }).await {
            ActionResult::Done => Ok(()),
            ActionResult::Rejected { reason } | ActionResult::Unavailable { reason } => Err(reason),
            other => Err(format!("unexpected response from tandemd: {other:?}")),
        }
    }
    async fn peer_changed(&mut self) -> Option<Peer> {
        self.agent.changed().await.ok()?;
        let status = self.agent.borrow_and_update().clone();
        Some(match self.app.handle.snapshot() {
            Snapshot::Connected { peer: PeerView::Paired { link, .. }, .. } => {
                update::peer_from(true, link.is_some(), status.as_ref())
            }
            Snapshot::Connected { peer: PeerView::Unpaired, .. } => Peer::Alone,
            _ => Peer::Unreachable,
        })
    }
    async fn install_here(&mut self, target: &Version) -> Result<(), String> {
        // Double: here the real updater downloads the package and restarts the app.
        self.app.record("install_here", json!(target.to_string()));
        Ok(())
    }
}

/// Minimal `latest.json` over plain HTTP (`http://host:port/path`): `{"version": "x.y.z"}`.
async fn fetch_latest(url: &str) -> Result<Version, String> {
    let rest = url.strip_prefix("http://").ok_or("http only in tests")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut s = TcpStream::connect(host).await.map_err(|e| format!("could not check: {e}"))?;
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next().unwrap_or("");
    if !status_line.contains(" 200") {
        return Err(format!("could not check: {status_line}"));
    }
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let v: Value = serde_json::from_str(body).map_err(|e| format!("could not check: invalid latest.json: {e}"))?;
    v["version"]
        .as_str()
        .ok_or("latest.json without a version")?
        .parse::<Version>()
        .map_err(|e| e.to_string())
}

type Reply = Result<Value, String>;

async fn invoke(app: &Arc<App>, cmd: &str, args: Value) -> Reply {
    match cmd {
        "agent_snapshot" => Ok(serde_json::to_value(app.handle.snapshot()).map_err(|e| e.to_string())?),
        "agent_act" => {
            let action: Action = serde_json::from_value(args["action"].clone()).map_err(|e| format!("invalid action: {e}"))?;
            let r = app.handle.act(action).await;
            Ok(json!(r_to_value(&r)))
        }
        "agent_retry" => {
            app.handle.retry_now();
            Ok(Value::Null)
        }
        "shortcuts_get" => Ok(serde_json::to_value(lock(&app.keys).view()).map_err(|e| e.to_string())?),
        "shortcuts_set" => {
            let b: Bindings = serde_json::from_value(args["bindings"].clone()).map_err(|e| e.to_string())?;
            let next = Shortcuts::from_bindings(&b).map_err(|e| e.to_string())?;
            let mut keys = lock(&app.keys);
            if let Some(file) = &keys.file {
                shortcuts::save(file, &next).map_err(|e| format!("could not save {}: {e}", file.display()))?;
            }
            keys.current = next;
            keys.warning = None;
            let view = keys.view();
            app.hub.emit("ui-shortcuts", &view);
            Ok(serde_json::to_value(view).map_err(|e| e.to_string())?)
        }
        "setup_get" => {
            let done = app.setup_file.as_deref().is_none_or(setup::is_done);
            let avail = std::env::var_os("ACC_MAC_SETUP").is_some();
            Ok(serde_json::to_value(setup::SetupView::new(avail, done, None)).map_err(|e| e.to_string())?)
        }
        "setup_open" => {
            app.record("setup_open", args);
            Err("estos permisos solo existen en macOS".into())
        }
        "agent_restart" => Err("the sign-in agent only exists on macOS".into()),
        "admin_status" => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let v = admin_service::view(&admin_service::platform_for(&exe), &admin_service::System, &admin_service::UnixAcl);
            Ok(serde_json::to_value(v).map_err(|e| e.to_string())?)
        }
        "admin_install" => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let platform = admin_service::platform_for(&exe);
            admin_service::install(&platform, &admin_service::System, &admin_service::System, &admin_service::UnixAcl).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(admin_service::view(&platform, &admin_service::System, &admin_service::UnixAcl)).map_err(|e| e.to_string())?)
        }
        "setup_finish" => {
            if let Some(file) = &app.setup_file {
                setup::mark_done(file, HERE).map_err(|e| e.to_string())?;
            }
            let done = app.setup_file.as_deref().is_none_or(setup::is_done);
            Ok(serde_json::to_value(setup::SetupView::new(false, done, None)).map_err(|e| e.to_string())?)
        }
        "update_get" => Ok(serde_json::to_value(lock(&app.updater).view.clone()).map_err(|e| e.to_string())?),
        "update_check" => Ok(serde_json::to_value(app.check().await).map_err(|e| e.to_string())?),
        "update_all" => Ok(serde_json::to_value(app.update_all()?).map_err(|e| e.to_string())?),
        "update_auto" => {
            let on = args["on"].as_bool().unwrap_or(false);
            let file = lock(&app.updater).file.clone();
            if let Some(file) = file {
                update::save_settings(&file, Settings { auto: on }).map_err(|e| e.to_string())?;
            }
            Ok(serde_json::to_value(app.change(|s| s.view.auto = on)).map_err(|e| e.to_string())?)
        }
        "ui_show_main" | "ui_hide_main" | "ui_hide_tray" | "ui_quit" => {
            app.record(cmd, args);
            Ok(Value::Null)
        }
        "files_info" => {
            let (config, logs) = files::locate()?;
            Ok(json!({ "config": config.display().to_string(), "logs": logs.display().to_string() }))
        }
        "files_open" => {
            let target = args["target"].as_str().unwrap_or("");
            let (config, logs) = files::locate()?;
            let (path, is_file) = if target == "config" { (config, true) } else { (logs, false) };
            let (program, argv) = files::reveal_command(std::env::consts::OS, &path, is_file);
            app.record("files_open", json!({ "target": target, "program": program, "args": argv, "exists": path.exists() }));
            Ok(Value::Null)
        }
        other => Err(format!("comando desconocido: {other}")),
    }
}

fn r_to_value(r: &ActionResult) -> Value {
    serde_json::to_value(r).unwrap_or(Value::Null)
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

async fn respond(s: &mut TcpStream, code: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes()).await;
    let _ = s.write_all(body).await;
    let _ = s.shutdown().await;
}

async fn serve(mut s: TcpStream, app: Arc<App>, root: Arc<PathBuf>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let (head_end, content_len) = loop {
        let Ok(n) = s.read(&mut tmp).await else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            break (pos + 4, len);
        }
    };
    while buf.len() < head_end + content_len {
        let Ok(n) = s.read(&mut tmp).await else { return };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let body = buf[head_end..].to_vec();
    let mut parts = head.lines().next().unwrap_or("").split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let path = path.split('?').next().unwrap_or("/");

    if path == "/__bridge/events" {
        let mut rx = app.hub.tx.subscribe();
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n: ok\n\n";
        if s.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        loop {
            match rx.recv().await {
                Ok(line) => {
                    if s.write_all(format!("data: {line}\n\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    }
    if path == "/__bridge/calls" {
        let v = serde_json::to_vec(&*lock(&app.calls)).unwrap_or_default();
        return respond(&mut s, "200 OK", "application/json", &v).await;
    }
    if let (Some(cmd), "POST") = (path.strip_prefix("/__bridge/invoke/"), method) {
        let args: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let (code, v) = match invoke(&app, cmd, args).await {
            Ok(v) => ("200 OK", v),
            Err(e) => ("500 Internal Server Error", json!({ "error": e })),
        };
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return respond(&mut s, code, "application/json", &bytes).await;
    }
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.contains("..") {
        return respond(&mut s, "400 Bad Request", "text/plain", b"no").await;
    }
    let file = root.join(rel);
    match std::fs::read(&file) {
        Ok(bytes) => respond(&mut s, "200 OK", mime(&file), &bytes).await,
        Err(_) => respond(&mut s, "404 Not Found", "text/plain", b"no existe").await,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut port = 0u16;
    let mut root = PathBuf::from("dist");
    let mut latest = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().ok_or("falta --port")?.parse()?,
            "--static" => root = PathBuf::from(args.next().ok_or("falta --static")?),
            "--latest-json" => latest = args.next(),
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    let (tx, _) = broadcast::channel(4096);
    let hub = Arc::new(Hub { tx, update: watch::Sender::new(None) });
    let endpoint = status::endpoint_from_env()?;
    let shown = Some(endpoint.display().to_string());
    let connect = move || {
        let endpoint = endpoint.clone();
        async move { ipc::connect(&endpoint).await }
    };
    let (handle, run) = session::start(connect, shown, hub.clone());
    tokio::spawn(run);

    let paths = Paths::resolve(None, None)?;
    let ui_file = shortcuts::ui_file_for(&paths.config);
    let loaded = shortcuts::load(&ui_file);
    let settings_file = update::settings_file_for(&paths.config);
    let auto = update::load_settings(&settings_file).auto;
    let app = Arc::new(App {
        hub: hub.clone(),
        handle,
        keys: Mutex::new(Keys { current: loaded.shortcuts, file: Some(ui_file), warning: loaded.warning }),
        setup_file: Some(setup::setup_file_for(&paths.config)),
        calls: Mutex::new(Vec::new()),
        updater: Mutex::new(UpdaterState {
            view: UpdateView {
                here: HERE.to_owned(),
                peer: None,
                paired: false,
                available: None,
                checking: false,
                step: None::<Step>,
                error: None,
                auto,
            },
            found: None,
            busy: false,
            file: Some(settings_file),
        }),
        latest,
    });
    // The other machine's version state, as the Updates tab would see it.
    {
        let app = app.clone();
        let mut rx = hub.update.subscribe();
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let st = rx.borrow_and_update().clone();
                let paired = matches!(app.handle.snapshot(), Snapshot::Connected { peer: PeerView::Paired { .. }, .. });
                app.change(|s| {
                    s.view.peer = st.as_ref().and_then(|x| x.peer.clone());
                    s.view.paired = paired;
                });
            }
        });
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("acc_bridge escuchando en {}", listener.local_addr()?);
    let root = Arc::new(root);
    loop {
        let (s, _) = listener.accept().await?;
        tokio::spawn(serve(s, app.clone(), root.clone()));
    }
}
