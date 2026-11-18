//! The real launcher (`SystemSpawner`) on the three systems: simple processes, a large 10 MB output
//! and process trees that die completely (T-104).
//!
//! The test process is `src/bin/tandem-tunnel-probe.rs`: it does the same on Linux, macOS and
//! Windows without depending on each shell's `echo` or `sleep`.

use std::path::PathBuf;
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::{
    Client, ClientKind, ExecSpec, OutputCursor, ProcessId, ProcessState, TunnelId, TunnelProcess,
};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions};

const TIMEOUT: Duration = Duration::from_secs(60);

/// The same as the test process's `pattern`.
fn pattern(i: u64) -> u8 {
    u8::try_from((i.wrapping_mul(2_654_435_761) >> 13) & 0xff).unwrap_or(0)
}

fn probe() -> String {
    // Cargo builds the binary before the integration tests and gives its path.
    env!("CARGO_BIN_EXE_tandem-tunnel-probe").to_owned()
}

struct Rig {
    origin: Origin,
    host: Host,
    tunnel: TunnelId,
    _path: MemPath,
}

async fn rig(capacity: usize) -> Rig {
    let host = Host::new(
        HostOptions {
            capacity,
            ..HostOptions::default()
        },
        Changes::default(),
    );
    let origin = Origin::new(
        OriginOptions {
            capacity,
            retry: Duration::from_millis(5),
            ..OriginOptions::default()
        },
        Changes::default(),
    );
    let path = MemPath::new(&host);
    origin.set_link(Some(path.dialer()));
    let t = origin
        .open(OpenSpec {
            title: "system".into(),
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
        origin,
        host,
        tunnel: t.id,
        _path: path,
    }
}

impl Rig {
    async fn run(&self, args: &[&str]) -> TunnelProcess {
        self.exec(ExecSpec {
            cmd: probe(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            ..ExecSpec::default()
        })
        .await
    }

    async fn exec(&self, spec: ExecSpec) -> TunnelProcess {
        self.origin
            .exec(self.tunnel, spec)
            .await
            .expect("launch the process")
    }

    fn process(&self, id: ProcessId) -> TunnelProcess {
        self.origin
            .processes(self.tunnel)
            .and_then(|l| l.into_iter().find(|p| p.id == id))
            .expect("the process is known")
    }

    async fn until(&self, what: &str, mut done: impl FnMut(&Rig) -> bool) {
        let mut output = self.origin.subscribe_output();
        let wait = async {
            while !done(self) {
                // Wakes with the output or, failing that, every so often.
                let _ = tokio::time::timeout(Duration::from_millis(20), output.changed()).await;
            }
        };
        tokio::time::timeout(TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| panic!("did not happen in time: {what}"));
    }

    /// Waits for the process to end and all its output to arrive; returns it (stdout, stderr).
    async fn finished(&self, id: ProcessId) -> (Vec<u8>, Vec<u8>, TunnelProcess) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut cursor = OutputCursor::default();
        let mut output = self.origin.subscribe_output();
        let wait = async {
            loop {
                let o = self
                    .origin
                    .output(self.tunnel, id, cursor, usize::MAX)
                    .expect("read the output");
                assert_eq!(o.from, cursor, "nothing was lost");
                out.extend_from_slice(&o.stdout);
                err.extend_from_slice(&o.stderr);
                cursor = o.next;
                if o.done {
                    return o.process;
                }
                let _ = tokio::time::timeout(Duration::from_millis(20), output.changed()).await;
            }
        };
        let p = tokio::time::timeout(TIMEOUT, wait)
            .await
            .expect("the process did not end in time");
        (out, err, p)
    }

    fn flows_ended(&self, p: &TunnelProcess) -> bool {
        self.origin
            .flows(self.tunnel)
            .iter()
            .filter(|f| f.info.id == p.stdout || f.info.id == p.stderr)
            .all(|f| f.ended.is_some())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_simple_process_gives_its_output_its_code_and_its_duration() {
    let rig = rig(1 << 20).await;
    let p = rig.run(&["echo", "hello", "world"]).await;
    assert_eq!(p.state, ProcessState::Running);
    assert!(p.pid.is_some_and(|pid| pid > 0));
    let (out, err, p) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "hello world");
    assert!(err.is_empty());
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
    assert!(p.duration.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdout_and_stderr_go_separately_and_the_exit_code_arrives() {
    let rig = rig(1 << 20).await;
    let a = rig.run(&["err", "bad"]).await;
    let b = rig.run(&["exit", "7"]).await;
    let (out, err, p) = rig.finished(a.id).await;
    assert!(out.is_empty());
    assert_eq!(String::from_utf8_lossy(&err).trim_end(), "bad");
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
    let (_, _, p) = rig.finished(b.id).await;
    assert_eq!(p.state, ProcessState::Exited { code: Some(7) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_megabytes_of_output_arrive_complete_and_in_order() {
    const TOTAL: u64 = 10 * 1024 * 1024;
    let rig = rig(16 << 20).await;
    let p = rig.run(&["flood", &TOTAL.to_string()]).await;
    let (out, _, p) = rig.finished(p.id).await;
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
    assert_eq!(out.len() as u64, TOTAL);
    let wrong = out
        .iter()
        .enumerate()
        .position(|(i, b)| *b != pattern(i as u64));
    assert_eq!(wrong, None, "different first byte");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folder_and_the_variables_reach_the_process() {
    let rig = rig(1 << 20).await;
    let dir = tempfile::tempdir().expect("temporary folder");
    let p = rig
        .exec(ExecSpec {
            cmd: probe(),
            args: vec!["cwd".into()],
            cwd: Some(dir.path().display().to_string()),
            ..ExecSpec::default()
        })
        .await;
    let (out, _, _) = rig.finished(p.id).await;
    let got = PathBuf::from(String::from_utf8_lossy(&out).trim_end());
    assert_eq!(
        got.canonicalize().expect("process folder"),
        dir.path().canonicalize().expect("requested folder")
    );
    let p = rig
        .exec(ExecSpec {
            cmd: probe(),
            args: vec!["env".into(), "TANDEM_TEST".into()],
            env: vec![("TANDEM_TEST".into(), "value \u{fc}".into())],
            ..ExecSpec::default()
        })
        .await;
    let (out, _, _) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "value \u{fc}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_a_shell_the_line_is_interpreted_and_the_arguments_are_quoted() {
    let rig = rig(1 << 20).await;
    let p = rig
        .exec(ExecSpec {
            cmd: "echo $((1 + 2))".into(),
            args: vec!["a b".into(), "it's".into()],
            shell: Some("sh".into()),
            ..ExecSpec::default()
        })
        .await;
    let (out, _, p) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "3 a b it's");
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_cmd_and_with_powershell_the_line_is_interpreted() {
    let rig = rig(1 << 20).await;
    let p = rig
        .exec(ExecSpec {
            cmd: "echo %OS%".into(),
            shell: Some("cmd".into()),
            ..ExecSpec::default()
        })
        .await;
    let (out, _, _) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "Windows_NT");
    let p = rig
        .exec(ExecSpec {
            cmd: "Write-Output (1 + 2)".into(),
            shell: Some("powershell".into()),
            ..ExecSpec::default()
        })
        .await;
    let (out, _, _) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "3");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_cannot_be_launched_is_rejected_with_the_reason() {
    let rig = rig(1 << 20).await;
    let err = rig
        .origin
        .exec(
            rig.tunnel,
            ExecSpec {
                cmd: "tandem-this-program-does-not-exist".into(),
                ..ExecSpec::default()
            },
        )
        .await
        .expect_err("does not exist");
    assert!(
        err.to_string()
            .contains("tandem-this-program-does-not-exist"),
        "{err}"
    );
    assert!(
        rig.origin
            .processes(rig.tunnel)
            .is_some_and(|l| l.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killing_a_process_kills_its_whole_tree_and_does_not_touch_the_others() {
    let rig = rig(1 << 20).await;
    // The tree: the process and a grandchild with the same output open. While the grandchild lives,
    // the output does not end; that it ends proves the whole tree died.
    let tree = rig.run(&["tree"]).await;
    let sleeper = rig.run(&["sleep", "600000"]).await;
    rig.until("the tree started", |r| {
        r.origin
            .output(r.tunnel, tree.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout.starts_with(b"ready"))
    })
    .await;
    assert!(!rig.flows_ended(&tree));

    rig.origin.kill(rig.tunnel, tree.id).await.expect("kill");
    rig.until("the tree died", |r| r.flows_ended(&r.process(tree.id)))
        .await;
    let (_, _, p) = rig.finished(tree.id).await;
    assert_eq!(p.state, ProcessState::Killed);
    assert!(p.duration.is_some());

    // The other keeps running and is listed as such.
    assert_eq!(rig.process(sleeper.id).state, ProcessState::Running);
    let running: Vec<ProcessId> = rig.origin.views()[0].running.iter().map(|p| p.id).collect();
    assert_eq!(running, [sleeper.id]);

    rig.origin.kill(rig.tunnel, sleeper.id).await.expect("kill");
    let (_, _, p) = rig.finished(sleeper.id).await;
    assert_eq!(p.state, ProcessState::Killed);
    // Killing one that already ended is not an error; one that does not exist, is.
    rig.origin
        .kill(rig.tunnel, sleeper.id)
        .await
        .expect("idempotent");
    assert!(rig.origin.kill(rig.tunnel, ProcessId(99)).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_tunnel_kills_the_remaining_trees() {
    let rig = rig(1 << 20).await;
    let tree = rig.run(&["tree"]).await;
    // It ended on its own but left a grandchild with the output open: the process shows as ended and
    // its output stays open.
    let orphan = rig.run(&["orphan"]).await;
    rig.until("the orphan ended", |r| {
        r.process(orphan.id).state == ProcessState::Exited { code: Some(0) }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !rig.flows_ended(&orphan),
        "the grandchild keeps the output open"
    );
    assert!(
        rig.origin
            .output(rig.tunnel, orphan.id, OutputCursor::default(), 10)
            .is_ok_and(|o| !o.done)
    );

    rig.origin.close(rig.tunnel).await.expect("close");
    rig.until("everything ended", |r| {
        r.flows_ended(&r.process(tree.id)) && r.flows_ended(&r.process(orphan.id))
    })
    .await;
    assert_eq!(rig.process(tree.id).state, ProcessState::Killed);
    // Already closed, nothing more is launched.
    assert!(
        rig.host
            .exec(
                rig.tunnel,
                ExecSpec {
                    cmd: probe(),
                    args: vec!["echo".into()],
                    ..ExecSpec::default()
                }
            )
            .is_err()
    );
}
