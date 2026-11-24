//! The Tauri app: main window, menu bar / tray icon with its mini
//! scene, global shortcuts, the session with `tandemd` and the commands the frontend calls.
//!
//! The session pushes two events to the windows: [`SNAPSHOT_EVENT`] with everything needed
//! to draw the scene and [`NOTICE_EVENT`] with the progress of transactions and
//! power notices. [`MESSAGE_EVENT`] carries what happened with a request made from outside
//! the window. [`SHORTCUTS_EVENT`] carries the new shortcuts when they change, so that
//! each window shows the current ones. [`crate::updater::UPDATE_EVENT`] carries the Updates
//! tab of Settings (T-122).
//!
//! On macOS, at launch it installs or updates `tandemd`'s LaunchAgent
//! ([`crate::launch_agent`]) and offers the first-launch permissions guide
//! ([`crate::setup`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tandemd::ipc;
use tandemd::paths::Paths;
use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcut, ShortcutState};

use crate::launch_agent::{Report, ReviveGate};
use crate::session::{self, Action, ActionResult, Handle, Notice, Sink, Snapshot};
use crate::setup::{self, Permission, SetupView};
use crate::shortcuts::{self, Bindings, ShortcutAction, Shortcuts, ShortcutsView};
use crate::status;
use crate::tray::{self, Area, MenuView};
use crate::update::UpdateView;
use crate::updater::Updater;

pub const SNAPSHOT_EVENT: &str = "agent-snapshot";
pub const NOTICE_EVENT: &str = "agent-notice";
/// A live resources sample (`{ machine, resources }`), only with `watch_resources`.
pub const RESOURCES_EVENT: &str = "agent-resources";
/// The network state (`NetworkView`), once per second and only with `watch_network`.
pub const NETWORK_EVENT: &str = "agent-network";
/// The tunnels by project (`ProjectItem[]`), on connecting and on each change.
pub const TUNNELS_EVENT: &str = "agent-tunnels";
/// The progress of a tunnel's file copy (`TransferItem`).
pub const TRANSFER_EVENT: &str = "agent-transfer";
/// New output of the process the terminal view watches.
pub const OUTPUT_EVENT: &str = "agent-output";
pub const MESSAGE_EVENT: &str = "ui-message";
pub const SHORTCUTS_EVENT: &str = "ui-shortcuts";

const MAIN_WINDOW: &str = "main";
/// The mini scene the icon opens (`tray.html`).
const TRAY_WINDOW: &str = "tray";

const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";
const MENU_STATUS: &str = "status";

/// A click on the icon with the mini scene open first takes focus away from it (and closes it) and
/// then arrives as a click: without this, it would reopen instantly.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

// In the macOS menu bar the icon is a template (black with alpha) that the system
// paints according to the theme; in the Windows tray the color logo goes.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-template.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // What these mutexes protect stays coherent even if a thread fell midway.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the app keeps besides the session.
struct Ui {
    menu: Option<TrayMenu>,
    keys: Mutex<Keys>,
    tray_hidden_at: Mutex<Option<Instant>>,
    first_run: FirstRun,
}

/// The first-launch guide and how the login agent ended up.
struct FirstRun {
    /// `setup.done`; without a configuration directory it cannot be remembered.
    file: Option<PathBuf>,
    agent: Option<Report>,
}

impl FirstRun {
    fn view(&self) -> SetupView {
        // Without a file to remember it in, the setup would appear on every launch: better not to offer it on its own.
        let done = self.file.as_deref().is_none_or(setup::is_done);
        SetupView::new(cfg!(target_os = "macos"), done, self.agent.clone())
    }
}

struct Keys {
    current: Shortcuts,
    failed: BTreeMap<ShortcutAction, String>,
    warning: Option<String>,
    file: Option<PathBuf>,
}

impl Keys {
    fn view(&self) -> ShortcutsView {
        ShortcutsView {
            bindings: self.current.bindings(),
            failed: self.failed.clone(),
            warning: self.warning.clone(),
            file: self.file.as_ref().map(|f| f.display().to_string()),
        }
    }
}

/// The icon menu: status, open and quit. The former modes no longer exist: scenes
/// (D6) replace them, which the menu does not offer yet.
struct TrayMenu {
    status: MenuItem<Wry>,
    tray: TrayIcon<Wry>,
}

impl TrayMenu {
    fn show(&self, v: &MenuView) {
        // If the system rejects a menu change, the menu stays as it was: nothing to do.
        let _ = self.status.set_text(&v.status);
        let _ = self.tray.set_tooltip(Some(&v.tooltip));
    }
}

