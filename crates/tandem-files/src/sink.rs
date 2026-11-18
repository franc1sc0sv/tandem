//! El lado que escribe: valida el manifiesto, dice qué falta, guarda fragmentos
//! verificados en un archivo `.tandem-part` y al terminar lo renombra con sus permisos y
//! fechas.
//!
//! Lo recibido vive en disco, no en memoria: si el enlace se corta (o el agente se
//! reinicia), la siguiente vuelta recorre los `.tandem-part`, compara hash por fragmento y
//! pide solo lo que no coincide.
//!
//! Todo es bloqueante (disco): los llamadores lo corren en `spawn_blocking`.

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
    /// La ruta final de cada entrada.
    finals: Vec<PathBuf>,
    /// Carpeta raíz (copias de carpeta), que se crea aunque no haya entradas.
    root: Option<PathBuf>,
}

impl Sink {
    /// Valida el manifiesto (que viene de la otra máquina) y decide dónde va cada cosa.
    /// No toca el disco.
    pub fn open(remote: &Path, manifest: Manifest) -> Result<Sink> {
        let bad = |m: &str| FilesError::Manifest(m.to_owned());
        if manifest.chunk_size == 0 || manifest.chunk_size > MAX_CHUNK_SIZE {
            return Err(bad("tamaño de fragmento fuera de rango"));
        }
        if manifest.single && manifest.entries.len() != 1 {
            return Err(bad("una copia de un archivo trae exactamente una entrada"));
        }
        let mut seen = HashSet::new();
        for e in &manifest.entries {
            if !seen.insert(e.path.as_str()) {
                return Err(bad("rutas repetidas"));
            }
            if let EntryKind::File { size, chunks, .. } = &e.kind
                && chunks.len() as u64 != chunk_count(*size, manifest.chunk_size)
            {
                return Err(bad("los hashes no corresponden al tamaño"));
            }
        }
        let (finals, root) = if manifest.single {
            let e = &manifest.entries[0];
            if !matches!(e.kind, EntryKind::File { .. }) {
                return Err(bad("una copia de un archivo no puede ser una carpeta"));
            }
            check_segment(&e.path, cfg!(windows)).map_err(|w| FilesError::path(&e.path, w))?;
            // Si el destino es una carpeta que ya existe, el archivo entra con su nombre.
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

    /// Largo esperado del fragmento `index` del archivo `entry`.
    pub fn chunk_len(&self, entry: u32, index: u32) -> Result<usize> {
        let bad = || FilesError::Manifest(format!("no existe el fragmento {index} de {entry}"));
        match self.manifest.entries.get(entry as usize).map(|e| &e.kind) {
            Some(EntryKind::File { size, .. }) => {
                chunk_len(*size, self.manifest.chunk_size, index).ok_or_else(bad)
            }
            _ => Err(bad()),
        }
    }

    /// Crea las carpetas, prepara los `.tandem-part` y dice qué fragmentos faltan. Lo que ya
    /// está completo en su lugar no se vuelve a pedir.
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

    /// Verifica un fragmento contra el manifiesto y lo escribe en su lugar.
    pub fn write_chunk(&self, entry: u32, index: u32, bytes: &[u8]) -> Result<()> {
        let len = self.chunk_len(entry, index)?;
        let path = &self.finals[entry as usize];
        let EntryKind::File { chunks, .. } = &self.manifest.entries[entry as usize].kind else {
            return Err(FilesError::Manifest("no es un archivo".into()));
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

    /// Verifica el hash de cada archivo, lo renombra a su nombre final y fija permisos y
    /// fechas (las carpetas al final, para que sus fechas no cambien al llenarlas).
    pub fn finish(&self) -> Result<FilesReport> {
        let cs = self.manifest.chunk_size;
        let mut report = FilesReport {
            bytes: self.manifest.total_bytes(),
            ..FilesReport::default()
        };
        for (i, e) in self.manifest.entries.iter().enumerate() {
            let EntryKind::File { hash, .. } = &e.kind else {
                continue;
            };
            let path = &self.finals[i];
            let part = part_path(path);
            if part.exists() {
                let (_, _, got) = hash_file(&part, cs).map_err(|e| FilesError::io(&part, e))?;
                if got != *hash {
                    // Se empieza de cero la próxima vez.
                    let _ = fs::remove_file(&part);
                    return Err(FilesError::BadFile(e.path.clone()));
                }
                replace(&part, path)?;
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

/// `path` ya es un archivo de `size` bytes con ese hash.
fn is_complete(path: &Path, size: u64, hash: &[u8; 32], chunk_size: u32) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == size => {
            hash_file(path, chunk_size).is_ok_and(|(_, _, h)| h == *hash)
        }
        _ => false,
    }
}

/// Renombra sobre el destino aunque exista (en Windows no se reemplaza uno de solo lectura).
fn replace(part: &Path, path: &Path) -> Result<()> {
    if let Ok(m) = fs::metadata(path) {
        let mut perms = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(path, perms);
    }
    fs::rename(part, path).map_err(|e| FilesError::io(path, e))
}

/// Mejor esfuerzo: un sistema que no admite permisos o fechas no hace fallar la copia.
fn apply_meta(path: &Path, mode: Option<u32>, mtime_ms: Option<u64>) {
    if let Some(ms) = mtime_ms {
        let when = UNIX_EPOCH + Duration::from_millis(ms);
        let r = File::options()
            .write(true)
            .open(path)
            .or_else(|_| File::open(path))
            .and_then(|f| f.set_modified(when));
        if let Err(e) = r {
            tracing::debug!("no se pudo fijar la fecha de {}: {e}", path.display());
        }
    }
    if let Some(mode) = mode
        && let Err(e) = set_mode(path, mode)
    {
        tracing::debug!("no se pudo fijar el permiso de {}: {e}", path.display());
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
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
    fn un_fragmento_malo_no_se_escribe() {
        let d = tempfile::tempdir().unwrap();
        let m = file_manifest(b"abcdefgh", 4);
        let s = Sink::open(&d.path().join("out"), m).unwrap();
        assert_eq!(s.plan().unwrap()[0].chunks, [0, 1]);
        assert!(matches!(
            s.write_chunk(0, 0, b"abcX"),
            Err(FilesError::BadChunk { .. })
        ));
        s.write_chunk(0, 1, b"efgh").unwrap();
        // Retomar: solo falta el primero.
        let again = Sink::open(&d.path().join("out"), file_manifest(b"abcdefgh", 4)).unwrap();
        assert_eq!(again.plan().unwrap()[0].chunks, [0]);
        again.write_chunk(0, 0, b"abcd").unwrap();
        let r = again.finish().unwrap();
        assert_eq!((r.files, r.bytes), (1, 8));
        assert_eq!(fs::read(d.path().join("out")).unwrap(), b"abcdefgh");
        assert!(!part_path(&d.path().join("out")).exists());
        // Ya completo: no falta nada.
        let done = Sink::open(&d.path().join("out"), file_manifest(b"abcdefgh", 4)).unwrap();
        assert!(done.plan().unwrap().is_empty());
    }

    #[test]
    fn rechaza_manifiestos_inconsistentes() {
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
        m.entries[0].path = "../fuera".into();
        assert!(Sink::open(d.path(), m).is_err());
    }

    #[test]
    fn el_hash_total_malo_borra_lo_recibido() {
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
    fn conserva_permisos_y_fecha() {
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
}
