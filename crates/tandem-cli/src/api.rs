//! Las herramientas sobre la IPC del agente. Es el único sitio que sabe qué `UiCommand`
//! hay detrás de cada una; el MCP y la CLI solo llaman a [`Api::call`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tandem_core::Machine;
use tandem_proto::files::Direction;
use tandem_proto::{
    ExecSpec, OutputCursor, ProcessId, Project, Reply, Tunnel, TunnelProcess, UiCommand,
    WorkspaceSpec,
};
use tandemd::ipc::IpcClient;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::spec::{Tool, Ty};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("herramienta desconocida: `{0}`")]
    Unknown(String),
    /// Argumentos que no cumplen la tabla de [`crate::spec`].
    #[error("{0}")]
    Args(String),
    /// El agente rechazó el pedido o no se pudo hablar con él.
    #[error("{0}")]
    Agent(String),
}

/// De dónde salen los valores por omisión de `tunnel_open`.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub cwd: Option<PathBuf>,
    /// La sesión que abre túneles (`CLAUDE_SESSION_ID` o el nombre del cliente).
    pub session: String,
}

/// Cuánto se espera como máximo por salida nueva en `output`.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// Bytes por flujo en cada lectura (el agente topa en 400 KiB).
const CHUNK: u32 = 256 * 1024;

pub struct Api<S> {
    ipc: IpcClient<S>,
    ctx: Context,
}

/// Los argumentos ya revisados contra la tabla.
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn str(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(Value::as_str)
    }
    fn req(&self, k: &str) -> Result<&str, ToolError> {
        self.str(k)
            .ok_or_else(|| ToolError::Args(format!("falta `{k}`")))
    }
    fn int(&self, k: &str) -> Option<u64> {
        self.0.get(k).and_then(Value::as_u64)
    }
    fn req_int(&self, k: &str) -> Result<u64, ToolError> {
        self.int(k)
            .ok_or_else(|| ToolError::Args(format!("falta `{k}`")))
    }
    fn flag(&self, k: &str) -> bool {
        self.0.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn list(&self, k: &str) -> Vec<String> {
        self.0
            .get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn tunnel(&self) -> Result<tandem_proto::TunnelId, ToolError> {
        let t = self.req("tunnel")?;
        let hex = t.trim().trim_start_matches("0x");
        u64::from_str_radix(hex, 16)
            .map(tandem_proto::TunnelId)
            .map_err(|_| ToolError::Args(format!("`{t}` no es un id de túnel (hex)")))
    }
    fn process(&self) -> Result<ProcessId, ToolError> {
        let n = self.req_int("process")?;
        u32::try_from(n)
            .map(ProcessId)
            .map_err(|_| ToolError::Args(format!("`{n}` no es un id de proceso")))
    }
    fn port(&self, k: &str) -> Result<u16, ToolError> {
        let n = self.req_int(k)?;
        u16::try_from(n).map_err(|_| ToolError::Args(format!("`{k}`: {n} no es un puerto")))
    }
}

/// Revisa tipos y obligatorios, y acepta números y booleanos escritos como texto (lo que
/// mandan algunos modelos). Devuelve los argumentos normalizados.
pub fn check_args(tool: &Tool, raw: &Value) -> Result<Value, ToolError> {
    let empty = serde_json::Map::new();
    let obj = match raw {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(ToolError::Args("los argumentos van en un objeto".into())),
    };
    if let Some(k) = obj
        .keys()
        .find(|k| !tool.params.iter().any(|p| p.name == *k))
    {
        return Err(ToolError::Args(format!(
            "`{}` no tiene el parámetro `{k}`",
            tool.name
        )));
    }
    let mut out = serde_json::Map::new();
    for p in tool.params {
        let Some(v) = obj.get(p.name).filter(|v| !v.is_null()) else {
            if p.required {
                return Err(ToolError::Args(format!("falta `{}`", p.name)));
            }
            continue;
        };
        let bad = || ToolError::Args(format!("`{}` no tiene el tipo esperado", p.name));
        let v = match (p.ty, v) {
            (Ty::Str, Value::String(_)) | (Ty::Bool, Value::Bool(_)) => v.clone(),
            (Ty::Int, Value::Number(n)) if n.is_u64() => v.clone(),
            (Ty::Int, Value::String(s)) => s.trim().parse::<u64>().map_err(|_| bad())?.into(),
            (Ty::Bool, Value::String(s)) => match s.as_str() {
                "true" => true.into(),
                "false" => false.into(),
                _ => return Err(bad()),
            },
            (Ty::List, Value::Array(a)) if a.iter().all(Value::is_string) => v.clone(),
            _ => return Err(bad()),
        };
        out.insert(p.name.into(), v);
    }
    Ok(Value::Object(out))
}

fn tunnel_json(t: &Tunnel) -> Value {
    let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
    // El id de 64 bits va como texto hex: un número así se pierde en JavaScript.
    v["id"] = t.id.to_string().into();
    v
}

fn process_json(p: &TunnelProcess) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    v["tunnel"] = p.tunnel.to_string().into();
    v
}

