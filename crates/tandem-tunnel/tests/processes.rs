//! Processes of a tunnel with the test double (T-104): two agents in the same process
//! (origin A, destination B) joined by in-memory paths. Five processes at once with
//! interleaved output, kill of one without affecting the others, and processes that continue while
//! the link is down.

use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use tandem_core::Machine;
use tandem_proto::{
    Client, ClientKind, ExecSpec, OutputCursor, ProcessId, ProcessState, TunnelId, TunnelProcess,
};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::process::fake::{FakeProcess, FakeSpawner};
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions, TunnelError};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Rig {
    host: Host,
    origin: Origin,
    fake: FakeSpawner,
    tunnel: TunnelId,
    path: MemPath,
}

async fn rig_with(host_opts: HostOptions, origin_opts: OriginOptions) -> Rig {
    let host = Host::new(host_opts, Changes::default());
    let fake = FakeSpawner::new();
    host.set_spawner(Arc::new(fake.clone()));
    let origin = Origin::new(origin_opts, Changes::default());
    let path = MemPath::new(&host);
    origin.set_link(Some(path.dialer()));
    let t = origin
        .open(OpenSpec {
            title: "processes".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
        })
        .await
        .expect("open the tunnel");
    Rig {
        host,
        origin,
        fake,
        tunnel: t.id,
        path,
    }
}

async fn rig() -> Rig {
    rig_with(
        HostOptions::default(),
        OriginOptions {
            retry: Duration::from_millis(5),
            ..OriginOptions::default()
        },
    )
    .await
}

fn spec(cmd: &str) -> ExecSpec {
    ExecSpec {
        cmd: cmd.into(),
        ..ExecSpec::default()
    }
}

impl Rig {
    async fn exec(&self, cmd: &str) -> (TunnelProcess, Arc<FakeProcess>) {
        let p = self
            .origin
            .exec(self.tunnel, spec(cmd))
            .await
            .expect("launch");
        let fake = self.fake.processes().pop().expect("the double launched it");
        (p, fake)
    }

    fn state(&self, id: ProcessId) -> ProcessState {
        self.origin
            .processes(self.tunnel)
            .and_then(|l| l.into_iter().find(|p| p.id == id))
            .map(|p| p.state)
            .expect("the process is known")
    }

    async fn until(&self, what: &str, mut done: impl FnMut(&Rig) -> bool) {
        let mut output = self.origin.subscribe_output();
        let wait = async {
            while !done(self) {
                let _ = tokio::time::timeout(Duration::from_millis(10), output.changed()).await;
            }
        };
        tokio::time::timeout(TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| panic!("did not happen in time: {what}"));
    }

