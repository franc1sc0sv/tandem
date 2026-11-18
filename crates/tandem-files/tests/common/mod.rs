//! Two agents in the same process (origin A, destination B) joined by in-memory paths.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use tandem_core::Machine;
use tandem_files::mem::MemPath;
use tandem_files::{Files, Options};
use tandem_proto::{Client, ClientKind, TunnelId};
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions};

pub const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Pair {
    pub origin: Origin,
    pub host: Host,
    /// That of the origin agent (the transfers and forwards) and that of the destination (the server).
    pub files_a: Files,
    pub files_b: Files,
    pub path: MemPath,
    pub tunnel: TunnelId,
}

pub fn options(chunk_size: u32) -> Options {
    Options {
        chunk_size,
        attempts: 1,
        retry_delay: Duration::from_millis(1),
    }
}

/// An open tunnel from A to B with chunks of `chunk_size` bytes.
pub async fn pair(chunk_size: u32) -> Pair {
    let origin = Origin::new(OriginOptions::default(), Changes::default());
    let host = Host::new(HostOptions::default(), Changes::default());
    let files_a = Files::new(options(chunk_size));
    let files_b = Files::new(options(chunk_size));
    let path = MemPath::new(&host, &files_b);
    origin.set_link(Some(path.dialer()));
    let tunnel = origin
        .open(OpenSpec {
            title: "pruebas".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
        })
        .await
        .unwrap()
        .id;
    Pair {
        origin,
        host,
        files_a,
        files_b,
        path,
        tunnel,
    }
}

/// Everything under `root`: relative path -> content (`None` for a folder).
pub fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = format!("{rel}{}", e.file_name().to_string_lossy());
            if e.file_type().unwrap().is_dir() {
                out.insert(name.clone(), None);
                walk(&e.path(), &format!("{name}/"), out);
            } else {
                out.insert(name, Some(fs::read(e.path()).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}
