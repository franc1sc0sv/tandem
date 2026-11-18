//! Transferencias de archivos por un stream de túnel.
//!
//! **Subir (`put`)**: el origen manda el manifiesto; la destino contesta qué fragmentos le
//! faltan (los que no coinciden con lo que ya tiene en disco, ver [`Sink::plan`]); el
//! origen manda solo esos, en pedazos de [`PIECE`], y al final pide `Finish`. No hay acuse
//! por fragmento: QUIC es ordenado y fiable, y si el stream se corta se vuelve a empezar
//! por el plan, que ya no pide lo que llegó.
//!
//! **Bajar (`get`)**: la destino manda el manifiesto de lo que hay en `remote`; el origen
//! calcula el plan contra su disco y pide los fragmentos por tandas con `Fetch`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tandem_proto::{
    CHUNK_SIZE, FilesDown, FilesReport, FilesUp, Manifest, Need, PIECE, TransferProgress,
    TunnelDown, TunnelId, TunnelUp,
};
use tandem_tunnel::{Host, Origin, OriginWire, TargetWire, WireError};

use crate::error::{FilesError, Result};
use crate::sink::Sink;
use crate::source::Source;

/// Fragmentos que se piden de una vez en `get`.
const FETCH_BATCH: usize = 8;

#[derive(Debug, Clone)]
pub struct Options {
    pub chunk_size: u32,
    /// Cuántas veces se intenta (reanudando) si el enlace se corta.
    pub attempts: u32,
    pub retry_delay: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            chunk_size: CHUNK_SIZE,
            attempts: 5,
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// Quien recibe el avance (la UI, el cliente).
pub type Progress<'a> = &'a (dyn Fn(TransferProgress) + Send + Sync);

/// Corre trabajo de disco fuera de los hilos del ejecutor.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| FilesError::Join)?
}

/// Junta los pedazos de un fragmento.
#[derive(Default)]
struct Assembler {
    key: (u32, u32),
    buf: Vec<u8>,
}

impl Assembler {
    /// `Some` cuando el fragmento `(entry, index)` de `expected` bytes ya está completo.
    fn push(
        &mut self,
        key: (u32, u32),
        offset: u32,
        bytes: &[u8],
        expected: usize,
    ) -> Result<Option<Vec<u8>>> {
        if offset == 0 {
            self.key = key;
            self.buf.clear();
        }
        if self.key != key || offset as usize != self.buf.len() {
            return Err(FilesError::Unexpected);
        }
        self.buf.extend_from_slice(bytes);
        match self.buf.len().cmp(&expected) {
            std::cmp::Ordering::Less => Ok(None),
            std::cmp::Ordering::Equal => Ok(Some(std::mem::take(&mut self.buf))),
            std::cmp::Ordering::Greater => Err(FilesError::Unexpected),
        }
    }
}

async fn send_files(wire: &mut Box<dyn OriginWire>, msg: FilesUp) -> Result<()> {
    wire.send(TunnelUp::Files(msg)).await?;
    Ok(())
}

async fn recv_files(wire: &mut Box<dyn OriginWire>) -> Result<FilesDown> {
    match wire.recv().await? {
        None => Err(WireError::new("la otra máquina cerró el stream").into()),
        Some(TunnelDown::Refused(why)) | Some(TunnelDown::Files(FilesDown::Failed(why))) => {
            Err(FilesError::Remote(why))
        }
        Some(TunnelDown::Files(m)) => Ok(m),
        Some(_) => Err(FilesError::Unexpected),
    }
}

fn need_bytes(manifest: &Manifest, needs: &[Need]) -> u64 {
    needs
        .iter()
        .map(|n| {
            let size = match manifest.entries.get(n.entry as usize).map(|e| &e.kind) {
                Some(tandem_proto::EntryKind::File { size, .. }) => *size,
                _ => 0,
            };
            n.chunks
                .iter()
                .filter_map(|i| crate::source::chunk_len(size, manifest.chunk_size, *i))
                .map(|l| l as u64)
                .sum::<u64>()
        })
        .sum()
}

