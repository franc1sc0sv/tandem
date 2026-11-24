//! Terminal processes over a tunnel: two agents in the same process (origin A, destination B)
//! joined by in-memory paths, with the process double. What the terminal view needs: what is
//! typed arrives in order and exactly once (also when a reply is lost or the link moves), the
//! output is pushed the moment it exists and as the exact continuation of what was already
//! pushed (a link switch never restarts it), and the input pause of a path does not freeze it.

use std::sync::Arc;
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::{Client, ClientKind, ExecSpec, OutputCursor, PtySize, TunnelId, TunnelProcess};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::process::fake::{FakeProcess, FakeSpawner};
use tandem_tunnel::{HostOptions, OpenSpec, OriginOptions, OutputFollower, TunnelError, Tunnels};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Rig {
    /// The origin machine's tunnels (A).
    a: Tunnels,
    /// The destination's (B).
    b: Tunnels,
    fake: FakeSpawner,
    tunnel: TunnelId,
    path: MemPath,
}

async fn rig() -> Rig {
    let opts = OriginOptions {
        retry: Duration::from_millis(5),
        ..OriginOptions::default()
    };
    let a = Tunnels::new(HostOptions::default(), opts);
    let b = Tunnels::new(HostOptions::default(), opts);
    let fake = FakeSpawner::new();
    b.host.set_spawner(Arc::new(fake.clone()));
    let path = MemPath::new(&b.host);
    a.origin.set_link(Some(path.dialer()));
    let t = a
        .origin
        .open(OpenSpec {
            title: "terminal".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
        })
        .await
        .expect("open the tunnel");
    Rig {
        a,
        b,
        fake,
        tunnel: t.id,
        path,
    }
}

impl Rig {
    async fn exec(&self, pty: Option<PtySize>) -> (TunnelProcess, Arc<FakeProcess>) {
        let p = self
            .a
            .origin
            .exec(
                self.tunnel,
                ExecSpec {
                    cmd: "pwsh".into(),
                    pty,
                    ..ExecSpec::default()
                },
            )
            .await
            .expect("launch");
        (
            p,
            self.fake.processes().pop().expect("the double launched it"),
        )
    }

    async fn terminal(&self) -> (TunnelProcess, Arc<FakeProcess>) {
        self.exec(Some(PtySize { cols: 80, rows: 24 })).await
    }

    /// A new path to B that becomes the active link (the previous one stays as it is).
    fn switch_link(&mut self) {
        self.path = MemPath::new(&self.b.host);
        self.a.origin.set_link(Some(self.path.dialer()));
    }
}

