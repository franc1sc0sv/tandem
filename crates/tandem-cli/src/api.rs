//! The tools over the agent's IPC. It is the only place that knows which `UiCommand`
//! is behind each one; the MCP and the CLI only call [`Api::call`].

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
    #[error("unknown tool: `{0}`")]
    Unknown(String),
    /// Arguments that do not satisfy the [`crate::spec`] table.
    #[error("{0}")]
    Args(String),
    /// The agent rejected the request or it could not be spoken to.
    #[error("{0}")]
    Agent(String),
}

/// Where `tunnel_open`'s default values come from.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub cwd: Option<PathBuf>,
    /// The session that opens tunnels (`CLAUDE_SESSION_ID` or the client's name).
    pub session: String,
}

/// How long to wait at most for new output in `output`.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// Bytes of `output` per call, between stdout and stderr, if nothing else is requested. Noisy
/// output must not fill the model's context: it is read in pages with `next`.
pub const DEFAULT_MAX_BYTES: u64 = 24 * 1024;
/// Hard cap of `max_bytes`.
pub const HARD_MAX_BYTES: u64 = 64 * 1024;
/// Floor of `max_bytes`: a whole UTF-8 character fits, so it always advances.
const MIN_MAX_BYTES: u64 = 16;

/// How many bytes of each stream fit in a page of `budget` bytes in total.
/// Each stream is entitled to half and what the other does not use goes to the first; then it is
/// trimmed to the last UTF-8 boundary if the cut fell in the middle of a character.
fn page(stdout: &[u8], stderr: &[u8], budget: usize, agent_max: usize) -> (usize, usize) {
    let half = budget / 2;
    let err0 = stderr.len().min(half);
    let out = stdout.len().min(budget - err0);
    let err = stderr.len().min(budget - out);
    (
        utf8_cut(stdout, out, agent_max),
        utf8_cut(stderr, err, agent_max),
    )
}

/// Trims `take` bytes of `s` so as not to leave a split character at the end. Only if the cut is
/// ours (`take < len`) or could have been the agent's (`len >= agent_max`): an incomplete tail at the
/// real end of the output is delivered as is, or it would never be delivered.
fn utf8_cut(s: &[u8], take: usize, agent_max: usize) -> usize {
    let cut_here = take < s.len() || s.len() >= agent_max;
    if !cut_here {
        return take;
    }
    for back in 1..=take.min(3) {
        let b = s[take - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation: keep going until the start of the character
        }
        let width = match b {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if width > back { take - back } else { take };
    }
    take
}

pub struct Api<S> {
    ipc: IpcClient<S>,
    ctx: Context,
}

/// The arguments already checked against the table.
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn str(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(Value::as_str)
    }
    fn req(&self, k: &str) -> Result<&str, ToolError> {
        self.str(k)
            .ok_or_else(|| ToolError::Args(format!("missing `{k}`")))
    }
    fn int(&self, k: &str) -> Option<u64> {
        self.0.get(k).and_then(Value::as_u64)
    }
    fn req_int(&self, k: &str) -> Result<u64, ToolError> {
        self.int(k)
            .ok_or_else(|| ToolError::Args(format!("missing `{k}`")))
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
            .map_err(|_| ToolError::Args(format!("`{t}` is not a tunnel id (hex)")))
    }
    fn process(&self) -> Result<ProcessId, ToolError> {
        let n = self.req_int("process")?;
        u32::try_from(n)
            .map(ProcessId)
            .map_err(|_| ToolError::Args(format!("`{n}` is not a process id")))
    }
    fn port(&self, k: &str) -> Result<u16, ToolError> {
        let n = self.req_int(k)?;
        u16::try_from(n).map_err(|_| ToolError::Args(format!("`{k}`: {n} is not a port")))
    }
}