/// Copia `local` a `remote` en la destino del túnel, reanudando tras cada corte.
pub async fn put(
    origin: &Origin,
    tunnel: TunnelId,
    local: &Path,
    remote: &str,
    opts: &Options,
    progress: Progress<'_>,
) -> Result<FilesReport> {
    if !origin.is_open(tunnel) {
        return Err(FilesError::NoTunnel(tunnel));
    }
    let (path, cs) = (local.to_path_buf(), opts.chunk_size);
    let src = Arc::new(blocking(move || Source::scan(&path, cs)).await?);
    let mut last = FilesError::Unexpected;
    for attempt in 0..opts.attempts.max(1) {
        if attempt > 0 {
            tokio::time::sleep(opts.retry_delay).await;
        }
        match put_once(origin, tunnel, &src, local, remote, progress).await {
            Ok(r) => return Ok(r),
            Err(e) if e.retryable() => {
                tracing::debug!(%tunnel, "subida cortada, se reanuda: {e}");
                last = e;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

async fn put_once(
    origin: &Origin,
    tunnel: TunnelId,
    src: &Arc<Source>,
    local: &Path,
    remote: &str,
    progress: Progress<'_>,
) -> Result<FilesReport> {
    let manifest = src.manifest();
    let mut wire = origin.dial().await?;
    send_files(
        &mut wire,
        FilesUp::Put {
            tunnel,
            remote: remote.to_owned(),
            manifest: manifest.clone(),
        },
    )
    .await?;
    let FilesDown::Plan(needs) = recv_files(&mut wire).await? else {
        return Err(FilesError::Unexpected);
    };
    wire.start_data()?;
    let total = manifest.total_bytes();
    let mut done = total.saturating_sub(need_bytes(manifest, &needs));
    let mut sent = 0u64;
    let report = |done: u64, finished: bool| {
        progress(TransferProgress {
            tunnel,
            upload: true,
            path: local.display().to_string(),
            done,
            total,
            finished,
        });
    };
    report(done, false);
    for need in &needs {
        for &index in &need.chunks {
            let (s, entry) = (src.clone(), need.entry);
            let data = blocking(move || s.read_chunk(entry, index)).await?;
            let mut offset = 0usize;
            for piece in data.chunks(PIECE) {
                send_files(
                    &mut wire,
                    FilesUp::Chunk {
                        entry,
                        index,
                        offset: offset as u32,
                        bytes: piece.to_vec(),
                    },
                )
                .await?;
                offset += piece.len();
            }
            sent += data.len() as u64;
            done += data.len() as u64;
            report(done, false);
        }
    }
    send_files(&mut wire, FilesUp::Finish).await?;
    let FilesDown::Done(mut r) = recv_files(&mut wire).await? else {
        return Err(FilesError::Unexpected);
    };
    r.sent = sent;
    report(done, true);
    Ok(r)
}

/// Trae `remote` de la destino a `local`, reanudando tras cada corte.
pub async fn get(
    origin: &Origin,
    tunnel: TunnelId,
    remote: &str,
    local: &Path,
    opts: &Options,
    progress: Progress<'_>,
) -> Result<FilesReport> {
    if !origin.is_open(tunnel) {
        return Err(FilesError::NoTunnel(tunnel));
    }
    let mut last = FilesError::Unexpected;
    for attempt in 0..opts.attempts.max(1) {
        if attempt > 0 {
            tokio::time::sleep(opts.retry_delay).await;
        }
        match get_once(origin, tunnel, remote, local, progress).await {
            Ok(r) => return Ok(r),
            Err(e) if e.retryable() => {
                tracing::debug!(%tunnel, "bajada cortada, se reanuda: {e}");
                last = e;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

async fn get_once(
    origin: &Origin,
    tunnel: TunnelId,
    remote: &str,
    local: &Path,
    progress: Progress<'_>,
) -> Result<FilesReport> {
    let mut wire = origin.dial().await?;
    send_files(
        &mut wire,
        FilesUp::Get {
            tunnel,
            remote: remote.to_owned(),
        },
    )
    .await?;
    let FilesDown::Manifest(manifest) = recv_files(&mut wire).await? else {
        return Err(FilesError::Unexpected);
    };
    let total = manifest.total_bytes();
    let path = local.to_path_buf();
    let sink = Arc::new(blocking(move || Sink::open(&path, manifest)).await?);
    let s = sink.clone();
    let needs = blocking(move || s.plan()).await?;
    let mut done = total.saturating_sub(need_bytes(sink.manifest(), &needs));
    let mut sent = 0u64;
    let report = |done: u64, finished: bool| {
        progress(TransferProgress {
            tunnel,
            upload: false,
            path: local.display().to_string(),
            done,
            total,
            finished,
        });
    };
    report(done, false);
    let mut asm = Assembler::default();
    for need in &needs {
        for batch in need.chunks.chunks(FETCH_BATCH) {
            send_files(
                &mut wire,
                FilesUp::Fetch {
                    entry: need.entry,
                    indices: batch.to_vec(),
                },
            )
            .await?;
            for &index in batch {
                let expected = sink.chunk_len(need.entry, index)?;
                let data = loop {
                    let FilesDown::Chunk {
                        entry,
                        index: i,
                        offset,
                        bytes,
                    } = recv_files(&mut wire).await?
                    else {
                        return Err(FilesError::Unexpected);
                    };
                    if (entry, i) != (need.entry, index) {
                        return Err(FilesError::Unexpected);
                    }
                    if let Some(d) = asm.push((entry, i), offset, &bytes, expected)? {
                        break d;
                    }
                };
                let (s, entry) = (sink.clone(), need.entry);
                let len = data.len() as u64;
                blocking(move || s.write_chunk(entry, index, &data)).await?;
                sent += len;
                done += len;
                report(done, false);
            }
        }
    }
    let s = sink.clone();
    let mut r = blocking(move || s.finish()).await?;
    r.sent = sent;
    report(done, true);
    Ok(r)
}

/// El lado de la destino de un pedido de archivos. `first` es el primer mensaje del
/// stream.
pub async fn serve<W: TargetWire>(
    host: &Host,
    wire: &mut W,
    first: FilesUp,
    chunk_size: u32,
) -> std::result::Result<(), WireError> {
    match first {
        FilesUp::Put {
            tunnel,
            remote,
            manifest,
        } => serve_put(host, wire, tunnel, remote, manifest).await,
        FilesUp::Get { tunnel, remote } => serve_get(host, wire, tunnel, remote, chunk_size).await,
        FilesUp::Chunk { .. } | FilesUp::Finish | FilesUp::Fetch { .. } => {
            fail(wire, "el stream tiene que empezar con Put o Get").await
        }
    }
}

async fn fail<W: TargetWire>(
    wire: &mut W,
    why: impl ToString,
) -> std::result::Result<(), WireError> {
    wire.send(&TunnelDown::Files(FilesDown::Failed(why.to_string())))
        .await
}

fn open_tunnel(host: &Host, tunnel: TunnelId) -> bool {
    host.get(tunnel).is_some_and(|t| t.is_open())
}

async fn serve_put<W: TargetWire>(
    host: &Host,
    wire: &mut W,
    tunnel: TunnelId,
    remote: String,
    manifest: Manifest,
) -> std::result::Result<(), WireError> {
    if !open_tunnel(host, tunnel) {
        return wire
            .send(&TunnelDown::Refused(format!(
                "no hay ningún túnel {tunnel} abierto en esta máquina"
            )))
            .await;
    }
    let prepared = blocking(move || {
        let sink = Arc::new(Sink::open(Path::new(&remote), manifest)?);
        let needs = sink.plan()?;
        Ok((sink, needs))
    })
    .await;
    let (sink, needs) = match prepared {
        Ok(p) => p,
        Err(e) => return fail(wire, e).await,
    };
    wire.send(&TunnelDown::Files(FilesDown::Plan(needs)))
        .await?;
    let mut asm = Assembler::default();
    loop {
        match wire.recv().await? {
            // El origen se fue: lo recibido queda en disco para retomar.
            None => return Ok(()),
            Some(TunnelUp::Files(FilesUp::Chunk {
                entry,
                index,
                offset,
                bytes,
            })) => {
                let step = async {
                    let expected = sink.chunk_len(entry, index)?;
                    let Some(data) = asm.push((entry, index), offset, &bytes, expected)? else {
                        return Ok(());
                    };
                    let s = sink.clone();
                    blocking(move || s.write_chunk(entry, index, &data)).await
                };
                if let Err(e) = step.await {
                    return fail(wire, e).await;
                }
            }
            Some(TunnelUp::Files(FilesUp::Finish)) => {
                let s = sink.clone();
                return match blocking(move || s.finish()).await {
                    Ok(r) => wire.send(&TunnelDown::Files(FilesDown::Done(r))).await,
                    Err(e) => fail(wire, e).await,
                };
            }
            Some(_) => return fail(wire, "mensaje inesperado durante la subida").await,
        }
    }
}

async fn serve_get<W: TargetWire>(
    host: &Host,
    wire: &mut W,
    tunnel: TunnelId,
    remote: String,
    chunk_size: u32,
) -> std::result::Result<(), WireError> {
    if !open_tunnel(host, tunnel) {
        return wire
            .send(&TunnelDown::Refused(format!(
                "no hay ningún túnel {tunnel} abierto en esta máquina"
            )))
            .await;
    }
    let scanned = blocking(move || Source::scan(Path::new(&remote), chunk_size)).await;
    let src = match scanned {
        Ok(s) => Arc::new(s),
        Err(e) => return fail(wire, e).await,
    };
    wire.send(&TunnelDown::Files(FilesDown::Manifest(
        src.manifest().clone(),
    )))
    .await?;
    wire.start_data()?;
    loop {
        match wire.recv().await? {
            None => return Ok(()),
            Some(TunnelUp::Files(FilesUp::Fetch { entry, indices })) => {
                for index in indices {
                    let s = src.clone();
                    let data = match blocking(move || s.read_chunk(entry, index)).await {
                        Ok(d) => d,
                        Err(e) => return fail(wire, e).await,
                    };
                    let mut offset = 0usize;
                    for piece in data.chunks(PIECE) {
                        wire.send(&TunnelDown::Files(FilesDown::Chunk {
                            entry,
                            index,
                            offset: offset as u32,
                            bytes: piece.to_vec(),
                        }))
                        .await?;
                        offset += piece.len();
                    }
                }
            }
            Some(_) => return fail(wire, "mensaje inesperado durante la bajada").await,
        }
    }
}
