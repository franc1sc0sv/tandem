//! Túneles entre las dos máquinas (DESIGN §8, DECISIONS D8, D9): el modelo y los mensajes.
//!
//! Un túnel lo abre un cliente local (Claude por `tandem mcp`, o una persona por la CLI)
//! en la máquina **origen**; su estado y sus recursos viven en la **destino**. Entre los
//! dos agentes van por un stream propio ([`crate::Channel::Tunnel`]): el origen manda un
//! [`TunnelUp`] y la destino contesta con [`TunnelDown`]. El pedido
//! [`TunnelUp::Attach`] convierte el stream en el canal de datos del túnel (prioridad de
//! datos, T-101): por ahí baja la salida de cada flujo con su posición en bytes, así que
//! al reconectar el origen la pide «desde el byte N» y no se pierde ni se repite nada.
//!
//! La lógica (registro, búfer circular, reconexión) está en `tandem-tunnel`.

use std::fmt;

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Identidad de un túnel. La elige el origen al azar: así abrir es idempotente (un
/// reintento con el mismo id devuelve el mismo túnel) y no choca entre las dos máquinas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TunnelId(pub u64);

impl fmt::Display for TunnelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// El repo o carpeta en el que trabaja el cliente; agrupa los túneles en la UI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Project {
    /// Para mostrar (el nombre de la carpeta, normalmente).
    pub name: String,
    /// Ruta en la máquina origen.
    pub root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientKind {
    /// `tandem mcp` (Claude).
    Mcp,
    /// `tandem` desde una terminal.
    Cli,
}

/// Quién abrió el túnel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Client {
    pub kind: ClientKind,
    /// La sesión del cliente (la de Claude), para reconocer sus túneles.
    pub session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TunnelState {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tunnel {
    pub id: TunnelId,
    /// El de la sesión de Claude, o el nombre del proyecto.
    pub title: String,
    pub project: Option<Project>,
    /// Donde está el cliente.
    pub origin: Machine,
    /// Donde viven los procesos, archivos y puertos del túnel.
    pub target: Machine,
    pub opened_by: Client,
    pub state: TunnelState,
    /// Milisegundos desde 1970 (UTC), según el reloj del origen.
    pub created: u64,
}

/// Un flujo de salida dentro de un túnel (la salida de un proceso, por ejemplo). Su id es
/// único dentro del túnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FlowId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowInfo {
    pub id: FlowId,
    /// Para mostrar («cargo test · stdout»).
    pub label: String,
}

/// Un proceso dentro de un túnel (T-104). Su id es único dentro del túnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessId(pub u32);

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p{}", self.0)
    }
}

/// Qué lanzar en la destino (`exec`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecSpec {
    /// El programa; con `shell`, la línea de órdenes que interpreta ese shell.
    pub cmd: String,
    pub args: Vec<String>,
    /// Un entorno detectado de la destino ([`crate::Environment::id`]: `zsh`, `pwsh`,
    /// `cmd`, `wsl:<distro>`), o `docker:<contenedor>`. `None`: el programa directo, sin
    /// shell.
    pub shell: Option<String>,
    /// Carpeta de trabajo en la destino (la del agente si no hay).
    pub cwd: Option<String>,
    /// Variables que se agregan a las del agente.
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessState {
    Running,
    /// Terminó solo; `code` es `None` si lo terminó una señal.
    Exited {
        code: Option<i32>,
    },
    /// Lo mató `kill` o el cierre del túnel (con todo su árbol).
    Killed,
}

/// Un proceso de un túnel tal como lo ven las dos máquinas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelProcess {
    pub id: ProcessId,
    pub tunnel: TunnelId,
    /// Para mostrar: el programa y sus argumentos.
    pub command: String,
    pub shell: Option<String>,
    pub cwd: Option<String>,
    /// El del sistema en la destino, si lo dio.
    pub pid: Option<u32>,
    pub stdout: FlowId,
    pub stderr: FlowId,
    /// Milisegundos desde 1970 (UTC), según el reloj de la destino.
    pub started: u64,
    /// Milisegundos que corrió, cuando ya terminó.
    pub duration: Option<u64>,
    pub state: ProcessState,
}