/// Checks types and required ones, and accepts numbers and booleans written as text (what
/// some models send). Returns the normalized arguments.
pub fn check_args(tool: &Tool, raw: &Value) -> Result<Value, ToolError> {
    let empty = serde_json::Map::new();
    let obj = match raw {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(ToolError::Args("the arguments must be an object".into())),
    };
    if let Some(k) = obj
        .keys()
        .find(|k| !tool.params.iter().any(|p| p.name == *k))
    {
        return Err(ToolError::Args(format!(
            "`{}` does not have the parameter `{k}`",
            tool.name
        )));
    }
    let mut out = serde_json::Map::new();
    for p in tool.params {
        let Some(v) = obj.get(p.name).filter(|v| !v.is_null()) else {
            if p.required {
                return Err(ToolError::Args(format!("missing `{}`", p.name)));
            }
            continue;
        };
        let bad = || ToolError::Args(format!("`{}` does not have the expected type", p.name));
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
    // The 64-bit id travels as hex text: a number like that is lost in JavaScript.
    v["id"] = t.id.to_string().into();
    v
}

/// Adds `skipped` and a warning when the credentials filter omitted something (T-130): the
/// calling agent has to learn that the destination does not have those files.
fn with_skipped(mut v: Value, skipped: &[String]) -> Value {
    if skipped.is_empty() {
        return v;
    }
    v["skipped"] = json!(skipped);
    v["warning"] = format!(
        "{} file(s) omitted for looking like credentials and did not reach the other machine: {}. \
         If they are really needed, name them in `include_secrets`.",
        skipped.len(),
        skipped.join(", ")
    )
    .into();
    v
}

/// Adds `history_secrets` and a warning: what is already committed travels with its history and the filter
/// cannot remove it (T-130). It is joined to the previous `warning` if there is one.
fn with_history(mut v: Value, history: &[String]) -> Value {
    if history.is_empty() {
        return v;
    }
    v["history_secrets"] = json!(history);
    let msg = format!(
        "the repo history that traveled contains files that look like credentials: {}. They are \
         already on the other machine; if they are real, rotate them.",
        history.join(", ")
    );
    v["warning"] = match v["warning"].as_str() {
        Some(prev) => format!("{prev} {msg}"),
        None => msg,
    }
    .into();
    v
}

fn process_json(p: &TunnelProcess) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    v["tunnel"] = p.tunnel.to_string().into();
    v
}

fn unexpected(r: &Reply) -> ToolError {
    ToolError::Agent(format!("unexpected reply from the agent: {r:?}"))
}

impl<S: AsyncRead + AsyncWrite + Unpin> Api<S> {
    pub fn new(ipc: IpcClient<S>, ctx: Context) -> Self {
        Api { ipc, ctx }
    }

    async fn ask(&mut self, cmd: UiCommand) -> Result<Reply, ToolError> {
        match self.ipc.request(cmd).await {
            Ok(Reply::Rejected(why)) => Err(ToolError::Agent(why)),
            Ok(r) => Ok(r),
            Err(e) => Err(ToolError::Agent(format!("agent IPC: {e}"))),
        }
    }

