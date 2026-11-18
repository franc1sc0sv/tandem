//! El lado que lee: recorre un árbol, calcula el manifiesto y entrega fragmentos.
//!
//! Todo es bloqueante (disco): los llamadores lo corren en `spawn_blocking`.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tandem_proto::{EntryKind, FileEntry, Hash, Manifest};

use crate::error::{FilesError, Result};
use crate::path::to_wire;

/// Cuántos fragmentos de `chunk_size` tiene un archivo de `size` bytes.
pub fn chunk_count(size: u64, chunk_size: u32) -> u64 {
    size.div_ceil(u64::from(chunk_size))
}

/// Largo del fragmento `index` de un archivo de `size` bytes; `None` si no existe.
pub fn chunk_len(size: u64, chunk_size: u32, index: u32) -> Option<usize> {
    let start = u64::from(index) * u64::from(chunk_size);
    if start >= size {
        return None;
    }
    usize::try_from((size - start).min(u64::from(chunk_size))).ok()
}

/// Lee hasta llenar `buf` o llegar al final; devuelve cuánto leyó.
pub(crate) fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Un árbol listo para mandar.
#[derive(Debug)]
pub struct Source {
    manifest: Manifest,
    /// La ruta local de cada entrada del manifiesto.
    paths: Vec<PathBuf>,
}

impl Source {
    /// Recorre `local` (un archivo o una carpeta). Los enlaces simbólicos y lo que no es
    /// archivo ni carpeta se omiten.
    pub fn scan(local: &Path, chunk_size: u32) -> Result<Source> {
        let meta = fs::metadata(local).map_err(|e| FilesError::io(local, e))?;
        let mut src = Source {
            manifest: Manifest {
                chunk_size,
                single: meta.is_file(),
                entries: Vec::new(),
            },
            paths: Vec::new(),
        };
        if meta.is_file() {
            let name = local
                .file_name()
                .map(Path::new)
                .ok_or_else(|| FilesError::path(&local.display().to_string(), "sin nombre"))?;
            src.add_file(local, to_wire(name)?, &meta)?;
        } else if meta.is_dir() {
            src.walk(local, Path::new(""))?;
        } else {
            return Err(FilesError::path(
                &local.display().to_string(),
                "no es archivo ni carpeta",
            ));
        }
        Ok(src)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn walk(&mut self, abs: &Path, rel: &Path) -> Result<()> {
        let mut names: Vec<_> = fs::read_dir(abs)
            .map_err(|e| FilesError::io(abs, e))?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<io::Result<_>>()
            .map_err(|e| FilesError::io(abs, e))?;
        names.sort();
        for name in names {
            let path = abs.join(&name);
            let rel = rel.join(&name);
            let meta = fs::symlink_metadata(&path).map_err(|e| FilesError::io(&path, e))?;
            if meta.is_dir() {
                self.manifest.entries.push(FileEntry {
                    path: to_wire(&rel)?,
                    kind: EntryKind::Dir,
                    mode: mode_of(&meta),
                    mtime_ms: mtime_of(&meta),
                });
                self.paths.push(path.clone());
                self.walk(&path, &rel)?;
            } else if meta.is_file() {
                self.add_file(&path, to_wire(&rel)?, &meta)?;
            } else {
                tracing::warn!("se omite {} (enlace o archivo especial)", path.display());
            }
        }
        Ok(())
    }

    fn add_file(&mut self, path: &Path, wire: String, meta: &fs::Metadata) -> Result<()> {
        let cs = self.manifest.chunk_size;
        let (size, chunks, hash) = hash_file(path, cs).map_err(|e| FilesError::io(path, e))?;
        self.manifest.entries.push(FileEntry {
            path: wire,
            kind: EntryKind::File { size, chunks, hash },
            mode: mode_of(meta),
            mtime_ms: mtime_of(meta),
        });
        self.paths.push(path.to_path_buf());
        Ok(())
    }

    /// Lee el fragmento `index` del archivo `entry`.
    pub fn read_chunk(&self, entry: u32, index: u32) -> Result<Vec<u8>> {
        let bad = || FilesError::Manifest(format!("no existe el fragmento {index} de {entry}"));
        let e = self.manifest.entries.get(entry as usize).ok_or_else(bad)?;
        let EntryKind::File { size, .. } = &e.kind else {
            return Err(bad());
        };
        let len = chunk_len(*size, self.manifest.chunk_size, index).ok_or_else(bad)?;
        let path = &self.paths[entry as usize];
        let mut f = File::open(path).map_err(|e| FilesError::io(path, e))?;
        f.seek(SeekFrom::Start(
            u64::from(index) * u64::from(self.manifest.chunk_size),
        ))
        .map_err(|e| FilesError::io(path, e))?;
        let mut buf = vec![0; len];
        let n = read_full(&mut f, &mut buf).map_err(|e| FilesError::io(path, e))?;
        if n != len {
            return Err(FilesError::io(
                path,
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "el archivo cambió mientras se copiaba",
                ),
            ));
        }
        Ok(buf)
    }
}