struct Windows(AppHandle);

impl Sink for Windows {
    fn snapshot(&self, snapshot: &Snapshot) {
        // Without windows yet there is no one to notify; on opening they ask for `agent_snapshot`.
        let _ = self.0.emit(SNAPSHOT_EVENT, snapshot);
        if let Some(ui) = self.0.try_state::<Ui>()
            && let Some(menu) = &ui.menu
        {
            menu.show(&MenuView::of(snapshot));
        }
    }

    fn notice(&self, notice: &Notice) {
        let _ = self.0.emit(NOTICE_EVENT, notice);
    }

    fn show_window(&self) {
        show_main(&self.0);
    }

    fn update(&self, status: &tandem_proto::UpdateStatus) {
        if let Some(u) = self.0.try_state::<Arc<Updater>>() {
            u.on_agent(&self.0, status);
        }
    }

    fn resources(&self, machine: tandem_core::Machine, resources: &tandem_proto::Resources) {
        let _ = self.0.emit(
            RESOURCES_EVENT,
            serde_json::json!({ "machine": machine, "resources": resources }),
        );
    }

    fn network(&self, network: &crate::session::NetworkView) {
        let _ = self.0.emit(NETWORK_EVENT, network);
    }

    fn tunnels(&self, tunnels: &[crate::tunnels::ProjectItem]) {
        let _ = self.0.emit(TUNNELS_EVENT, tunnels);
    }

    fn transfer(&self, transfer: &crate::tunnels::TransferItem) {
        let _ = self.0.emit(TRANSFER_EVENT, transfer);
    }

    fn output(&self, output: &crate::tunnels::LiveOutputItem) {
        let _ = self.0.emit(OUTPUT_EVENT, output);
    }
}

// ---------------------------------------------------------------------------------------
// Commands

/// The latest known about the agent, for the window that just opened.
#[tauri::command]
fn agent_snapshot(session: tauri::State<'_, Handle>) -> Snapshot {
    session.snapshot()
}

#[tauri::command]
async fn agent_act(
    session: tauri::State<'_, Handle>,
    action: Action,
) -> Result<ActionResult, String> {
    Ok(session.act(action).await)
}

/// The "Retry" button: do not wait for the next automatic attempt.
#[tauri::command]
fn agent_retry(session: tauri::State<'_, Handle>) {
    session.retry_now();
}

#[tauri::command]
fn shortcuts_get(ui: tauri::State<'_, Ui>) -> ShortcutsView {
    lock(&ui.keys).view()
}

/// Changes the shortcuts: validates them, saves them in `ui.toml` and registers them again.
#[tauri::command]
fn shortcuts_set(
    app: AppHandle,
    ui: tauri::State<'_, Ui>,
    bindings: Bindings,
) -> Result<ShortcutsView, String> {
    let next = Shortcuts::from_bindings(&bindings).map_err(|e| e.to_string())?;
    let mut keys = lock(&ui.keys);
    if let Some(file) = &keys.file {
        shortcuts::save(file, &next)
            .map_err(|e| format!("could not save {}: {e}", file.display()))?;
    }
    keys.failed = register(&app, &next);
    keys.current = next;
    keys.warning = None;
    let view = keys.view();
    // The mini scene is not destroyed on closing: without the notice it would keep the old ones.
    let _ = app.emit(SHORTCUTS_EVENT, &view);
    Ok(view)
}

// Updates (T-122).

#[tauri::command]
fn update_get(updater: tauri::State<'_, Arc<Updater>>) -> UpdateView {
    updater.view()
}

#[tauri::command]
async fn update_check(
    app: AppHandle,
    updater: tauri::State<'_, Arc<Updater>>,
) -> Result<UpdateView, String> {
    Ok(updater.check(&app).await)
}

#[tauri::command]
fn update_all(
    app: AppHandle,
    updater: tauri::State<'_, Arc<Updater>>,
) -> Result<UpdateView, String> {
    updater.update_all(&app)
}

#[tauri::command]
fn update_auto(
    app: AppHandle,
    updater: tauri::State<'_, Arc<Updater>>,
    on: bool,
) -> Result<UpdateView, String> {
    updater.set_auto(&app, on)
}

#[tauri::command]
fn setup_get(ui: tauri::State<'_, Ui>) -> SetupView {
    ui.first_run.view()
}

/// Opens the System Settings panel that grants `permission`.
#[tauri::command]
fn setup_open(permission: Permission) -> Result<(), String> {
    open_settings(permission)
}