async fn within<T>(what: &str, f: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .unwrap_or_else(|_| panic!("did not happen in time: {what}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_process_says_so_and_types_in_order() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    assert!(p.pty, "the process runs in a terminal");
    assert_eq!(fake.launch.pty, Some(PtySize { cols: 80, rows: 24 }));
    for key in ["l", "s", " ", "-", "l", "a", "\r"] {
        rig.a.input(rig.tunnel, p.id, key.as_bytes()).expect("type");
    }
    assert_eq!(
        within("the keys arrive", fake.wait_typed(7)).await,
        b"ls -la\r"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_piped_process_takes_no_input() {
    let rig = rig().await;
    let (p, _) = rig.exec(None).await;
    assert!(!p.pty);
    assert_eq!(
        rig.a.input(rig.tunnel, p.id, b"x"),
        Err(TunnelError::NoTerminal(rig.tunnel, p.id))
    );
    assert!(rig.b.host.input(rig.tunnel, p.id, 0, b"x").is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_whose_ack_is_lost_is_resent_over_the_new_link_and_typed_once() {
    let mut rig = rig().await;
    let (p, fake) = rig.terminal().await;
    // B types it, but the acknowledgement dies with the path.
    rig.path.cut_before_reply();
    rig.a.input(rig.tunnel, p.id, b"echo one\r").expect("type");
    within("the path is cut", rig.path.wait_cut()).await;
    assert_eq!(fake.wait_typed(9).await, b"echo one\r");
    // More is typed while there is no link; all of it goes over the next one.
    rig.a.input(rig.tunnel, p.id, b"echo two\r").expect("type");
    rig.switch_link();
    let typed = within("the rest arrives", fake.wait_typed(18)).await;
    assert_eq!(typed, b"echo one\recho two\r", "nothing typed twice");
    within("nothing is left pending", async {
        while rig.a.origin.input_pending(rig.tunnel, p.id) > 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert_eq!(fake.typed(), b"echo one\recho two\r");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resend_from_an_older_position_types_only_what_is_new() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    let host = &rig.b.host;
    assert_eq!(host.input(rig.tunnel, p.id, 0, b"abc"), Ok(3));
    assert_eq!(host.input(rig.tunnel, p.id, 1, b"bcde"), Ok(5));
    assert_eq!(host.input(rig.tunnel, p.id, 0, b"ab"), Ok(5));
    assert!(
        host.input(rig.tunnel, p.id, 9, b"z").is_err(),
        "a gap is refused"
    );
    assert_eq!(within("typed", fake.wait_typed(5)).await, b"abcde");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resizing_reaches_the_terminal() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    let size = PtySize {
        cols: 200,
        rows: 50,
    };
    rig.a.resize(rig.tunnel, p.id, size).await.expect("resize");
    assert_eq!(fake.sizes().last(), Some(&size));
}

/// The bug behind "it only refreshes every second": nothing woke the reader when output arrived.
/// With the clock paused, the answer comes at the instant of the echo, not at the deadline.
#[tokio::test(start_paused = true)]
async fn waiting_for_output_answers_the_moment_the_echo_arrives() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    fake.echo(true);
    let started = tokio::time::Instant::now();
    let typing = {
        let a = rig.a.clone();
        let tunnel = rig.tunnel;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            a.input(tunnel, p.id, b"x").expect("type");
        })
    };
    let out = rig
        .a
        .wait_output(
            rig.tunnel,
            p.id,
            OutputCursor::default(),
            1024,
            Duration::from_secs(30),
        )
        .await
        .expect("output");
    typing.await.expect("typing task");
    assert_eq!(out.stdout, b"x");
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(150),
        "answered after {waited:?}, not when the echo arrived"
    );
}

/// The view is fed by a follower: each piece starts where the previous one ended, also across a
/// link switch, so the terminal view only appends and never redraws from zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_follower_continues_across_a_link_switch_without_repeating() {
    let mut rig = rig().await;
    let (p, fake) = rig.terminal().await;
    let mut follow = OutputFollower::new(rig.a.clone(), rig.tunnel, p.id, OutputCursor::default());
    fake.stdout(b"before ");
    let first = within("first piece", follow.next()).await.expect("output");
    assert_eq!(
        (first.from.stdout, first.stdout.as_slice()),
        (0, &b"before "[..])
    );

    rig.path.cut();
    fake.stdout(b"during the cut ");
    rig.switch_link();
    fake.stdout(b"after");
    let mut rest = Vec::new();
    let mut at = first.next.stdout;
    while rest.len() < b"during the cut after".len() {
        let o = within("the rest", follow.next()).await.expect("output");
        assert_eq!(o.from.stdout, at, "each piece continues the previous one");
        at = o.next.stdout;
        rest.extend_from_slice(&o.stdout);
    }
    assert_eq!(rest, b"during the cut after");
    fake.exit(0);
    let last = within("the end", follow.next())
        .await
        .expect("the last piece");
    assert!(last.done && last.stdout.is_empty());
    assert!(follow.next().await.is_none(), "nothing after the end");
}

/// Logs on the PC showed "data paused: input exceeded its target … after_ms=90000": the output
/// channel was bulk data, so a terminal froze for as long as the pause lasted. Process output must
/// keep flowing during the pause (bulk transfers still wait).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_output_flows_while_bulk_data_is_paused() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    rig.path.pause_data(true);
    fake.stdout(b"PS C:\\> ");
    let out = within(
        "the prompt arrives during the pause",
        rig.a
            .wait_output(rig.tunnel, p.id, OutputCursor::default(), 1024, TIMEOUT),
    )
    .await
    .expect("output");
    assert_eq!(out.stdout, b"PS C:\\> ");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_into_an_ended_process_is_accepted_and_dropped() {
    let rig = rig().await;
    let (p, fake) = rig.terminal().await;
    fake.exit(0);
    within("the end is known", async {
        loop {
            let ended = rig
                .a
                .processes(rig.tunnel)
                .and_then(|l| l.into_iter().find(|x| x.id == p.id))
                .is_some_and(|x| !x.is_running());
            if ended {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert_eq!(rig.a.input(rig.tunnel, p.id, b"x"), Ok(()));
    assert_eq!(rig.a.origin.input_pending(rig.tunnel, p.id), 0);
}