/// Tamaño, hash de cada fragmento y hash del total, en una sola pasada.
pub(crate) fn hash_file(path: &Path, chunk_size: u32) -> io::Result<(u64, Vec<Hash>, Hash)> {
    let mut f = File::open(path)?;
    let mut total = blake3::Hasher::new();
    let mut chunks = Vec::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; chunk_size as usize];
    loop {
        let n = read_full(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        chunks.push(*blake3::hash(&buf[..n]).as_bytes());
        total.update(&buf[..n]);
        size += n as u64;
        if n < buf.len() {
            break;
        }
    }
    Ok((size, chunks, *total.finalize().as_bytes()))
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

/// Windows solo conoce «solo lectura»: se manda como los permisos de Unix equivalentes.
#[cfg(not(unix))]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    if meta.is_dir() {
        None
    } else if meta.permissions().readonly() {
        Some(0o444)
    } else {
        Some(0o644)
    }
}

fn mtime_of(meta: &fs::Metadata) -> Option<u64> {
    let d = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(d.as_millis()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_ultimo_fragmento_es_el_resto() {
        assert_eq!(chunk_count(0, 4), 0);
        assert_eq!(chunk_count(8, 4), 2);
        assert_eq!(chunk_count(9, 4), 3);
        assert_eq!(chunk_len(9, 4, 2), Some(1));
        assert_eq!(chunk_len(9, 4, 3), None);
        assert_eq!(chunk_len(0, 4, 0), None);
    }

    #[test]
    fn el_manifiesto_trae_hashes_y_deja_los_vacios() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("sub dir")).unwrap();
        fs::write(d.path().join("sub dir/ñ.bin"), [1u8, 2, 3, 4, 5]).unwrap();
        fs::write(d.path().join("vacio"), b"").unwrap();
        fs::create_dir(d.path().join("hueca")).unwrap();
        let s = Source::scan(d.path(), 4).unwrap();
        let m = s.manifest();
        let paths: Vec<_> = m.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["hueca", "sub dir", "sub dir/ñ.bin", "vacio"]);
        let EntryKind::File { size, chunks, hash } = &m.entries[2].kind else {
            panic!("no es archivo");
        };
        assert_eq!((*size, chunks.len()), (5, 2));
        assert_eq!(*hash, *blake3::hash(&[1, 2, 3, 4, 5]).as_bytes());
        assert_eq!(chunks[1], *blake3::hash(&[5]).as_bytes());
        assert_eq!(s.read_chunk(2, 1).unwrap(), [5]);
        assert!(s.read_chunk(2, 2).is_err());
        assert!(
            s.read_chunk(0, 0).is_err(),
            "una carpeta no tiene fragmentos"
        );
        let EntryKind::File { chunks, .. } = &m.entries[3].kind else {
            panic!("no es archivo");
        };
        assert!(chunks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn los_enlaces_simbolicos_se_omiten() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a"), b"x").unwrap();
        std::os::unix::fs::symlink("a", d.path().join("enlace")).unwrap();
        let s = Source::scan(d.path(), 4).unwrap();
        assert_eq!(s.manifest().entries.len(), 1);
    }
}