/// "Restart agent": `launchctl kickstart -k` of `tandemd`, so the permissions just
/// granted in Settings take effect without asking the user for commands.
#[tauri::command]
fn agent_restart() -> Result<(), String> {
    restart_agent()
}

#[cfg(target_os = "macos")]
fn restart_agent() -> Result<(), String> {
    use crate::launch_agent::{self, system::SystemLaunchd};
    launch_agent::restart(&SystemLaunchd::current_user()?)
}

#[cfg(not(target_os = "macos"))]
fn restart_agent() -> Result<(), String> {
    Err("the login agent only exists on macOS".to_owned())
}

/// State of the "Administrator service" row of the setup (T-131).
#[tauri::command]
fn admin_status() -> crate::admin_service::AdminView {
    use crate::admin_service as a;
    a::view(&admin_platform(), &a::System, &admin_acl())
}

/// "Install": asks for credentials (macOS) or UAC (Windows) and returns the resulting state.
#[tauri::command]
fn admin_install() -> Result<crate::admin_service::AdminView, String> {
    use crate::admin_service as a;
    let platform = admin_platform();
    a::install(&platform, &a::System, &a::System, &admin_acl()).map_err(|e| e.to_string())?;
    Ok(a::view(&platform, &a::System, &admin_acl()))
}

pub(crate) fn admin_platform() -> crate::admin_service::Platform {
    match std::env::current_exe() {
        Ok(exe) => crate::admin_service::platform_for(&exe),
        Err(e) => crate::admin_service::Platform::Unsupported(format!(
            "could not find the app's executable: {e}"
        )),
    }
}

#[cfg(windows)]
pub(crate) fn admin_acl() -> tandemd::install::WinAcl {
    tandemd::install::WinAcl
}

#[cfg(not(windows))]
pub(crate) fn admin_acl() -> crate::admin_service::UnixAcl {
    crate::admin_service::UnixAcl
}

#[tauri::command]
fn setup_finish(ui: tauri::State<'_, Ui>) -> Result<SetupView, String> {
    if let Some(file) = &ui.first_run.file {
        setup::mark_done(file, env!("CARGO_PKG_VERSION"))
            .map_err(|e| format!("could not save {}: {e}", file.display()))?;
    }
    Ok(ui.first_run.view())
}

#[cfg(target_os = "macos")]
fn open_settings(permission: Permission) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(permission.settings_url())
        .status()
        .map_err(|e| format!("could not open System Settings: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "open {} ended with {status}",
            permission.settings_url()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_settings(_: Permission) -> Result<(), String> {
    Err("these permissions only exist on macOS".to_owned())
}

/// macOS: installs or updates `tandemd`'s LaunchAgent. It is a couple of calls to
/// `launchctl` (milliseconds); if something fails, the first-launch guide says so.
#[cfg(target_os = "macos")]
fn install_agent() -> Option<Report> {
    use crate::launch_agent::{self, system::SystemLaunchd};
    let failed = |text: String| Report { ok: false, text };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Some(failed(format!("could not find where the app is: {e}"))),
    };
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return Some(failed("HOME is not defined".to_owned()));
    };
    let launchd = match SystemLaunchd::current_user() {
        Ok(l) => l,
        Err(e) => return Some(failed(e)),
    };
    let report = launch_agent::install_from(
        &exe,
        std::path::Path::new(&home),
        env!("CARGO_PKG_VERSION"),
        &launchd,
    );
    if !report.ok {
        eprintln!("tandem-ui: {}", report.text);
    }
    Some(report)
}

#[cfg(not(target_os = "macos"))]
fn install_agent() -> Option<Report> {
    None
}

/// "Open Tandem" from the mini scene.
#[tauri::command]
fn ui_show_main(app: AppHandle) {
    hide_tray(&app);
    show_main(&app);
}

/// The show or hide shortcut, handled by the window itself when the system did not let
/// it be registered: if the window receives the key it is because it is in front, so it hides.
#[tauri::command]
fn ui_hide_main(app: AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.hide();
    }
}

#[tauri::command]
fn ui_hide_tray(app: AppHandle) {
    hide_tray(&app);
}

/// The paths of the configuration file and the logs (Settings → Advanced).
#[tauri::command]
fn files_info() -> Result<crate::files::FilesInfo, String> {
    let (config, logs) = crate::files::locate()?;
    Ok(crate::files::FilesInfo {
        config: config.display().to_string(),
        logs: logs.display().to_string(),
    })
}