fn unexpected(r: &Reply) -> ToolError {
    ToolError::Agent(format!("respuesta inesperada del agente: {r:?}"))
}

impl<S: AsyncRead + AsyncWrite + Unpin> Api<S> {
    pub fn new(ipc: IpcClient<S>, ctx: Context) -> Self {
        Api { ipc, ctx }
    }

    async fn ask(&mut self, cmd: UiCommand) -> Result<Reply, ToolError> {
        match self.ipc.request(cmd).await {
            Ok(Reply::Rejected(why)) => Err(ToolError::Agent(why)),
            Ok(r) => Ok(r),
            Err(e) => Err(ToolError::Agent(format!("IPC del agente: {e}"))),
        }
    }

    /// Ejecuta la herramienta `name`; el resultado es el mismo JSON para el MCP y para
    /// `tandem --json`.
    pub async fn call(&mut self, name: &str, raw: &Value) -> Result<Value, ToolError> {
        let tool = crate::spec::find(name).ok_or_else(|| ToolError::Unknown(name.into()))?;
        let args = check_args(tool, raw)?;
        let a = Args(&args);
        match name {
            "machines" => self.machines().await,
            "resources" => self.resources(&a).await,
            "tunnels" => match self.ask(UiCommand::GetTunnels).await? {
                Reply::Tunnels(groups) => {
                    let groups: Vec<Value> = groups
                        .iter()
                        .map(|g| {
                            let tunnels: Vec<Value> = g
                                .tunnels
                                .iter()
                                .map(|v| {
                                    json!({
                                        "tunnel": tunnel_json(&v.tunnel),
                                        "running": v.running.iter().map(process_json).collect::<Vec<_>>(),
                                    })
                                })
                                .collect();
                            json!({"project": g.project, "tunnels": tunnels})
                        })
                        .collect();
                    Ok(json!({"projects": groups}))
                }
                r => Err(unexpected(&r)),
            },
            "tunnel_open" => self.tunnel_open(&a).await,
            "tunnel_close" => {
                let id = a.tunnel()?;
                match self.ask(UiCommand::CloseTunnel { id }).await? {
                    Reply::Done | Reply::Accepted { .. } => Ok(json!({"closed": id.to_string()})),
                    r => Err(unexpected(&r)),
                }
            }
            "exec" => {
                let spec = ExecSpec {
                    cmd: a.req("cmd")?.to_owned(),
                    args: a.list("args"),
                    shell: a.str("shell").map(str::to_owned),
                    cwd: a.str("cwd").map(str::to_owned),
                    env: a
                        .list("env")
                        .into_iter()
                        .map(|kv| match kv.split_once('=') {
                            Some((k, v)) => Ok((k.to_owned(), v.to_owned())),
                            None => {
                                Err(ToolError::Args(format!("`env`: `{kv}` no es NOMBRE=valor")))
                            }
                        })
                        .collect::<Result<_, _>>()?,
                };
                match self
                    .ask(UiCommand::Exec {
                        tunnel: a.tunnel()?,
                        spec,
                    })
                    .await?
                {
                    Reply::Process(p) => Ok(process_json(&p)),
                    r => Err(unexpected(&r)),
                }
            }
            "processes" => match self
                .ask(UiCommand::GetProcesses {
                    tunnel: a.tunnel()?,
                })
                .await?
            {
                Reply::Processes(list) => {
                    Ok(json!({"processes": list.iter().map(process_json).collect::<Vec<_>>()}))
                }
                r => Err(unexpected(&r)),
            },
            "output" => self.output(&a).await,
            "kill" => {
                let (tunnel, process) = (a.tunnel()?, a.process()?);
                match self.ask(UiCommand::Kill { tunnel, process }).await? {
                    Reply::Process(p) => Ok(process_json(&p)),
                    Reply::Done | Reply::Accepted { .. } => {
                        Ok(json!({"killed": process.0, "tunnel": tunnel.to_string()}))
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "files_put" | "files_get" => {
                let tunnel = a.tunnel()?;
                let (local, remote) = (a.req("local")?.to_owned(), a.req("remote")?.to_owned());
                let cmd = if name == "files_put" {
                    UiCommand::FilesPut {
                        tunnel,
                        local,
                        remote,
                    }
                } else {
                    UiCommand::FilesGet {
                        tunnel,
                        remote,
                        local,
                    }
                };
                match self.ask(cmd).await? {
                    Reply::Files(r) => Ok(serde_json::to_value(r).unwrap_or(Value::Null)),
                    r => Err(unexpected(&r)),
                }
            }
            "port_forward" => {
                let direction = match a.str("direction").unwrap_or("local_to_remote") {
                    "local_to_remote" => Direction::LocalToRemote,
                    "remote_to_local" => Direction::RemoteToLocal,
                    other => {
                        return Err(ToolError::Args(format!(
                            "`direction`: `{other}` no es `local_to_remote` ni `remote_to_local`"
                        )));
                    }
                };
                let cmd = UiCommand::PortForward {
                    tunnel: a.tunnel()?,
                    local_port: a.port("local_port")?,
                    remote_host: a.str("remote_host").unwrap_or("127.0.0.1").to_owned(),
                    remote_port: a.port("remote_port")?,
                    direction,
                };
                match self.ask(cmd).await? {
                    Reply::Forward(f) => {
                        let mut v = serde_json::to_value(&f).unwrap_or(Value::Null);
                        v["tunnel"] = f.tunnel.to_string().into();
                        Ok(v)
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "port_stop" => {
                let id = a.req_int("id")?;
                match self.ask(UiCommand::StopForward { id }).await? {
                    Reply::Done | Reply::Accepted { .. } => Ok(json!({"stopped": id})),
                    r => Err(unexpected(&r)),
                }
            }
            "bring_back" => {
                let cmd = UiCommand::BringBack {
                    tunnel: a.tunnel()?,
                    commit: a.str("commit").map(str::to_owned),
                    name: a.str("name").map(str::to_owned),
                };
                match self.ask(cmd).await? {
                    Reply::BroughtBack(r) => Ok(serde_json::to_value(r).unwrap_or(Value::Null)),
                    r => Err(unexpected(&r)),
                }
            }
            other => Err(ToolError::Unknown(other.into())),
        }
    }

    async fn caps(&mut self) -> Result<tandem_proto::DeskCapabilities, ToolError> {
        match self.ask(UiCommand::GetCapabilities).await? {
            Reply::Capabilities(c) => Ok(*c),
            r => Err(unexpected(&r)),
        }
    }

    async fn machines(&mut self) -> Result<Value, ToolError> {
        let this = self.ipc.agent();
        let caps = self.caps().await?;
        Ok(json!({
            "this": this.as_str(),
            "other": this.other().as_str(),
            "capabilities": caps,
        }))
    }

    async fn resources(&mut self, a: &Args<'_>) -> Result<Value, ToolError> {
        let this = self.ipc.agent();
        let machine = match a.str("machine") {
            Some(m) => parse_machine(m)?,
            None => this.other(),
        };
        if a.flag("rescan") {
            self.ask(UiCommand::Rescan { machine }).await?;
        }
        let caps = self.caps().await?;
        let found = if machine == this {
            Some(caps.here)
        } else {
            caps.peer
        };
        match found {
            Some(c) => Ok(json!({"machine": machine.as_str(), "resources": c})),
            None => Err(ToolError::Agent(format!(
                "todavía no hay recursos de la máquina {}: ¿está conectada?",
                machine.as_str()
            ))),
        }
    }

    async fn tunnel_open(&mut self, a: &Args<'_>) -> Result<Value, ToolError> {
        let target = match a.str("target") {
            Some(t) => parse_machine(t)?,
            None => self.ipc.agent().other(),
        };
        let root = a.str("project_root").map(str::to_owned).or_else(|| {
            self.ctx
                .cwd
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
        });
        let name = a.str("project").map(str::to_owned).or_else(|| {
            let root = root.as_deref()?;
            std::path::Path::new(root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        });
        let project = match (name, &root) {
            (Some(name), Some(root)) => Some(Project {
                name,
                root: root.clone(),
            }),
            _ => None,
        };
        let workspace = if a.flag("workspace") {
            let repo_path = root.clone().ok_or_else(|| {
                ToolError::Args(
                    "`workspace` necesita `project_root` (no hay carpeta actual)".into(),
                )
            })?;
            Some(WorkspaceSpec {
                repo_path,
                keep: a.flag("keep"),
            })
        } else {
            None
        };
        let cmd = UiCommand::OpenTunnel {
            // Sin título, el agente usa el del proyecto.
            title: a.str("title").unwrap_or_default().to_owned(),
            project,
            target,
            session: a
                .str("session")
                .map_or_else(|| self.ctx.session.clone(), str::to_owned),
            workspace,
        };
        match self.ask(cmd).await? {
            Reply::Tunnel(t) => Ok(tunnel_json(&t)),
            r => Err(unexpected(&r)),
        }
    }

    async fn output(&mut self, a: &Args<'_>) -> Result<Value, ToolError> {
        let (tunnel, process) = (a.tunnel()?, a.process()?);
        let from = OutputCursor {
            stdout: a.int("from_stdout").unwrap_or(0),
            stderr: a.int("from_stderr").unwrap_or(0),
        };
        let wait = Duration::from_millis(a.int("wait_ms").unwrap_or(0)).min(MAX_WAIT);
        let deadline = Instant::now() + wait;
        loop {
            let reply = self
                .ask(UiCommand::Output {
                    tunnel,
                    process,
                    from,
                    max: CHUNK,
                })
                .await?;
            let Reply::Output(o) = reply else {
                return Err(unexpected(&reply));
            };
            // El agente no espera: aquí se sondea hasta que haya algo o venza `wait_ms`.
            let empty = o.stdout.is_empty() && o.stderr.is_empty();
            if !(empty && !o.done && Instant::now() < deadline) {
                return Ok(json!({
                    "process": process_json(&o.process),
                    "stdout": String::from_utf8_lossy(&o.stdout),
                    "stderr": String::from_utf8_lossy(&o.stderr),
                    "from": {"stdout": o.from.stdout, "stderr": o.from.stderr},
                    "next": {"stdout": o.next.stdout, "stderr": o.next.stderr},
                    "done": o.done,
                }));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn parse_machine(s: &str) -> Result<Machine, ToolError> {
    Machine::parse(&s.trim().to_ascii_lowercase())
        .ok_or_else(|| ToolError::Args(format!("`{s}` no es una máquina: usa `a` o `b`")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::find;

    fn check(tool: &str, v: Value) -> Result<Value, ToolError> {
        check_args(find(tool).unwrap(), &v)
    }

    #[test]
    fn rechaza_lo_que_falta_lo_desconocido_y_los_tipos() {
        assert!(check("exec", json!({"tunnel": "1"})).is_err());
        assert!(check("kill", json!({"tunnel": "1", "process": 1, "x": 1})).is_err());
        assert!(check("kill", json!({"tunnel": "1", "process": -1})).is_err());
        assert!(check("exec", json!({"tunnel": "1", "cmd": "ls", "args": [1]})).is_err());
    }

    #[test]
    fn acepta_numeros_y_booleanos_como_texto() {
        let v = check("kill", json!({"tunnel": "ab", "process": "3"})).unwrap();
        assert_eq!(v["process"], 3);
        let v = check("resources", json!({"rescan": "true", "machine": null})).unwrap();
        assert_eq!(v, json!({"rescan": true}));
    }

    #[test]
    fn el_id_del_tunel_es_hex_y_la_maquina_a_o_b() {
        let args = json!({"tunnel": "0x00000000000000ff"});
        assert_eq!(Args(&args).tunnel().unwrap().0, 255);
        assert!(Args(&json!({"tunnel": "zz"})).tunnel().is_err());
        assert_eq!(parse_machine(" B ").unwrap(), Machine::B);
        assert!(parse_machine("mac").is_err());
    }
}
