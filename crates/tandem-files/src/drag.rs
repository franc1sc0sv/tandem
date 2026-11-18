//! Arrastrar archivos entre máquinas (D14, T-121).
//!
//! El sistema operativo pone lo suyo detrás de traits, para probar el flujo en Linux:
//! - [`DragDetector`] (origen): ¿hay un arrastre de archivos en curso? macOS lo lee de
//!   `NSPasteboard(name: .drag)`; Windows, del `DoDragDrop` activo por hook.
//! - [`DragStarter`] (destino): arranca un arrastre sintético con los archivos ya copiados
//!   (macOS: `NSDraggingSession` desde una ventana invisible; Windows: `DoDragDrop` con un
//!   `IDataObject`). Termina cuando el usuario suelta o cancela, y el sistema hace la copia.
//!
//! Flujo: el origen detecta el arrastre al llegar al borde ([`DragOut::detect`]) y deja
//! cruzar; mientras el cursor viaja, [`DragOut::run`] pide una carpeta temporal al destino
//! ([`DragPeer::begin`]), copia ahí con `tandem-files` y avisa ([`DragPeer::ready`]); el
//! destino ([`DragIn`]) inicia el arrastre sintético. Cancelar o soltar borra lo copiado.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tandem_proto::TunnelId;
use tandem_tunnel::Origin;

use crate::Files;
use crate::error::{FilesError, Result};
use crate::path::check_segment;

/// Cómo terminó el arrastre en el destino.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEnd {
    /// El usuario soltó sobre una app o carpeta: el sistema copió los archivos.
    Dropped,
    /// Esc, o soltó fuera de un destino válido.
    Cancelled,
}

/// Origen: detecta el arrastre de archivos del sistema.
pub trait DragDetector {
    /// Las rutas que se están arrastrando, o `None` si no hay un arrastre de archivos.
    fn file_drag(&self) -> Option<Vec<PathBuf>>;
}

/// Destino: arranca el arrastre sintético con archivos que ya están en disco.
pub trait DragStarter {
    /// Resuelve cuando el usuario suelta o cancela; al soltar, el sistema ya terminó de
    /// leer los archivos (así se pueden borrar de la carpeta temporal).
    fn start(&self, staged: Vec<PathBuf>) -> impl Future<Output = Result<DragEnd>>;
}

/// Lo que el origen pide al destino por el canal de control.
pub trait DragPeer {
    /// Prepara una carpeta temporal para `names`; devuelve su ruta en el destino.
    fn begin(&self, names: Vec<String>) -> impl Future<Output = Result<String>>;
    /// Todo está copiado: inicia el arrastre sintético y espera su final.
    fn ready(&self) -> impl Future<Output = Result<DragEnd>>;
    /// Aborta y borra lo copiado.
    fn cancel(&self) -> impl Future<Output = Result<()>>;
}

/// Copia `local` a `remote` en el destino (la implementación real usa `Files::put`).
pub trait Upload {
    fn upload(&self, local: &Path, remote: &str) -> impl Future<Output = Result<()>>;
}

/// [`Upload`] por un túnel abierto, con `Files::put` (reanudable y con progreso).
pub struct TunnelUpload<'a> {
    pub files: &'a Files,
    pub origin: &'a Origin,
    pub tunnel: TunnelId,
}

impl Upload for TunnelUpload<'_> {
    async fn upload(&self, local: &Path, remote: &str) -> Result<()> {
        self.files
            .put(self.origin, self.tunnel, local, remote)
            .await
            .map(|_| ())
    }
}

/// Avance del aviso en la UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragProgress {
    pub name: String,
    pub index: usize,
    pub total: usize,
}

/// Lado origen del arrastre.
pub struct DragOut<D, U, P> {
    pub detector: D,
    pub uploader: U,
    pub peer: P,
}

impl<D: DragDetector, U: Upload, P: DragPeer> DragOut<D, U, P> {
    /// Al llegar al borde: los archivos arrastrados, si los hay. Con `Some` el cruce se
    /// permite con el botón presionado aunque «Arrastrar entre máquinas» esté apagado.
    pub fn detect(&self) -> Option<Vec<PathBuf>> {
        self.detector.file_drag().filter(|f| !f.is_empty())
    }

