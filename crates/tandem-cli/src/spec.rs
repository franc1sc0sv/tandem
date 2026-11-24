//! The tools table: a single description for the MCP (JSON schema) and the CLI
//! (flags). Descriptions are in English only.

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Bool,
    /// Repeatable text (`--arg a --arg b`; in MCP, an array of strings).
    List,
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    pub desc: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub desc: &'static str,
    pub params: &'static [Param],
}

const fn p(name: &'static str, ty: Ty, required: bool, desc: &'static str) -> Param {
    Param {
        name,
        ty,
        required,
        desc,
    }
}

const TUNNEL: Param = p(
    "tunnel",
    Ty::Str,
    true,
    "Tunnel id (hex, from `tunnel_open` or `tunnels`)",
);
const PROCESS: Param = p(
    "process",
    Ty::Int,
    true,
    "Process id inside the tunnel (number, from `exec`)",
);

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "machines",
        desc: "The two machines of the desk (which one is this, hardware and tools of each).",
        params: &[],
    },
    Tool {
        name: "resources",
        desc: "Resources of a machine: CPU, RAM, GPU and detected environments (shells, WSL, Docker, toolchains). \
               Use it before `exec` to pick `shell`.",
        params: &[
            p(
                "machine",
                Ty::Str,
                false,
                "`a` or `b`; defaults to the other machine",
            ),
            p("rescan", Ty::Bool, false, "Rescan before reading"),
        ],
    },
    Tool {
        name: "monitors",
        desc: "The shared monitors and their link state: which machine each one shows, whether it is on, \
               in standby or off, and each machine's cable (connected, disconnected or unknown).",
        params: &[],
    },
    Tool {
        name: "tunnels",
        desc: "Open tunnels grouped by project, with their running processes.",
        params: &[],
    },
    Tool {
        name: "tunnel_open",
        desc: "Opens a tunnel to the other machine; run processes, copy files and open ports inside. \
               With `workspace` the repo travels with the tunnel and comes back as a branch via `bring_back`.",
        params: &[
            p(
                "target",
                Ty::Str,
                false,
                "Target machine, `a` or `b`; defaults to the other",
            ),
            p(
                "title",
                Ty::Str,
                false,
                "Tunnel title (e.g. the Claude session's); defaults to the project name",
            ),
            p(
                "project",
                Ty::Str,
                false,
                "Project name; defaults to the current folder's",
            ),
            p(
                "project_root",
                Ty::Str,
                false,
                "Project folder; defaults to the current folder",
            ),
            p(
                "session",
                Ty::Str,
                false,
                "Id of the session opening the tunnel",
            ),
            p(
                "workspace",
                Ty::Bool,
                false,
                "Carry the repo at `project_root` to the target",
            ),
            p(
                "keep",
                Ty::Bool,
                false,
                "With `workspace`: keep the target folder on close",
            ),
            p(
                "include_secrets",
                Ty::List,
                false,
                "With `workspace`: untracked credentials that do travel, repo paths (by default `.env`, keys, `.ssh/`… do not)",
            ),
        ],
    },
    Tool {
        name: "tunnel_close",
        desc: "Closes a tunnel: kills its processes and releases its ports.",
        params: &[TUNNEL],
    },
    Tool {
        name: "exec",
        desc: "Starts a process on the other machine (several in parallel per tunnel) and returns its id without waiting. \
               Read the output with `output`.",
        params: &[
            TUNNEL,
            p(
                "cmd",
                Ty::Str,
                true,
                "Program; with `shell`, the line that shell interprets",
            ),
            p("args", Ty::List, false, "Arguments"),
            p("shell", Ty::Str, false, "Environment from `resources`"),
            p("cwd", Ty::Str, false, "Working directory on the target"),
            p(
                "env",
                Ty::List,
                false,
                "Variables, `NAME=value`; secret-looking ones (`GH_TOKEN`, `*_KEY`…) are rejected unless `allow_secret_env`; `SSH_AUTH_SOCK` never",
            ),
            p(
                "allow_secret_env",
                Ty::Bool,
                false,
                "Lets secret-looking `env` variables through for this call",
            ),
            p(
                "pty",
                Ty::Bool,
                false,
                "Run it in a terminal (ConPTY on Windows): it sees a TTY, stdout and stderr come together, and it takes `input`",
            ),
            p("cols", Ty::Int, false, "Terminal width with `pty` (120)"),
            p("rows", Ty::Int, false, "Terminal height with `pty` (32)"),
        ],
    },
    Tool {
        name: "input",
        desc: "Types into a process started with `pty` (a shell, a REPL, a prompt). Delivered in order and exactly once,                also across link drops. Use `\\r` for Enter and `\\u0003` for Ctrl+C.",
        params: &[TUNNEL, PROCESS, p("text", Ty::Str, true, "What to type")],
    },
    Tool {
        name: "resize",
        desc: "Resizes the terminal of a process started with `pty`.",
        params: &[
            TUNNEL,
            PROCESS,
            p("cols", Ty::Int, true, "Width in columns"),
            p("rows", Ty::Int, true, "Height in rows"),
        ],
    },
    Tool {
        name: "processes",
        desc: "Processes of a tunnel (finished ones stay 10 min).",
        params: &[TUNNEL],
    },
    Tool {
        name: "output",
        desc: "Output of a process from a point (stdout and stderr apart) and whether it finished. \
               Returns at most `max_bytes` (24 KiB default, 64 KiB cap); if `more` is true, ask again \
               from `next` to keep reading. `wait_ms` waits for new output.",
        params: &[
            TUNNEL,
            PROCESS,
            p(
                "from_stdout",
                Ty::Int,
                false,
                "stdout byte to read from (0)",
            ),
            p(
                "from_stderr",
                Ty::Int,
                false,
                "stderr byte to read from (0)",
            ),
            p(
                "wait_ms",
                Ty::Int,
                false,
                "Wait up to this long for new output or the end of the process (max 60000)",
            ),
            p(
                "max_bytes",
                Ty::Int,
                false,
                "Max bytes across stdout and stderr (24576; cap 65536)",
            ),
        ],
    },
    Tool {
        name: "kill",
        desc: "Kills a process and its whole tree.",
        params: &[TUNNEL, PROCESS],
    },
    Tool {
        name: "files_put",
        desc: "Copies a file or folder from this machine to the other. Anything that looks like a credential (`.env`, keys, `.ssh/`…) is not copied and is listed in `skipped`.",
        params: &[
            TUNNEL,
            p("local", Ty::Str, true, "Path here"),
            p("remote", Ty::Str, true, "Path on the target"),
            p(
                "include_secrets",
                Ty::List,
                false,
                "Credentials that do get copied, paths relative to `local`",
            ),
        ],
    },
    Tool {
        name: "files_get",
        desc: "Fetches a file or folder from the other machine to this one.",
        params: &[
            TUNNEL,
            p("remote", Ty::Str, true, "Path on the target"),
            p("local", Ty::Str, true, "Path here"),
        ],
    },
    Tool {
        name: "port_forward",
        desc: "Forwards a port through the tunnel. `local_to_remote`: listens here and exits on the target; \
               `remote_to_local`: listens on the target and exits here.",
        params: &[
            TUNNEL,
            p("local_port", Ty::Int, true, "Port on this machine"),
            p("remote_port", Ty::Int, true, "Port on the target"),
            p("remote_host", Ty::Str, false, "Target host (127.0.0.1)"),
            p(
                "direction",
                Ty::Str,
                false,
                "`local_to_remote` (default) or `remote_to_local`",
            ),
        ],
    },
    Tool {
        name: "port_stop",
        desc: "Closes a port forward.",
        params: &[p("id", Ty::Int, true, "Id returned by `port_forward`")],
    },
    Tool {
        name: "bring_back",
        desc: "Brings the tunnel workspace's work back as a `tandem/<name>` branch of the local repo (with a commit for pending changes).",
        params: &[
            TUNNEL,
            p(
                "commit",
                Ty::Str,
                false,
                "Message for the pending-changes commit",
            ),
            p("name", Ty::Str, false, "Branch name (without `tandem/`)"),
        ],
    },
];

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

