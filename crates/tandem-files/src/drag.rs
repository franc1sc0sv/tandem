//! Dragging files between machines (D14, T-121).
//!
//! The operating system puts its part behind traits, to test the flow on Linux:
//! - [`DragDetector`] (origin): is a file drag in progress? macOS reads it from
//!   `NSPasteboard(name: .drag)`; Windows, from the active `DoDragDrop` via hook.
//! - [`DragStarter`] (destination): starts a synthetic drag with the already copied files
//!   (macOS: `NSDraggingSession` from an invisible window; Windows: `DoDragDrop` with an
//!   `IDataObject`). It ends when the user drops or cancels, and the system does the copy.
//!
//! Flow: the origin detects the drag on reaching the edge ([`DragOut::detect`]) and lets
//! it cross; while the cursor travels, [`DragOut::run`] asks the destination for a temporary folder
//! ([`DragPeer::begin`]), copies there with `tandem-files` and notifies ([`DragPeer::ready`]); the
//! destination ([`DragIn`]) starts the synthetic drag. Canceling or dropping deletes what was copied.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tandem_proto::TunnelId;
use tandem_tunnel::Origin;

use crate::Files;
use crate::error::{FilesError, Result};
use crate::path::check_segment;

/// How the drag ended on the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEnd {
    /// The user dropped on an app or folder: the system copied the files.
    Dropped,
    /// Esc, or dropped outside a valid target.
    Cancelled,
}

/// Origin: detects the system's file drag.
pub trait DragDetector {
    /// The paths being dragged, or `None` if there is no file drag.
    fn file_drag(&self) -> Option<Vec<PathBuf>>;
}

/// Destination: starts the synthetic drag with files already on disk.
pub trait DragStarter {
    /// Resolves when the user drops or cancels; on drop, the system has already finished
    /// reading the files (so they can be deleted from the temporary folder).
    fn start(&self, staged: Vec<PathBuf>) -> impl Future<Output = Result<DragEnd>>;
}

/// What the origin asks of the destination over the control channel.
pub trait DragPeer {
    /// Prepares a temporary folder for `names`; returns its path on the destination.
    fn begin(&self, names: Vec<String>) -> impl Future<Output = Result<String>>;
    /// Everything is copied: starts the synthetic drag and waits for its end.
    fn ready(&self) -> impl Future<Output = Result<DragEnd>>;
    /// Aborts and deletes what was copied.
    fn cancel(&self) -> impl Future<Output = Result<()>>;
}

/// Copies `local` to `remote` on the destination (the real implementation uses `Files::put`).
pub trait Upload {
    fn upload(&self, local: &Path, remote: &str) -> impl Future<Output = Result<()>>;
}

/// [`Upload`] over an open tunnel, with `Files::put` (resumable and with progress).
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

/// Progress of the UI notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragProgress {
    pub name: String,
    pub index: usize,
    pub total: usize,
}

/// Origin side of the drag.
pub struct DragOut<D, U, P> {
    pub detector: D,
    pub uploader: U,
    pub peer: P,
}

impl<D: DragDetector, U: Upload, P: DragPeer> DragOut<D, U, P> {
    /// On reaching the edge: the dragged files, if any. With `Some` the crossing is
    /// allowed with the button pressed even if "Drag between machines" is off.
    pub fn detect(&self) -> Option<Vec<PathBuf>> {
        self.detector.file_drag().filter(|f| !f.is_empty())
    }

    /// Copies `files` to the destination and waits for the drag to end there. If `cancelled`
    /// resolves before the copy finishes, or something fails, the destination deletes what was copied.
    pub async fn run(
        &self,
        files: &[PathBuf],
        cancelled: impl Future<Output = ()>,
        progress: &(dyn Fn(DragProgress) + Sync),
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
                // The original error matters more than one while cleaning up.
                let _ = self.peer.cancel().await;
                Err(e)
            }
        }
    }
}

/// The top-level names; the destination cannot interpret them as paths.
fn names_of(files: &[PathBuf]) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::with_capacity(files.len());
    for f in files {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| FilesError::path(&f.to_string_lossy(), "no name"))?
            .to_owned();
        check_name(&name)?;
        if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            return Err(FilesError::path(&name, "repeated name"));
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
    /// Canceled before `ready` arrived.
    Cancelled,
}

/// Destination side: temporary folder and synthetic drag.
pub struct DragIn<S> {
    starter: S,
    base: PathBuf,
    state: Mutex<State>,
}

