//! Dos agentes en el mismo proceso (origen A, destino B) unidos por caminos en memoria que
//! se cortan a mitad de la salida: lo que llega al origen es exactamente lo que escribió la
//! destino, en orden, sin huecos ni repetidos.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use proptest::prelude::*;
use tandem_core::Machine;
use tandem_proto::{Client, ClientKind, FlowId, Project, TunnelId, TunnelState};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::{
    Changes, FlowStatus, Host, HostOptions, OpenSpec, Origin, OriginOptions, Read, Resource,
    TunnelError, Tunnels,
};

const TIMEOUT: Duration = Duration::from_secs(20);

fn spec(title: &str) -> OpenSpec {
    OpenSpec {
        title: title.into(),
        project: Some(Project {
            name: "tandem".into(),
            root: "/src/tandem".into(),
        }),
        origin: Machine::A,
        target: Machine::B,
        opened_by: Client {
            kind: ClientKind::Mcp,
            session: "sesión".into(),
        },
    }
}

fn origin() -> Origin {
    Origin::new(
        OriginOptions {
            retry: Duration::from_millis(5),
            ..OriginOptions::default()
        },
        Changes::default(),
    )
}

/// Bytes deterministas que no se repiten en ciclos cortos.
fn bytes(len: usize, seed: u8) -> Vec<u8> {
    let mut x = u32::from(seed) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x.to_le_bytes()[0]
        })
        .collect()
}

/// Espera a que el origen tenga `want` (por flujo) completo y terminado.
async fn until_complete(origin: &Origin, id: TunnelId, want: &[(FlowId, usize)]) {
    let mut output = origin.subscribe_output();
    let done = |flows: Vec<FlowStatus>| {
        want.iter().all(|(f, len)| {
            flows.iter().any(|s| {
                s.info.id == *f && s.ended == Some(*len as u64) && s.received == *len as u64
            })
        })
    };
    tokio::time::timeout(TIMEOUT, async {
        while !done(origin.flows(id)) {
            output.changed().await.unwrap();
        }
    })
    .await
    .expect("la salida no llegó completa a tiempo");
}

fn received(origin: &Origin, id: TunnelId, flow: FlowId) -> Vec<u8> {
    match origin.read(id, flow, 0, usize::MAX) {
        Some(Read::Data { offset: 0, bytes }) => bytes,
        Some(Read::Pending) => Vec::new(),
        other => panic!("lectura inesperada: {other:?}"),
    }
}

