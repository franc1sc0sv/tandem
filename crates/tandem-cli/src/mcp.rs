//! Servidor MCP por stdio: JSON-RPC 2.0, un mensaje por línea. A mano, sin crate: solo
//! se necesitan `initialize`, `ping`, `tools/list` y `tools/call` (DECISIONS, T-108).

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::api::{Api, ToolError};
use crate::spec::TOOLS;

/// Versión del protocolo que se ofrece si el cliente no pide una conocida.
const LATEST: &str = "2025-06-18";
const KNOWN: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Tandem conecta las dos máquinas del puesto (Mac y PC). Abre un túnel con \
`tunnel_open`, lanza procesos con `exec`, lee su salida con `output`, copia archivos con \
`files_put`/`files_get` y cierra con `tunnel_close`. / Tandem links the desk's two machines. \
Open a tunnel with `tunnel_open`, run processes with `exec`, read output with `output`, copy \
files with `files_put`/`files_get` and close with `tunnel_close`.";

/// Lee mensajes de `input` hasta que se cierra y contesta por `output`.
pub async fn serve<S, R, W>(api: &mut Api<S>, input: R, mut output: W) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(api, &line).await {
            output.write_all(reply.to_string().as_bytes()).await?;
            output.write_all(b"\n").await?;
            output.flush().await?;
        }
    }
    Ok(())
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// `None` para las notificaciones (no llevan respuesta).
pub async fn handle<S>(api: &mut Api<S>, line: &str) -> Option<Value>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("JSON inválido: {e}"))),
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // Una respuesta a algo que no pedimos (no pedimos nada al cliente): se ignora.
        return id.map(|id| error(id, -32600, "falta `method`"));
    };
    let Some(id) = id else {
        return None; // notificación, p. ej. `notifications/initialized`
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| KNOWN.contains(v)).unwrap_or(LATEST);
            json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "tandem", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }})
        }
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools_list()}}),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "falta `name`"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let result = match api.call(name, &args).await {
                Ok(v) => json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                    "structuredContent": v,
                    "isError": false,
                }),
                // Un fallo de la herramienta es resultado (la ve el modelo), no del protocolo.
                Err(ToolError::Unknown(n)) => {
                    return Some(error(id, -32602, &format!("herramienta desconocida: {n}")));
                }
                Err(e) => json!({
                    "content": [{"type": "text", "text": e.to_string()}],
                    "isError": true,
                }),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        other => error(id, -32601, &format!("método desconocido: {other}")),
    })
}

fn tools_list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| json!({"name": t.name, "description": t.desc, "inputSchema": t.schema()}))
        .collect()
}
