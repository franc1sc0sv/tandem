//! La app de Tauri: ventana principal, ícono en la barra de menú / bandeja con su mini
//! escena, atajos globales, la sesión con `tandemd` y los comandos que llama el frontend.
//!
//! La sesión empuja a las ventanas dos eventos: [`SNAPSHOT_EVENT`] con todo lo necesario
//! para dibujar la escena y [`NOTICE_EVENT`] con el avance de las transacciones y los
//! pedidos de cambio manual. [`MESSAGE_EVENT`] lleva lo que pasó con un pedido hecho desde
//! fuera de la ventana (un atajo global o el menú del ícono). [`SHORTCUTS_EVENT`] lleva los
//! atajos nuevos cuando se cambian, para que cada ventana muestre los vigentes.
//!
//! En macOS, al arrancar instala o pone al día el LaunchAgent de `tandemd`
//! ([`crate::launch_agent`]) y ofrece la guía de permisos del primer arranque
//! ([`crate::setup`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tandemd::ipc;
use tandemd::paths::Paths;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcut, ShortcutState};

use crate::launch_agent::Report;
use crate::session::{self, Action, ActionResult, Handle, Notice, Sink, Snapshot};
use crate::setup::{self, Permission, SetupView};
use crate::shortcuts::{self, Bindings, ShortcutAction, Shortcuts, ShortcutsView};
use crate::status;
use crate::tray::{self, Area, MenuView, ModeKey};

pub const SNAPSHOT_EVENT: &str = "agent-snapshot";
pub const NOTICE_EVENT: &str = "agent-notice";
pub const MESSAGE_EVENT: &str = "ui-message";
pub const SHORTCUTS_EVENT: &str = "ui-shortcuts";

const MAIN_WINDOW: &str = "main";
/// La mini escena que abre el ícono (`tray.html`).
const TRAY_WINDOW: &str = "tray";

const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";
const MENU_STATUS: &str = "status";

/// Un clic en el ícono con la mini escena abierta primero le quita el foco (y la cierra) y
/// después llega como clic: sin esto, se volvería a abrir al instante.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

// En la barra de menú de macOS el ícono es una plantilla (negro con alfa) que el sistema
// pinta según el tema; en la bandeja de Windows va el logo a color.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-template.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Lo que protegen estos mutex queda coherente aunque un hilo haya caído a medias.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lo que la app guarda además de la sesión.
struct Ui {
    menu: Option<TrayMenu>,
    keys: Mutex<Keys>,
    tray_hidden_at: Mutex<Option<Instant>>,
    first_run: FirstRun,
}

/// La guía del primer arranque y cómo quedó el agente de inicio de sesión.
struct FirstRun {
    /// `setup.done`; sin directorio de configuración no se puede recordar.
    file: Option<PathBuf>,
    agent: Option<Report>,
}

impl FirstRun {
    fn view(&self) -> SetupView {
        // Sin archivo donde recordarlo, la guía saldría en cada arranque: mejor no ofrecerla sola.
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

/// El menú del ícono: estado, los tres modos, abrir y salir.
struct TrayMenu {
    status: MenuItem<Wry>,
    modes: Vec<(ModeKey, CheckMenuItem<Wry>)>,
    tray: TrayIcon<Wry>,
}

impl TrayMenu {
    fn show(&self, v: &MenuView) {
        // Si el sistema rechaza un cambio del menú, el menú queda como estaba: nada que hacer.
        let _ = self.status.set_text(&v.status);
        for (key, item) in &self.modes {
            let _ = item.set_checked(v.current == Some(*key));
            let _ = item.set_enabled(v.modes_enabled);
        }
        let _ = self.tray.set_tooltip(Some(&v.tooltip));
    }
}

fn mode_menu_id(key: ModeKey) -> String {
    format!("mode:{}", mode_name(key))
}

fn mode_name(key: ModeKey) -> &'static str {
    match key {
        ModeKey::SoloMac => "solo_mac",
        ModeKey::Shared => "shared",
        ModeKey::SoloPc => "solo_pc",
    }
}

struct Windows(AppHandle);

impl Sink for Windows {
    fn snapshot(&self, snapshot: &Snapshot) {
        // Sin ventanas todavía no hay a quién avisar; al abrirse piden `agent_snapshot`.
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
}

// ---------------------------------------------------------------------------------------
// Comandos

/// Lo último que se sabe del agente, para la ventana que recién se abre.
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

/// El botón «Reintentar»: no esperar al próximo intento automático.
#[tauri::command]
fn agent_retry(session: tauri::State<'_, Handle>) {
    session.retry_now();
}

#[tauri::command]
fn shortcuts_get(ui: tauri::State<'_, Ui>) -> ShortcutsView {
    lock(&ui.keys).view()
}

/// Cambia los atajos: los valida, los guarda en `ui.toml` y los vuelve a registrar.
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
            .map_err(|e| format!("no se pudo guardar {}: {e}", file.display()))?;
    }
    keys.failed = register(&app, &next);
    keys.current = next;
    keys.warning = None;
    let view = keys.view();
    // La mini escena no se destruye al cerrarla: sin el aviso seguiría con los de antes.
    let _ = app.emit(SHORTCUTS_EVENT, &view);
    Ok(view)
}