/// Abre un túnel por el primer camino, escribe dos flujos de a pedazos y corta cada camino
/// después de `cuts[i]` bytes, pasando el enlace al siguiente.
async fn scenario(sizes: (usize, usize), chunk: usize, cuts: Vec<u64>) {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = origin();
    let paths: Vec<MemPath> = (0..=cuts.len()).map(|_| MemPath::new(&host)).collect();
    for (p, c) in paths.iter().zip(&cuts) {
        p.cut_after(*c);
    }
    origin.set_link(Some(paths[0].dialer()));
    let t = origin.open(spec("cortes")).await.unwrap();
    let ht = host.get(t.id).unwrap();
    let out = bytes(sizes.0, 7);
    let err = bytes(sizes.1, 9);
    let (wo, we) = (
        ht.add_flow("stdout").unwrap(),
        ht.add_flow("stderr").unwrap(),
    );
    let ids = [(wo.id(), out.len()), (we.id(), err.len())];
    let writer = {
        let (out, err) = (out.clone(), err.clone());
        tokio::spawn(async move {
            let (mut a, mut b) = (out.chunks(chunk), err.chunks(chunk));
            loop {
                let (x, y) = (a.next(), b.next());
                if x.is_none() && y.is_none() {
                    break;
                }
                x.inspect(|c| wo.write(c));
                y.inspect(|c| we.write(c));
                tokio::task::yield_now().await;
            }
        })
    };
    // El enlace activo pasa al camino siguiente cuando se corta el actual.
    let switcher = {
        let (paths, origin) = (paths.clone(), origin.clone());
        tokio::spawn(async move {
            for pair in paths.windows(2) {
                pair[0].wait_cut().await;
                origin.set_link(Some(pair[1].dialer()));
            }
        })
    };
    until_complete(&origin, t.id, &ids).await;
    writer.await.unwrap();
    switcher.abort();
    assert_eq!(received(&origin, t.id, ids[0].0), out);
    assert_eq!(received(&origin, t.id, ids[1].0), err);
    assert!(origin.flows(t.id).iter().all(|f| f.lost == 0));
    // El primer camino no alcanzaba para todo: se cortó a mitad de la salida.
    if let Some(first) = cuts.first() {
        assert_eq!(paths[0].is_cut(), *first < (sizes.0 + sizes.1) as u64);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Cortes en cualquier punto (también antes del primer byte o después del último) y
    /// reconexión por otro camino: la salida llega completa y en orden.
    #[test]
    fn la_salida_llega_completa_con_cortes_en_cualquier_punto(
        sizes in (0usize..200_000, 0usize..50_000),
        chunk in 1usize..20_000,
        cuts in prop::collection::vec(0u64..250_000, 0..4),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(scenario(sizes, chunk, cuts));
    }
}

#[tokio::test]
async fn con_el_enlace_caido_se_guarda_lo_ultimo_y_se_avisa_lo_perdido() {
    let opts = HostOptions {
        capacity: 1000,
        ..HostOptions::default()
    };
    let host = Host::new(opts, Changes::default());
    let origin = origin();
    let (first, second) = (MemPath::new(&host), MemPath::new(&host));
    origin.set_link(Some(first.dialer()));
    let t = origin.open(spec("búfer")).await.unwrap();
    first.cut();
    let w = host.get(t.id).unwrap().add_flow("stdout").unwrap();
    let data = bytes(5000, 3);
    w.write(&data);
    w.finish();
    origin.set_link(Some(second.dialer()));
    until_complete(&origin, t.id, &[(w.id(), 5000)]).await;
    let status = &origin.flows(t.id)[0];
    assert_eq!(status.lost, 4000);
    assert_eq!(
        origin.read(t.id, w.id(), 0, usize::MAX),
        Some(Read::Lost { from: 0, to: 4000 })
    );
    assert_eq!(
        origin.read(t.id, w.id(), 4000, usize::MAX),
        Some(Read::Data {
            offset: 4000,
            bytes: data[4000..].to_vec()
        })
    );
}

struct Flag(Arc<AtomicBool>);

impl Resource for Flag {
    fn close(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn cerrar_desde_el_origen_suelta_los_recursos_en_la_destino() {
    let tunnels_b = Tunnels::default();
    let tunnels_a = Tunnels::new(
        HostOptions::default(),
        OriginOptions {
            retry: Duration::from_millis(5),
            ..OriginOptions::default()
        },
    );
    let path = MemPath::new(&tunnels_b.host);
    tunnels_a.origin.set_link(Some(path.dialer()));
    let mut changes_a = tunnels_a.subscribe();
    let t = tunnels_a.origin.open(spec("cerrar")).await.unwrap();

    // Las dos máquinas lo muestran en su proyecto.
    for view in [tunnels_a.view(), tunnels_b.view()] {
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].project.as_ref().unwrap().name, "tandem");
        assert_eq!(view[0].tunnels[0].id, t.id);
    }

    let ht = tunnels_b.host.get(t.id).unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    ht.add_resource(Box::new(Flag(closed.clone()))).unwrap();
    let w = ht.add_flow("stdout").unwrap();
    w.write(b"antes de cerrar");

    tunnels_a.origin.close(t.id).await.unwrap();
    assert!(closed.load(Ordering::SeqCst), "el recurso sigue abierto");
    assert!(!ht.is_open());

    // La salida que faltaba llega antes del cierre, y el origen lo da por cerrado.
    until_complete(&tunnels_a.origin, t.id, &[(w.id(), 15)]).await;
    tokio::time::timeout(TIMEOUT, async {
        while tunnels_a.origin.tunnels()[0].state != TunnelState::Closed {
            changes_a.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        received(&tunnels_a.origin, t.id, w.id()),
        b"antes de cerrar"
    );
    assert!(tunnels_a.view().is_empty());
    assert!(tunnels_b.view().is_empty());
}

#[tokio::test]
async fn sin_enlace_o_hacia_la_misma_maquina_no_se_abre() {
    let origin = origin();
    assert_eq!(
        origin.open(spec("x")).await.unwrap_err(),
        TunnelError::NoLink
    );
    let mut same = spec("x");
    same.target = Machine::A;
    assert_eq!(
        origin.open(same).await.unwrap_err(),
        TunnelError::SameMachine(Machine::A)
    );
    assert_eq!(
        origin.close(TunnelId(1)).await.unwrap_err(),
        TunnelError::Unknown(TunnelId(1))
    );
}

#[tokio::test]
async fn la_destino_reiniciada_ya_no_tiene_el_tunel_y_el_origen_lo_da_por_cerrado() {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = origin();
    let path = MemPath::new(&host);
    origin.set_link(Some(path.dialer()));
    let t = origin.open(spec("reinicio")).await.unwrap();
    path.cut();
    // Otro agente en la destino (se reinició): no conoce el túnel.
    let fresh = Host::new(HostOptions::default(), Changes::default());
    origin.set_link(Some(MemPath::new(&fresh).dialer()));
    let mut changes = origin.subscribe_output();
    tokio::time::timeout(TIMEOUT, async {
        while origin.tunnels()[0].state != TunnelState::Closed {
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(origin.remote_list().await.unwrap(), []);
    let _ = t;
}
