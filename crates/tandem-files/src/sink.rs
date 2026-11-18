//! The writing side: validates the manifest, says what is missing, stores verified
//! chunks in a `.tandem-part` file and on finishing renames it with its permissions and
//! dates.
//!
//! What is received lives on disk, not in memory: if the link is cut (or the agent
//! restarts), the next round walks the `.tandem-part` files, compares the hash per chunk and
//! asks only for what does not match.
//!
//! Everything is blocking (disk): callers run it in `spawn_blocking`.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tandem_proto::{EntryKind, FilesReport, Manifest, Need};

use crate::error::{FilesError, Result};
use crate::path::{check_segment, from_wire};
use crate::source::{chunk_count, chunk_len, hash_file, read_full};

const MAX_CHUNK_SIZE: u32 = 64 << 20;
const PART_EXT: &str = ".tandem-part";

pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s: OsString = final_path.as_os_str().to_owned();
    s.push(PART_EXT);
    s.into()
}

#[derive(Debug)]
pub struct Sink {
    manifest: Manifest,
    /// The final path of each entry.
    finals: Vec<PathBuf>,
    /// Root folder (folder copies), created even if there are no entries.
    root: Option<PathBuf>,
}

impl Sink {
    /// Validates the manifest (which comes from the other machine) and decides where each thing goes.
    /// Does not touch the disk.
    pub fn open(remote: &Path, manifest: Manifest) -> Result<Sink> {
        let bad = |m: &str| FilesError::Manifest(m.to_owned());
        if manifest.chunk_size == 0 || manifest.chunk_size > MAX_CHUNK_SIZE {
            return Err(bad("chunk size out of range"));
        }
        if manifest.single && manifest.entries.len() != 1 {
            return Err(bad("a single-file copy carries exactly one entry"));
        }
        let mut seen = HashSet::new();
        for e in &manifest.entries {
            if !seen.insert(e.path.as_str()) {
                return Err(bad("repeated paths"));
            }
            if let EntryKind::File { size, chunks, .. } = &e.kind
                && chunks.len() as u64 != chunk_count(*size, manifest.chunk_size)
            {
                return Err(bad("the hashes do not match the size"));
            }
        }
        let (finals, root) = if manifest.single {
            let e = &manifest.entries[0];
            if !matches!(e.kind, EntryKind::File { .. }) {
                return Err(bad("a single-file copy cannot be a folder"));
            }
            check_segment(&e.path, cfg!(windows)).map_err(|w| FilesError::path(&e.path, w))?;
            // If the destination is a folder that already exists, the file goes in under its name.
            let target = if remote.is_dir() {
                remote.join(&e.path)
            } else {
                remote.to_path_buf()
            };
            (vec![target], None)
        } else {
            let finals = manifest
                .entries
                .iter()
                .map(|e| from_wire(remote, &e.path))
                .collect::<Result<Vec<_>>>()?;
            (finals, Some(remote.to_path_buf()))
        };
        Ok(Sink {
            manifest,
            finals,
            root,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Expected length of chunk `index` of file `entry`.
    pub fn chunk_len(&self, entry: u32, index: u32) -> Result<usize> {
        let bad = || FilesError::Manifest(format!("chunk {index} of {entry} does not exist"));
        match self.manifest.entries.get(entry as usize).map(|e| &e.kind) {
            Some(EntryKind::File { size, .. }) => {
                chunk_len(*size, self.manifest.chunk_size, index).ok_or_else(bad)
            }
            _ => Err(bad()),
        }
    }

    /// Creates the folders, prepares the `.tandem-part` files and says which chunks are missing. What is already
    /// complete in place is not requested again.
    pub fn plan(&self) -> Result<Vec<Need>> {
        if let Some(root) = &self.root {
            fs::create_dir_all(root).map_err(|e| FilesError::io(root, e))?;
        }
        let mut needs = Vec::new();
        let cs = self.manifest.chunk_size;
        for (i, e) in self.manifest.entries.iter().enumerate() {
            let path = &self.finals[i];
            match &e.kind {
                EntryKind::Dir => fs::create_dir_all(path).map_err(|e| FilesError::io(path, e))?,
                EntryKind::File { size, chunks, hash } => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(|e| FilesError::io(parent, e))?;
                    }
                    if is_complete(path, *size, hash, cs) {
                        continue;
                    }
                    let part = part_path(path);
                    let mut f = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&part)
                        .map_err(|e| FilesError::io(&part, e))?;
                    let io = |e| FilesError::io(&part, e);
                    if f.metadata().map_err(io)?.len() != *size {
                        f.set_len(*size).map_err(io)?;
                    }
                    let mut missing = Vec::new();
                    for (idx, want) in chunks.iter().enumerate() {
                        let idx = idx as u32;
                        let len = chunk_len(*size, cs, idx).unwrap_or(0);
                        let mut buf = vec![0; len];
                        f.seek(SeekFrom::Start(u64::from(idx) * u64::from(cs)))
                            .map_err(io)?;
                        let n = read_full(&mut f, &mut buf).map_err(io)?;
                        if n != len || blake3::hash(&buf).as_bytes() != want {
                            missing.push(idx);
                        }
                    }
                    if !missing.is_empty() {
                        needs.push(Need {
                            entry: i as u32,
                            chunks: missing,
                        });
                    }
                }
            }
        }
        Ok(needs)
    }