impl TunnelProcess {
    pub fn is_running(&self) -> bool {
        self.state == ProcessState::Running
    }
}

/// Cuántos bytes de cada salida ya tiene quien lee (`output { from_byte }`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputCursor {
    pub stdout: u64,
    pub stderr: u64,
}

/// La salida de un proceso desde un [`OutputCursor`], con stdout y stderr por separado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub process: TunnelProcess,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Dónde empieza lo devuelto: lo pedido, o más adelante si eso ya se descartó.
    pub from: OutputCursor,
    /// Lo que hay que pedir la próxima vez.
    pub next: OutputCursor,
    /// El proceso terminó y ya no queda salida por leer.
    pub done: bool,
}

/// Del origen a la destino. Cada stream lleva un solo pedido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelUp {
    /// Abrir (o, si ya existe con ese id, devolver) el túnel. Contesta
    /// [`TunnelDown::Opened`].
    Open(Tunnel),
    /// Cerrar el túnel y todo lo suyo en la destino. Contesta [`TunnelDown::Closed`].
    Close(TunnelId),
    /// Los túneles de la destino. Contesta [`TunnelDown::List`].
    List,
    /// Seguir la salida del túnel: desde aquí el stream es su canal de datos. `from` dice
    /// cuántos bytes de cada flujo ya tiene el origen; los que no aparecen, desde 0.
    Attach {
        tunnel: TunnelId,
        from: Vec<(FlowId, u64)>,
    },
    /// Archivos (T-106), por su propio stream.
    Files(crate::FilesUp),
    /// Puertos (T-106), por su propio stream.
    Port(crate::PortUp),
    /// Lanzar un proceso en el túnel (T-104). Contesta [`TunnelDown::Started`].
    Exec { tunnel: TunnelId, spec: ExecSpec },
    /// Matar un proceso con todo su árbol. Contesta [`TunnelDown::Killed`].
    Kill {
        tunnel: TunnelId,
        process: ProcessId,
    },
}

/// De la destino al origen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelDown {
    Opened(Tunnel),
    Closed(TunnelId),
    List(Vec<Tunnel>),
    /// El pedido no se pudo atender (túnel desconocido, el destino no es esta máquina).
    Refused(String),
    /// Por el canal de datos: un flujo que el origen todavía no conocía.
    Flow(FlowInfo),
    /// Bytes de `flow` desde la posición `offset`.
    Data {
        flow: FlowId,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Los bytes `from..to` de `flow` ya no están: el búfer circular los descartó mientras
    /// nadie los leía. Lo siguiente empieza en `to`.
    Lost {
        flow: FlowId,
        from: u64,
        to: u64,
    },
    /// `flow` terminó con `total` bytes y ya se mandaron todos.
    End {
        flow: FlowId,
        total: u64,
    },
    /// El túnel se cerró y ya se mandó toda su salida: el canal de datos termina aquí.
    TunnelClosed,
    /// Archivos (T-106), por su propio stream.
    Files(crate::FilesDown),
    /// Puertos (T-106), por su propio stream.
    Port(crate::PortDown),
    /// El proceso que se lanzó.
    Started(TunnelProcess),
    Killed(ProcessId),
    /// Por el canal de datos: un proceso del túnel, cada vez que cambia (termina).
    Process(TunnelProcess),
}

/// Un túnel en la UI, con los procesos que están corriendo en él (los terminados salen de
/// la lista; por MCP se pueden consultar un rato más).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelView {
    pub tunnel: Tunnel,
    pub running: Vec<TunnelProcess>,
}

impl From<Tunnel> for TunnelView {
    fn from(tunnel: Tunnel) -> Self {
        TunnelView {
            tunnel,
            running: Vec::new(),
        }
    }
}

/// Los túneles de un proyecto, para la UI (solo lectura, D9). `project` es `None` para
/// los abiertos sin proyecto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTunnels {
    pub project: Option<Project>,
    pub tunnels: Vec<TunnelView>,
}

