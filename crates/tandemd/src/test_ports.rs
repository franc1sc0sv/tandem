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

use crate::daemon::StartError;
use crate::network::NetError;

/// Cuántas veces se vuelve a elegir antes de rendirse.
pub const ATTEMPTS: usize = 20;

/// El arranque falló porque el puerto elegido ya estaba tomado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortTaken;

/// Un puerto UDP de loopback que estaba libre hace un instante (puede dejar de estarlo).
pub fn free_port() -> io::Result<u16> {
    Ok(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
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
