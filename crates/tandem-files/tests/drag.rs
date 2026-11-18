//! Arrastrar archivos entre dos agentes: detectar, copiar por el túnel, arrastre en el
//! destino (doble) y soltar o cancelar.

mod common;

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;

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

/// El «sistema» del destino: al soltar copia lo arrastrado a `drop_to`, como el Explorador.
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
    fs::create_dir_all(src.join("carpeta")).unwrap();
    fs::write(src.join("nota.txt"), b"contenido de la nota").unwrap();
    fs::write(src.join("carpeta/dentro.bin"), [7u8; 30]).unwrap();
    let (staging, desk) = (tmp.path().join("staging"), tmp.path().join("escritorio"));
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&desk).unwrap();
    let din = DragIn::new(
        Os {
            drop_to: drop.then(|| desk.clone()),
        },
        staging.clone(),
    );
    let files = vec![src.join("nota.txt"), src.join("carpeta")];
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
    let steps = Cell::new(0);
    let end = tokio::time::timeout(
        TIMEOUT,
        out.run(&files, std::future::pending(), &|_| {
            steps.set(steps.get() + 1)
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 0, "se limpia");
    (end, tmp, steps.get())
}

#[tokio::test]
async fn soltar_deja_los_archivos_en_el_destino_y_limpia() {
    let (end, tmp, steps) = flow(true).await;
    assert_eq!(end, DragEnd::Dropped);
    assert_eq!(steps, 3);
    let desk = snapshot(&tmp.path().join("escritorio"));
    assert_eq!(
        desk["nota.txt"].as_deref(),
        Some(&b"contenido de la nota"[..])
    );
    assert_eq!(desk["carpeta/dentro.bin"].as_deref(), Some(&[7u8; 30][..]));
}

#[tokio::test]
async fn cancelar_no_deja_nada_en_el_destino() {
    let (end, tmp, _) = flow(false).await;
    assert_eq!(end, DragEnd::Cancelled);
    assert!(snapshot(&tmp.path().join("escritorio")).is_empty());
}