    /// Copia `files` al destino y espera el final del arrastre allí. Si `cancelled`
    /// resuelve antes de terminar la copia, o algo falla, el destino borra lo copiado.
    pub async fn run(
        &self,
        files: &[PathBuf],
        cancelled: impl Future<Output = ()>,
        progress: &dyn Fn(DragProgress),
    ) -> Result<DragEnd> {
        let names = names_of(files)?;
        let dir = self.peer.begin(names.clone()).await?;
        let dir = dir.trim_end_matches(['/', '\\']);
        let copy = async {
            for (i, (file, name)) in files.iter().zip(&names).enumerate() {
                progress(DragProgress {
                    name: name.clone(),
                    index: i,
                    total: files.len(),
                });
                self.uploader.upload(file, &format!("{dir}/{name}")).await?;
            }
            progress(DragProgress {
                name: String::new(),
                index: files.len(),
                total: files.len(),
            });
            Ok(())
        };
        tokio::pin!(copy, cancelled);
        let copied: Result<bool> = tokio::select! {
            r = &mut copy => r.map(|()| true),
            () = &mut cancelled => Ok(false),
        };
        match copied {
            Ok(true) => self.peer.ready().await,
            Ok(false) => {
                self.peer.cancel().await?;
                Ok(DragEnd::Cancelled)
            }
            Err(e) => {
                // El error original importa más que uno al limpiar.
                let _ = self.peer.cancel().await;
                Err(e)
            }
        }
    }
}

/// Los nombres de nivel superior; el destino no los puede interpretar como rutas.
fn names_of(files: &[PathBuf]) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::with_capacity(files.len());
    for f in files {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| FilesError::path(&f.to_string_lossy(), "sin nombre"))?
            .to_owned();
        check_name(&name)?;
        if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            return Err(FilesError::path(&name, "nombre repetido"));
        }
        names.push(name);
    }
    Ok(names)
}

fn check_name(name: &str) -> Result<()> {
    check_segment(name, true).map_err(|why| FilesError::path(name, why))
}

#[derive(Debug)]
enum State {
    Idle,
    Staging(PathBuf, Vec<String>),
    /// Se canceló antes de que llegara `ready`.
    Cancelled,
}

/// Lado destino: carpeta temporal y arrastre sintético.
pub struct DragIn<S> {
    starter: S,
    base: PathBuf,
    state: Mutex<State>,
}

impl<S: DragStarter> DragIn<S> {
    /// `base`: carpeta temporal del agente donde se dejan las copias.
    pub fn new(starter: S, base: PathBuf) -> Self {
        DragIn {
            starter,
            base,
            state: Mutex::new(State::Idle),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Crea la carpeta de un arrastre nuevo (borra la de uno anterior sin terminar).
    pub fn begin(&self, names: &[String]) -> Result<PathBuf> {
        for n in names {
            check_name(n)?;
        }
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).map_err(|_| FilesError::Unexpected)?;
        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let dir = self.base.join(format!("drag-{hex}"));
        std::fs::create_dir_all(&dir).map_err(|e| FilesError::io(&dir, e))?;
        let new = State::Staging(dir.clone(), names.to_vec());
        if let State::Staging(old, _) = std::mem::replace(&mut *self.state(), new) {
            let _ = std::fs::remove_dir_all(old);
        }
        Ok(dir)
    }

    /// Inicia el arrastre sintético con lo copiado y limpia al terminar.
    pub async fn ready(&self) -> Result<DragEnd> {
        let taken = std::mem::replace(&mut *self.state(), State::Idle);
        let (dir, names) = match taken {
            State::Staging(d, n) => (d, n),
            State::Cancelled => return Ok(DragEnd::Cancelled),
            State::Idle => return Err(FilesError::Unexpected),
        };
        let staged: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        let end = match staged.iter().find(|p| !p.exists()) {
            Some(missing) => Err(FilesError::path(&missing.to_string_lossy(), "no llegó")),
            None => self.starter.start(staged).await,
        };
        let _ = std::fs::remove_dir_all(&dir);
        end
    }

    /// Cancelación desde el origen: borra lo copiado.
    pub fn cancel(&self) {
        let taken = std::mem::replace(&mut *self.state(), State::Cancelled);
        if let State::Staging(dir, _) = taken {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::rc::Rc;

    use super::*;

    struct Detector(Option<Vec<PathBuf>>);
    impl DragDetector for Detector {
        fn file_drag(&self) -> Option<Vec<PathBuf>> {
            self.0.clone()
        }
    }

    type Seen = Rc<RefCell<Vec<(PathBuf, bool)>>>;

    /// Doble del sistema destino: registra qué recibió y termina como se le diga.
    struct Starter {
        end: Option<DragEnd>,
        seen: Seen,
    }
    impl DragStarter for Starter {
        async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd> {
            for p in staged {
                let exists = p.exists();
                self.seen.borrow_mut().push((p, exists));
            }
            self.end.ok_or(FilesError::Unexpected)
        }
    }

    struct CopyUpload;
    impl Upload for CopyUpload {
        async fn upload(&self, local: &Path, remote: &str) -> Result<()> {
            fs::copy(local, remote)
                .map(|_| ())
                .map_err(|e| FilesError::io(local, e))
        }
    }

    struct Failing;
    impl Upload for Failing {
        async fn upload(&self, _: &Path, _: &str) -> Result<()> {
            Err(FilesError::Unexpected)
        }
    }

    struct Hanging;
    impl Upload for Hanging {
        async fn upload(&self, _: &Path, _: &str) -> Result<()> {
            std::future::pending().await
        }
    }

    /// El canal de control en el mismo proceso.
    struct Direct<'a, S>(&'a DragIn<S>);
    impl<S: DragStarter> DragPeer for Direct<'_, S> {
        async fn begin(&self, names: Vec<String>) -> Result<String> {
            Ok(self.0.begin(&names)?.to_string_lossy().into_owned())
        }
        async fn ready(&self) -> Result<DragEnd> {
            self.0.ready().await
        }
        async fn cancel(&self) -> Result<()> {
            self.0.cancel();
            Ok(())
        }
    }

    fn fixture(end: Option<DragEnd>) -> (tempfile::TempDir, Vec<PathBuf>, DragIn<Starter>, Seen) {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.bin");
        fs::write(&a, b"hola").unwrap();
        fs::write(&b, [1, 2, 3]).unwrap();
        let seen: Seen = Rc::default();
        let base = tmp.path().join("staging");
        fs::create_dir(&base).unwrap();
        let starter = Starter {
            end,
            seen: seen.clone(),
        };
        (tmp, vec![a, b], DragIn::new(starter, base), seen)
    }

    fn staging_empty(tmp: &tempfile::TempDir) -> bool {
        fs::read_dir(tmp.path().join("staging")).unwrap().count() == 0
    }

    fn out<U: Upload>(
        din: &DragIn<Starter>,
        uploader: U,
        files: Option<Vec<PathBuf>>,
    ) -> DragOut<Detector, U, Direct<'_, Starter>> {
        DragOut {
            detector: Detector(files),
            uploader,
            peer: Direct(din),
        }
    }

