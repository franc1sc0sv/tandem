//! Files and ports inside a tunnel (DESIGN §8, T-106): the messages.
//!
//! They travel as [`crate::TunnelUp`] / [`crate::TunnelDown`] requests of their own on streams of
//! [`crate::Channel::Tunnel`]. One stream = one operation (a transfer, a forwarded
//! connection), so a cut only affects that operation and is retried without touching the
//! rest. The logic is in `tandem-files`.

use serde::{Deserialize, Serialize};

use crate::TunnelId;

/// Chunk size: 1 MiB.
pub const CHUNK_SIZE: u32 = 1 << 20;

/// The most a data message carries: a chunk travels in pieces because a
/// frame cannot exceed [`crate::MAX_FRAME_LEN`] (1 MiB, with the message overhead).
pub const PIECE: usize = 256 * 1024;

/// BLAKE3 de 256 bits.
pub type Hash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Dir,
    File {
        size: u64,
        /// BLAKE3 of each chunk (the last one may be shorter).
        chunks: Vec<Hash>,
        /// BLAKE3 of the whole content.
        hash: Hash,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative to the root, with `/` as separator on both platforms; without `..`.
    pub path: String,
    pub kind: EntryKind,
    /// Unix permission bits (`0o7777`); `None` if the source does not have them.
    pub mode: Option<u32>,
    /// Modification date in milliseconds since 1970.
    pub mtime_ms: Option<u64>,
}

/// What is going to be copied, with the hashes to verify and resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub chunk_size: u32,
    /// A single file entry, whose path is the file name.
    pub single: bool,
    pub entries: Vec<FileEntry>,
}

impl Manifest {
    /// Content bytes of all the files.
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

/// What fits in a manifest or plan page: well below
/// [`crate::MAX_FRAME_LEN`], so a tree of any size travels in several messages.
pub const PAGE_BUDGET: usize = 256 * 1024;

/// Cap on manifest entries, so the other machine does not ask for unbounded memory.
pub const MAX_ENTRIES: usize = 4_000_000;

/// Approximate weight of an entry once encoded.
pub fn entry_cost(e: &FileEntry) -> usize {
    let chunks = match &e.kind {
        EntryKind::File { chunks, .. } => 32 * chunks.len() + 32,
        EntryKind::Dir => 0,
    };
    e.path.len() + chunks + 40
}

/// Splits the entries into contiguous pages of at most [`PAGE_BUDGET`] (a single entry
/// may exceed it; [`entry_fits`] says whether it still fits in a frame).
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

/// Whether the entry, alone, fits in a frame.
pub fn entry_fits(e: &FileEntry) -> bool {
    entry_cost(e) < crate::MAX_FRAME_LEN - 1024
}

/// Splits a plan into pages by number of chunks (5 B per index at most); a
/// file with many chunks is spread over several [`Need`]s with the same `entry`.
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

/// The chunks a file is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    pub entry: u32,
    pub chunks: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesReport {
    pub files: u64,
    pub dirs: u64,
    /// Content bytes of the total.
    pub bytes: u64,
    /// Bytes that actually traveled (fewer than `bytes` if it resumed or was already there).
    pub sent: u64,
    /// What was not copied for looking like a credential (T-130); folders end in
    /// `/`. It never travels to the other machine unless named in `include`.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// Transfer progress, for the UI and the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub tunnel: TunnelId,
    /// `true` if it goes to the other machine (`files_put`).
    pub upload: bool,
    /// The local path being copied.
    pub path: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesUp {
    /// Copy to the destination. `remote` is the destination folder (or the file, if the
    /// manifest is for a single one). The manifest continues in [`FilesUp::Entries`], in
    /// pages; with the last one it answers [`FilesDown::Plan`].
    Put {
        tunnel: TunnelId,
        remote: String,
        chunk_size: u32,
        single: bool,
    },
    /// A page of entries of the `Put` manifest; `last` closes the manifest.
    Entries { entries: Vec<FileEntry>, last: bool },
    /// A piece (`offset` within the chunk, up to [`PIECE`] bytes) of a chunk that
    /// the plan asked for. No reply.
    Chunk {
        entry: u32,
        index: u32,
        offset: u32,
        bytes: Vec<u8>,
    },
    /// Everything has been sent: verify, rename and set permissions and dates. It answers
    /// [`FilesDown::Done`] o [`FilesDown::Failed`].
    Finish,
    /// Bring from the destination. Answers [`FilesDown::ManifestHead`] and the pages of entries.
    Get { tunnel: TunnelId, remote: String },
    /// Ask for chunks of a file of the `Get` manifest; answers one
    /// [`FilesDown::Chunk`] per piece of each one, in order.
    Fetch { entry: u32, indices: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesDown {
    /// A page of the plan; with `last` it ends.
    Plan {
        needs: Vec<Need>,
        last: bool,
    },
    Done(FilesReport),
    Failed(String),
    /// Header of the `Get` manifest; the [`FilesDown::Entries`] pages follow.
    ManifestHead {
        chunk_size: u32,
        single: bool,
        /// What the destination did not send for looking like a credential (T-130).
        skipped: Vec<String>,
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

/// Where a port forward goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Listens on `local_port` here and exits through `remote_host:remote_port` on the destination.
    LocalToRemote,
    /// Listens on `remote_port` on the destination (on `remote_host`) and exits through
    /// `127.0.0.1:local_port` here.
    RemoteToLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardInfo {
    pub id: u64,
    pub tunnel: TunnelId,
    pub direction: Direction,
    /// The real port on this side (if 0 was requested, the one the system gave).
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortUp {
    /// Open a TCP connection to `host:port` on the destination, identified by `conn`.
    /// Answers [`PortDown::Ready`] or [`PortDown::Failed`].
    Connect {
        tunnel: TunnelId,
        conn: u64,
        host: String,
        port: u16,
    },
    /// Listen on the destination. Answers [`PortDown::Listening`] and then one
    /// [`PortDown::Incoming`] per connection, while the stream stays open.
    Listen {
        tunnel: TunnelId,
        bind: String,
        port: u16,
    },
    /// This stream carries what goes from the source to connection `conn`. Answers
    /// [`PortDown::Ready`]; afterwards only `Bytes` and `Eof` toward the destination.
    Send {
        tunnel: TunnelId,
        conn: u64,
    },
    /// This stream carries what comes out of connection `conn`. Answers [`PortDown::Ready`];
    /// afterwards only `Bytes` and `Eof` toward the source.
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
    /// Heartbeat of a `Listen` stream (every few seconds): if it cannot be sent, the
    /// source is gone and the destination stops listening.
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
            path: format!("dir/file-{i}.txt"),
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
    fn entry_pages_fit_in_a_frame_and_lose_nothing() {
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
    fn plan_pages_fit_in_a_frame_and_lose_nothing() {
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
    fn messages_round_trip() {
        let manifest = Manifest {
            chunk_size: CHUNK_SIZE,
            single: false,
            entries: vec![
                FileEntry {
                    // Unicode and a space in the name (test data).
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
                    path: "empty".into(),
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
