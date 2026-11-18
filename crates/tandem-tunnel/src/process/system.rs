//! El [`Spawner`] real: `tokio::process`, con un grupo de procesos propio por proceso en
//! Unix y un objeto de trabajo (job object) en Windows, para poder matar el árbol entero.
//!
//! Un descendiente que se sale a propósito del grupo (`setsid`, un servicio que se
//! desacopla) escapa del kill en Unix; en Windows, de los objetos de trabajo no se sale.

use std::process::Stdio;
use std::sync::Arc;

use tokio::sync::watch;

use super::{Child, Launch, SpawnError, Spawned, Spawner};
use crate::wire::BoxFuture;

/// Lanza procesos de verdad en esta máquina.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn spawn(&self, launch: &Launch) -> Result<Spawned, SpawnError> {
        let fail = |e: &dyn std::fmt::Display| SpawnError(format!("{}: {e}", launch.program));
        let mut cmd = std::process::Command::new(&launch.program);
        cmd.args(&launch.args);
        #[cfg(windows)]
        if let Some(raw) = &launch.raw {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(raw);
        }
        if let Some(cwd) = &launch.cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(launch.env.iter().map(|(k, v)| (k, v)));
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tree::prepare(&mut cmd);

        let mut child = tokio::process::Command::from(cmd)
            .spawn()
            .map_err(|e| fail(&e))?;
        let pid = child.id();
        let tree = match tree::Tree::attach(&child) {
            Ok(t) => t,
            Err(e) => {
                let _ = child.start_kill();
                return Err(fail(&format!("no se pudo agrupar el proceso: {e}")));
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.start_kill();
            return Err(fail(&"no se pudo abrir su salida"));
        };
        let (tx, exit) = watch::channel(None);
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            let _ = tx.send(Some(code));
        });
        Ok(Spawned {
            pid,
            stdout: Box::new(stdout),
            stderr: Box::new(stderr),
            child: Arc::new(SystemChild { exit, tree }),
        })
    }
}

struct SystemChild {
    exit: watch::Receiver<Option<Option<i32>>>,
    tree: tree::Tree,
}

impl Child for SystemChild {
    fn wait(&self) -> BoxFuture<'_, Option<i32>> {
        let mut rx = self.exit.clone();
        Box::pin(async move {
            match rx.wait_for(Option::is_some).await {
                Ok(v) => (*v).flatten(),
                Err(_) => None,
            }
        })
    }

    fn kill_tree(&self) {
        self.tree.kill();
    }

    fn release(&self) {
        self.tree.release();
    }
}

#[cfg(unix)]
mod tree {
    use std::os::unix::process::CommandExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// El grupo de procesos del hijo (su pid, porque es el líder).
    pub struct Tree {
        pgid: Option<i32>,
        live: AtomicBool,
    }

    pub fn prepare(cmd: &mut std::process::Command) {
        cmd.process_group(0);
    }

    impl Tree {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Tree> {
            // Sin pid (ya lo recogieron) no hay grupo al que apuntar: `killpg(0)` sería el
            // del propio agente.
            let pgid = child
                .id()
                .and_then(|p| i32::try_from(p).ok())
                .filter(|p| *p > 1);
            Ok(Tree {
                pgid,
                live: AtomicBool::new(true),
            })
        }

        pub fn kill(&self) {
            let Some(pgid) = self.pgid else { return };
            if self.live.load(Ordering::SeqCst) {
                // SAFETY: `killpg` solo recibe enteros; un grupo que ya no existe da ESRCH,
                // que aquí no importa.
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        }

        pub fn release(&self) {
            self.live.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(windows)]
mod tree {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    use windows::core::PCWSTR;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;

    /// Un objeto de trabajo con el proceso adentro: lo que lance después queda en él.
    pub struct Tree {
        job: Job,
        live: AtomicBool,
    }

    struct Job(HANDLE);

    // SAFETY: un HANDLE de objeto de trabajo es un identificador del núcleo, válido desde
    // cualquier hilo; solo se cierra al soltar `Job`.
    unsafe impl Send for Job {}
    // SAFETY: igual: las funciones del núcleo que se le aplican se pueden llamar a la vez.
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: el identificador es nuestro y se cierra una sola vez.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// Sin ventana de consola: el agente corre sin una y cada `cmd` abriría la suya. Nace
    /// suspendido: [`Tree::attach`] lo mete en el objeto de trabajo y solo entonces lo
    /// reanuda, así que ningún nieto puede lanzarse antes de quedar dentro.
    pub fn prepare(cmd: &mut std::process::Command) {
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }

    /// Reanuda los hilos del proceso recién creado (solo tiene el principal).
    fn resume(pid: u32) -> std::io::Result<()> {
        let err = |e: windows::core::Error| std::io::Error::other(e.to_string());
        // SAFETY: instantánea de hilos; el identificador se cierra abajo.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(err)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = false;
        // SAFETY: `snap` es válido y `entry` tiene `dwSize` puesto.
        let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: abre el hilo solo con permiso de suspender/reanudar.
                if let Ok(thread) =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                {
                    // SAFETY: `thread` es válido; `u32::MAX` es el error de ResumeThread.
                    resumed |= unsafe { ResumeThread(thread) } != u32::MAX;
                    // SAFETY: lo abrimos arriba.
                    let _ = unsafe { CloseHandle(thread) };
                }
            }
            // SAFETY: igual que en la primera llamada.
            more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
        }
        // SAFETY: `snap` se abrió arriba y no se usa después.
        let _ = unsafe { CloseHandle(snap) };
        if resumed {
            Ok(())
        } else {
            Err(std::io::Error::other("no se pudo reanudar el proceso"))
        }
    }

    impl Tree {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Tree> {
            let process = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("el proceso ya terminó antes de agruparlo"))?;
            // SAFETY: sin atributos ni nombre; devuelve un identificador nuevo o un error.
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let job = Job(job);
            // SAFETY: `process` es el identificador del hijo, que `child` mantiene abierto.
            unsafe { AssignProcessToJobObject(job.0, HANDLE(process.cast::<c_void>())) }
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            resume(
                child
                    .id()
                    .ok_or_else(|| std::io::Error::other("el proceso ya terminó"))?,
            )?;
            Ok(Tree {
                job,
                live: AtomicBool::new(true),
            })
        }

        pub fn kill(&self) {
            if self.live.load(Ordering::SeqCst) {
                // SAFETY: el identificador es del objeto de trabajo que `self` mantiene abierto.
                let _ = unsafe { TerminateJobObject(self.job.0, 1) };
            }
        }

        pub fn release(&self) {
            self.live.store(false, Ordering::SeqCst);
        }
    }
}
