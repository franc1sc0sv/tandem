//! Bounds for the terminal path over loopback (in-memory link, two agents in one process, real
//! clocks): a keystroke's echo comes back in well under a frame, and output streams at many
//! megabytes per second, so over a real link the link is the limit. The bounds are loose on
//! purpose (debug builds, busy CI runners); they catch a poll or a per-message sleep coming back,
//! which costs orders of magnitude.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tandem_core::Machine;
use tandem_proto::{Client, ClientKind, ExecSpec, OutputCursor, PtySize};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::process::fake::FakeSpawner;
use tandem_tunnel::{HostOptions, OpenSpec, OriginOptions, Tunnels};

async fn terminal() -> (
    Tunnels,
    tandem_proto::TunnelId,
    tandem_proto::ProcessId,
    Arc<tandem_tunnel::process::fake::FakeProcess>,
    Tunnels,
) {
    let a = Tunnels::new(HostOptions::default(), OriginOptions::default());
    let b = Tunnels::new(HostOptions::default(), OriginOptions::default());
    let fake = FakeSpawner::new();
    b.host.set_spawner(Arc::new(fake.clone()));
    a.origin.set_link(Some(MemPath::new(&b.host).dialer()));
    let t = a
        .origin
        .open(OpenSpec {
            title: "bench".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
        })
        .await
        .expect("open");
    let p = a
        .origin
        .exec(
            t.id,
            ExecSpec {
                cmd: "sh".into(),
                pty: Some(PtySize::default()),
                ..ExecSpec::default()
            },
        )
        .await
        .expect("exec");
    let proc = fake.processes().pop().expect("launched");
    (a, t.id, p.id, proc, b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keystroke_echo_round_trip_stays_under_a_frame() {
    let (a, tunnel, process, fake, _b) = terminal().await;
    fake.echo(true);
    let mut cursor = OutputCursor::default();
    let mut rtts = Vec::new();
    for _ in 0..300 {
        let t0 = Instant::now();
        a.input(tunnel, process, b"k").expect("type");
        let out = a
            .wait_output(tunnel, process, cursor, 1024, Duration::from_secs(5))
            .await
            .expect("echo");
        assert_eq!(out.stdout, b"k", "one echo per key");
        cursor = out.next;
        rtts.push(t0.elapsed());
    }
    rtts.sort();
    let p50 = rtts[rtts.len() / 2];
    let p99 = rtts[rtts.len() * 99 / 100];
    eprintln!("keystroke echo over loopback: p50 {p50:?}, p99 {p99:?}");
    assert!(p50 < Duration::from_millis(5), "p50 {p50:?}");
    assert!(p99 < Duration::from_millis(16), "p99 {p99:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_streams_at_link_speed() {
    let (a, tunnel, process, fake, _b) = terminal().await;
    const TOTAL: usize = 64 * 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    // Below both ring buffers (8 MiB): nothing is discarded, everything is measured.
    const AHEAD: u64 = 4 * 1024 * 1024;
    let chunk = vec![b'x'; CHUNK];
    let t0 = Instant::now();
    let mut written = 0u64;
    let mut cursor = OutputCursor::default();
    while cursor.stdout < TOTAL as u64 {
        while written < TOTAL as u64 && written < cursor.stdout + AHEAD {
            fake.stdout(&chunk);
            written += CHUNK as u64;
        }
        let out = a
            .wait_output(tunnel, process, cursor, 400 * 1024, Duration::from_secs(5))
            .await
            .expect("output");
        assert_eq!(out.from, cursor, "nothing lost or repeated");
        cursor = out.next;
    }
    let secs = t0.elapsed().as_secs_f64();
    let mbps = TOTAL as f64 / secs / 1e6;
    eprintln!("terminal output over loopback: {mbps:.0} MB/s");
    assert!(mbps > 40.0, "{mbps:.0} MB/s");
}
