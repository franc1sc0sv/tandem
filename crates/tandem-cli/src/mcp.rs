//! MCP server over stdio: JSON-RPC 2.0, one message per line. By hand, without a crate: only
//! `initialize`, `ping`, `tools/list` and `tools/call` are needed (DECISIONS, T-108).

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::api::{Api, ToolError};
use crate::spec::TOOLS;

/// Protocol version offered if the client does not ask for a known one.
const LATEST: &str = "2025-06-18";
const KNOWN: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Tandem links the desk's two machines (Mac and PC). \
Open a tunnel with `tunnel_open`, run processes with `exec`, read output with `output`, copy \
files with `files_put`/`files_get` and close with `tunnel_close`.";

/// Reads messages from `input` until it closes and answers on `output`.
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

/// `None` for notifications (they carry no reply).
pub async fn handle<S>(api: &mut Api<S>, line: &str) -> Option<Value>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("Invalid JSON: {e}"))),
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A reply to something we did not ask for (we ask the client nothing): ignored.
        return id.map(|id| error(id, -32600, "missing `method`"));
    };
    let Some(id) = id else {
        return None; // notification, e.g. `notifications/initialized`
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
                return Some(error(id, -32602, "missing `name`"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let result = match api.call(name, &args).await {
                Ok(v) => ok_result(&v),
                // A tool failure is a result (the model sees it), not a protocol one.
                Err(ToolError::Unknown(n)) => {
                    return Some(error(id, -32602, &format!("unknown tool: {n}")));
                }
                Err(e) => json!({
                    "content": [{"type": "text", "text": e.to_string()}],
                    "isError": true,
                }),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        other => error(id, -32601, &format!("unknown method: {other}")),
    })
}

/// From this size (compact JSON) the result goes only in `content`.
pub const STRUCTURED_MAX: usize = 4 * 1024;

/// A tool's result without duplicating it: `content` carries the compact JSON (without
/// indentation) and `structuredContent` is added only if it is small. Large replies
/// (`output`) would go twice into the model's context; clients read `content`.
fn ok_result(v: &Value) -> Value {
    let text = v.to_string();
    let small = text.len() <= STRUCTURED_MAX;
    let mut r = json!({"content": [{"type": "text", "text": text}], "isError": false});
    if small {
        r["structuredContent"] = v.clone();
    }
    r
}

fn tools_list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| json!({"name": t.name, "description": t.desc, "inputSchema": t.schema()}))
        .collect()
}
