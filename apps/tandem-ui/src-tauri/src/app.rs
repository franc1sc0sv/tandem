//! La app de Tauri: ventana principal, la sesión con `tandemd` y los comandos que
//! llama el frontend.
//!
//! La sesión empuja a la ventana dos eventos: [`SNAPSHOT_EVENT`] con todo lo necesario
//! para dibujar la escena y [`NOTICE_EVENT`] con el avance de las transacciones y los
//! pedidos de cambio manual.

use std::sync::Arc;

use tandemd::ipc;
use tauri::{AppHandle, Emitter, Manager};

use crate::session::{self, Action, ActionResult, Handle, Notice, Sink, Snapshot};
use crate::status;

pub const SNAPSHOT_EVENT: &str = "agent-snapshot";
pub const NOTICE_EVENT: &str = "agent-notice";

struct Window(AppHandle);

impl Sink for Window {
    fn snapshot(&self, snapshot: &Snapshot) {
        // Sin ventana todavía no hay a quién avisar; al abrirse pide `agent_snapshot`.
        let _ = self.0.emit(SNAPSHOT_EVENT, snapshot);
    }

    fn notice(&self, notice: &Notice) {
        let _ = self.0.emit(NOTICE_EVENT, notice);
    }
}

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

fn start_session(app: &AppHandle) -> Handle {
    let sink = Arc::new(Window(app.clone()));
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

pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .setup(|app| {
            let handle = start_session(app.handle());
            app.manage(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            agent_snapshot,
            agent_act,
            agent_retry
        ])
        .run(tauri::generate_context!())
}
