//! Archivos y puertos dentro de un túnel (DESIGN §8, T-106): los mensajes.
//!
//! Van como pedidos propios de [`crate::TunnelUp`] / [`crate::TunnelDown`] en streams de
//! [`crate::Channel::Tunnel`]. Un stream = una operación (una transferencia, una conexión
//! reenviada), así que un corte solo afecta a esa operación y se reintenta sin tocar lo
//! demás. La lógica está en `tandem-files`.

use serde::{Deserialize, Serialize};

use crate::TunnelId;

/// Tamaño de los fragmentos: 1 MiB.
pub const CHUNK_SIZE: u32 = 1 << 20;

/// Lo máximo que lleva un mensaje de datos: un fragmento viaja en pedazos porque un
/// marco no puede pasar de [`crate::MAX_FRAME_LEN`] (1 MiB, con el sobrecosto del mensaje).
pub const PIECE: usize = 256 * 1024;

/// BLAKE3 de 256 bits.
pub type Hash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Dir,
    File {
        size: u64,
        /// BLAKE3 de cada fragmento (el último puede ser más corto).
        chunks: Vec<Hash>,
        /// BLAKE3 del contenido completo.
        hash: Hash,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relativa a la raíz, con `/` como separador en las dos plataformas; sin `..`.
    pub path: String,
    pub kind: EntryKind,
    /// Bits de permisos de Unix (`0o7777`); `None` si el origen no los tiene.
    pub mode: Option<u32>,
    /// Fecha de modificación en milisegundos desde 1970.
    pub mtime_ms: Option<u64>,
}

/// Lo que se va a copiar, con los hashes para verificar y reanudar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub chunk_size: u32,
    /// Una sola entrada de archivo, cuya ruta es el nombre del archivo.
    pub single: bool,
    pub entries: Vec<FileEntry>,
}

impl Manifest {
    /// Bytes de contenido de todos los archivos.
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| match &e.kind {
                EntryKind::File { size, .. } => *size,
                EntryKind::Dir => 0,
            })
            .sum()
    }
}

/// Los fragmentos que le faltan a un archivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    pub entry: u32,
    pub chunks: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesReport {
    pub files: u64,
    pub dirs: u64,
    /// Bytes de contenido del total.
    pub bytes: u64,
    /// Bytes que de verdad viajaron (menos que `bytes` si se reanudó o ya estaba).
    pub sent: u64,
}

/// Avance de una transferencia, para la UI y el cliente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub tunnel: TunnelId,
    /// `true` si va hacia la otra máquina (`files_put`).
    pub upload: bool,
    /// La ruta local que se copia.
    pub path: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesUp {
    /// Copiar hacia la destino. `remote` es la carpeta de destino (o el archivo, si el
    /// manifiesto es de uno solo). Contesta [`FilesDown::Plan`].
    Put {
        tunnel: TunnelId,
        remote: String,
        manifest: Manifest,
    },
    /// Un pedazo (`offset` dentro del fragmento, hasta [`PIECE`] bytes) de un fragmento que
    /// pidió el plan. Sin respuesta.
    Chunk {
        entry: u32,
        index: u32,
        offset: u32,
        bytes: Vec<u8>,
    },
    /// Ya se mandó todo: verificar, renombrar y fijar permisos y fechas. Contesta
    /// [`FilesDown::Done`] o [`FilesDown::Failed`].
    Finish,
    /// Traer de la destino. Contesta [`FilesDown::Manifest`].
    Get { tunnel: TunnelId, remote: String },
    /// Pedir fragmentos de un archivo del manifiesto de `Get`; contesta un
    /// [`FilesDown::Chunk`] por cada pedazo de cada uno, en orden.
    Fetch { entry: u32, indices: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesDown {
    Plan(Vec<Need>),
    Done(FilesReport),
    Failed(String),
    Manifest(Manifest),
    Chunk {
        entry: u32,
        index: u32,
        offset: u32,
        bytes: Vec<u8>,
    },
}

/// Hacia dónde va un reenvío de puerto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Escucha `local_port` aquí y sale por `remote_host:remote_port` en la destino.
    LocalToRemote,
    /// Escucha `remote_port` en la destino (en `remote_host`) y sale por
    /// `127.0.0.1:local_port` de aquí.
    RemoteToLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardInfo {
    pub id: u64,
    pub tunnel: TunnelId,
    pub direction: Direction,
    /// El puerto real de este lado (si se pidió el 0, el que dio el sistema).
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortUp {
    /// Abrir en la destino una conexión TCP a `host:port`, identificada por `conn`.
    /// Contesta [`PortDown::Ready`] o [`PortDown::Failed`].
    Connect {
        tunnel: TunnelId,
        conn: u64,
        host: String,
        port: u16,
    },
    /// Escuchar en la destino. Contesta [`PortDown::Listening`] y luego un
    /// [`PortDown::Incoming`] por conexión, mientras el stream siga abierto.
    Listen {
        tunnel: TunnelId,
        bind: String,
        port: u16,
    },
    /// Este stream lleva lo que va del origen a la conexión `conn`. Contesta
    /// [`PortDown::Ready`]; luego solo `Bytes` y `Eof` hacia la destino.
    Send {
        tunnel: TunnelId,
        conn: u64,
    },
    /// Este stream lleva lo que sale de la conexión `conn`. Contesta [`PortDown::Ready`];
    /// luego solo `Bytes` y `Eof` hacia el origen.
    Recv {
        tunnel: TunnelId,
        conn: u64,
    },
    Bytes(Vec<u8>),
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortDown {
    Ready,
    Failed(String),
    Listening {
        port: u16,
    },
    Incoming {
        conn: u64,
    },
    /// Latido de un stream de `Listen` (cada pocos segundos): si no se puede mandar, el
    /// origen se fue y la destino deja de escuchar.
    Beat,
    Bytes(Vec<u8>),
    Eof,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode};

    #[test]
    fn los_mensajes_hacen_ida_y_vuelta() {
        let manifest = Manifest {
            chunk_size: CHUNK_SIZE,
            single: false,
            entries: vec![
                FileEntry {
                    path: "a b/ñandú.txt".into(),
                    kind: EntryKind::File {
                        size: 3,
                        chunks: vec![[7; 32]],
                        hash: [9; 32],
                    },
                    mode: Some(0o644),
                    mtime_ms: Some(1),
                },
                FileEntry {
                    path: "vacia".into(),
                    kind: EntryKind::Dir,
                    mode: None,
                    mtime_ms: None,
                },
            ],
        };
        let up = crate::TunnelUp::Files(FilesUp::Put {
            tunnel: TunnelId(4),
            remote: "C:\\x".into(),
            manifest: manifest.clone(),
        });
        assert_eq!(
            decode::<crate::TunnelUp>(&encode(&up).unwrap()).unwrap(),
            up
        );
        let down = crate::TunnelDown::Files(FilesDown::Manifest(manifest));
        assert_eq!(
            decode::<crate::TunnelDown>(&encode(&down).unwrap()).unwrap(),
            down
        );
        let port = crate::TunnelUp::Port(PortUp::Bytes(vec![0, 1, 255]));
        assert_eq!(
            decode::<crate::TunnelUp>(&encode(&port).unwrap()).unwrap(),
            port
        );
    }
}
