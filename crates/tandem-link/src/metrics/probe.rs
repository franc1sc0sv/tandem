//! La prueba de ancho de banda, sobre cualquier par de flujos de bytes: el stream de
//! datos de QUIC ([`ControlStream::measure`](crate::transport::ControlStream::measure)) o,
//! en las pruebas, el Wi-Fi simulado.
//!
//! Mide quien recibe: lo que el que manda escribe se queda un rato en sus colas (las de
//! QUIC, las del sistema), así que contar del lado que escribe da de más.

use std::io;
use std::time::Duration;

use tandem_proto::MeasureReport;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{self, Instant};

const CHUNK: usize = 64 * 1024;

/// Escribe sin parar durante `duration` y cierra la escritura. Devuelve lo escrito.
pub async fn send_probe<W: AsyncWrite + Unpin>(w: &mut W, duration: Duration) -> io::Result<u64> {
    let chunk = vec![0u8; CHUNK];
    let end = Instant::now() + duration;
    let mut sent = 0u64;
    // Una escritura frenada por el control de flujo no alarga la prueba: se corta al
    // final (si se cancela, no escribió nada).
    while let Ok(n) = time::timeout_at(end, w.write(&chunk)).await {
        let n = n?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        sent += n as u64;
    }
    w.shutdown().await?;
    Ok(sent)
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
        let sent = send_probe(&mut a, Duration::from_secs(3)).await.unwrap();
        assert_eq!(sent, 1024);
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
