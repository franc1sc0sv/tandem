//! Procesos de un túnel con el doble de prueba (T-104): dos agentes en el mismo proceso
//! (origen A, destino B) unidos por caminos en memoria. Cinco procesos a la vez con la
//! salida intercalada, kill de uno sin afectar a los demás, y procesos que siguen mientras
//! el enlace está caído.

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
            title: "procesos".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
        })
        .await
        .expect("abrir el túnel");
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
            .expect("lanzar");
        let fake = self.fake.processes().pop().expect("el doble lo lanzó");
        (p, fake)
    }

    fn state(&self, id: ProcessId) -> ProcessState {
        self.origin
            .processes(self.tunnel)
            .and_then(|l| l.into_iter().find(|p| p.id == id))
            .map(|p| p.state)
            .expect("el proceso se conoce")
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
            .unwrap_or_else(|_| panic!("no ocurrió a tiempo: {what}"));
    }

    /// Toda la salida del proceso, esperando a que termine.
    async fn drain(&self, id: ProcessId) -> (Vec<u8>, Vec<u8>) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut cursor = OutputCursor::default();
        self.until("la salida terminó", |r| {
            let o = r
                .origin
                .output(r.tunnel, id, cursor, 7)
                .expect("leer la salida");
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
async fn un_proceso_pasa_de_corriendo_a_terminado_y_sale_de_la_lista_de_la_ui() {
    let rig = rig().await;
    let (p, fake) = rig.exec("cargo test").await;
    assert_eq!(p.command, "cargo test");
    assert_eq!(p.pid, Some(fake.pid));
    assert_eq!(p.state, ProcessState::Running);
    assert_eq!(fake.launch.program, "cargo test");
    assert_eq!(rig.origin.views()[0].running.len(), 1);
    assert_eq!(rig.host.views()[0].running.len(), 1);

    fake.stdout(b"compilando\n");
    fake.stderr(b"aviso\n");
    fake.exit(101);
    let (out, err) = rig.drain(p.id).await;
    assert_eq!(
        (out.as_slice(), err.as_slice()),
        (&b"compilando\n"[..], &b"aviso\n"[..])
    );
    let done = rig
        .origin
        .processes(rig.tunnel)
        .expect("túnel")
        .pop()
        .expect("proceso");
    assert_eq!(done.state, ProcessState::Exited { code: Some(101) });
    assert!(done.duration.is_some());
    assert!(
        fake.is_released(),
        "con la salida cerrada, el árbol ya no queda"
    );
    // Fuera de la lista de la UI, pero consultable.
    assert!(rig.origin.views()[0].running.is_empty());
    assert!(rig.host.views()[0].running.is_empty());
    assert_eq!(rig.origin.processes(rig.tunnel).expect("túnel").len(), 1);
    assert_eq!(
        rig.host.get(rig.tunnel).expect("túnel").processes().len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cinco_procesos_a_la_vez_con_salida_intercalada_y_kill_de_uno() {
    let rig = rig().await;
    let mut procs = Vec::new();
    for i in 0..5 {
        procs.push(rig.exec(&format!("p{i}")).await);
    }
    // Todos corriendo a la vez, cada uno con su id.
    assert_eq!(rig.origin.views()[0].running.len(), 5);
    let ids: Vec<ProcessId> = procs.iter().map(|(p, _)| p.id).collect();
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        5
    );

    // La salida de los cinco se intercala, por stdout y por stderr.
    let line = |i: usize, round: usize| format!("proceso {i} ronda {round}\n");
    for round in 0..3 {
        for (i, (_, fake)) in procs.iter().enumerate() {
            fake.stdout(line(i, round).as_bytes());
            fake.stderr(format!("e{i}.{round}\n").as_bytes());
        }
        tokio::task::yield_now().await;
    }

    // Matar el tercero: solo él cae y su salida termina.
    rig.origin.kill(rig.tunnel, ids[2]).await.expect("matar");
    assert!(procs[2].1.is_killed());
    for (i, (_, fake)) in procs.iter().enumerate() {
        assert_eq!(fake.is_killed(), i == 2, "kill del proceso {i}");
    }
    rig.until("el tercero figura matado", |r| {
        r.state(ids[2]) == ProcessState::Killed
    })
    .await;
    assert_eq!(rig.origin.views()[0].running.len(), 4);

    // Los demás siguen escribiendo y terminan con su código.
    for round in 3..5 {
        for (i, (_, fake)) in procs.iter().enumerate() {
            if i != 2 {
                fake.stdout(line(i, round).as_bytes());
            }
        }
    }
    for (i, (_, fake)) in procs.iter().enumerate() {
        if i != 2 {
            fake.exit(i32::try_from(i).expect("chico"));
        }
    }
    for (i, (p, _)) in procs.iter().enumerate() {
        let (out, err) = rig.drain(p.id).await;
        let rounds = if i == 2 { 0..3 } else { 0..5 };
        let want: String = rounds.clone().map(|r| line(i, r)).collect();
        let want_err: String = (0..3).map(|r| format!("e{i}.{r}\n")).collect();
        // El matado conserva lo que escribió antes del kill; los otros, todo.
        assert_eq!(
            String::from_utf8_lossy(&out),
            want,
            "stdout del proceso {i}"
        );
        assert_eq!(
            String::from_utf8_lossy(&err),
            want_err,
            "stderr del proceso {i}"
        );
        let want_state = if i == 2 {
            ProcessState::Killed
        } else {
            ProcessState::Exited {
                code: Some(i32::try_from(i).expect("chico")),
            }
        };
        assert_eq!(rig.state(p.id), want_state, "estado del proceso {i}");
    }
    assert!(rig.origin.views()[0].running.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_proceso_sigue_con_el_enlace_caido_y_al_volver_llega_todo() {
    let rig = rig().await;
    let (p, fake) = rig.exec("larga").await;
    fake.stdout(b"antes\n");
    rig.until("llegó lo primero", |r| {
        r.origin
            .output(r.tunnel, p.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout == b"antes\n")
    })
    .await;

    // Se cae el enlace: el proceso sigue, escribe y hasta termina.
    rig.path.cut();
    rig.origin.set_link(None);
    fake.stdout(b"durante\n");
    fake.exit(0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.state(p.id),
        ProcessState::Running,
        "el origen aún no sabe"
    );
    assert!(
        rig.origin.exec(rig.tunnel, spec("x")).await.is_err(),
        "sin enlace no se lanza nada"
    );

    // Vuelve por otro camino: retoma desde el último byte y se entera de cómo terminó.
    let second = MemPath::new(&rig.host);
    rig.origin.set_link(Some(second.dialer()));
    let (out, _) = rig.drain(p.id).await;
    assert_eq!(out, b"antes\ndurante\n");
    assert_eq!(rig.state(p.id), ProcessState::Exited { code: Some(0) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cerrar_el_tunel_mata_todos_los_procesos_y_ya_no_se_lanza_nada() {
    let rig = rig().await;
    let (a, fake_a) = rig.exec("a").await;
    let (b, fake_b) = rig.exec("b").await;
    fake_b.exit(0);
    rig.until("b terminó", |r| r.state(b.id) != ProcessState::Running)
        .await;
    rig.origin.close(rig.tunnel).await.expect("cerrar");
    assert!(fake_a.is_killed());
    rig.until("a figura matado", |r| r.state(a.id) == ProcessState::Killed)
        .await;
    // El que ya había terminado conserva su estado.
    assert_eq!(rig.state(b.id), ProcessState::Exited { code: Some(0) });
    assert!(rig.host.exec(rig.tunnel, spec("c")).is_err());
    assert_eq!(rig.fake.processes().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn los_pedidos_invalidos_se_rechazan_con_su_razon() {
    let rig = rig().await;
    // Sin programa.
    let e = rig
        .origin
        .exec(rig.tunnel, spec("  "))
        .await
        .expect_err("vacío");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("programa")));
    // Un entorno que no se sabe lanzar.
    let mut s = spec("ls");
    s.shell = Some("tcsh".into());
    let e = rig.origin.exec(rig.tunnel, s).await.expect_err("shell");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("tcsh")));
    // El lanzador falla.
    rig.fake.fail_with(Some("sin permiso"));
    let e = rig
        .origin
        .exec(rig.tunnel, spec("ls"))
        .await
        .expect_err("falla");
    assert!(matches!(e, TunnelError::Refused(why) if why.contains("sin permiso")));
    rig.fake.fail_with(None);
    // Túnel y proceso desconocidos.
    let e = rig
        .origin
        .exec(TunnelId(1), spec("ls"))
        .await
        .expect_err("túnel");
    assert!(matches!(e, TunnelError::Unknown(_)));
    let e = rig
        .origin
        .kill(rig.tunnel, ProcessId(42))
        .await
        .expect_err("proceso");
    assert!(matches!(e, TunnelError::Refused(_)));
    assert!(matches!(
        rig.origin
            .output(rig.tunnel, ProcessId(42), OutputCursor::default(), 10),
        Err(TunnelError::UnknownProcess(..))
    ));
    // Nada de eso dejó procesos.
    assert!(rig.fake.processes().is_empty());
    assert!(rig.origin.processes(rig.tunnel).expect("túnel").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn los_terminados_se_olvidan_con_sus_flujos_pasado_el_tiempo() {
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
    let (p, fake) = rig.exec("corta").await;
    fake.stdout(b"hola");
    fake.exit(0);
    rig.drain(p.id).await;
    assert_eq!(rig.origin.flows(rig.tunnel).len(), 2);
    tokio::time::sleep(linger * 2).await;
    // Ni el origen ni la destino lo recuerdan, ni a sus flujos.
    assert!(rig.origin.processes(rig.tunnel).expect("túnel").is_empty());
    assert!(rig.origin.flows(rig.tunnel).is_empty());
    assert!(
        rig.host
            .get(rig.tunnel)
            .expect("túnel")
            .processes()
            .is_empty()
    );
    // Uno que sigue corriendo no se olvida nunca.
    let (q, _) = rig.exec("larga").await;
    tokio::time::sleep(linger * 2).await;
    assert_eq!(rig.state(q.id), ProcessState::Running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_salida_por_partes_se_arma_con_los_cursores() {
    let rig = rig().await;
    let (p, fake) = rig.exec("x").await;
    fake.stdout(b"0123456789");
    fake.stderr(b"abc");
    rig.until("llegó", |r| {
        r.origin
            .output(r.tunnel, p.id, OutputCursor::default(), 100)
            .is_ok_and(|o| o.stdout.len() == 10 && o.stderr.len() == 3)
    })
    .await;
    let first = rig
        .origin
        .output(rig.tunnel, p.id, OutputCursor::default(), 4)
        .expect("leer");
    assert_eq!(
        (first.stdout.as_slice(), first.stderr.as_slice()),
        (&b"0123"[..], &b"abc"[..])
    );
    let second = rig
        .origin
        .output(rig.tunnel, p.id, first.next, 4)
        .expect("leer");
    assert_eq!(second.from, first.next);
    assert_eq!(
        (second.stdout.as_slice(), second.stderr.as_slice()),
        (&b"4567"[..], &b""[..])
    );
    assert!(!second.done, "el proceso sigue corriendo");
    // Pedir más allá de lo que hay no da nada ni se adelanta.
    let ahead = OutputCursor {
        stdout: 99,
        stderr: 99,
    };
    let none = rig.origin.output(rig.tunnel, p.id, ahead, 4).expect("leer");
    assert!(none.stdout.is_empty() && none.next == ahead);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Escriba como escriba el proceso y lea como lea el cliente, lo leído es exactamente lo
    /// escrito, y `done` solo se da con todo leído.
    #[test]
    fn lo_leido_es_lo_escrito(
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
            let ht = host.get(id).expect("túnel");
            let p = host.exec(id, spec("x")).expect("lanzar");
            let proc_ = fake.processes().pop().expect("proceso");
            let (mut want_out, mut want_err) = (Vec::new(), Vec::new());
            let (mut got_out, mut got_err) = (Vec::new(), Vec::new());
            let mut cursor = OutputCursor::default();
            let mut read = |cursor: &mut OutputCursor, finished: bool| {
                // Con el proceso aún sin terminar, `done` no puede ser verdadero.
                let o = ht.output(p.id, *cursor, max).expect("leer");
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
                assert!(tokio::time::Instant::now() < deadline, "no terminó");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            prop_assert_eq!(got_out, want_out);
            prop_assert_eq!(got_err, want_err);
            Ok(())
        })?;
    }
}
