//! Los túneles de Claude para la pantalla Túneles (solo lectura, D9): lo que dicen
//! `tandem_proto::ProjectTunnels`, `TunnelProcess` y `ProcessOutput` con la forma que lee la
//! ventana.
//!
//! Los ids de túnel son de 64 bits al azar: JSON los pasaría como número y JavaScript pierde
//! los bits de arriba, así que viajan como texto (el `Display` de `TunnelId`, 16 hex).

use serde::{Deserialize, Serialize};
use tandem_core::Machine;
use tandem_proto::{
    OutputCursor, ProcessId, ProcessOutput, ProcessState, ProjectTunnels, TransferProgress,
    TunnelId, TunnelProcess, TunnelState,
};

/// Cuántos bytes de cada flujo pide la ventana por consulta.
pub const OUTPUT_CHUNK: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessStateView {
    Running,
    Exited { code: Option<i32> },
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessItem {
    pub id: u32,
    pub command: String,
    pub pid: Option<u32>,
    /// Milisegundos Unix, según el reloj de la máquina destino.
    pub started: u64,
    pub duration: Option<u64>,
    pub state: ProcessStateView,
}

impl From<TunnelProcess> for ProcessItem {
    fn from(p: TunnelProcess) -> Self {
        ProcessItem {
            id: p.id.0,
            command: p.command,
            pid: p.pid,
            started: p.started,
            duration: p.duration,
            state: match p.state {
                ProcessState::Running => ProcessStateView::Running,
                ProcessState::Exited { code } => ProcessStateView::Exited { code },
                ProcessState::Killed => ProcessStateView::Killed,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelItem {
    pub id: String,
    pub title: String,
    pub origin: Machine,
    pub target: Machine,
    pub open: bool,
    /// Milisegundos Unix, según el reloj del origen.
    pub created: u64,
    /// Solo los procesos que están corriendo.
    pub running: Vec<ProcessItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectItem {
    /// `None`: túneles abiertos sin proyecto.
    pub name: Option<String>,
    pub root: Option<String>,
    pub tunnels: Vec<TunnelItem>,
}

/// Los túneles de las dos máquinas, agrupados por proyecto (evento `agent-tunnels`).
pub fn projects_of(groups: Vec<ProjectTunnels>) -> Vec<ProjectItem> {
    groups
        .into_iter()
        .map(|g| ProjectItem {
            name: g.project.as_ref().map(|p| p.name.clone()),
            root: g.project.map(|p| p.root),
            tunnels: g
                .tunnels
                .into_iter()
                .map(|t| TunnelItem {
                    id: t.tunnel.id.to_string(),
                    title: t.tunnel.title,
                    origin: t.tunnel.origin,
                    target: t.tunnel.target,
                    open: t.tunnel.state == TunnelState::Open,
                    created: t.tunnel.created,
                    running: t.running.into_iter().map(Into::into).collect(),
                })
                .collect(),
        })
        .collect()
}

/// La salida de un proceso desde un cursor, ya como texto (respuesta de `Output`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputItem {
    pub process: ProcessItem,
    pub stdout: String,
    pub stderr: String,
    pub next_stdout: u64,
    pub next_stderr: u64,
    /// El proceso terminó y ya se entregó todo.
    pub done: bool,
}

impl From<ProcessOutput> for OutputItem {
    fn from(o: ProcessOutput) -> Self {
        OutputItem {
            process: o.process.into(),
            // Un carácter partido entre dos consultas se ve como «�»: la salida es para leerla.
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            next_stdout: o.next.stdout,
            next_stderr: o.next.stderr,
            done: o.done,
        }
    }
}

/// El avance de una copia de archivos de un túnel (evento `agent-transfer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferItem {
    pub tunnel: String,
    pub upload: bool,
    pub path: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

impl From<TransferProgress> for TransferItem {
    fn from(t: TransferProgress) -> Self {
        TransferItem {
            tunnel: t.tunnel.to_string(),
            upload: t.upload,
            path: t.path,
            done: t.done,
            total: t.total,
            finished: t.finished,
        }
    }
}

/// `TunnelId` desde el texto con que viaja a la ventana.
pub fn parse_tunnel(s: &str) -> Result<TunnelId, String> {
    u64::from_str_radix(s, 16)
        .map(TunnelId)
        .map_err(|_| format!("«{s}» no es un id de túnel"))
}

/// Deserializa el id de túnel de una acción.
pub fn tunnel_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TunnelId, D::Error> {
    let s = String::deserialize(d)?;
    parse_tunnel(&s).map_err(serde::de::Error::custom)
}

/// Desde dónde seguir leyendo la salida de un proceso.
pub fn cursor(stdout: u64, stderr: u64) -> OutputCursor {
    OutputCursor { stdout, stderr }
}

pub fn process_id(id: u32) -> ProcessId {
    ProcessId(id)
}

#[cfg(test)]
mod tests {
    use tandem_proto::{Client, ClientKind, FlowId, Project, Tunnel, TunnelView, group_by_project};

    use super::*;

    fn tunnel(id: u64, project: Option<&str>, state: TunnelState) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: format!("sesión {id}"),
            project: project.map(|p| Project {
                name: p.into(),
                root: format!("/src/{p}"),
            }),
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
            state,
            created: 1_000 + id,
        }
    }

    fn process(id: u32, state: ProcessState) -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(id),
            tunnel: TunnelId(1),
            command: "cargo test".into(),
            shell: None,
            cwd: None,
            pid: Some(77),
            stdout: FlowId(1),
            stderr: FlowId(2),
            started: 5,
            duration: None,
            state,
        }
    }

    #[test]
    fn el_id_viaja_como_texto_y_vuelve_sin_perder_bits() {
        let id = TunnelId(u64::MAX - 3);
        let text = id.to_string();
        assert_eq!(text.len(), 16);
        assert_eq!(parse_tunnel(&text), Ok(id));
        assert!(parse_tunnel("no es hex").is_err());
    }

    #[test]
    fn los_proyectos_conservan_orden_procesos_y_estado() {
        let mut running = TunnelView::from(tunnel(1, Some("tandem"), TunnelState::Open));
        running.running = vec![process(3, ProcessState::Running)];
        let groups = group_by_project([
            running,
            TunnelView::from(tunnel(2, None, TunnelState::Closed)),
        ]);
        let items = projects_of(groups);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name.as_deref(), Some("tandem"));
        assert_eq!(items[0].root.as_deref(), Some("/src/tandem"));
        assert_eq!(
            items[0].tunnels[0].running[0].state,
            ProcessStateView::Running
        );
        assert_eq!(items[0].tunnels[0].running[0].id, 3);
        assert_eq!(items[1].name, None);
        assert!(!items[1].tunnels[0].open);
    }

    #[test]
    fn la_salida_llega_como_texto_con_su_cursor() {
        let out = ProcessOutput {
            process: process(3, ProcessState::Exited { code: Some(0) }),
            stdout: b"ok\n".to_vec(),
            stderr: vec![b'e', 0xff],
            from: cursor(0, 0),
            next: cursor(3, 2),
            done: true,
        };
        let item = OutputItem::from(out);
        assert_eq!(item.stdout, "ok\n");
        assert_eq!(item.stderr, "e\u{fffd}");
        assert_eq!((item.next_stdout, item.next_stderr), (3, 2));
        assert!(item.done);
        assert_eq!(
            item.process.state,
            ProcessStateView::Exited { code: Some(0) }
        );
    }

    #[test]
    fn la_transferencia_nombra_el_tunel_con_su_id_de_texto() {
        let t = TransferItem::from(TransferProgress {
            tunnel: TunnelId(255),
            upload: true,
            path: "/a".into(),
            done: 1,
            total: 2,
            finished: false,
        });
        assert_eq!(t.tunnel, "00000000000000ff");
    }
}