/// Shows the configuration file or the logs folder in the system's explorer.
#[tauri::command]
fn files_open(target: crate::files::Target) -> Result<(), String> {
    let (config, logs) = crate::files::locate()?;
    let (path, is_file) = match target {
        crate::files::Target::Config => (config, true),
        crate::files::Target::Logs => (logs, false),
    };
    if !is_file {
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    }
    let (program, args) = crate::files::reveal_command(std::env::consts::OS, &path, is_file);
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// "Quit" from the tray / menu bar (V3-Tray), the same as the icon menu.
#[tauri::command]
fn ui_quit(app: AppHandle) {
    app.exit(0);
}

// ---------------------------------------------------------------------------------------
// Windows

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let in_front = w.is_visible().unwrap_or(false)
        && w.is_focused().unwrap_or(false)
        && !w.is_minimized().unwrap_or(false);
    if in_front {
        let _ = w.hide();
    } else {
        show_main(app);
    }
}

fn hide_tray(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(TRAY_WINDOW)
        && w.is_visible().unwrap_or(false)
    {
        let _ = w.hide();
        if let Some(ui) = app.try_state::<Ui>() {
            *lock(&ui.tray_hidden_at) = Some(Instant::now());
        }
    }
}

/// Opens or closes the mini scene next to the icon that was clicked.
fn toggle_tray(app: &AppHandle, click: PhysicalPosition<f64>) {
    let Some(w) = app.get_webview_window(TRAY_WINDOW) else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        hide_tray(app);
        return;
    }
    if let Some(ui) = app.try_state::<Ui>()
        && lock(&ui.tray_hidden_at).is_some_and(|t| t.elapsed() < REOPEN_GUARD)
    {
        return;
    }
    place_tray(app, &w, click);
    let _ = w.show();
    let _ = w.set_focus();
}