#[tauri::command]
fn setup_get(ui: tauri::State<'_, Ui>) -> SetupView {
    ui.first_run.view()
}

/// Abre el panel de Ajustes del Sistema que concede `permission`.
#[tauri::command]
fn setup_open(permission: Permission) -> Result<(), String> {
    open_settings(permission)
}

#[tauri::command]
fn setup_finish(ui: tauri::State<'_, Ui>) -> Result<SetupView, String> {
    if let Some(file) = &ui.first_run.file {
        setup::mark_done(file, env!("CARGO_PKG_VERSION"))
            .map_err(|e| format!("no se pudo guardar {}: {e}", file.display()))?;
    }
    Ok(ui.first_run.view())
}

#[cfg(target_os = "macos")]
fn open_settings(permission: Permission) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(permission.settings_url())
        .status()
        .map_err(|e| format!("no se pudo abrir Ajustes del Sistema: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "open {} terminó con {status}",
            permission.settings_url()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_settings(_: Permission) -> Result<(), String> {
    Err("estos permisos solo existen en macOS".to_owned())
}

/// macOS: instala o pone al día el LaunchAgent de `tandemd`. Son un par de llamadas a
/// `launchctl` (milisegundos); si algo falla, la guía del primer arranque lo dice.
#[cfg(target_os = "macos")]
fn install_agent() -> Option<Report> {
    use crate::launch_agent::{self, system::SystemLaunchd};
    let failed = |text: String| Report { ok: false, text };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Some(failed(format!("no se pudo saber dónde está la app: {e}"))),
    };
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return Some(failed("no está definido HOME".to_owned()));
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

/// «Abrir Tandem» desde la mini escena.
#[tauri::command]
fn ui_show_main(app: AppHandle) {
    hide_tray(&app);
    show_main(&app);
}

/// El atajo de mostrar u ocultar, atendido por la ventana misma cuando el sistema no dejó
/// registrarlo: si la ventana recibe la tecla es porque está al frente, así que se esconde.
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

// ---------------------------------------------------------------------------------------
// Ventanas

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

/// Abre o cierra la mini escena junto al ícono en el que se hizo clic.
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
        // Es una app de barra de menú: cerrar la ventana la esconde; se sale desde el menú.
        (MAIN_WINDOW, WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = window.hide();
        }
        // La mini escena se comporta como un menú: se cierra al hacer clic fuera.
        (TRAY_WINDOW, WindowEvent::Focused(false)) => hide_tray(window.app_handle()),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------
// Ícono y menú

fn build_tray(app: &AppHandle) -> tauri::Result<TrayMenu> {
    let status = MenuItem::with_id(app, MENU_STATUS, "Conectando…", false, None::<&str>)?;
    let mut modes = Vec::new();
    for key in ModeKey::ALL {
        let item = CheckMenuItem::with_id(
            app,
            mode_menu_id(key),
            key.label(),
            false,
            false,
            None::<&str>,
        )?;
        modes.push((key, item));
    }
    let open = MenuItem::with_id(app, MENU_OPEN, "Abrir Tandem", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Salir de Tandem", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &sep1,
            &modes[0].1,
            &modes[1].1,
            &modes[2].1,
            &sep2,
            &open,
            &quit,
        ],
    )?;
    let tray = TrayIconBuilder::with_id("tandem")
        .icon(Image::from_bytes(TRAY_ICON)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Tandem")
        .menu(&menu)
        // Clic izquierdo: la mini escena. Clic derecho: el menú.
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
    Ok(TrayMenu {
        status,
        modes,
        tray,
    })
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_OPEN => show_main(app),
        MENU_QUIT => app.exit(0),
        id => {
            if let Some(key) = ModeKey::ALL.into_iter().find(|&k| mode_menu_id(k) == id) {
                choose_mode(app, key);
            }
        }
    }
    // El sistema marca la entrada al hacer clic; la marca la manda el estado del agente.
    if let (Some(ui), Some(session)) = (app.try_state::<Ui>(), app.try_state::<Handle>())
        && let Some(menu) = &ui.menu
    {
        menu.show(&MenuView::of(&session.snapshot()));
    }
}

// ---------------------------------------------------------------------------------------
// Atajos globales y modos pedidos desde fuera de la ventana

/// Cuéntale a las ventanas lo que pasó con un pedido hecho desde un atajo o el menú.
fn tell(app: &AppHandle, text: String) {
    let _ = app.emit(MESSAGE_EVENT, text);
}

fn choose_mode(app: &AppHandle, key: ModeKey) {
    let Some(session) = app.try_state::<Handle>() else {
        return;
    };
    let session = session.inner().clone();
    let mode = match tray::mode_for(key, &session.snapshot()) {
        Ok(mode) => mode,
        Err(why) => return tell(app, format!("{}: {why}", key.label())),
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match session.act(Action::SetMode { mode }).await {
            ActionResult::Rejected { reason } => tell(&app, reason),
            ActionResult::Unavailable { reason } => tell(&app, format!("Sin agente: {reason}")),
            // El avance llega como cualquier otra transacción.
            ActionResult::Accepted { .. } | ActionResult::Done => {}
        }
    });
}

