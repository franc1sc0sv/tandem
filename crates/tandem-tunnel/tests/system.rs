//! El lanzador real (`SystemSpawner`) en los tres sistemas: procesos simples, salida grande
//! de 10 MB y árboles de procesos que mueren completos (T-104).
//!
//! El proceso de prueba es `src/bin/tandem-tunnel-probe.rs`: hace lo mismo en Linux, macOS y
//! Windows sin depender del `echo` o el `sleep` de cada shell.

use std::path::PathBuf;
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::{
    Client, ClientKind, ExecSpec, OutputCursor, ProcessId, ProcessState, TunnelId, TunnelProcess,
};
use tandem_tunnel::mem::MemPath;
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Lo mismo que `pattern` del proceso de prueba.
fn pattern(i: u64) -> u8 {
    u8::try_from((i.wrapping_mul(2_654_435_761) >> 13) & 0xff).unwrap_or(0)
}

fn probe() -> String {
    // Cargo construye el binario antes de las pruebas de integración y da su ruta.
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
            title: "sistema".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
        })
        .await
        .expect("abrir el túnel");
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
            .expect("lanzar el proceso")
    }

    fn process(&self, id: ProcessId) -> TunnelProcess {
        self.origin
            .processes(self.tunnel)
            .and_then(|l| l.into_iter().find(|p| p.id == id))
            .expect("el proceso se conoce")
    }

    async fn until(&self, what: &str, mut done: impl FnMut(&Rig) -> bool) {
        let mut output = self.origin.subscribe_output();
        let wait = async {
            while !done(self) {
                // Despierta con la salida o, en su defecto, cada tanto.
                let _ = tokio::time::timeout(Duration::from_millis(20), output.changed()).await;
            }
        };
        tokio::time::timeout(TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| panic!("no ocurrió a tiempo: {what}"));
    }

    /// Espera a que el proceso termine y toda su salida llegue; la devuelve (stdout, stderr).
    async fn finished(&self, id: ProcessId) -> (Vec<u8>, Vec<u8>, TunnelProcess) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut cursor = OutputCursor::default();
        let mut output = self.origin.subscribe_output();
        let wait = async {
            loop {
                let o = self
                    .origin
                    .output(self.tunnel, id, cursor, usize::MAX)
                    .expect("leer la salida");
                assert_eq!(o.from, cursor, "no se perdió nada");
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
            .expect("el proceso no terminó a tiempo");
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
async fn un_proceso_simple_da_su_salida_su_codigo_y_su_duracion() {
    let rig = rig(1 << 20).await;
    let p = rig.run(&["echo", "hola", "mundo"]).await;
    assert_eq!(p.state, ProcessState::Running);
    assert!(p.pid.is_some_and(|pid| pid > 0));
    let (out, err, p) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "hola mundo");
    assert!(err.is_empty());
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
    assert!(p.duration.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdout_y_stderr_van_por_separado_y_el_codigo_de_salida_llega() {
    let rig = rig(1 << 20).await;
    let a = rig.run(&["err", "mal"]).await;
    let b = rig.run(&["exit", "7"]).await;
    let (out, err, p) = rig.finished(a.id).await;
    assert!(out.is_empty());
    assert_eq!(String::from_utf8_lossy(&err).trim_end(), "mal");
    assert_eq!(p.state, ProcessState::Exited { code: Some(0) });
    let (_, _, p) = rig.finished(b.id).await;
    assert_eq!(p.state, ProcessState::Exited { code: Some(7) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diez_megabytes_de_salida_llegan_completos_y_en_orden() {
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
    assert_eq!(wrong, None, "primer byte distinto");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_carpeta_y_las_variables_llegan_al_proceso() {
    let rig = rig(1 << 20).await;
    let dir = tempfile::tempdir().expect("carpeta temporal");
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
        got.canonicalize().expect("carpeta del proceso"),
        dir.path().canonicalize().expect("carpeta pedida")
    );
    let p = rig
        .exec(ExecSpec {
            cmd: probe(),
            args: vec!["env".into(), "TANDEM_PRUEBA".into()],
            env: vec![("TANDEM_PRUEBA".into(), "valor ñ".into())],
            ..ExecSpec::default()
        })
        .await;
    let (out, _, _) = rig.finished(p.id).await;
    assert_eq!(String::from_utf8_lossy(&out).trim_end(), "valor ñ");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn con_un_shell_la_linea_se_interpreta_y_los_argumentos_van_citados() {
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
async fn con_cmd_y_con_powershell_la_linea_se_interpreta() {
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
async fn lo_que_no_se_puede_lanzar_se_rechaza_con_la_razon() {
    let rig = rig(1 << 20).await;
    let err = rig
        .origin
        .exec(
            rig.tunnel,
            ExecSpec {
                cmd: "tandem-no-existe-este-programa".into(),
                ..ExecSpec::default()
            },
        )
        .await
        .expect_err("no existe");
    assert!(
        err.to_string().contains("tandem-no-existe-este-programa"),
        "{err}"
    );
    assert!(
        rig.origin
            .processes(rig.tunnel)
            .is_some_and(|l| l.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matar_un_proceso_mata_a_todo_su_arbol_y_no_toca_a_los_demas() {
    let rig = rig(1 << 20).await;
    // El árbol: el proceso y un nieto con la misma salida abierta. Mientras el nieto viva,
    // la salida no termina; que termine prueba que el árbol entero murió.
    let tree = rig.run(&["tree"]).await;
    let sleeper = rig.run(&["sleep", "600000"]).await;
    rig.until("el árbol arrancó", |r| {
        r.origin
            .output(r.tunnel, tree.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout.starts_with(b"listo"))
    })
    .await;
    assert!(!rig.flows_ended(&tree));

    rig.origin.kill(rig.tunnel, tree.id).await.expect("matar");
    rig.until("el árbol murió", |r| r.flows_ended(&r.process(tree.id)))
        .await;
    let (_, _, p) = rig.finished(tree.id).await;
    assert_eq!(p.state, ProcessState::Killed);
    assert!(p.duration.is_some());

    // El otro sigue corriendo y se lista como tal.
    assert_eq!(rig.process(sleeper.id).state, ProcessState::Running);
    let running: Vec<ProcessId> = rig.origin.views()[0].running.iter().map(|p| p.id).collect();
    assert_eq!(running, [sleeper.id]);

    rig.origin
        .kill(rig.tunnel, sleeper.id)
        .await
        .expect("matar");
    let (_, _, p) = rig.finished(sleeper.id).await;
    assert_eq!(p.state, ProcessState::Killed);
    // Matar uno que ya terminó no es un error; uno que no existe, sí.
    rig.origin
        .kill(rig.tunnel, sleeper.id)
        .await
        .expect("idempotente");
    assert!(rig.origin.kill(rig.tunnel, ProcessId(99)).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cerrar_el_tunel_mata_los_arboles_que_quedaban() {
    let rig = rig(1 << 20).await;
    let tree = rig.run(&["tree"]).await;
    // Terminó solo pero dejó un nieto con la salida abierta: el proceso figura terminado y
    // su salida sigue abierta.
    let orphan = rig.run(&["orphan"]).await;
    rig.until("el huérfano terminó", |r| {
        r.process(orphan.id).state == ProcessState::Exited { code: Some(0) }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !rig.flows_ended(&orphan),
        "el nieto sigue con la salida abierta"
    );
    assert!(
        rig.origin
            .output(rig.tunnel, orphan.id, OutputCursor::default(), 10)
            .is_ok_and(|o| !o.done)
    );

    rig.origin.close(rig.tunnel).await.expect("cerrar");
    rig.until("todo terminó", |r| {
        r.flows_ended(&r.process(tree.id)) && r.flows_ended(&r.process(orphan.id))
    })
    .await;
    assert_eq!(rig.process(tree.id).state, ProcessState::Killed);
    // Ya cerrado, no se lanza nada más.
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
