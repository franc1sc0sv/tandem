//! Puertos para las pruebas con agentes reales por loopback (los arneses de
//! `tandemd/tests` y `tandem-sim`).
//!
//! Elegir un puerto «libre» y abrirlo después deja una carrera: otra suite que corre a
//! la vez puede tomarlo entre medio. Con los puertos de reserva de la red el agente se
//! pasaba sin error a otro puerto mientras el otro agente seguía marcando el viejo, y la
//! prueba se colgaba hasta su tiempo máximo. Por eso los arneses arrancan sin puertos de
//! reserva ([`NetOptions::fallback_ports`](crate::network::NetOptions) vacío): un choque
//! falla enseguida con [`NetError::NoPort`], y [`with_free_ports`] vuelve a elegir.

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};

use crate::daemon::StartError;
use crate::network::NetError;

/// Cuántas veces se vuelve a elegir antes de rendirse.
pub const ATTEMPTS: usize = 20;

/// El arranque falló porque el puerto elegido ya estaba tomado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortTaken;

/// Puertos que el sistema no reparte solo. Pedirle uno al sistema (bind al 0) y soltarlo
/// dejaba que otra prueba, o el puerto de origen de un cliente QUIC, lo recibiera enseguida
/// (Windows reparte los efímeros en orden y reutiliza el recién soltado: «Address already in
/// use», error 10013). Por debajo del rango efímero (49152-65535) y del que Windows reserva
/// con Hyper-V (47000-48999), un contador del proceso no repite ninguno entre pruebas.
static NEXT: AtomicU16 = AtomicU16::new(0);
const SPAN: u16 = 6_000;

fn base() -> u16 {
    // Cada proceso de pruebas en su tramo, por si dos corren a la vez.
    20_000 + (std::process::id() % 2_000) as u16 * 10
}

fn is_free(port: u16) -> bool {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Un puerto UDP de loopback libre ahora y que el proceso no vuelve a entregar.
pub fn free_port() -> io::Result<u16> {
    loop {
        let p = base() + NEXT.fetch_add(1, Ordering::Relaxed) % SPAN;
        if is_free(p) {
            return Ok(p);
        }
    }
}

/// Un puerto libre con el siguiente también libre: el agente escucha en el primero y
/// empareja en el segundo. Los pares no se solapan entre llamadas.
pub fn free_pair() -> u16 {
    loop {
        let p = base() + NEXT.fetch_add(2, Ordering::Relaxed) % SPAN;
        if is_free(p) && is_free(p + 1) {
            return p;
        }
    }
}

/// Dos puertos de loopback distintos, libres hace un instante.
pub fn two_ports() -> io::Result<(u16, u16)> {
    loop {
        let (a, b) = (free_port()?, free_port()?);
        if a != b {
            return Ok((a, b));
        }
    }
}

/// Arranca dos agentes de prueba con un par de puertos nuevo: `start_pair!(|pa, pb|
/// arranque_a, arranque_b)`, donde cada arranque es un futuro de `Result<_, PortTaken>`
/// que usa `pa` y `pb`. Si alguno encuentra su puerto tomado, se detiene el que arrancó
/// (`.stop().await`) y se eligen otros.
#[macro_export]
macro_rules! start_pair {
    (|$pa:ident, $pb:ident| $a:expr, $b:expr) => {
        $crate::start_pair!(ports: || $crate::test_ports::free_port().expect("puerto de loopback"); |$pa, $pb| $a, $b)
    };
    // Con otra forma de elegir cada puerto (por ejemplo, uno con su vecino libre).
    (ports: $gen:expr; |$pa:ident, $pb:ident| $a:expr, $b:expr) => {{
        let mut left = $crate::test_ports::ATTEMPTS;
        loop {
            let ($pa, $pb) = loop {
                let (x, y) = (($gen)(), ($gen)());
                if x != y {
                    break (x, y);
                }
            };
            if let Ok(a) = $a.await {
                match $b.await {
                    Ok(b) => break (a, b),
                    Err(_) => a.stop().await,
                }
            }
            left -= 1;
            assert!(left > 0, "no se encontraron puertos libres");
        }
    }};
}

/// Como [`start_pair!`], con un solo agente (el puerto del otro no se abre).
#[macro_export]
macro_rules! start_solo {
    (|$pa:ident, $pb:ident| $a:expr) => {{
        let mut left = $crate::test_ports::ATTEMPTS;
        loop {
            let ($pa, $pb) = $crate::test_ports::two_ports().expect("puertos de loopback");
            if let Ok(a) = $a.await {
                break a;
            }
            left -= 1;
            assert!(left > 0, "no se encontraron puertos libres");
        }
    }};
}

/// `Err(PortTaken)` si el agente no arrancó por el puerto; cualquier otro error, tal cual.
pub fn port_taken<T>(r: Result<T, StartError>) -> Result<Result<T, PortTaken>, StartError> {
    match r {
        Ok(t) => Ok(Ok(t)),
        Err(StartError::Net(NetError::NoPort { .. })) => Ok(Err(PortTaken)),
        Err(e) => Err(e),
    }
}

/// Elige `n` puertos libres distintos y llama a `start`; si devuelve [`PortTaken`] (otra
/// suite tomó alguno entre medio), elige otros y vuelve a empezar. `start` tiene que
/// soltar lo que haya arrancado antes de devolver el error.
pub async fn with_free_ports<T, F, Fut>(n: usize, mut start: F) -> io::Result<T>
where
    F: FnMut(Vec<u16>) -> Fut,
    Fut: Future<Output = Result<T, PortTaken>>,
{
    for _ in 0..ATTEMPTS {
        let mut ports = Vec::with_capacity(n);
        while ports.len() < n {
            let p = free_port()?;
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
        match start(ports.clone()).await {
            Ok(t) => return Ok(t),
            Err(PortTaken) => tracing::debug!(?ports, "puerto tomado: se eligen otros"),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        "no se encontraron puertos libres para la prueba",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ports_never_repeat_and_stay_out_of_the_ephemeral_range() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            let single = free_port().unwrap();
            let pair = free_pair();
            for p in [single, pair, pair + 1] {
                assert!(p < 47_000, "{p} cae en el rango que reparte el sistema");
                assert!(seen.insert(p), "{p} se entregó dos veces");
            }
        }
    }

    #[tokio::test]
    async fn a_taken_port_is_replaced_by_a_new_one() {
        let held = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let mut first = true;
        let got = with_free_ports(2, |ports| {
            // El primer intento usa el puerto que alguien más tiene abierto.
            let ports = if std::mem::take(&mut first) {
                vec![taken, ports[1]]
            } else {
                ports
            };
            async move {
                match UdpSocket::bind((Ipv4Addr::LOCALHOST, ports[0])) {
                    Ok(s) => Ok((ports, s)),
                    Err(_) => Err(PortTaken),
                }
            }
        })
        .await
        .unwrap();
        assert!(!first);
        assert_ne!(got.0[0], taken);
        assert_ne!(got.0[0], got.0[1]);
    }
}
