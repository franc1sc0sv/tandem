//! Ports for tests with real agents over loopback (the harnesses of
//! `tandemd/tests` y `tandem-sim`).
//!
//! Choosing a "free" port and opening it afterwards leaves a race: another suite running at
//! the same time can take it in between. With the network's reserve ports the agent
//! silently moved to another port while the other agent kept dialing the old one, and the
//! test hung until its maximum time. That is why the harnesses start without reserve ports
//! ([`NetOptions::fallback_ports`](crate::network::NetOptions) empty): a clash
//! fails right away with [`NetError::NoPort`], and [`with_free_ports`] chooses again.

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};

use crate::daemon::StartError;
use crate::network::NetError;

/// How many times it chooses again before giving up.
pub const ATTEMPTS: usize = 20;

/// The start failed because the chosen port was already taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortTaken;

/// Ports the system does not hand out on its own. Asking the system for one (bind to 0) and releasing it
/// let another test, or a QUIC client's source port, receive it right away
/// (Windows hands out ephemerals in order and reuses the just-released one: "Address already in
/// use", error 10013). Below the ephemeral range (49152-65535) and the one Windows reserves
/// with Hyper-V (47000-48999), a process counter does not repeat any between tests.
static NEXT: AtomicU16 = AtomicU16::new(0);
const SPAN: u16 = 6_000;

fn base() -> u16 {
    // Each test process in its own stretch, in case two run at once.
    20_000 + (std::process::id() % 2_000) as u16 * 10
}

fn is_free(port: u16) -> bool {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// A loopback UDP port free right now and that the process does not hand out again.
pub fn free_port() -> io::Result<u16> {
    loop {
        let p = base() + NEXT.fetch_add(1, Ordering::Relaxed) % SPAN;
        if is_free(p) {
            return Ok(p);
        }
    }
}

/// A free port with the next one also free: the agent listens on the first and
/// pairs on the second. Pairs do not overlap between calls.
pub fn free_pair() -> u16 {
    loop {
        let p = base() + NEXT.fetch_add(2, Ordering::Relaxed) % SPAN;
        if is_free(p) && is_free(p + 1) {
            return p;
        }
    }
}

/// Two distinct loopback ports, free a moment ago.
pub fn two_ports() -> io::Result<(u16, u16)> {
    loop {
        let (a, b) = (free_port()?, free_port()?);
        if a != b {
            return Ok((a, b));
        }
    }
}

/// Starts two test agents with a new pair of ports: `start_pair!(|pa, pb|
/// start_a, start_b)`, where each start is a future of `Result<_, PortTaken>`
/// that uses `pa` and `pb`. If one finds its port taken, the one that started is stopped
/// (`.stop().await`) and others are chosen.
#[macro_export]
macro_rules! start_pair {
    (|$pa:ident, $pb:ident| $a:expr, $b:expr) => {
        $crate::start_pair!(ports: || $crate::test_ports::free_port().expect("loopback port")
    };
    // With another way of choosing each port (for example, one with its free neighbor).
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
            assert!(left > 0, "no free ports were found");
        }
    }};
}

/// Like [`start_pair!`], with a single agent (the other's port is not opened).
#[macro_export]
macro_rules! start_solo {
    (|$pa:ident, $pb:ident| $a:expr) => {{
        let mut left = $crate::test_ports::ATTEMPTS;
        loop {
            let ($pa, $pb) = $crate::test_ports::two_ports().expect("loopback ports");
            if let Ok(a) = $a.await {
                break a;
            }
            left -= 1;
            assert!(left > 0, "no free ports were found");
        }
    }};
}

/// `Err(PortTaken)` if the agent did not start because of the port; any other error, as is.
pub fn port_taken<T>(r: Result<T, StartError>) -> Result<Result<T, PortTaken>, StartError> {
    match r {
        Ok(t) => Ok(Ok(t)),
        Err(StartError::Net(NetError::NoPort { .. })) => Ok(Err(PortTaken)),
        Err(e) => Err(e),
    }
}

/// Chooses `n` distinct free ports and calls `start`; if it returns [`PortTaken`] (another
/// suite took one in between), it chooses others and starts over. `start` must
/// release whatever it started before returning the error.
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
            Err(PortTaken) => tracing::debug!(?ports, "port taken: choosing others"),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        "no free ports were found for the test",
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
                assert!(p < 47_000, "{p} falls in the range the system hands out");
                assert!(seen.insert(p), "{p} was handed out twice");
            }
        }
    }

    #[tokio::test]
    async fn a_taken_port_is_replaced_by_a_new_one() {
        let held = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let mut first = true;
        let got = with_free_ports(2, |ports| {
            // The first attempt uses the port someone else has open.
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
