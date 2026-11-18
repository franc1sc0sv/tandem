//! La tabla de herramientas: una sola descripción para el MCP (esquema JSON) y la CLI
//! (banderas). Las descripciones van en español e inglés para que Claude elija bien.

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Bool,
    /// Texto repetible (`--arg a --arg b`; en MCP, un arreglo de textos).
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
    "Id del túnel (hex, de `tunnel_open` o `tunnels`) / Tunnel id (hex, from `tunnel_open` or `tunnels`)",
);
const PROCESS: Param = p(
    "process",
    Ty::Int,
    true,
    "Id del proceso dentro del túnel (número, de `exec`) / Process id inside the tunnel (number, from `exec`)",
);

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "machines",
        desc: "Las dos máquinas del puesto (cuál es esta, qué hardware y herramientas tiene cada una). \
               / The two machines of the desk (which one is this, hardware and tools of each).",
        params: &[],
    },
    Tool {
        name: "resources",
        desc: "Recursos de una máquina: CPU, RAM, GPU y entornos detectados (shells, WSL, Docker, toolchains). \
               Úsala antes de `exec` para elegir `shell`. \
               / Resources of a machine: CPU, RAM, GPU and detected environments (shells, WSL, Docker, toolchains). \
               Use it before `exec` to pick `shell`.",
        params: &[
            p(
                "machine",
                Ty::Str,
                false,
                "`a` o `b`; por omisión, la otra máquina / `a` or `b`; defaults to the other machine",
            ),
            p(
                "rescan",
                Ty::Bool,
                false,
                "Volver a escanear antes de leer / Rescan before reading",
            ),
        ],
    },
    Tool {
        name: "tunnels",
        desc: "Túneles abiertos, agrupados por proyecto, con sus procesos corriendo. \
               / Open tunnels grouped by project, with their running processes.",
        params: &[],
    },
    Tool {
        name: "tunnel_open",
        desc: "Abre un túnel hacia la otra máquina; dentro se ejecutan procesos, se copian archivos y se abren puertos. \
               Con `workspace` el repo viaja con el túnel y vuelve como rama con `bring_back`. \
               / Opens a tunnel to the other machine; run processes, copy files and open ports inside. \
               With `workspace` the repo travels with the tunnel and comes back as a branch via `bring_back`.",
        params: &[
            p(
                "target",
                Ty::Str,
                false,
                "Máquina destino, `a` o `b`; por omisión, la otra / Target machine, `a` or `b`; defaults to the other",
            ),
            p(
                "title",
                Ty::Str,
                false,
                "Título del túnel (p. ej. el de la sesión de Claude); sin él, el nombre del proyecto / Tunnel title (e.g. the Claude session's); defaults to the project name",
            ),
            p(
                "project",
                Ty::Str,
                false,
                "Nombre del proyecto; por omisión, el de la carpeta actual / Project name; defaults to the current folder's",
            ),
            p(
                "project_root",
                Ty::Str,
                false,
                "Carpeta del proyecto; por omisión, la carpeta actual / Project folder; defaults to the current folder",
            ),
            p(
                "session",
                Ty::Str,
                false,
                "Id de la sesión que abre el túnel / Id of the session opening the tunnel",
            ),
            p(
                "workspace",
                Ty::Bool,
                false,
                "Llevar el repo de `project_root` al destino / Carry the repo at `project_root` to the target",
            ),
            p(
                "keep",
                Ty::Bool,
                false,
                "Con `workspace`: conservar la carpeta del destino al cerrar / With `workspace`: keep the target folder on close",
            ),
        ],
    },
    Tool {
        name: "tunnel_close",
        desc: "Cierra un túnel: mata sus procesos y suelta sus puertos. / Closes a tunnel: kills its processes and releases its ports.",
        params: &[TUNNEL],
    },
    Tool {
        name: "exec",
        desc: "Lanza un proceso en la otra máquina (varios en paralelo por túnel) y devuelve su id sin esperar. \
               Lee la salida con `output`. \
               / Starts a process on the other machine (several in parallel per tunnel) and returns its id without waiting. \
               Read the output with `output`.",
        params: &[
            TUNNEL,
            p(
                "cmd",
                Ty::Str,
                true,
                "Programa; con `shell`, la línea que ese shell interpreta / Program; with `shell`, the line that shell interprets",
            ),
            p("args", Ty::List, false, "Argumentos / Arguments"),
            p(
                "shell",
                Ty::Str,
                false,
                "Entorno de `resources` (`zsh`, `pwsh`, `cmd`, `wsl:<distro>`, `docker:<contenedor>`) / Environment from `resources`",
            ),
            p(
                "cwd",
                Ty::Str,
                false,
                "Carpeta de trabajo en el destino / Working directory on the target",
            ),
            p(
                "env",
                Ty::List,
                false,
                "Variables, `NOMBRE=valor` / Variables, `NAME=value`",
            ),
        ],
    },
    Tool {
        name: "processes",
        desc: "Procesos de un túnel (los terminados quedan 10 min). / Processes of a tunnel (finished ones stay 10 min).",
        params: &[TUNNEL],
    },
    Tool {
        name: "output",
        desc: "Salida de un proceso desde un punto (stdout y stderr por separado) y si ya terminó. \
               Vuelve a pedir desde `next` para seguirlo; `wait_ms` espera salida nueva. \
               / Output of a process from a point (stdout and stderr apart) and whether it finished. \
               Ask again from `next` to follow it; `wait_ms` waits for new output.",
        params: &[
            TUNNEL,
            PROCESS,
            p(
                "from_stdout",
                Ty::Int,
                false,
                "Byte de stdout desde el que leer (0) / stdout byte to read from (0)",
            ),
            p(
                "from_stderr",
                Ty::Int,
                false,
                "Byte de stderr desde el que leer (0) / stderr byte to read from (0)",
            ),
            p(
                "wait_ms",
                Ty::Int,
                false,
                "Esperar hasta tanto por salida nueva o el fin del proceso (máx. 60000) / Wait up to this long for new output or the end of the process (max 60000)",
            ),
        ],
    },
    Tool {
        name: "kill",
        desc: "Mata un proceso y todo su árbol. / Kills a process and its whole tree.",
        params: &[TUNNEL, PROCESS],
    },
    Tool {
        name: "files_put",
        desc: "Copia un archivo o carpeta de esta máquina a la otra. / Copies a file or folder from this machine to the other.",
        params: &[
            TUNNEL,
            p("local", Ty::Str, true, "Ruta aquí / Path here"),
            p(
                "remote",
                Ty::Str,
                true,
                "Ruta en el destino / Path on the target",
            ),
        ],
    },
    Tool {
        name: "files_get",
        desc: "Trae un archivo o carpeta de la otra máquina a esta. / Fetches a file or folder from the other machine to this one.",
        params: &[
            TUNNEL,
            p(
                "remote",
                Ty::Str,
                true,
                "Ruta en el destino / Path on the target",
            ),
            p("local", Ty::Str, true, "Ruta aquí / Path here"),
        ],
    },
    Tool {
        name: "port_forward",
        desc: "Reenvía un puerto por el túnel. `local_to_remote`: escucha aquí y sale por el destino; \
               `remote_to_local`: escucha en el destino y sale por aquí. \
               / Forwards a port through the tunnel. `local_to_remote`: listens here and exits on the target; \
               `remote_to_local`: listens on the target and exits here.",
        params: &[
            TUNNEL,
            p(
                "local_port",
                Ty::Int,
                true,
                "Puerto de esta máquina / Port on this machine",
            ),
            p(
                "remote_port",
                Ty::Int,
                true,
                "Puerto del destino / Port on the target",
            ),
            p(
                "remote_host",
                Ty::Str,
                false,
                "Host del destino (127.0.0.1) / Target host (127.0.0.1)",
            ),
            p(
                "direction",
                Ty::Str,
                false,
                "`local_to_remote` (por omisión) o `remote_to_local` / `local_to_remote` (default) or `remote_to_local`",
            ),
        ],
    },
    Tool {
        name: "port_stop",
        desc: "Cierra un reenvío de puerto. / Closes a port forward.",
        params: &[p(
            "id",
            Ty::Int,
            true,
            "Id que devolvió `port_forward` / Id returned by `port_forward`",
        )],
    },
    Tool {
        name: "bring_back",
        desc: "Trae el trabajo del workspace del túnel como rama `tandem/<nombre>` del repo local (con un commit de lo pendiente). \
               / Brings the tunnel workspace's work back as a `tandem/<name>` branch of the local repo (with a commit for pending changes).",
        params: &[
            TUNNEL,
            p(
                "commit",
                Ty::Str,
                false,
                "Mensaje del commit de lo pendiente / Message for the pending-changes commit",
            ),
            p(
                "name",
                Ty::Str,
                false,
                "Nombre de la rama (sin `tandem/`) / Branch name (without `tandem/`)",
            ),
        ],
    },
];

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

impl Tool {
    /// El esquema JSON de los argumentos (`inputSchema` de MCP).
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
    fn nombres_unicos_y_las_herramientas_de_la_tarea_estan() {
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        for want in [
            "machines",
            "resources",
            "tunnels",
            "tunnel_open",
            "tunnel_close",
            "exec",
            "output",
            "kill",
            "files_put",
            "files_get",
            "port_forward",
            "bring_back",
        ] {
            assert!(names.contains(&want), "falta {want}");
        }
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }

    #[test]
    fn cada_descripcion_trae_español_e_ingles() {
        for t in TOOLS {
            assert!(t.desc.contains(" / "), "{}", t.name);
            for p in t.params {
                assert!(p.desc.contains(" / "), "{}.{}", t.name, p.name);
            }
        }
    }

    #[test]
    fn el_esquema_marca_lo_obligatorio() {
        let s = find("exec").unwrap().schema();
        assert_eq!(s["required"], json!(["tunnel", "cmd"]));
        assert_eq!(s["properties"]["args"]["type"], "array");
    }
}