    /// Verifies a chunk against the manifest and writes it in place.
    pub fn write_chunk(&self, entry: u32, index: u32, bytes: &[u8]) -> Result<()> {
        let len = self.chunk_len(entry, index)?;
        let path = &self.finals[entry as usize];
        let EntryKind::File { chunks, .. } = &self.manifest.entries[entry as usize].kind else {
            return Err(FilesError::Manifest("not a file".into()));
        };
        if bytes.len() != len || blake3::hash(bytes).as_bytes() != &chunks[index as usize] {
            return Err(FilesError::BadChunk {
                path: self.manifest.entries[entry as usize].path.clone(),
                index,
            });
        }
        let part = part_path(path);
        let io = |e| FilesError::io(&part, e);
        let mut f = OpenOptions::new().write(true).open(&part).map_err(io)?;
        f.seek(SeekFrom::Start(
            u64::from(index) * u64::from(self.manifest.chunk_size),
        ))
        .map_err(io)?;
        f.write_all(bytes).map_err(io)
    }

    /// Verifies each file's hash, renames it to its final name and sets permissions and
    /// dates (folders last, so their dates do not change as they fill up).
    pub fn finish(&self) -> Result<FilesReport> {
        let cs = self.manifest.chunk_size;
        let mut report = FilesReport {
            bytes: self.manifest.total_bytes(),
            ..FilesReport::default()
        };
        for (i, e) in self.manifest.entries.iter().enumerate() {
            let EntryKind::File { size, hash, .. } = &e.kind else {
                continue;
            };
            let path = &self.finals[i];
            let part = part_path(path);
            if part.exists() {
                let (_, _, got) = hash_file(&part, cs).map_err(|e| FilesError::io(&part, e))?;
                if got != *hash {
                    let _ = fs::remove_file(&part);
                    // An old `.part` next to a file that was already fine does not count.
                    if !is_complete(path, *size, hash, cs) {
                        return Err(FilesError::BadFile(e.path.clone()));
                    }
                } else {
                    replace(&part, path)?;
                }
            } else if !path.is_file() {
                return Err(FilesError::BadFile(e.path.clone()));
            }
            apply_meta(path, e.mode, e.mtime_ms);
            report.files += 1;
        }
        for (i, e) in self.manifest.entries.iter().enumerate().rev() {
            if matches!(e.kind, EntryKind::Dir) {
                apply_meta(&self.finals[i], e.mode, e.mtime_ms);
                report.dirs += 1;
            }
        }
        Ok(report)
    }
}

/// `path` is already a file of `size` bytes with that hash.
fn is_complete(path: &Path, size: u64, hash: &[u8; 32], chunk_size: u32) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == size => {
            hash_file(path, chunk_size).is_ok_and(|(_, _, h)| h == *hash)
        }
        _ => false,
    }
}

/// Renames over the destination even if it exists (on Windows a read-only one is not replaced).
fn replace(part: &Path, path: &Path) -> Result<()> {
    if let Ok(m) = fs::metadata(path) {
        let mut perms = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(path, perms);
    }
    fs::rename(part, path).map_err(|e| FilesError::io(path, e))
}

/// Best effort: a system that does not support permissions or dates does not fail the copy.
fn apply_meta(path: &Path, mode: Option<u32>, mtime_ms: Option<u64>) {
    if let Some(ms) = mtime_ms {
        let when = UNIX_EPOCH + Duration::from_millis(ms);
        let r = File::options()
            .write(true)
            .open(path)
            .or_else(|_| File::open(path))
            .and_then(|f| f.set_modified(when));
        if let Err(e) = r {
            tracing::debug!("could not set the date of {}: {e}", path.display());
        }
    }
    if let Some(mode) = mode
        && let Err(e) = set_mode(path, mode)
    {
        tracing::debug!("could not set the permission of {}: {e}", path.display());
    }
}

/// Without setuid, setgid or sticky: what arrives from the other machine does not escalate privileges.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
}