    /// Runs tool `name`; the result is the same JSON for the MCP and for
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
                                Err(ToolError::Args(format!("`env`: `{kv}` is not NAME=value")))
                            }
                        })
                        .collect::<Result<_, _>>()?,
                    allow_secret_env: a.flag("allow_secret_env"),
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
                        include: a.list("include_secrets"),
                    }
                } else {
                    UiCommand::FilesGet {
                        tunnel,
                        remote,
                        local,
                    }
                };
                match self.ask(cmd).await? {
                    Reply::Files(r) => {
                        let skipped = r.skipped.clone();
                        Ok(with_skipped(
                            serde_json::to_value(r).unwrap_or(Value::Null),
                            &skipped,
                        ))
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "port_forward" => {
                let direction = match a.str("direction").unwrap_or("local_to_remote") {
                    "local_to_remote" => Direction::LocalToRemote,
                    "remote_to_local" => Direction::RemoteToLocal,
                    other => {
                        return Err(ToolError::Args(format!(
                            "`direction`: `{other}` is neither `local_to_remote` nor `remote_to_local`"
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
                "no resources of machine {} yet: is it connected?",
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
                    "`workspace` needs `project_root` (there is no current folder)".into(),
                )
            })?;
            Some(WorkspaceSpec {
                repo_path,
                keep: a.flag("keep"),
                include: a.list("include_secrets"),
            })
        } else {
            None
        };
        let cmd = UiCommand::OpenTunnel {
            // Without a title, the agent uses the project's.
            title: a.str("title").unwrap_or_default().to_owned(),
            project,
            target,
            session: a
                .str("session")
                .map_or_else(|| self.ctx.session.clone(), str::to_owned),
            workspace,
        };
        match self.ask(cmd).await? {
            Reply::Tunnel {
                tunnel,
                skipped,
                history_secrets,
            } => Ok(with_history(
                with_skipped(tunnel_json(&tunnel), &skipped),
                &history_secrets,
            )),
            r => Err(unexpected(&r)),
        }
    }

    async fn output(&mut self, a: &Args<'_>) -> Result<Value, ToolError> {
        let (tunnel, process) = (a.tunnel()?, a.process()?);
        let from = OutputCursor {
            stdout: a.int("from_stdout").unwrap_or(0),
            stderr: a.int("from_stderr").unwrap_or(0),
        };
        let budget = a
            .int("max_bytes")
            .unwrap_or(DEFAULT_MAX_BYTES)
            .clamp(MIN_MAX_BYTES, HARD_MAX_BYTES) as u32;
        let wait = Duration::from_millis(a.int("wait_ms").unwrap_or(0)).min(MAX_WAIT);
        let deadline = Instant::now() + wait;
        loop {
            let reply = self
                .ask(UiCommand::Output {
                    tunnel,
                    process,
                    from,
                    max: budget,
                })
                .await?;
            let Reply::Output(o) = reply else {
                return Err(unexpected(&reply));
            };
            // The agent does not wait: here we poll until there is something or `wait_ms` expires.
            let empty = o.stdout.is_empty() && o.stderr.is_empty();
            if !(empty && !o.done && Instant::now() < deadline) {
                let (so, se) = page(&o.stdout, &o.stderr, budget as usize, budget as usize);
                // There is more if we trimmed or if the agent may have hit `max`.
                let more = so < o.stdout.len()
                    || se < o.stderr.len()
                    || o.stdout.len() >= budget as usize
                    || o.stderr.len() >= budget as usize;
                // The cursor advances only by what was delivered, not by what the agent returned.
                let next_out = o.from.stdout + so as u64;
                let next_err = o.from.stderr + se as u64;
                return Ok(json!({
                    "process": process_json(&o.process),
                    "stdout": String::from_utf8_lossy(&o.stdout[..so]),
                    "stderr": String::from_utf8_lossy(&o.stderr[..se]),
                    "from": {"stdout": o.from.stdout, "stderr": o.from.stderr},
                    "next": {"stdout": next_out, "stderr": next_err},
                    "more": more,
                    "done": o.done && !more,
                }));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn parse_machine(s: &str) -> Result<Machine, ToolError> {
    Machine::parse(&s.trim().to_ascii_lowercase())
        .ok_or_else(|| ToolError::Args(format!("`{s}` is not a machine: use `a` or `b`")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::find;

    fn check(tool: &str, v: Value) -> Result<Value, ToolError> {
        check_args(find(tool).unwrap(), &v)
    }

    #[test]
    fn rejects_missing_unknown_and_wrong_types() {
        assert!(check("exec", json!({"tunnel": "1"})).is_err());
        assert!(check("kill", json!({"tunnel": "1", "process": 1, "x": 1})).is_err());
        assert!(check("kill", json!({"tunnel": "1", "process": -1})).is_err());
        assert!(check("exec", json!({"tunnel": "1", "cmd": "ls", "args": [1]})).is_err());
    }

    #[test]
    fn accepts_numbers_and_booleans_as_text() {
        let v = check("kill", json!({"tunnel": "ab", "process": "3"})).unwrap();
        assert_eq!(v["process"], 3);
        let v = check("resources", json!({"rescan": "true", "machine": null})).unwrap();
        assert_eq!(v, json!({"rescan": true}));
    }

    #[test]
    fn page_splits_the_cap_between_streams() {
        let big = vec![b'a'; 1000];
        assert_eq!(page(&big, &big, 100, 100), (50, 50));
        assert_eq!(page(&big, b"xy", 100, 100), (98, 2));
        assert_eq!(page(b"", &big, 100, 100), (0, 100));
    }

    #[test]
    fn trims_at_the_last_utf8_boundary() {
        // Unicode test data of 1, 2, 3 and 4 bytes per character.
        let s = "añ€😀".as_bytes(); // 1 + 2 + 3 + 4 bytes
        assert_eq!(s.len(), 10);
        // Cuts that split a character move back to the previous one.
        assert_eq!(utf8_cut(s, 2, 100), 1);
        assert_eq!(utf8_cut(s, 5, 100), 3);
        assert_eq!(utf8_cut(s, 7, 100), 6);
        assert_eq!(utf8_cut(s, 9, 100), 6);
        assert_eq!(utf8_cut(s, 6, 100), 6);
        // The real end of the output is delivered whole even if a broken tail remains.
        assert_eq!(utf8_cut(&s[..9], 9, 100), 9);
        // But if the agent may have cut (len == max), it is trimmed too.
        assert_eq!(utf8_cut(&s[..9], 9, 9), 6);
    }

    proptest::proptest! {
        #[test]
        fn paging_never_loses_or_repeats_bytes(
            text in "\\PC{0,300}", budget in 16usize..64,
        ) {
            let bytes = text.as_bytes();
            let mut pos = 0;
            let mut got = Vec::new();
            while pos < bytes.len() {
                let rest = &bytes[pos..];
                let win = &rest[..rest.len().min(budget)];
                let (n, _) = page(win, b"", budget, budget);
                proptest::prop_assert!(n > 0);
                proptest::prop_assert!(std::str::from_utf8(&win[..n]).is_ok());
                got.extend_from_slice(&win[..n]);
                pos += n;
            }
            proptest::prop_assert_eq!(got, bytes);
        }
    }

    #[test]
    fn tunnel_id_is_hex_and_machine_is_a_or_b() {
        let args = json!({"tunnel": "0x00000000000000ff"});
        assert_eq!(Args(&args).tunnel().unwrap().0, 255);
        assert!(Args(&json!({"tunnel": "zz"})).tunnel().is_err());
        assert_eq!(parse_machine(" B ").unwrap(), Machine::B);
        assert!(parse_machine("mac").is_err());
    }
}
