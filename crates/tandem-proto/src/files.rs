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

/// Lo que cabe en una página de manifiesto o de plan: bien por debajo de
/// [`crate::MAX_FRAME_LEN`], para que un árbol de cualquier tamaño viaje en varios mensajes.
pub const PAGE_BUDGET: usize = 256 * 1024;

/// Tope de entradas de un manifiesto, para que la otra máquina no pida memoria sin límite.
pub const MAX_ENTRIES: usize = 4_000_000;

/// Peso aproximado de una entrada una vez codificada.
pub fn entry_cost(e: &FileEntry) -> usize {
    let chunks = match &e.kind {
        EntryKind::File { chunks, .. } => 32 * chunks.len() + 32,
        EntryKind::Dir => 0,
    };
    e.path.len() + chunks + 40
}

/// Parte las entradas en páginas contiguas de a lo sumo [`PAGE_BUDGET`] (una entrada sola
/// puede pasarse; [`entry_fits`] dice si aun así cabe en un marco).
pub fn page_entries(entries: &[FileEntry]) -> Vec<&[FileEntry]> {
    let (mut pages, mut start, mut cost) = (Vec::new(), 0, 0);
    for (i, e) in entries.iter().enumerate() {
        let c = entry_cost(e);
        if i > start && cost + c > PAGE_BUDGET {
            pages.push(&entries[start..i]);
            (start, cost) = (i, 0);
        }
        cost += c;
    }
    if start < entries.len() {
        pages.push(&entries[start..]);
    }
    pages
}

/// Si la entrada, sola, cabe en un marco.
pub fn entry_fits(e: &FileEntry) -> bool {
    entry_cost(e) < crate::MAX_FRAME_LEN - 1024
}

/// Parte un plan en páginas por cantidad de fragmentos (5 B por índice como máximo); un
/// archivo con muchos fragmentos se reparte en varios [`Need`] con el mismo `entry`.
pub fn page_needs(needs: &[Need]) -> Vec<Vec<Need>> {
    const PER_PAGE: usize = PAGE_BUDGET / 8;
    let (mut pages, mut cur, mut room) = (Vec::new(), Vec::new(), PER_PAGE);
    for n in needs {
        for part in n.chunks.chunks(PER_PAGE) {
            if part.len() > room {
                pages.push(std::mem::take(&mut cur));
                room = PER_PAGE;
            }
            room -= part.len();
            cur.push(Need {
                entry: n.entry,
                chunks: part.to_vec(),
            });
        }
    }
    if !cur.is_empty() {
        pages.push(cur);
    }
    pages
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
    /// manifiesto es de uno solo). El manifiesto sigue en [`FilesUp::Entries`], por
    /// páginas; con la última contesta [`FilesDown::Plan`].
    Put {
        tunnel: TunnelId,
        remote: String,
        chunk_size: u32,
        single: bool,
    },
    /// Una página de entradas del manifiesto de `Put`; `last` cierra el manifiesto.
    Entries { entries: Vec<FileEntry>, last: bool },
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
    /// Traer de la destino. Contesta [`FilesDown::ManifestHead`] y las páginas de entradas.
    Get { tunnel: TunnelId, remote: String },
    /// Pedir fragmentos de un archivo del manifiesto de `Get`; contesta un
    /// [`FilesDown::Chunk`] por cada pedazo de cada uno, en orden.
    Fetch { entry: u32, indices: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesDown {
    /// Una página del plan; con `last` termina.
    Plan {
        needs: Vec<Need>,
        last: bool,
    },
    Done(FilesReport),
    Failed(String),
    /// Encabezado del manifiesto de `Get`; siguen las páginas [`FilesDown::Entries`].
    ManifestHead {
        chunk_size: u32,
        single: bool,
    },
    Entries {
        entries: Vec<FileEntry>,
        last: bool,
    },
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

    fn entry(i: usize, chunks: usize) -> FileEntry {
        FileEntry {
            path: format!("dir/archivo-{i}.txt"),
            kind: EntryKind::File {
                size: 1,
                chunks: vec![[1; 32]; chunks],
                hash: [2; 32],
            },
            mode: Some(0o644),
            mtime_ms: Some(1),
        }
    }

    #[test]
    fn las_paginas_de_entradas_caben_en_un_marco_y_no_pierden_nada() {
        let entries: Vec<_> = (0..30_000).map(|i| entry(i, 3)).collect();
        let pages = page_entries(&entries);
        assert!(pages.len() > 1);
        assert_eq!(pages.concat(), entries);
        for p in pages {
            let msg = crate::TunnelUp::Files(FilesUp::Entries {
                entries: p.to_vec(),
                last: false,
            });
            assert!(encode(&msg).unwrap().len() < crate::MAX_FRAME_LEN);
        }
    }

    #[test]
    fn las_paginas_del_plan_caben_en_un_marco_y_no_pierden_nada() {
        let needs: Vec<_> = (0..300u32)
            .map(|e| Need {
                entry: e,
                chunks: (0..2_000).collect(),
            })
            .chain([Need {
                entry: 999,
                chunks: (0..200_000).collect(),
            }])
            .collect();
        let pages = page_needs(&needs);
        let mut got: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for p in &pages {
            let msg = crate::TunnelDown::Files(FilesDown::Plan {
                needs: p.clone(),
                last: false,
            });
            assert!(encode(&msg).unwrap().len() < crate::MAX_FRAME_LEN);
            for n in p {
                got.entry(n.entry).or_default().extend(&n.chunks);
            }
        }
        assert_eq!(got.len(), 301);
        assert_eq!(got[&999], (0..200_000).collect::<Vec<u32>>());
        assert_eq!(got[&7].len(), 2_000);
    }

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
            chunk_size: manifest.chunk_size,
            single: false,
        });
        assert_eq!(
            decode::<crate::TunnelUp>(&encode(&up).unwrap()).unwrap(),
            up
        );
        let down = crate::TunnelDown::Files(FilesDown::Entries {
            entries: manifest.entries,
            last: true,
        });
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