#[cfg(not(unix))]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, perms)
}

#[cfg(test)]
mod tests {
    use tandem_proto::FileEntry;

    use super::*;

    fn file_manifest(content: &[u8], cs: u32) -> Manifest {
        let chunks = content
            .chunks(cs as usize)
            .map(|c| *blake3::hash(c).as_bytes())
            .collect();
        Manifest {
            chunk_size: cs,
            single: true,
            entries: vec![FileEntry {
                path: "f.bin".into(),
                kind: EntryKind::File {
                    size: content.len() as u64,
                    chunks,
                    hash: *blake3::hash(content).as_bytes(),
                },
                mode: Some(0o640),
                mtime_ms: Some(1_700_000_000_000),
            }],
        }
    }

    #[test]
    fn a_bad_chunk_is_not_written() {
        let d = tempfile::tempdir().unwrap();
        let m = file_manifest(b"abcdefgh", 4);
        let s = Sink::open(&d.path().join("out"), m).unwrap();
        assert_eq!(s.plan().unwrap()[0].chunks, [0, 1]);
        assert!(matches!(
            s.write_chunk(0, 0, b"abcX"),
            Err(FilesError::BadChunk { .. })
        ));
        s.write_chunk(0, 1, b"efgh").unwrap();
        // Resume: only the first one is missing.
        let again = Sink::open(&d.path().join("out"), file_manifest(b"abcdefgh", 4)).unwrap();
        assert_eq!(again.plan().unwrap()[0].chunks, [0]);
        again.write_chunk(0, 0, b"abcd").unwrap();
        let r = again.finish().unwrap();
        assert_eq!((r.files, r.bytes), (1, 8));
        assert_eq!(fs::read(d.path().join("out")).unwrap(), b"abcdefgh");
        assert!(!part_path(&d.path().join("out")).exists());
        // Already complete: nothing is missing.
        let done = Sink::open(&d.path().join("out"), file_manifest(b"abcdefgh", 4)).unwrap();
        assert!(done.plan().unwrap().is_empty());
    }

    #[test]
    fn rejects_inconsistent_manifests() {
        let d = tempfile::tempdir().unwrap();
        let mut m = file_manifest(b"abcdefgh", 4);
        if let EntryKind::File { chunks, .. } = &mut m.entries[0].kind {
            chunks.pop();
        }
        assert!(Sink::open(d.path(), m).is_err());
        let mut m = file_manifest(b"ab", 4);
        m.chunk_size = 0;
        assert!(Sink::open(d.path(), m).is_err());
        let mut m = file_manifest(b"ab", 4);
        m.single = false;
        m.entries[0].path = "../outside".into();
        assert!(Sink::open(d.path(), m).is_err());
    }

    #[test]
    fn a_bad_total_hash_deletes_what_was_received() {
        let d = tempfile::tempdir().unwrap();
        let mut m = file_manifest(b"abcd", 4);
        if let EntryKind::File { hash, .. } = &mut m.entries[0].kind {
            *hash = [0; 32];
        }
        let out = d.path().join("o");
        let s = Sink::open(&out, m).unwrap();
        s.plan().unwrap();
        s.write_chunk(0, 0, b"abcd").unwrap();
        assert!(matches!(s.finish(), Err(FilesError::BadFile(_))));
        assert!(!part_path(&out).exists() && !out.exists());
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions_and_date() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("o");
        let s = Sink::open(&out, file_manifest(b"abcd", 4)).unwrap();
        s.plan().unwrap();
        s.write_chunk(0, 0, b"abcd").unwrap();
        s.finish().unwrap();
        let meta = fs::metadata(&out).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o640);
        let ms = meta.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(ms.as_millis(), 1_700_000_000_000);
    }

    #[test]
    fn an_old_part_next_to_a_complete_file_does_not_break_it() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("o");
        fs::write(&out, b"abcd").unwrap();
        fs::write(part_path(&out), b"old junk").unwrap();
        let s = Sink::open(&out, file_manifest(b"abcd", 4)).unwrap();
        assert!(s.plan().unwrap().is_empty());
        s.finish().unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"abcd");
        assert!(!part_path(&out).exists());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_apply_setuid_setgid_or_sticky() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("o");
        let mut m = file_manifest(b"abcd", 4);
        m.entries[0].mode = Some(0o7755);
        let s = Sink::open(&out, m).unwrap();
        s.plan().unwrap();
        s.write_chunk(0, 0, b"abcd").unwrap();
        s.finish().unwrap();
        assert_eq!(
            fs::metadata(&out).unwrap().permissions().mode() & 0o7777,
            0o755
        );
    }
}
