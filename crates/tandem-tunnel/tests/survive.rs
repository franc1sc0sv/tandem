//! Two agents in the same process (origin A, destination B) joined by in-memory paths that
//! are cut midway through the output: what reaches the origin is exactly what the
//! destination wrote, in order, with no gaps or repeats.

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
            session: "session".into(),
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

/// Deterministic bytes that do not repeat in short cycles.
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

/// Waits for the origin to have `want` (per flow) complete and ended.
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
    .expect("the output did not arrive complete in time");
}

fn received(origin: &Origin, id: TunnelId, flow: FlowId) -> Vec<u8> {
    match origin.read(id, flow, 0, usize::MAX) {
        Some(Read::Data { offset: 0, bytes }) => bytes,
        Some(Read::Pending) => Vec::new(),
        other => panic!("lectura inesperada: {other:?}"),
    }
}

/// Opens a tunnel over the first path, writes two flows in pieces and cuts each path
/// after `cuts[i]` bytes, moving the link to the next.
async fn scenario(sizes: (usize, usize), chunk: usize, cuts: Vec<u64>) {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = origin();
    let paths: Vec<MemPath> = (0..=cuts.len()).map(|_| MemPath::new(&host)).collect();
    for (p, c) in paths.iter().zip(&cuts) {
        p.cut_after(*c);
    }
    origin.set_link(Some(paths[0].dialer()));
    let t = origin.open(spec("cuts")).await.unwrap();
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
    // The active link moves to the next path when the current one is cut.
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
    // The first path was not enough for everything: it was cut midway through the output.
    if let Some(first) = cuts.first() {
        assert_eq!(paths[0].is_cut(), *first < (sizes.0 + sizes.1) as u64);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Cuts at any point (also before the first byte or after the last) and
    /// reconnection over another path: the output arrives complete and in order.
    #[test]
    fn the_output_arrives_complete_with_cuts_at_any_point(
        sizes in (0usize..200_000, 0usize..50_000),
        chunk in 1usize..20_000,
        cuts in prop::collection::vec(0u64..250_000, 0..4),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(scenario(sizes, chunk, cuts));
    }
}

#[tokio::test]
async fn with_the_link_down_the_latest_is_kept_and_what_was_lost_is_reported() {
    let opts = HostOptions {
        capacity: 1000,
        ..HostOptions::default()
    };
    let host = Host::new(opts, Changes::default());
    let origin = origin();
    let (first, second) = (MemPath::new(&host), MemPath::new(&host));
    origin.set_link(Some(first.dialer()));
    let t = origin.open(spec("buffer")).await.unwrap();
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
async fn closing_from_the_origin_releases_the_resources_on_the_destination() {
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
    let t = tunnels_a.origin.open(spec("close")).await.unwrap();

    // Both machines show it in their project.
    for view in [tunnels_a.view(), tunnels_b.view()] {
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].project.as_ref().unwrap().name, "tandem");
        assert_eq!(view[0].tunnels[0].tunnel.id, t.id);
    }

    let ht = tunnels_b.host.get(t.id).unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    ht.add_resource(Box::new(Flag(closed.clone()))).unwrap();
    let w = ht.add_flow("stdout").unwrap();
    w.write(b"before closing.");

    tunnels_a.origin.close(t.id).await.unwrap();
    assert!(closed.load(Ordering::SeqCst), "the resource is still open");
    assert!(!ht.is_open());

    // The output that was missing arrives before the close, and the origin considers it closed.
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
        b"before closing."
    );
    assert!(tunnels_a.view().is_empty());
    assert!(tunnels_b.view().is_empty());
}

#[tokio::test]
async fn without_a_link_or_toward_the_same_machine_it_does_not_open() {
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
async fn the_restarted_destination_no_longer_has_the_tunnel_and_the_origin_considers_it_closed() {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = origin();
    let path = MemPath::new(&host);
    origin.set_link(Some(path.dialer()));
    let t = origin.open(spec("restart")).await.unwrap();
    path.cut();
    // Another agent on the destination (restarted): does not know the tunnel.
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

#[tokio::test(start_paused = true)]
async fn the_origin_forgets_closed_tunnels_after_a_while() {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = Origin::new(
        OriginOptions {
            retry: Duration::from_millis(5),
            linger: Duration::from_secs(60),
            ..OriginOptions::default()
        },
        Changes::default(),
    );
    origin.set_link(Some(MemPath::new(&host).dialer()));
    let t = origin.open(spec("forget")).await.unwrap();
    let w = host.get(t.id).unwrap().add_flow("stdout").unwrap();
    w.write(b"hello");
    origin.close(t.id).await.unwrap();
    let mut changes = origin.subscribe_output();
    while origin.tunnels()[0].state != TunnelState::Closed {
        changes.changed().await.unwrap();
    }
    // While `linger` lasts what remained can be read.
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(origin.tunnels().len(), 1);
    assert_eq!(origin.flows(t.id).len(), 1);
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(origin.tunnels().is_empty());
    assert!(origin.flows(t.id).is_empty());
    assert_eq!(origin.read(t.id, w.id(), 0, 10), None);
}

#[tokio::test]
async fn an_unanswered_open_does_not_leave_an_orphan_tunnel_on_the_destination() {
    let host = Host::new(HostOptions::default(), Changes::default());
    let origin = origin();
    let (first, second) = (MemPath::new(&host), MemPath::new(&host));
    // The destination opens the tunnel, but the path is cut before the response arrives.
    first.cut_before_reply();
    origin.set_link(Some(first.dialer()));
    assert!(matches!(
        origin.open(spec("orphan")).await,
        Err(TunnelError::Wire(_))
    ));
    let orphan = host.list();
    assert_eq!(orphan.len(), 1);
    assert_eq!(orphan[0].state, TunnelState::Open);
    // When the link returns, the origin closes it on its own.
    origin.set_link(Some(second.dialer()));
    tokio::time::timeout(TIMEOUT, async {
        while host.list()[0].state != TunnelState::Closed {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the orphan tunnel is still open on the destination");
    assert!(origin.tunnels().is_empty());
}
