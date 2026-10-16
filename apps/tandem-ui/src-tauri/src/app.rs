//! La app de Tauri: ventana principal y comandos que llama el frontend.

use crate::status::{self, AgentStatus};

/// Estado de `tandemd`, o por qué no se pudo leer. Nunca falla: el error es un dato
/// que la ventana muestra.
#[tauri::command]
async fn agent_status() -> AgentStatus {
    match status::endpoint_from_env() {
        Ok(endpoint) => status::fetch_status(&endpoint, status::QUERY_TIMEOUT).await,
        Err(reason) => AgentStatus::Unavailable {
            endpoint: None,
            reason,
        },
    }
}

pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![agent_status])
        .run(tauri::generate_context!())
}
