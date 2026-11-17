//! Un Wi-Fi simulado con ancho de banda, para probar que los datos no le meten latencia a
//! la entrada (T-101, DESIGN §6).
//!
//! El Wi-Fi es un solo medio: todo lo que sale de una máquina pasa por la misma cola (la
//! del sistema y la del punto de acceso) y esa cola sale en orden de llegada a la
//! capacidad del momento. La prioridad de QUIC ordena lo que todavía no salió, pero no
//! puede adelantar una tecla a los datos que ya están en esa cola: si los datos la llenan,
//! cada tecla espera lo que tarde en vaciarse. El modelo es solo eso: capacidad, una
//! demora fija de propagación y una cola de tamaño fijo.
//!
//! - La entrada ([`SimWifi::send_input`]) nunca espera para entrar a la cola: son unos
//!   bytes y van con la prioridad más alta, así que entran en el primer hueco.
//! - Los datos ([`SimWifi::data_writer`]) esperan a que haya lugar en la cola, como un
//!   stream de QUIC frenado por la ventana de congestión.
//!
//! Corre con el reloj de tokio (pausado en las pruebas): los tiempos son exactos.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::AsyncWrite;
use tokio::time::{Instant, Sleep};

/// Lo más chico que se espera tener lugar para escribir datos: un paquete.
const PACKET: u64 = 1500;

#[derive(Debug)]
struct Air {
    /// Bytes por segundo.
    capacity: u64,
    /// Propagación, igual para todo.
    base: Duration,
    /// Lo que cabe en la cola.
    buffer: u64,
    /// Cuándo sale de la cola lo último que entró.
    busy_until: Instant,
    /// Datos que entraron a la cola: cuándo y cuántos bytes.
    data: Vec<(Instant, usize)>,
}

impl Air {
    fn transmit_time(&self, bytes: u64) -> Duration {
        let ns = u128::from(bytes) * 1_000_000_000 / u128::from(self.capacity.max(1));
        Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX))
    }

    /// Bytes que esperan en la cola en `now`.
    fn backlog(&self, now: Instant) -> u64 {
        let left = self.busy_until.saturating_duration_since(now).as_nanos();
        u64::try_from(left * u128::from(self.capacity) / 1_000_000_000).unwrap_or(u64::MAX)
    }

    /// Pone `bytes` al final de la cola; devuelve cuándo llegan al otro lado.
    fn enqueue(&mut self, now: Instant, bytes: u64) -> Instant {
        let start = self.busy_until.max(now);
        self.busy_until = start + self.transmit_time(bytes);
        self.busy_until + self.base
    }
}

/// El Wi-Fi entre las dos máquinas, en un sentido. Se clona barato.
#[derive(Debug, Clone)]
pub struct SimWifi {
    air: Arc<Mutex<Air>>,
}

impl SimWifi {
    /// `capacity` en bytes por segundo, `base` de propagación y una cola de `buffer` bytes.
    pub fn new(capacity: u64, base: Duration, buffer: u64) -> Self {
        SimWifi {
            air: Arc::new(Mutex::new(Air {
                capacity: capacity.max(1),
                base,
                buffer: buffer.max(PACKET),
                busy_until: Instant::now(),
                data: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Air> {
        self.air.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// El Wi-Fi empeora o mejora (interferencia, otro equipo descargando). Lo que ya está
    /// en la cola sale a la capacidad nueva.
    pub fn set_capacity(&self, capacity: u64) {
        let now = Instant::now();
        let mut air = self.lock();
        let backlog = air.backlog(now);
        air.capacity = capacity.max(1);
        air.busy_until = now + air.transmit_time(backlog);
        tracing::info!(capacity, "capacidad del Wi-Fi (simulado)");
    }

    pub fn capacity(&self) -> u64 {
        self.lock().capacity
    }

    /// Un mensaje de entrada de `bytes`: entra a la cola ya y devuelve cuándo llega.
    pub fn send_input(&self, bytes: usize) -> Instant {
        let now = Instant::now();
        self.lock().enqueue(now, bytes as u64)
    }

    /// Para escribir datos: espera a que haya lugar en la cola.
    pub fn data_writer(&self) -> WifiWriter {
        WifiWriter {
            wifi: self.clone(),
            wait: None,
        }
    }

    /// Los datos que salieron (entraron a la cola): cuándo y cuántos bytes.
    pub fn data_sent(&self) -> Vec<(Instant, usize)> {
        self.lock().data.clone()
    }
}

/// Escritura de datos por el [`SimWifi`].
pub struct WifiWriter {
    wifi: SimWifi,
    wait: Option<Pin<Box<Sleep>>>,
}

impl AsyncWrite for WifiWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(wait) = self.wait.as_mut() {
                if wait.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                self.wait = None;
            }
            let now = Instant::now();
            let mut air = self.wifi.lock();
            let backlog = air.backlog(now);
            let room = air.buffer.saturating_sub(backlog);
            let want = buf.len() as u64;
            let need = want.min(PACKET);
            if room >= need {
                let n = want.min(room);
                air.enqueue(now, n);
                let n = usize::try_from(n).unwrap_or(buf.len());
                air.data.push((now, n));
                return Poll::Ready(Ok(n));
            }
            // Hasta que salga lo que falta para que entre un paquete.
            let until = now + air.transmit_time(need - room);
            drop(air);
            self.wait = Some(Box::pin(tokio::time::sleep_until(until)));
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[tokio::test(start_paused = true)]
    async fn sin_cola_la_entrada_tarda_lo_que_tarda_el_aire() {
        let wifi = SimWifi::new(1_000_000, ms(2), 100_000);
        let t0 = Instant::now();
        // 1000 bytes a 1 MB/s: 1 ms, más 2 ms de propagación.
        assert_eq!(wifi.send_input(1000), t0 + ms(3));
    }

    #[tokio::test(start_paused = true)]
    async fn los_datos_llenan_la_cola_y_la_entrada_espera_detrás() {
        let wifi = SimWifi::new(1_000_000, ms(2), 100_000);
        let mut w = wifi.data_writer();
        let t0 = Instant::now();
        w.write_all(&[0; 100_000]).await.unwrap();
        // Entró todo sin esperar: la cola tiene lugar para 100 kB.
        assert_eq!(Instant::now(), t0);
        assert_eq!(wifi.send_input(1000), t0 + ms(103));
        // Lo que no entra espera a que se vacíe.
        w.write_all(&[0; 50_000]).await.unwrap();
        assert!(Instant::now() >= t0 + ms(49), "{:?}", Instant::now() - t0);
        let sent: usize = wifi.data_sent().iter().map(|(_, n)| n).sum();
        assert_eq!(sent, 150_000);
    }

    #[tokio::test(start_paused = true)]
    async fn bajar_la_capacidad_alarga_la_cola() {
        let wifi = SimWifi::new(1_000_000, Duration::ZERO, 100_000);
        let mut w = wifi.data_writer();
        w.write_all(&[0; 10_000]).await.unwrap();
        let t0 = Instant::now();
        wifi.set_capacity(500_000);
        assert_eq!(wifi.capacity(), 500_000);
        // 10 kB a 500 kB/s son 20 ms, y la tecla (500 B) 1 ms más.
        assert_eq!(wifi.send_input(500), t0 + ms(21));
    }
}