impl Tool {
    /// The JSON schema of the arguments (MCP `inputSchema`).
    pub fn schema(&self) -> Value {
        let mut props = serde_json::Map::new();
        for p in self.params {
            let ty = match p.ty {
                Ty::Str => json!({"type": "string"}),
                Ty::Int => json!({"type": "integer", "minimum": 0}),
                Ty::Bool => json!({"type": "boolean"}),
                Ty::List => json!({"type": "array", "items": {"type": "string"}}),
            };
            let mut ty = ty;
            ty["description"] = p.desc.into();
            props.insert(p.name.into(), ty);
        }
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name)
            .collect();
        json!({"type": "object", "properties": props, "required": required})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_and_the_task_tools_are_present() {
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        for want in [
            "machines",
            "resources",
            "monitors",
            "tunnels",
            "tunnel_open",
            "tunnel_close",
            "exec",
            "input",
            "resize",
            "output",
            "kill",
            "files_put",
            "files_get",
            "port_forward",
            "bring_back",
        ] {
            assert!(names.contains(&want), "missing {want}");
        }
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }

    #[test]
    fn every_description_is_non_empty() {
        for t in TOOLS {
            assert!(!t.desc.trim().is_empty(), "{}", t.name);
            for p in t.params {
                assert!(!p.desc.trim().is_empty(), "{}.{}", t.name, p.name);
            }
        }
    }

    #[test]
    fn the_schema_marks_required_ones() {
        let s = find("exec").unwrap().schema();
        assert_eq!(s["required"], json!(["tunnel", "cmd"]));
        assert_eq!(s["properties"]["args"]["type"], "array");
    }
}