fn fire(app: &AppHandle, action: ShortcutAction) {
    match ModeKey::of_action(action) {
        Some(key) => choose_mode(app, key),
        None => toggle_main(app),
    }
}

/// Registra los atajos (quitando los anteriores). Devuelve los que el sistema no aceptó.
fn register(app: &AppHandle, keys: &Shortcuts) -> BTreeMap<ShortcutAction, String> {
    let Some(global) = app.try_state::<GlobalShortcut<Wry>>() else {
        // Sin registro de atajos (por ejemplo, Wayland): la ventana los atiende ella.
        return keys
            .active()
            .map(|(a, _)| (a, "este sistema no permite atajos globales".to_owned()))
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
                format!("el sistema no dejó registrar {text} (¿lo usa otra app?): {e}"),
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
            warning: Some(format!("{e}; los atajos no se guardarán")),
            file: None,
        },
    }
}

// ---------------------------------------------------------------------------------------
// Arranque

fn start_session(app: &AppHandle) -> Handle {
    let sink = Arc::new(Windows(app.clone()));
    match status::endpoint_from_env() {
        Ok(endpoint) => {
            let shown = Some(endpoint.display().to_string());
            let connect = move || {
                let endpoint = endpoint.clone();
                async move { ipc::connect(&endpoint).await }
            };
            let (handle, run) = session::start(connect, shown, sink);
            tauri::async_runtime::spawn(run);
            handle
        }
        Err(reason) => session::unavailable(None, reason),
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    // Sin registro de atajos o sin bandeja la app sigue: la ventana hace de respaldo.
    if let Err(e) = handle.plugin(tauri_plugin_global_shortcut::Builder::new().build()) {
        eprintln!("tandem-ui: sin atajos globales: {e}");
    }
    let menu = match build_tray(&handle) {
        Ok(menu) => Some(menu),
        Err(e) => {
            eprintln!("tandem-ui: sin ícono en la barra de menú / bandeja: {e}");
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
            setup_finish,
            ui_show_main,
            ui_hide_main,
            ui_hide_tray
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // macOS: clic en el ícono del Dock con la ventana escondida.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_name_each_mode() {
        let ids: Vec<String> = ModeKey::ALL.into_iter().map(mode_menu_id).collect();
        assert_eq!(ids, ["mode:solo_mac", "mode:shared", "mode:solo_pc"]);
    }
}