    /// All the process's output, waiting for it to end.
    async fn drain(&self, id: ProcessId) -> (Vec<u8>, Vec<u8>) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut cursor = OutputCursor::default();
        self.until("the output ended", |r| {
            let o = r
                .origin
                .output(r.tunnel, id, cursor, 7)
                .expect("read the output");
            out.extend_from_slice(&o.stdout);
            err.extend_from_slice(&o.stderr);
            cursor = o.next;
            o.done
        })
        .await;
        (out, err)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_process_goes_from_running_to_ended_and_leaves_the_ui_list() {
    let rig = rig().await;
    let (p, fake) = rig.exec("cargo test").await;
    assert_eq!(p.command, "cargo test");
    assert_eq!(p.pid, Some(fake.pid));
    assert_eq!(p.state, ProcessState::Running);
    assert_eq!(fake.launch.program, "cargo test");
    assert_eq!(rig.origin.views()[0].running.len(), 1);
    assert_eq!(rig.host.views()[0].running.len(), 1);

    fake.stdout(b"compiling\n");
    fake.stderr(b"notice\n");
    fake.exit(101);
    let (out, err) = rig.drain(p.id).await;
    assert_eq!(
        (out.as_slice(), err.as_slice()),
        (&b"compiling\n"[..], &b"notice\n"[..])
    );
    let done = rig
        .origin
        .processes(rig.tunnel)
        .expect("tunnel")
        .pop()
        .expect("process");
    assert_eq!(done.state, ProcessState::Exited { code: Some(101) });
    assert!(done.duration.is_some());
    assert!(
        fake.is_released(),
        "with the output closed, the tree no longer remains"
    );
    // Out of the UI's list, but queryable.
    assert!(rig.origin.views()[0].running.is_empty());
    assert!(rig.host.views()[0].running.is_empty());
    assert_eq!(rig.origin.processes(rig.tunnel).expect("tunnel").len(), 1);
    assert_eq!(
        rig.host.get(rig.tunnel).expect("tunnel").processes().len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_processes_at_once_with_interleaved_output_and_kill_of_one() {
    let rig = rig().await;
    let mut procs = Vec::new();
    for i in 0..5 {
        procs.push(rig.exec(&format!("p{i}")).await);
    }
    // All running at once, each with its id.
    assert_eq!(rig.origin.views()[0].running.len(), 5);
    let ids: Vec<ProcessId> = procs.iter().map(|(p, _)| p.id).collect();
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        5
    );

    // The output of the five is interleaved, over stdout and stderr.
    let line = |i: usize, round: usize| format!("proceso {i} ronda {round}\n");
    for round in 0..3 {
        for (i, (_, fake)) in procs.iter().enumerate() {
            fake.stdout(line(i, round).as_bytes());
            fake.stderr(format!("e{i}.{round}\n").as_bytes());
        }
        tokio::task::yield_now().await;
    }

    // Kill the third: only it falls and its output ends.
    rig.origin.kill(rig.tunnel, ids[2]).await.expect("kill");
    assert!(procs[2].1.is_killed());
    for (i, (_, fake)) in procs.iter().enumerate() {
        assert_eq!(fake.is_killed(), i == 2, "kill of process {i}");
    }
    rig.until("the third shows as killed", |r| {
        r.state(ids[2]) == ProcessState::Killed
    })
    .await;
    assert_eq!(rig.origin.views()[0].running.len(), 4);

    // The others keep writing and end with their code.
    for round in 3..5 {
        for (i, (_, fake)) in procs.iter().enumerate() {
            if i != 2 {
                fake.stdout(line(i, round).as_bytes());
            }
        }
    }
    for (i, (_, fake)) in procs.iter().enumerate() {
        if i != 2 {
            fake.exit(i32::try_from(i).expect("small"));
        }
    }
    for (i, (p, _)) in procs.iter().enumerate() {
        let (out, err) = rig.drain(p.id).await;
        let rounds = if i == 2 { 0..3 } else { 0..5 };
        let want: String = rounds.clone().map(|r| line(i, r)).collect();
        let want_err: String = (0..3).map(|r| format!("e{i}.{r}\n")).collect();
        // The killed one keeps what it wrote before the kill; the others, everything.
        assert_eq!(String::from_utf8_lossy(&out), want, "stdout of process {i}");
        assert_eq!(
            String::from_utf8_lossy(&err),
            want_err,
            "stderr of process {i}"
        );
        let want_state = if i == 2 {
            ProcessState::Killed
        } else {
            ProcessState::Exited {
                code: Some(i32::try_from(i).expect("small")),
            }
        };
        assert_eq!(rig.state(p.id), want_state, "state of process {i}");
    }
    assert!(rig.origin.views()[0].running.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_continues_with_the_link_down_and_everything_arrives_on_return() {
    let rig = rig().await;
    let (p, fake) = rig.exec("long").await;
    fake.stdout(b"before\n");
    rig.until("the first arrived", |r| {
        r.origin
            .output(r.tunnel, p.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout == b"before\n")
    })
    .await;

    // The link drops: the process continues, writes and even ends.
    rig.path.cut();
    rig.origin.set_link(None);
    fake.stdout(b"during\n");
    fake.exit(0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.state(p.id),
        ProcessState::Running,
        "the origin does not know yet"
    );
    assert!(
        rig.origin.exec(rig.tunnel, spec("x")).await.is_err(),
        "without a link nothing is launched"
    );

    // It returns by another path: resumes from the last byte and finds out how it ended.
    let second = MemPath::new(&rig.host);
    rig.origin.set_link(Some(second.dialer()));
    let (out, _) = rig.drain(p.id).await;
    assert_eq!(out, b"before\nduring\n");
    assert_eq!(rig.state(p.id), ProcessState::Exited { code: Some(0) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_tunnel_kills_all_processes_and_nothing_more_is_launched() {
    let rig = rig().await;
    let (a, fake_a) = rig.exec("a").await;
    let (b, fake_b) = rig.exec("b").await;
    fake_b.exit(0);
    rig.until("b ended", |r| r.state(b.id) != ProcessState::Running)
        .await;
    rig.origin.close(rig.tunnel).await.expect("close");
    assert!(fake_a.is_killed());
    rig.until("a shows as killed", |r| {
        r.state(a.id) == ProcessState::Killed
    })
    .await;
    // The one that had already ended keeps its state.
    assert_eq!(rig.state(b.id), ProcessState::Exited { code: Some(0) });
    assert!(rig.host.exec(rig.tunnel, spec("c")).is_err());
    assert_eq!(rig.fake.processes().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_requests_are_rejected_with_their_reason() {
    let rig = rig().await;
    // No program.
    let e = rig
        .origin
        .exec(rig.tunnel, spec("  "))
        .await
        .expect_err("empty");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("program")));
    // An environment it does not know how to launch.
    let mut s = spec("ls");
    s.shell = Some("tcsh".into());
    let e = rig.origin.exec(rig.tunnel, s).await.expect_err("shell");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("tcsh")));
    // The launcher fails.
    rig.fake.fail_with(Some("no permission"));
    let e = rig
        .origin
        .exec(rig.tunnel, spec("ls"))
        .await
        .expect_err("fails");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("no permission")));
    rig.fake.fail_with(None);
    // Unknown tunnel and process.
    let e = rig
        .origin
        .exec(TunnelId(1), spec("ls"))
        .await
        .expect_err("tunnel");
    assert!(matches!(e, TunnelError::Unknown(_)));
    let e = rig
        .origin
        .kill(rig.tunnel, ProcessId(42))
        .await
        .expect_err("process");
    assert!(matches!(e, TunnelError::Refused(_)));
    assert!(matches!(
        rig.origin
            .output(rig.tunnel, ProcessId(42), OutputCursor::default(), 10),
        Err(TunnelError::UnknownProcess(..))
    ));
    // None of that left processes.
    assert!(rig.fake.processes().is_empty());
    assert!(rig.origin.processes(rig.tunnel).expect("tunnel").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ended_ones_are_forgotten_with_their_flows_after_the_time() {
    let linger = Duration::from_millis(80);
    let rig = rig_with(
        HostOptions {
            linger,
            ..HostOptions::default()
        },
        OriginOptions {
            linger,
            retry: Duration::from_millis(5),
            ..OriginOptions::default()
        },
    )
    .await;
    let (p, fake) = rig.exec("short").await;
    fake.stdout(b"hello");
    fake.exit(0);
    rig.drain(p.id).await;
    assert_eq!(rig.origin.flows(rig.tunnel).len(), 2);
    tokio::time::sleep(linger * 2).await;
    // Neither the origin nor the destination remember it, or its flows.
    assert!(rig.origin.processes(rig.tunnel).expect("tunnel").is_empty());
    assert!(rig.origin.flows(rig.tunnel).is_empty());
    assert!(
        rig.host
            .get(rig.tunnel)
            .expect("tunnel")
            .processes()
            .is_empty()
    );
    // One that keeps running is never forgotten.
    let (q, _) = rig.exec("long").await;
    tokio::time::sleep(linger * 2).await;
    assert_eq!(rig.state(q.id), ProcessState::Running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_output_in_parts_is_assembled_with_the_cursors() {
    let rig = rig().await;
    let (p, fake) = rig.exec("x").await;
    fake.stdout(b"0123456789");
    fake.stderr(b"abc");
    rig.until("it arrived", |r| {
        r.origin
            .output(r.tunnel, p.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout.len() == 10 && o.stderr.len() == 3)
    })
    .await;
    let first = rig
        .origin
        .output(rig.tunnel, p.id, OutputCursor::default(), 4)
        .expect("read");
    assert_eq!(
        (first.stdout.as_slice(), first.stderr.as_slice()),
        (&b"0123"[..], &b"abc"[..])
    );
    let second = rig
        .origin
        .output(rig.tunnel, p.id, first.next, 4)
        .expect("read");
    assert_eq!(second.from, first.next);
    assert_eq!(
        (second.stdout.as_slice(), second.stderr.as_slice()),
        (&b"4567"[..], &b""[..])
    );
    assert!(!second.done, "the process keeps running");
    // Asking beyond what there is gives nothing and does not advance.
    let ahead = OutputCursor {
        stdout: 99,
        stderr: 99,
    };
    let none = rig.origin.output(rig.tunnel, p.id, ahead, 4).expect("read");
    assert!(none.stdout.is_empty() && none.next == ahead);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// However the process writes and however the client reads, what is read is exactly what
    /// was written, and `done` is given only with everything read.
    #[test]
    fn what_is_read_is_what_was_written(
        writes in proptest::collection::vec((any::<bool>(), proptest::collection::vec(any::<u8>(), 0..40)), 0..30),
        max in 1usize..50,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let host = Host::new(HostOptions::default(), Changes::default());
            let fake = FakeSpawner::new();
            host.set_spawner(Arc::new(fake.clone()));
            let id = TunnelId(1);
            host.open(tandem_proto::Tunnel {
                id,
                title: "t".into(),
                project: None,
                origin: Machine::A,
                target: Machine::B,
                opened_by: Client { kind: ClientKind::Cli, session: "s".into() },
                state: tandem_proto::TunnelState::Open,
                created: 0,
            });
            let ht = host.get(id).expect("tunnel");
            let p = host.exec(id, spec("x")).expect("launch");
            let proc_ = fake.processes().pop().expect("process");
            let (mut want_out, mut want_err) = (Vec::new(), Vec::new());
            let (mut got_out, mut got_err) = (Vec::new(), Vec::new());
            let mut cursor = OutputCursor::default();
            let mut read = |cursor: &mut OutputCursor, finished: bool| {
                // With the process not yet ended, `done` cannot be true.
                let o = ht.output(p.id, *cursor, max).expect("read");
                got_out.extend_from_slice(&o.stdout);
                got_err.extend_from_slice(&o.stderr);
                *cursor = o.next;
                assert!(finished || !o.done);
                o.done
            };
            for (to_err, bytes) in &writes {
                if *to_err {
                    proc_.stderr(bytes);
                    want_err.extend_from_slice(bytes);
                } else {
                    proc_.stdout(bytes);
                    want_out.extend_from_slice(bytes);
                }
                tokio::task::yield_now().await;
                read(&mut cursor, false);
            }
            proc_.exit(0);
            let deadline = tokio::time::Instant::now() + TIMEOUT;
            while !read(&mut cursor, true) {
                assert!(tokio::time::Instant::now() < deadline, "did not end");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            prop_assert_eq!(got_out, want_out);
            prop_assert_eq!(got_err, want_err);
            Ok(())
        })?;
    }
}

#[tokio::test]
async fn the_default_folder_applies_to_local_shells_and_not_to_docker() {
    let r = rig().await;
    r.host
        .get(r.tunnel)
        .unwrap()
        .set_default_cwd("/data/workspaces/t");
    for (shell, cwd) in [
        (None, None),
        (Some("zsh"), None),
        (None, Some("/other")),
        (Some("docker:web"), None),
    ] {
        let spec = ExecSpec {
            cmd: "ls".into(),
            shell: shell.map(str::to_owned),
            cwd: cwd.map(str::to_owned),
            ..ExecSpec::default()
        };
        tokio::time::timeout(TIMEOUT, r.origin.exec(r.tunnel, spec))
            .await
            .unwrap()
            .unwrap();
    }
    let got: Vec<_> = r
        .fake
        .processes()
        .iter()
        .map(|p| p.launch.cwd.clone())
        .collect();
    let some = |s: &str| Some(s.to_owned());
    // On WSL (Windows only) and Docker the folder is set by the environment.
    assert_eq!(
        got,
        [
            some("/data/workspaces/t"),
            some("/data/workspaces/t"),
            some("/other"),
            None
        ]
    );
}
