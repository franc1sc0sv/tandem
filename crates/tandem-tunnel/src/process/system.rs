//! The real [`Spawner`]: `tokio::process`, with a process group of its own per process on
//! Unix and a job object on Windows, to be able to kill the whole tree.
//!
//! A descendant that deliberately leaves the group (`setsid`, a service that
//! detaches) escapes the kill on Unix; on Windows, job objects cannot be left.

use std::process::Stdio;
use std::sync::Arc;

use tokio::sync::watch;

use super::{Child, Launch, SpawnError, Spawned, Spawner};
use crate::wire::BoxFuture;

/// Launches real processes on this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn spawn(&self, launch: &Launch) -> Result<Spawned, SpawnError> {
        if let Some(size) = launch.pty {
            return super::pty::spawn(launch, size);
        }
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
                return Err(fail(&format!("could not group the process: {e}")));
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.start_kill();
            return Err(fail(&"could not open its output"));
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
            stdin: None,
            terminal: None,
            child: Arc::new(SystemChild { exit, tree }),
        })
    }
}

pub(super) struct SystemChild {
    pub(super) exit: watch::Receiver<Option<Option<i32>>>,
    pub(super) tree: tree::Tree,
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
pub(super) mod tree {
    use std::os::unix::process::CommandExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The child's process group (its pid, because it is the leader).
    pub struct Tree {
        pgid: Option<i32>,
        live: AtomicBool,
    }

    pub fn prepare(cmd: &mut std::process::Command) {
        cmd.process_group(0);
    }

    impl Tree {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Tree> {
            Ok(Tree::of_leader(child.id()))
        }

        /// The group led by `pid` (its own, or its session's after `setsid`).
        pub fn of_leader(pid: Option<u32>) -> Tree {
            // Without a pid (already reaped) there is no group to aim at: `killpg(0)` would be the
            // agent's own.
            let pgid = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1);
            Tree {
                pgid,
                live: AtomicBool::new(true),
            }
        }

        pub fn kill(&self) {
            let Some(pgid) = self.pgid else { return };
            if self.live.load(Ordering::SeqCst) {
                // SAFETY: `killpg` only receives integers; a group that no longer exists gives ESRCH,
                // which does not matter here.
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
pub(super) mod tree {
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

    /// A job object with the process inside: whatever it launches afterwards stays in it.
    pub struct Tree {
        job: Job,
        live: AtomicBool,
    }

    struct Job(HANDLE);

    // SAFETY: a job object HANDLE is a kernel identifier, valid from
    // any thread; it is only closed when `Job` is dropped.
    unsafe impl Send for Job {}
    // SAFETY: likewise: the kernel functions applied to it can be called concurrently.
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and is closed only once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// No console window: the agent runs without one and each `cmd` would open its own. Born
    /// suspended: [`Tree::attach`] puts it in the job object and only then
    /// resumes it, so no grandchild can launch before being inside.
    pub fn prepare(cmd: &mut std::process::Command) {
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }

    /// Resumes the threads of the newly created process (it only has the main one).
    fn resume(pid: u32) -> std::io::Result<()> {
        let err = |e: windows::core::Error| std::io::Error::other(e.to_string());
        // SAFETY: thread snapshot; the handle is closed below.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(err)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = false;
        // SAFETY: `snap` is valid and `entry` has `dwSize` set.
        let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opens the thread only with suspend/resume permission.
                if let Ok(thread) =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                {
                    // SAFETY: `thread` is valid; `u32::MAX` is ResumeThread's error.
                    resumed |= unsafe { ResumeThread(thread) } != u32::MAX;
                    // SAFETY: we opened it above.
                    let _ = unsafe { CloseHandle(thread) };
                }
            }
            // SAFETY: same as in the first call.
            more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
        }
        // SAFETY: `snap` was opened above and is not used afterwards.
        let _ = unsafe { CloseHandle(snap) };
        if resumed {
            Ok(())
        } else {
            Err(std::io::Error::other("could not resume the process"))
        }
    }

    impl Tree {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Tree> {
            let process = child.raw_handle().ok_or_else(|| {
                std::io::Error::other("the process already ended before being grouped")
            })?;
            let tree = Tree::adopt(HANDLE(process.cast::<c_void>()))?;
            resume(
                child
                    .id()
                    .ok_or_else(|| std::io::Error::other("the process already ended"))?,
            )?;
            Ok(tree)
        }

        /// Puts a process born suspended in a new job object; whoever calls resumes it after.
        pub fn adopt(process: HANDLE) -> std::io::Result<Tree> {
            // SAFETY: no attributes or name; returns a new handle or an error.
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let job = Job(job);
            // SAFETY: the caller keeps `process` open during the call.
            unsafe { AssignProcessToJobObject(job.0, process) }
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(Tree {
                job,
                live: AtomicBool::new(true),
            })
        }

        pub fn kill(&self) {
            if self.live.load(Ordering::SeqCst) {
                // SAFETY: the handle is of the job object that `self` keeps open.
                let _ = unsafe { TerminateJobObject(self.job.0, 1) };
            }
        }

        pub fn release(&self) {
            self.live.store(false, Ordering::SeqCst);
        }
    }
}
