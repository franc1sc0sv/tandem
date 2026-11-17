//! La prueba de ancho de banda, sobre cualquier par de flujos de bytes: el stream de
//! datos de QUIC ([`ControlStream::measure`](crate::transport::ControlStream::measure)) o,
//! en las pruebas, el Wi-Fi simulado.
//!
//! Mide quien recibe: lo que el que manda escribe se queda un rato en sus colas (las de
//! QUIC, las del sistema), así que contar del lado que escribe da de más.
//!
//! Va sin el tope de los datos (medir a través del tope daría el tope), pero respeta la
//! pausa de T-101: si la entrada pasa su objetivo, la prueba se corta (DECISIONS).

use std::io;
use std::time::Duration;

use tandem_proto::MeasureReport;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{self, Instant};

use crate::qos::Qos;

const CHUNK: usize = 64 * 1024;

/// Cómo terminó la parte que manda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeEnd {
    /// Bytes escritos.
    pub sent: u64,
    /// Se cortó antes de tiempo porque la entrada pasó su objetivo (`qos` en pausa).
    pub paused: bool,
}

/// Escribe sin parar durante `duration` y cierra la escritura; antes, si `qos` (el del
/// camino que se mide) pausa los datos.
pub async fn send_probe<W: AsyncWrite + Unpin>(
    w: &mut W,
    duration: Duration,
    qos: &Qos,
) -> io::Result<ProbeEnd> {
    let chunk = vec![0u8; CHUNK];
    let end = Instant::now() + duration;
    let mut gate = qos.watch_gate();
    let mut sent = 0u64;
    let mut paused = !gate.borrow_and_update().is_open();
    // Una escritura frenada por el control de flujo no alarga la prueba ni demora el
    // corte: se cancela (y entonces no escribió nada).
    while !paused {
        tokio::select! {
            written = time::timeout_at(end, w.write(&chunk)) => match written {
                Ok(n) => {
                    let n = n?;
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    sent += n as u64;
                }
                Err(_) => break,
            },
            changed = gate.changed() => {
                if changed.is_err() {
                    break;
                }
                paused = !gate.borrow_and_update().is_open();
            }
        }
    }
    if paused {
        tracing::info!(
            sent,
            "prueba de ancho de banda cortada: la entrada pasó su objetivo"
        );
    }
    w.shutdown().await?;
    Ok(ProbeEnd { sent, paused })
}

/// Lee hasta el fin y cuenta lo que llegó después de la primera lectura, en el tiempo que
/// va de esa primera lectura a la última: así no cuenta lo que tardó en arrancar.
/// `already` son bytes de la prueba que ya se leyeron (junto con la apertura del canal):
/// cuentan como la primera lectura.
pub async fn receive_probe<R: AsyncRead + Unpin>(
    r: &mut R,
    already: usize,
) -> io::Result<MeasureReport> {
    let mut buf = vec![0u8; CHUNK];
    let mut first = (already > 0).then(Instant::now);
    let mut last = first;
    let mut bytes = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let now = Instant::now();
        if first.is_none() {
            first = Some(now);
        } else {
            bytes += n as u64;
        }
        last = Some(now);
    }
    let micros = match (first, last) {
        (Some(f), Some(l)) => u64::try_from(l.duration_since(f).as_micros()).unwrap_or(u64::MAX),
        _ => 0,
    };
    Ok(MeasureReport { bytes, micros })
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sin_datos_no_hay_medición() {
        let (mut a, mut b) = duplex(1024);
        a.shutdown().await.unwrap();
        let r = receive_probe(&mut b, 0).await.unwrap();
        assert_eq!(r.bytes_per_sec(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn quien_manda_para_a_tiempo_aunque_nadie_lea() {
        let (mut a, _b) = duplex(1024);
        let t0 = Instant::now();
        let end = send_probe(&mut a, Duration::from_secs(3), &Qos::default())
            .await
            .unwrap();
        assert_eq!(
            end,
            ProbeEnd {
                sent: 1024,
                paused: false
            }
        );
        assert_eq!(Instant::now() - t0, Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn lo_ya_leído_con_la_apertura_marca_el_comienzo() {
        let (mut a, mut b) = duplex(1024);
        let recv = tokio::spawn(async move { receive_probe(&mut b, 10).await.unwrap() });
        time::sleep(Duration::from_secs(1)).await;
        a.write_all(&[0; 500]).await.unwrap();
        a.shutdown().await.unwrap();
        let r = recv.await.unwrap();
        assert_eq!((r.bytes, r.micros), (500, 1_000_000));
        assert_eq!(r.bytes_per_sec(), Some(500));
    }
}