/// Agrupa por proyecto, en orden de proyecto (los sin proyecto al final) y, dentro de cada
/// uno, del más viejo al más nuevo.
pub fn group_by_project(
    tunnels: impl IntoIterator<Item = impl Into<TunnelView>>,
) -> Vec<ProjectTunnels> {
    let mut groups: Vec<ProjectTunnels> = Vec::new();
    for t in tunnels {
        let t: TunnelView = t.into();
        match groups.iter_mut().find(|g| g.project == t.tunnel.project) {
            Some(g) => g.tunnels.push(t),
            None => groups.push(ProjectTunnels {
                project: t.tunnel.project.clone(),
                tunnels: vec![t],
            }),
        }
    }
    for g in &mut groups {
        g.tunnels.sort_by_key(|t| (t.tunnel.created, t.tunnel.id));
    }
    // `None` < `Some` en `Option`: se invierte para dejarlos al final.
    groups.sort_by(|a, b| match (&a.project, &b.project) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode};

    fn tunnel(id: u64, project: Option<&str>, created: u64) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: format!("t{id}"),
            project: project.map(|p| Project {
                name: p.to_owned(),
                root: format!("/src/{p}"),
            }),
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
            state: TunnelState::Open,
            created,
        }
    }

    #[test]
    fn agrupa_por_proyecto_con_los_sin_proyecto_al_final() {
        let groups = group_by_project([
            tunnel(1, None, 5),
            tunnel(2, Some("tandem"), 9),
            tunnel(3, Some("api"), 1),
            tunnel(4, Some("tandem"), 2),
        ]);
        let shape: Vec<(Option<String>, Vec<u64>)> = groups
            .iter()
            .map(|g| {
                (
                    g.project.as_ref().map(|p| p.name.clone()),
                    g.tunnels.iter().map(|t| t.tunnel.id.0).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("api".into()), vec![3]),
                (Some("tandem".into()), vec![4, 2]),
                (None, vec![1]),
            ]
        );
    }

    #[test]
    fn los_mensajes_hacen_ida_y_vuelta() {
        let up = TunnelUp::Attach {
            tunnel: TunnelId(7),
            from: vec![(FlowId(1), 1 << 40)],
        };
        assert_eq!(decode::<TunnelUp>(&encode(&up).unwrap()).unwrap(), up);
        let down = TunnelDown::Data {
            flow: FlowId(3),
            offset: 99,
            bytes: vec![0, 255, 10],
        };
        assert_eq!(decode::<TunnelDown>(&encode(&down).unwrap()).unwrap(), down);
        let open = TunnelUp::Open(tunnel(9, Some("x"), 3));
        assert_eq!(decode::<TunnelUp>(&encode(&open).unwrap()).unwrap(), open);
        let exec = TunnelUp::Exec {
            tunnel: TunnelId(1),
            spec: ExecSpec {
                cmd: "cargo test".into(),
                args: vec!["--".into(), "ñ".into()],
                shell: Some("wsl:Ubuntu-24.04".into()),
                cwd: Some("/src".into()),
                env: vec![("RUST_LOG".into(), "debug".into())],
            },
        };
        assert_eq!(decode::<TunnelUp>(&encode(&exec).unwrap()).unwrap(), exec);
        let process = TunnelDown::Process(process_sample());
        assert_eq!(
            decode::<TunnelDown>(&encode(&process).unwrap()).unwrap(),
            process
        );
    }

    pub(crate) fn process_sample() -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(3),
            tunnel: TunnelId(7),
            command: "cargo test".into(),
            shell: Some("pwsh".into()),
            cwd: None,
            pid: Some(4242),
            stdout: FlowId(5),
            stderr: FlowId(6),
            started: 1_763_400_000_000,
            duration: Some(1500),
            state: ProcessState::Exited { code: Some(101) },
        }
    }

    #[test]
    fn el_id_se_muestra_en_hexadecimal_fijo() {
        assert_eq!(TunnelId(0xab).to_string(), "00000000000000ab");
    }
}
