//! Dos agentes en el mismo proceso, por loopback, con QUIC real.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_link::transport::{Connection, Endpoint, PeerTrust, TransportError};
use tandem_proto::{ControlMsg, Hello, Peer, ProtoError};

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
/// Tope para que una prueba rota falle en vez de colgarse.
const TIMEOUT: Duration = Duration::from_secs(10);

struct Keys(BTreeSet<PublicKey>);

impl PeerTrust for Keys {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.0.contains(key)
    }
}

fn identity(seed: u8) -> Identity {
    Identity::from_seed([seed; 32])
}

fn trusting(peers: &[&Identity]) -> Arc<dyn PeerTrust> {
    Arc::new(Keys(peers.iter().map(|i| i.public_key()).collect()))
}

fn endpoint(id: &Identity, trust: Arc<dyn PeerTrust>) -> Endpoint {
    Endpoint::bind(LOOPBACK, id, trust).unwrap()
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// Conecta `client` con `server` y devuelve las dos puntas.
async fn pair(client: &Endpoint, server: &Endpoint) -> (Connection, Connection) {
    let addr = server.local_addr().unwrap();
    let (c, s) = within(async { tokio::join!(client.connect(addr), server.accept()) }).await;
    (c.unwrap(), s.unwrap().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pares_emparejados_intercambian_mensajes_y_datagramas() {
    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));

    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;
    assert_eq!(to_pc.peer(), pc.public_key());
    assert_eq!(to_mac.peer(), mac.public_key());

    let mac_hello = Hello::new(Peer::Agent(Machine::A));
    let pc_hello = Hello::new(Peer::Agent(Machine::B));
    let (opened, accepted) = within(async {
        tokio::join!(
            to_pc.open_control(&mac_hello),
            to_mac.accept_control(&pc_hello)
        )
    })
    .await;
    let (mut mac_ctl, their_hello) = opened.unwrap();
    assert_eq!(their_hello, pc_hello);
    let (mut pc_ctl, their_hello) = accepted.unwrap();
    assert_eq!(their_hello, mac_hello);

    // Control en los dos sentidos.
    let msgs = [
        ControlMsg::Ping { nonce: 7 },
        ControlMsg::GetState,
        ControlMsg::Pong { nonce: 8 },
    ];
    for m in &msgs {
        mac_ctl.send(m).await.unwrap();
    }
    for m in &msgs {
        assert_eq!(
            within(pc_ctl.recv::<ControlMsg>()).await.unwrap(),
            Some(m.clone())
        );
    }
    pc_ctl.send(&ControlMsg::Pong { nonce: 7 }).await.unwrap();
    assert_eq!(
        within(mac_ctl.recv::<ControlMsg>()).await.unwrap(),
        Some(ControlMsg::Pong { nonce: 7 })
    );

    // Datagramas en los dos sentidos.
    assert!(to_pc.max_datagram_size().is_some_and(|n| n >= 512));
    let payloads: Vec<Bytes> = (0u8..5).map(|i| Bytes::from(vec![i; 32])).collect();
    for p in &payloads {
        to_pc.send_datagram(p.clone()).unwrap();
    }
    for p in &payloads {
        assert_eq!(&within(to_mac.read_datagram()).await.unwrap(), p);
    }
    to_mac.send_datagram(Bytes::from_static(b"mouse")).unwrap();
    assert_eq!(
        within(to_pc.read_datagram()).await.unwrap(),
        Bytes::from_static(b"mouse")
    );

    // Cierre limpio del stream: el otro lado ve el fin y no un error.
    mac_ctl.finish().unwrap();
    assert_eq!(within(pc_ctl.recv::<ControlMsg>()).await.unwrap(), None);

    mac_ep.close();
    pc_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latencia_de_ida_y_vuelta_en_loopback() {
    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;

    let hello = Hello::new(Peer::Agent(Machine::A));
    let (opened, accepted) =
        within(async { tokio::join!(to_pc.open_control(&hello), to_mac.accept_control(&hello)) })
            .await;
    let (mut mac_ctl, _) = opened.unwrap();
    let (mut pc_ctl, _) = accepted.unwrap();

    // El otro agente contesta cada Ping con su Pong.
    let echo = tokio::spawn(async move {
        while let Some(msg) = pc_ctl.recv::<ControlMsg>().await.unwrap() {
            if let ControlMsg::Ping { nonce } = msg {
                pc_ctl.send(&ControlMsg::Pong { nonce }).await.unwrap();
            }
        }
    });

    const ROUNDS: u64 = 200;
    let mut samples = Vec::with_capacity(ROUNDS as usize);
    for nonce in 0..ROUNDS {
        let start = Instant::now();
        mac_ctl.send(&ControlMsg::Ping { nonce }).await.unwrap();
        let reply = within(mac_ctl.recv::<ControlMsg>()).await.unwrap();
        samples.push(start.elapsed());
        assert_eq!(reply, Some(ControlMsg::Pong { nonce }));
    }
    mac_ctl.finish().unwrap();
    within(echo).await.unwrap();

    samples.sort();
    let mean = samples.iter().sum::<Duration>() / ROUNDS as u32;
    let p50 = samples[samples.len() / 2];
    let p99 = samples[samples.len() * 99 / 100];
    // Informativo: `cargo test -p tandem-link --test transport -- --nocapture`.
    println!(
        "RTT control (loopback, {ROUNDS} pings): media {mean:?}, p50 {p50:?}, p99 {p99:?}, \
         máx {:?}; estimación QUIC {:?}",
        samples[samples.len() - 1],
        to_pc.rtt()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_servidor_rechaza_a_un_cliente_no_emparejado() {
    let mac = identity(1);
    let pc = identity(2);
    let intruder = identity(9);
    // El intruso confía en la PC, pero la PC solo conoce a la Mac.
    let intruder_ep = endpoint(&intruder, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let addr = pc_ep.local_addr().unwrap();

    let (client, server) =
        within(async { tokio::join!(intruder_ep.connect(addr), pc_ep.accept()) }).await;

    // El servidor lo corta en el handshake.
    assert!(matches!(
        server,
        Some(Err(TransportError::Connection(
            quinn::ConnectionError::TransportError(_)
        )))
    ));
    // El cliente puede haber dado el handshake por bueno (TLS 1.3); en ese
    // caso nada pasa por la conexión.
    if let Ok(conn) = client {
        let hello = Hello::new(Peer::Agent(Machine::B));
        assert!(within(conn.open_control(&hello)).await.is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_cliente_rechaza_a_un_servidor_no_emparejado() {
    let mac = identity(1);
    let pc = identity(2);
    let impostor = identity(9);
    // El impostor acepta a la Mac, pero la Mac solo confía en la PC.
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let impostor_ep = endpoint(&impostor, trusting(&[&mac]));
    let addr = impostor_ep.local_addr().unwrap();

    let (client, server) =
        within(async { tokio::join!(mac_ep.connect(addr), impostor_ep.accept()) }).await;
    assert!(matches!(
        client,
        Err(TransportError::Connection(
            quinn::ConnectionError::TransportError(_)
        ))
    ));
    assert!(matches!(server, Some(Err(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_par_emparejado_despues_de_abrir_el_endpoint_se_acepta() {
    let mac = identity(1);
    let pc = identity(2);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(RwLock::new(
        TrustStore::open(dir.path().join("trust.toml")).unwrap(),
    ));
    let pc_ep = endpoint(&pc, store.clone());
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let addr = pc_ep.local_addr().unwrap();

    let (_, server) = within(async { tokio::join!(mac_ep.connect(addr), pc_ep.accept()) }).await;
    assert!(matches!(server, Some(Err(_))));

    store.write().unwrap().add(TrustedPeer {
        public_key: mac.public_key(),
        name: "Mac".to_owned(),
    });
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;
    assert_eq!(to_mac.peer(), mac.public_key());
    assert_eq!(to_pc.peer(), pc.public_key());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_hello_de_otra_version_se_rechaza_en_los_dos_lados() {
    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;

    let mut old = Hello::new(Peer::Agent(Machine::A));
    old.version = old.version.wrapping_add(1);
    let current = Hello::new(Peer::Agent(Machine::B));
    let (opened, accepted) =
        within(async { tokio::join!(to_pc.open_control(&old), to_mac.accept_control(&current)) })
            .await;

    assert!(matches!(
        accepted,
        Err(TransportError::Proto(
            ProtoError::IncompatibleVersion { .. }
        ))
    ));
    // El Hello de vuelta sale igual, así que quien abrió no se queda colgado.
    // Aquí recibe uno de su misma versión compilada; en el caso real (otro
    // binario) también fallaría con `IncompatibleVersion`.
    assert_eq!(opened.unwrap().1, current);
}

/// T-084: un agente de antes del anuncio de capacidades (protocolo 8) se rechaza en el
/// saludo, con un error que dice qué pasa y qué hacer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_agente_sin_anuncio_de_capacidades_se_rechaza_con_un_error_claro() {
    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;

    let mut old = Hello::new(Peer::Agent(Machine::A));
    old.version = tandem_proto::CAPABILITIES_SINCE - 1;
    let current = Hello::new(Peer::Agent(Machine::B));
    let (_, accepted) =
        within(async { tokio::join!(to_pc.open_control(&old), to_mac.accept_control(&current)) })
            .await;
    let Err(e) = accepted else {
        panic!("se aceptó un agente sin capacidades");
    };
    assert!(
        matches!(
            e,
            TransportError::Proto(ProtoError::IncompatibleVersion { theirs: 9, .. })
        ),
        "{e}"
    );
    let text = e.to_string();
    assert!(text.contains("versión de protocolo incompatible"), "{text}");
    assert!(text.contains("sin anuncio de capacidades"), "{text}");
}

/// Con el stream partido, los dos lados pueden mandar mucho a la vez: cada uno
/// sigue leyendo mientras el control de flujo frena su escritura.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_partido_escribe_y_lee_a_la_vez_en_los_dos_sentidos() {
    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;
    let hello = Hello::new(Peer::Agent(Machine::A));
    let (opened, accepted) =
        within(async { tokio::join!(to_pc.open_control(&hello), to_mac.accept_control(&hello)) })
            .await;

    // Más de lo que cabe en las ventanas de QUIC, en los dos sentidos.
    const FRAMES: usize = 64;
    let block = vec![0xA5u8; 128 * 1024];
    async fn exchange(stream: tandem_link::transport::ControlStream, block: &[u8]) -> Vec<Vec<u8>> {
        let (mut tx, mut rx) = stream.split();
        let write = async {
            for i in 0..FRAMES {
                let mut b = block.to_vec();
                b[0] = i as u8;
                tx.send(&b).await.unwrap();
            }
            tx.finish().unwrap();
        };
        let read = async {
            let mut got = Vec::new();
            while let Some(b) = rx.recv::<Vec<u8>>().await.unwrap() {
                got.push(b);
            }
            got
        };
        tokio::join!(write, read).1
    }
    let (a, b) = within(async {
        tokio::join!(
            exchange(opened.unwrap().0, &block),
            exchange(accepted.unwrap().0, &block)
        )
    })
    .await;
    for got in [a, b] {
        assert_eq!(got.len(), FRAMES);
        for (i, b) in got.iter().enumerate() {
            assert_eq!(b[0], i as u8);
            assert_eq!(b.len(), block.len());
        }
    }
}

/// El endpoint de emparejamiento conecta con llaves que nadie conoce todavía, y cada
/// lado ve la llave real del otro (la que firmó el handshake).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_endpoint_de_emparejamiento_acepta_llaves_desconocidas() {
    let (mac, pc) = (identity(30), identity(31));
    let mac_ep = Endpoint::bind_pairing(LOOPBACK, &mac).unwrap();
    let pc_ep = Endpoint::bind_pairing(LOOPBACK, &pc).unwrap();
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;
    assert_eq!(to_pc.peer(), pc.public_key());
    assert_eq!(to_mac.peer(), mac.public_key());

    let (mac_hello, pc_hello) = (
        Hello::new(Peer::Agent(Machine::A)),
        Hello::new(Peer::Agent(Machine::B)),
    );
    let (opened, accepted) = within(async {
        tokio::join!(
            to_pc.open_control(&mac_hello),
            to_mac.accept_control(&pc_hello)
        )
    })
    .await;
    let (mut a, _) = opened.unwrap();
    let (mut b, _) = accepted.unwrap();
    a.send(&ControlMsg::Ping { nonce: 1 }).await.unwrap();
    assert_eq!(
        within(b.recv::<ControlMsg>()).await.unwrap(),
        Some(ControlMsg::Ping { nonce: 1 })
    );
}

/// Con otro ALPN, el endpoint de emparejamiento no habla con uno normal en ningún
/// sentido, aunque el normal confíe en la llave: un par a medio emparejar no llega al
/// plano de control, y el endpoint normal nunca acepta llaves desconocidas.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emparejamiento_y_control_no_se_mezclan() {
    let (mac, pc) = (identity(32), identity(33));
    let pairing = Endpoint::bind_pairing(LOOPBACK, &mac).unwrap();
    let normal = endpoint(&pc, trusting(&[&mac]));

    // quinn solo avanza el handshake de quien acepta: los dos lados a la vez.
    let (c, s) = within(async {
        tokio::join!(
            pairing.connect(normal.local_addr().unwrap()),
            normal.accept()
        )
    })
    .await;
    assert!(c.is_err(), "conectó con otro ALPN");
    assert!(matches!(s, Some(Err(_))), "aceptó otro ALPN");
    let (c, s) = within(async {
        tokio::join!(
            normal.connect(pairing.local_addr().unwrap()),
            pairing.accept()
        )
    })
    .await;
    assert!(c.is_err(), "conectó con otro ALPN");
    assert!(matches!(s, Some(Err(_))), "aceptó otro ALPN");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn los_datos_van_por_un_stream_de_prioridad_mínima_y_con_el_tope_del_wifi() {
    use tandem_link::manager::LinkKind;
    use tandem_link::qos::StreamClass;

    let a = identity(1);
    let b = identity(2);
    let a_ep = endpoint(&a, trusting(&[&b]));
    let b_ep = endpoint(&b, trusting(&[&a]));
    let (to_b, to_a) = pair(&a_ep, &b_ep).await;
    let a_hello = Hello::new(Peer::Agent(Machine::A));
    let b_hello = Hello::new(Peer::Agent(Machine::B));

    // El de la entrada, con la prioridad más alta.
    let (opened, accepted) =
        within(async { tokio::join!(to_b.open_control(&a_hello), to_a.accept_control(&b_hello)) })
            .await;
    let (input, _) = opened.unwrap();
    let _keep = accepted.unwrap();
    input.set_class(StreamClass::Input).unwrap();
    let (input_tx, _) = input.split();
    assert_eq!(input_tx.priority().unwrap(), StreamClass::Input.priority());

    // Por Wi-Fi medido en 400 kB/s, los datos tienen 200 kB/s.
    to_b.qos().set_link(Some(LinkKind::Wifi));
    to_b.qos().set_measured(400_000);
    assert_eq!(to_b.qos().cap(), Some(200_000));
    let (opened, accepted) =
        within(async { tokio::join!(to_b.open_control(&a_hello), to_a.accept_control(&b_hello)) })
            .await;
    let (mut data, _) = opened.unwrap().0.into_data(to_b.qos()).unwrap();
    let (mut rx, _) = accepted.unwrap();
    assert_eq!(
        data.get_ref().priority().unwrap(),
        StreamClass::Data.priority()
    );

    let chunk = vec![7u8; 30_000];
    let start = Instant::now();
    let reader = tokio::spawn(async move {
        for _ in 0..10 {
            let msg = within(rx.recv::<Vec<u8>>()).await.unwrap().unwrap();
            assert_eq!(msg.len(), 30_000);
        }
    });
    for _ in 0..10 {
        within(data.send(&chunk)).await.unwrap();
    }
    within(reader).await.unwrap();
    // 300 kB a 200 kB/s, con la ráfaga del principio (2 kB): no menos de 1,4 s.
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(1400), "tardó {took:?}");
}

/// Entre las dos máquinas hay dos conexiones (la que abre cada una). Con el `Qos` del camino
/// compartido, los datos que A manda por las dos a la vez no pasan juntos el tope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn las_dos_conexiones_de_un_camino_comparten_el_tope() {
    use tandem_link::manager::LinkKind;
    use tandem_link::qos::{LinkQos, StreamClass};

    let a = identity(1);
    let b = identity(2);
    let a_ep = endpoint(&a, trusting(&[&b]));
    let b_ep = endpoint(&b, trusting(&[&a]));
    let a_hello = Hello::new(Peer::Agent(Machine::A));
    let b_hello = Hello::new(Peer::Agent(Machine::B));

    let qos = LinkQos::default();
    let wifi = qos.get(LinkKind::Wifi);
    wifi.set_measured(400_000);
    assert_eq!(wifi.cap(), Some(200_000));
    // La que abre A y la que A acepta de B, las dos por Wi-Fi.
    let (a_out, b_in) = pair(&a_ep, &b_ep).await;
    let (b_out, a_in) = pair(&b_ep, &a_ep).await;
    let a_out = a_out.with_qos(qos.get(LinkKind::Wifi));
    let a_in = a_in.with_qos(qos.get(LinkKind::Wifi));
    assert!(a_out.qos().same(a_in.qos()));

    let mut streams = Vec::new();
    for (mine, theirs) in [(&a_out, &b_in), (&a_in, &b_out)] {
        let (opened, accepted) = within(async {
            tokio::join!(mine.open_control(&a_hello), theirs.accept_control(&b_hello))
        })
        .await;
        let (data, _) = opened.unwrap().0.into_data(mine.qos()).unwrap();
        let (accepted, _) = accepted.unwrap();
        // El lado que acepta pone su propia prioridad para lo que mande.
        accepted.set_class(StreamClass::Clipboard).unwrap();
        let (tx, rx) = accepted.split();
        assert_eq!(tx.priority().unwrap(), StreamClass::Clipboard.priority());
        streams.push((data, rx));
    }

    // Las dos escrituras (150 kB cada una) a la vez: 300 kB a 200 kB/s entre las dos.
    let start = Instant::now();
    let mut tasks = Vec::new();
    for (mut data, mut rx) in streams {
        tasks.push(tokio::spawn(async move {
            let chunk = vec![1u8; 30_000];
            for _ in 0..5 {
                within(data.send(&chunk)).await.unwrap();
            }
            // Que el stream siga vivo hasta que el otro lado lea todo.
            let mut got = 0;
            while got < 5 {
                let msg = within(rx.recv::<Vec<u8>>()).await.unwrap().unwrap();
                assert_eq!(msg.len(), 30_000);
                got += 1;
            }
        }));
    }
    for t in tasks {
        within(t).await.unwrap();
    }
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(1400), "tardó {took:?}");
}

/// T-102: la prueba de ancho de banda por un stream de datos de QUIC real. Mide quien
/// recibe y quien la pidió recibe ese número.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_prueba_de_ancho_de_banda_devuelve_lo_que_midió_el_otro_lado() {
    use tandem_proto::Channel;

    let mac = identity(1);
    let pc = identity(2);
    let mac_ep = endpoint(&mac, trusting(&[&pc]));
    let pc_ep = endpoint(&pc, trusting(&[&mac]));
    let (to_pc, to_mac) = pair(&mac_ep, &pc_ep).await;
    let mac_hello = Hello::new(Peer::Agent(Machine::A));
    let pc_hello = Hello::new(Peer::Agent(Machine::B));
    let answer = tokio::spawn(async move {
        let (mut s, _) = to_mac.accept_control(&pc_hello).await.unwrap();
        assert_eq!(
            s.recv::<ControlMsg>().await.unwrap(),
            Some(ControlMsg::Open(Channel::Measure))
        );
        // La conexión sigue viva hasta que el otro lea la respuesta.
        (s.answer_measure().await.unwrap(), to_mac)
    });
    let (mut s, _) = within(to_pc.open_control(&mac_hello)).await.unwrap();
    s.send(&ControlMsg::Open(Channel::Measure)).await.unwrap();
    let got = within(s.measure(Duration::from_millis(500))).await.unwrap();
    let (answered, _conn) = within(answer).await.unwrap();
    assert_eq!(got, answered);
    assert!(got.bytes > 0, "{got:?}");
    // Por loopback, bastante más que 1 MB/s.
    assert!(got.bytes_per_sec().unwrap() > 1_000_000, "{got:?}");
}