    #[test]
    fn sin_arrastre_de_archivos_no_se_cruza() {
        let (_t, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        assert_eq!(out(&din, CopyUpload, None).detect(), None);
        assert_eq!(out(&din, CopyUpload, Some(vec![])).detect(), None);
        let f = vec![PathBuf::from("/x/a")];
        assert_eq!(out(&din, CopyUpload, Some(f.clone())).detect(), Some(f));
    }

    #[tokio::test]
    async fn soltar_arranca_el_arrastre_con_lo_copiado_y_limpia() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let steps = Cell::new(0);
        let end = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|p| {
                steps.set(steps.get() + 1);
                assert_eq!(p.total, 2);
            })
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Dropped);
        assert_eq!(steps.get(), 3);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|(_, existed)| *existed));
        assert!(seen[0].0.ends_with("a.txt"));
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn cancelar_en_el_destino_borra_lo_copiado() {
        let (tmp, files, din, _s) = fixture(Some(DragEnd::Cancelled));
        let end = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|_| {})
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Cancelled);
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn cancelar_durante_la_copia_borra_y_no_arranca_el_arrastre() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let end = out(&din, Hanging, None)
            .run(&files, std::future::ready(()), &|_| {})
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Cancelled);
        assert!(seen.borrow().is_empty());
        assert!(staging_empty(&tmp));
        // Un `ready` tardío tampoco arranca nada.
        assert_eq!(din.ready().await.unwrap(), DragEnd::Cancelled);
    }

    #[tokio::test]
    async fn si_la_copia_falla_se_borra_y_se_devuelve_el_error() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let r = out(&din, Failing, None)
            .run(&files, std::future::pending(), &|_| {})
            .await;
        assert!(matches!(r, Err(FilesError::Unexpected)));
        assert!(seen.borrow().is_empty());
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn si_el_sistema_falla_igual_se_limpia() {
        let (tmp, files, din, _s) = fixture(None);
        let r = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|_| {})
            .await;
        assert!(r.is_err());
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn rechaza_nombres_peligrosos_o_repetidos() {
        let (tmp, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        assert!(din.begin(&["..".into()]).is_err());
        assert!(din.begin(&["a/b".into()]).is_err());
        assert!(din.begin(&["con".into()]).is_err());
        let dup = vec![PathBuf::from("/x/a.txt"), PathBuf::from("/y/A.TXT")];
        let r = out(&din, CopyUpload, None)
            .run(&dup, std::future::pending(), &|_| {})
            .await;
        assert!(matches!(r, Err(FilesError::Path { .. })));
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn ready_sin_begin_es_un_error() {
        let (_t, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        assert!(din.ready().await.is_err());
    }

    #[test]
    fn un_begin_nuevo_borra_el_anterior() {
        let (tmp, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        din.begin(&["a".into()]).unwrap();
        din.begin(&["b".into()]).unwrap();
        assert_eq!(fs::read_dir(tmp.path().join("staging")).unwrap().count(), 1);
    }
}