fn place_tray(app: &AppHandle, w: &WebviewWindow, click: PhysicalPosition<f64>) {
    let Ok(size) = w.outer_size() else { return };
    let monitor = app
        .monitor_from_point(click.x, click.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let work = monitor.work_area();
    let area = Area {
        x: f64::from(work.position.x),
        y: f64::from(work.position.y),
        w: f64::from(work.size.width),
        h: f64::from(work.size.height),
    };
    let (x, y) = tray::popover_origin(
        (click.x, click.y),
        f64::from(size.width),
        f64::from(size.height),
        area,
    );
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    match (window.label(), event) {
        // It is a menu bar app: closing the window hides it; quitting is done from the menu.
        (MAIN_WINDOW, WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = window.hide();
        }
        // The mini scene behaves like a menu: it closes on clicking outside.
        (TRAY_WINDOW, WindowEvent::Focused(false)) => hide_tray(window.app_handle()),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------
// Icon and menu

fn build_tray(app: &AppHandle) -> tauri::Result<TrayMenu> {
    let status = MenuItem::with_id(app, MENU_STATUS, "Connecting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Tandem", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Tandem", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&status, &sep, &open, &quit])?;
    let tray = TrayIconBuilder::with_id("tandem")
        .icon(Image::from_bytes(TRAY_ICON)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Tandem")
        .menu(&menu)
        // Left click: the mini scene. Right click: the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_tray(tray.app_handle(), position);
            }
        })
        .build(app)?;
    Ok(TrayMenu { status, tray })
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_OPEN => show_main(app),
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------
// Global shortcuts

fn fire(app: &AppHandle, action: ShortcutAction) {
    match action {
        ShortcutAction::ToggleWindow => toggle_main(app),
    }
}

/// Registers the shortcuts (removing the previous ones). Returns those the system did not accept.
fn register(app: &AppHandle, keys: &Shortcuts) -> BTreeMap<ShortcutAction, String> {
    let Some(global) = app.try_state::<GlobalShortcut<Wry>>() else {
        // Without a shortcut registry (for example, Wayland): the window handles them itself.
        return keys
            .active()
            .map(|(a, _)| (a, "this system does not allow global shortcuts".to_owned()))
            .collect();
    };
    let _ = global.unregister_all();
    let mut failed = BTreeMap::new();
    for (action, accel) in keys.active() {
        let text = accel.to_string();
        let done = global.on_shortcut(text.as_str(), move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                fire(app, action);
            }
        });
        if let Err(e) = done {
            failed.insert(
                action,
                format!(
                    "the system did not let {text} be registered (is another app using it?): {e}"
                ),
            );
        }
    }
    failed
}

fn load_keys() -> Keys {
    match Paths::resolve(None, None) {
        Ok(paths) => {
            let file = shortcuts::ui_file_for(&paths.config);
            let loaded = shortcuts::load(&file);
            Keys {
                current: loaded.shortcuts,
                failed: BTreeMap::new(),
                warning: loaded.warning,
                file: Some(file),
            }
        }
        Err(e) => Keys {
            current: Shortcuts::default(),
            failed: BTreeMap::new(),
            warning: Some(format!("{e}; the shortcuts will not be saved")),
            file: None,
        },
    }
}

// ---------------------------------------------------------------------------------------
// Launch

fn start_session(app: &AppHandle) -> Handle {
    let sink = Arc::new(Windows(app.clone()));
    match status::endpoint_from_env() {
        Ok(endpoint) => {
            let shown = Some(endpoint.display().to_string());
            let gate = Arc::new(Mutex::new(ReviveGate::default()));
            let connect = move || {
                let endpoint = endpoint.clone();
                let gate = gate.clone();
                async move {
                    let connected = ipc::connect(&endpoint).await;
                    watch_agent(&gate, connected.as_ref().err());
                    connected
                }
            };
            let (handle, run) = session::start(connect, shown, sink);
            tauri::async_runtime::spawn(run);
            handle
        }
        Err(reason) => session::unavailable(None, reason),
    }
}

/// While the app is open, a `tandemd` that launchd keeps loaded but without a process (it
/// exited in a way `KeepAlive` does not relaunch) is started again (T-331). The gate keeps it
/// to the launchd throttle.
fn watch_agent(gate: &Mutex<ReviveGate>, failed: Option<&ipc::IpcError>) {
    let ask = {
        let mut gate = gate.lock().unwrap_or_else(PoisonError::into_inner);
        match failed {
            Some(e) if status::nobody_listening(e) => gate.missed(Instant::now()),
            _ => {
                gate.reached();
                false
            }
        }
    };
    if ask {
        revive_agent();
    }
}

#[cfg(target_os = "macos")]
fn revive_agent() {
    // `launchctl` blocks for milliseconds: off the session's task.
    tauri::async_runtime::spawn_blocking(|| {
        use crate::launch_agent::{self, Revived, system::SystemLaunchd};
        match SystemLaunchd::current_user().and_then(|l| launch_agent::revive(&l)) {
            Ok(Revived::Started) => {
                eprintln!("tandem-ui: tandemd was loaded but not running; started it");
            }
            Ok(Revived::NotLoaded | Revived::Running) => {}
            Err(e) => eprintln!("tandem-ui: could not start tandemd: {e}"),
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn revive_agent() {}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    // Without a shortcut registry or tray the app continues: the window acts as a fallback.
    if let Err(e) = handle.plugin(tauri_plugin_global_shortcut::Builder::new().build()) {
        eprintln!("tandem-ui: no global shortcuts: {e}");
    }
    // Without the updater the app continues; the Updates tab shows the error when checking.
    if let Err(e) = handle.plugin(tauri_plugin_updater::Builder::new().build()) {
        eprintln!("tandem-ui: no automatic updates: {e}");
    }
    // Before the session: its first notices already carry the versions.
    let updater = Arc::new(Updater::new());
    app.manage(updater.clone());
    let menu = match build_tray(&handle) {
        Ok(menu) => Some(menu),
        Err(e) => {
            eprintln!("tandem-ui: no menu bar / tray icon: {e}");
            None
        }
    };

    let mut keys = load_keys();
    keys.failed = register(&handle, &keys.current);
    let first_run = FirstRun {
        file: Paths::resolve(None, None)
            .ok()
            .map(|p| setup::setup_file_for(&p.config)),
        agent: install_agent(),
    };
    app.manage(Ui {
        menu,
        keys: Mutex::new(keys),
        tray_hidden_at: Mutex::new(None),
        first_run,
    });

    let session = start_session(&handle);
    if let Some(ui) = app.try_state::<Ui>()
        && let Some(menu) = &ui.menu
    {
        menu.show(&MenuView::of(&session.snapshot()));
    }
    app.manage(session);
    updater.spawn_auto(&handle);
    Ok(())
}

pub fn run() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        .setup(setup)
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            agent_snapshot,
            agent_act,
            agent_retry,
            shortcuts_get,
            shortcuts_set,
            setup_get,
            setup_open,
            agent_restart,
            admin_status,
            admin_install,
            setup_finish,
            update_get,
            update_check,
            update_all,
            update_auto,
            ui_show_main,
            ui_hide_main,
            ui_hide_tray,
            ui_quit,
            files_info,
            files_open
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // macOS: click on the Dock icon with the window hidden.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
    Ok(())
}
