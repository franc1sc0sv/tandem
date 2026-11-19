//! Dragging files between two agents: detect, copy over the tunnel, drag on the
//! destination (double) and drop or cancel.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{TIMEOUT, pair, snapshot};
use tandem_files::FilesError;
use tandem_files::drag::{
    DragDetector, DragEnd, DragIn, DragOut, DragPeer, DragStarter, TunnelUpload,
};

struct Detector(Vec<PathBuf>);
impl DragDetector for Detector {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        Some(self.0.clone())
    }
}

/// The destination "system": on drop it copies what was dragged to `drop_to`, like Explorer.
struct Os {
    drop_to: Option<PathBuf>,
}
impl DragStarter for Os {
    async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd, FilesError> {
        let Some(to) = &self.drop_to else {
            return Ok(DragEnd::Cancelled);
        };
        for p in staged {
            let dst = to.join(p.file_name().unwrap());
            if p.is_dir() {
                copy_dir(&p, &dst);
            } else {
                fs::copy(&p, &dst).unwrap();
            }
        }
        Ok(DragEnd::Dropped)
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            fs::copy(e.path(), dst).unwrap();
        }
    }
}

struct Direct<'a>(&'a DragIn<Os>);
impl DragPeer for Direct<'_> {
    async fn begin(&self, names: Vec<String>) -> Result<String, FilesError> {
        Ok(self.0.begin(&names)?.to_string_lossy().into_owned())
    }
    async fn ready(&self) -> Result<DragEnd, FilesError> {
        self.0.ready().await
    }
    async fn cancel(&self) -> Result<(), FilesError> {
        self.0.cancel();
        Ok(())
    }
}

async fn flow(drop: bool) -> (DragEnd, tempfile::TempDir, usize) {
    let p = pair(4).await;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(src.join("folder")).unwrap();
    fs::write(src.join("note.txt"), b"contents of the note").unwrap();
    fs::write(src.join("folder/inside.bin"), [7u8; 30]).unwrap();
    let (staging, desk) = (tmp.path().join("staging"), tmp.path().join("desktop"));
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&desk).unwrap();
    let din = DragIn::new(
        Os {
            drop_to: drop.then(|| desk.clone()),
        },
        staging.clone(),
    );
    let files = vec![src.join("note.txt"), src.join("folder")];
    let out = DragOut {
        detector: Detector(files.clone()),
        uploader: TunnelUpload {
            files: &p.files_a,
            origin: &p.origin,
            tunnel: p.tunnel,
        },
        peer: Direct(&din),
    };
    assert_eq!(out.detect().as_deref(), Some(files.as_slice()));
    let steps = AtomicUsize::new(0);
    let end = tokio::time::timeout(
        TIMEOUT,
        out.run(&files, std::future::pending(), &|_| {
            steps.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        fs::read_dir(&staging).unwrap().count(),
        0,
        "it is cleaned up"
    );
    (end, tmp, steps.load(Ordering::Relaxed))
}

#[tokio::test]
async fn drop_leaves_the_files_on_the_destination_and_cleans_up() {
    let (end, tmp, steps) = flow(true).await;
    assert_eq!(end, DragEnd::Dropped);
    assert_eq!(steps, 3);
    let desk = snapshot(&tmp.path().join("desktop"));
    assert_eq!(
        desk["note.txt"].as_deref(),
        Some(&b"contents of the note"[..])
    );
    assert_eq!(desk["folder/inside.bin"].as_deref(), Some(&[7u8; 30][..]));
}

#[tokio::test]
async fn cancel_leaves_nothing_on_the_destination() {
    let (end, tmp, _) = flow(false).await;
    assert_eq!(end, DragEnd::Cancelled);
    assert!(snapshot(&tmp.path().join("desktop")).is_empty());
}