impl<S: DragStarter> DragIn<S> {
    /// `base`: the agent's temporary folder where the copies are left.
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

    /// Creates the folder of a new drag (deletes that of an unfinished previous one).
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

    /// Starts the synthetic drag with what was copied and cleans up at the end.
    pub async fn ready(&self) -> Result<DragEnd> {
        let taken = std::mem::replace(&mut *self.state(), State::Idle);
        let (dir, names) = match taken {
            State::Staging(d, n) => (d, n),
            State::Cancelled => return Ok(DragEnd::Cancelled),
            State::Idle => return Err(FilesError::Unexpected),
        };
        let staged: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        let end = match staged.iter().find(|p| !p.exists()) {
            Some(missing) => Err(FilesError::path(
                &missing.to_string_lossy(),
                "did not arrive",
            )),
            None => self.starter.start(staged).await,
        };
        let _ = std::fs::remove_dir_all(&dir);
        end
    }

    /// Cancellation from the origin: deletes what was copied.
    pub fn cancel(&self) {
        let taken = std::mem::replace(&mut *self.state(), State::Cancelled);
        if let State::Staging(dir, _) = taken {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Detector(Option<Vec<PathBuf>>);
    impl DragDetector for Detector {
        fn file_drag(&self) -> Option<Vec<PathBuf>> {
            self.0.clone()
        }
    }

    type Seen = Rc<RefCell<Vec<(PathBuf, bool)>>>;

    /// Double of the destination system: records what it received and ends as told.
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

    /// The control channel in the same process.
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
        fs::write(&a, b"hello").unwrap();
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
    fn without_a_file_drag_there_is_no_crossing() {
        let (_t, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        assert_eq!(out(&din, CopyUpload, None).detect(), None);
        assert_eq!(out(&din, CopyUpload, Some(vec![])).detect(), None);
        let f = vec![PathBuf::from("/x/a")];
        assert_eq!(out(&din, CopyUpload, Some(f.clone())).detect(), Some(f));
    }

    #[tokio::test]
    async fn drop_starts_the_drag_with_what_was_copied_and_cleans_up() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let steps = AtomicUsize::new(0);
        let end = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|p| {
                steps.fetch_add(1, Ordering::Relaxed);
                assert_eq!(p.total, 2);
            })
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Dropped);
        assert_eq!(steps.load(Ordering::Relaxed), 3);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|(_, existed)| *existed));
        assert!(seen[0].0.ends_with("a.txt"));
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn cancel_on_the_destination_deletes_what_was_copied() {
        let (tmp, files, din, _s) = fixture(Some(DragEnd::Cancelled));
        let end = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|_| {})
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Cancelled);
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn cancel_during_the_copy_deletes_and_does_not_start_the_drag() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let end = out(&din, Hanging, None)
            .run(&files, std::future::ready(()), &|_| {})
            .await
            .unwrap();
        assert_eq!(end, DragEnd::Cancelled);
        assert!(seen.borrow().is_empty());
        assert!(staging_empty(&tmp));
        // A late `ready` does not start anything either.
        assert_eq!(din.ready().await.unwrap(), DragEnd::Cancelled);
    }

    #[tokio::test]
    async fn if_the_copy_fails_it_is_deleted_and_the_error_is_returned() {
        let (tmp, files, din, seen) = fixture(Some(DragEnd::Dropped));
        let r = out(&din, Failing, None)
            .run(&files, std::future::pending(), &|_| {})
            .await;
        assert!(matches!(r, Err(FilesError::Unexpected)));
        assert!(seen.borrow().is_empty());
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn if_the_system_fails_it_is_still_cleaned_up() {
        let (tmp, files, din, _s) = fixture(None);
        let r = out(&din, CopyUpload, None)
            .run(&files, std::future::pending(), &|_| {})
            .await;
        assert!(r.is_err());
        assert!(staging_empty(&tmp));
    }

    #[tokio::test]
    async fn rejects_dangerous_or_repeated_names() {
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
    async fn ready_without_begin_is_an_error() {
        let (_t, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        assert!(din.ready().await.is_err());
    }

    #[test]
    fn a_new_begin_deletes_the_previous_one() {
        let (tmp, _f, din, _s) = fixture(Some(DragEnd::Dropped));
        din.begin(&["a".into()]).unwrap();
        din.begin(&["b".into()]).unwrap();
        assert_eq!(fs::read_dir(tmp.path().join("staging")).unwrap().count(), 1);
    }
}
