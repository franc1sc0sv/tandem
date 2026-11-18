//! Reenvío de puertos entre dos agentes con un servidor TCP de eco.

mod common;

use std::time::Duration;

use common::{TIMEOUT, pair};
use tandem_files::ForwardSpec;
use tandem_proto::Direction;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, f).await.unwrap()
}

/// Un servidor de eco en un puerto libre.
async fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// Manda `payload` por una conexión y comprueba que vuelve igual.
async fn round_trip(port: u16, payload: Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut rd, mut wr) = s.split();
    let send = async {
        wr.write_all(&payload).await.unwrap();
        wr.shutdown().await.unwrap();
    };
    let recv = async {
        let mut got = Vec::new();
        rd.read_to_end(&mut got).await.unwrap();
        got
    };
    let (_, got) = tokio::join!(send, recv);
    assert_eq!(got, payload);
}

fn payload(i: usize) -> Vec<u8> {
    (0..50_000 + i * 7).map(|n| (n * (i + 3)) as u8).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn hacia_la_destino_con_20_conexiones_a_la_vez() {
    let echo = echo_server().await;
    let p = pair(1024).await;
    let info = p
        .files_a
        .forward(
            &p.origin,
            ForwardSpec {
                tunnel: p.tunnel,
                local_port: 0,
                remote_host: "127.0.0.1".into(),
                remote_port: echo,
                direction: Direction::LocalToRemote,
            },
        )
        .await
        .unwrap();
    assert_ne!(info.local_port, 0);
    let all = (0..20).map(|i| tokio::spawn(round_trip(info.local_port, payload(i))));
    within(async {
        for h in all {
            h.await.unwrap();
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hacia_el_origen_con_20_conexiones_a_la_vez() {
    // El eco vive en la máquina origen (A); la destino (B) escucha y todo vuelve a A.
    let echo = echo_server().await;
    let p = pair(1024).await;
    let info = p
        .files_a
        .forward(
            &p.origin,
            ForwardSpec {
                tunnel: p.tunnel,
                local_port: echo,
                remote_host: "127.0.0.1".into(),
                remote_port: 0,
                direction: Direction::RemoteToLocal,
            },
        )
        .await
        .unwrap();
    assert_ne!(info.remote_port, 0);
    let all = (0..20).map(|i| tokio::spawn(round_trip(info.remote_port, payload(i))));
    within(async {
        for h in all {
            h.await.unwrap();
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cerrar_el_tunel_suelta_los_puertos_de_los_dos_lados() {
    let echo = echo_server().await;
    let p = pair(1024).await;
    let spec = |direction, local_port, remote_port| ForwardSpec {
        tunnel: p.tunnel,
        local_port,
        remote_host: "127.0.0.1".into(),
        remote_port,
        direction,
    };
    let out = p
        .files_a
        .forward(&p.origin, spec(Direction::LocalToRemote, 0, echo))
        .await
        .unwrap();
    let back = p
        .files_a
        .forward(&p.origin, spec(Direction::RemoteToLocal, echo, 0))
        .await
        .unwrap();
    round_trip(out.local_port, payload(1)).await;
    round_trip(back.remote_port, payload(2)).await;
    assert_eq!(p.files_a.forwards().list().len(), 2);

    p.origin.close(p.tunnel).await.unwrap();
    within(async {
        loop {
            let a = TcpStream::connect(("127.0.0.1", out.local_port)).await;
            let b = TcpStream::connect(("127.0.0.1", back.remote_port)).await;
            if a.is_err() && b.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    within(async {
        while !p.files_a.forwards().list().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn un_puerto_que_nadie_escucha_cierra_la_conexion() {
    let p = pair(1024).await;
    // Un puerto libre que se suelta enseguida.
    let free = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let info = p
        .files_a
        .forward(
            &p.origin,
            ForwardSpec {
                tunnel: p.tunnel,
                local_port: 0,
                remote_host: "127.0.0.1".into(),
                remote_port: free,
                direction: Direction::LocalToRemote,
            },
        )
        .await
        .unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", info.local_port))
        .await
        .unwrap();
    let mut buf = Vec::new();
    within(s.read_to_end(&mut buf)).await.unwrap();
    assert!(buf.is_empty());
    assert!(p.files_a.forwards().stop(info.id));
}
