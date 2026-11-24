//! Processes in a pseudo-terminal ([`Launch::pty`]): `openpty` on Unix, ConPTY on Windows.
//!
//! The program sees a real terminal: colors, progress bars, line editing and line-buffered
//! output instead of the block buffering it uses on a pipe (which is why the output of a piped
//! process can arrive late and in bursts). Its stdout and stderr both go to the terminal; the
//! master side is read and written by two plain threads, which is portable (ConPTY only
//! has synchronous pipes) and hands each read over in microseconds.
//!
//! The tree is killed as for piped processes: the session's process group on Unix (the child
//! is its leader after `setsid`) and a job object on Windows, entered before the process runs.

use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tandem_proto::PtySize;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use super::{Launch, SpawnError, Spawned};

/// What a terminal process gets when nothing says otherwise: the UI and the CLI render it as
/// an xterm.
const TERM: &str = "xterm-256color";

pub(super) fn spawn(launch: &Launch, size: PtySize) -> Result<Spawned, SpawnError> {
    imp::spawn(launch, size)
}

fn with_term(env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = env.to_vec();
    if !env.iter().any(|(k, _)| k == "TERM") {
        env.push(("TERM".into(), TERM.into()));
    }
    env
}

/// The master side's output, read by a thread of its own; it ends when the terminal closes.
struct ThreadReader {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pending: Vec<u8>,
    at: usize,
}

impl ThreadReader {
    fn start(mut from: impl Read + Send + 'static) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                // EIO on Linux once the last slave closes, 0 on macOS and Windows: both are the end.
                match from.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        ThreadReader {
            rx,
            pending: Vec::new(),
            at: 0,
        }
    }
}

impl AsyncRead for ThreadReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.at < self.pending.len() {
                let n = buf.remaining().min(self.pending.len() - self.at);
                let at = self.at;
                buf.put_slice(&self.pending[at..at + n]);
                self.at += n;
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.pending = chunk;
                    self.at = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// The master side's input, written by a thread of its own in the order it was given.
struct ThreadWriter {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
}

impl ThreadWriter {
    fn start(mut to: impl Write + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if to.write_all(&bytes).and_then(|()| to.flush()).is_err() {
                    break;
                }
            }
        });
        ThreadWriter { tx }
    }
}

impl AsyncWrite for ThreadWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.tx.send(buf.to_vec()) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn no_stderr() -> Box<dyn AsyncRead + Send + Unpin> {
    Box::new(tokio::io::empty())
}

#[cfg(unix)]
mod imp {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    use tokio::sync::watch;

    use super::super::Terminal;
    use super::super::system::{SystemChild, tree};
    use super::*;

    fn winsize(size: PtySize) -> libc::winsize {
        libc::winsize {
            ws_row: size.rows.max(1),
            ws_col: size.cols.max(1),
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    struct Master(OwnedFd);

    impl Terminal for Master {
        fn resize(&self, size: PtySize) -> Result<(), SpawnError> {
            let ws = winsize(size);
            // SAFETY: `TIOCSWINSZ` reads a `winsize` from the pointer, which lives for the call;
            // the descriptor is the master, open while `self` lives.
            let r = unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
            if r == 0 {
                Ok(())
            } else {
                Err(SpawnError(format!(
                    "could not resize the terminal: {}",
                    std::io::Error::last_os_error()
                )))
            }
        }
    }

    /// A new pseudo-terminal pair `(master, slave)`.
    fn open(size: PtySize) -> std::io::Result<(OwnedFd, OwnedFd)> {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut ws = winsize(size);
        // SAFETY: `openpty` writes two descriptors into the integers we pass; name and termios
        // are optional (null) and `ws` lives for the call.
        let r = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(ws),
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openpty` succeeded, so both are open descriptors that nobody else owns.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        for fd in [&master, &slave] {
            // SAFETY: plain flag change on a descriptor we own: neither leaks into other children.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        Ok((master, slave))
    }

    pub(super) fn spawn(launch: &Launch, size: PtySize) -> Result<Spawned, SpawnError> {
        let fail = |e: &dyn std::fmt::Display| SpawnError(format!("{}: {e}", launch.program));
        let (master, slave) = open(size).map_err(|e| fail(&e))?;
        let dup = |fd: &OwnedFd| fd.try_clone().map_err(|e| fail(&e));
        let mut cmd = std::process::Command::new(&launch.program);
        cmd.args(&launch.args);
        if let Some(cwd) = &launch.cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(with_term(&launch.env));
        cmd.stdin(Stdio::from(dup(&slave)?))
            .stdout(Stdio::from(dup(&slave)?))
            .stderr(Stdio::from(dup(&slave)?));
        // SAFETY: between fork and exec only async-signal-safe calls: a new session (so the
        // terminal can become its controlling one and the child leads the group that is killed)
        // and the controlling terminal, which is stdin.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = tokio::process::Command::from(cmd)
            .spawn()
            .map_err(|e| fail(&e))?;
        // Only the child keeps the slave: when it (and what it left running) closes it, reading
        // the master ends.
        drop(slave);
        let pid = child.id();
        let tree = tree::Tree::of_leader(pid);
        let reader = std::fs::File::from(dup(&master)?);
        let writer = std::fs::File::from(dup(&master)?);
        let (tx, exit) = watch::channel(None);
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            let _ = tx.send(Some(code));
        });
        Ok(Spawned {
            pid,
            stdout: Box::new(ThreadReader::start(reader)),
            stderr: no_stderr(),
            stdin: Some(Box::new(ThreadWriter::start(writer))),
            terminal: Some(Arc::new(Master(master))),
            child: Arc::new(SystemChild { exit, tree }),
        })
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};

    use tokio::sync::watch;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows::Win32::System::Console::{
        COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::super::Terminal;
    use super::super::system::{SystemChild, tree};
    use super::*;

    fn coord(size: PtySize) -> COORD {
        let clamp = |v: u16| i16::try_from(v.max(1)).unwrap_or(i16::MAX);
        COORD {
            X: clamp(size.cols),
            Y: clamp(size.rows),
        }
    }

    /// The pseudo-console. Closing it ends the output pipe once its last bytes were read: it is
    /// closed when the process ends, even if someone still holds it to resize.
    struct Console(std::sync::Mutex<Option<HPCON>>);

    // SAFETY: an HPCON is an opaque identifier that the console functions accept from any thread.
    unsafe impl Send for Console {}
    // SAFETY: likewise, and every use goes through the mutex.
    unsafe impl Sync for Console {}

    impl Console {
        fn close(&self) {
            let taken = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(h) = taken {
                // SAFETY: created by `CreatePseudoConsole` and taken out, so closed only once.
                unsafe { ClosePseudoConsole(h) };
            }
        }
    }

    impl Drop for Console {
        fn drop(&mut self) {
            self.close();
        }
    }

    impl Terminal for Console {
        fn resize(&self, size: PtySize) -> Result<(), SpawnError> {
            let h = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(h) = *h else {
                return Err(SpawnError("the terminal already closed".into()));
            };
            // SAFETY: the console is open while the lock is held.
            unsafe { ResizePseudoConsole(h, coord(size)) }
                .map_err(|e| SpawnError(format!("could not resize the terminal: {e}")))
        }
    }

    struct Process(HANDLE);

    // SAFETY: a process HANDLE is a kernel identifier valid from any thread; closed on drop.
    unsafe impl Send for Process {}
    // SAFETY: waiting on it and reading its exit code can happen concurrently.
    unsafe impl Sync for Process {}

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: ours, closed only here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// One argument quoted by the rules of `CommandLineToArgvW` (those of the C runtime).
    fn quote(arg: &str, out: &mut String) {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
            out.push_str(arg);
            return;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }

    fn command_line(launch: &Launch) -> String {
        let mut line = String::new();
        quote(&launch.program, &mut line);
        for a in &launch.args {
            line.push(' ');
            quote(a, &mut line);
        }
        if let Some(raw) = &launch.raw {
            line.push(' ');
            line.push_str(raw);
        }
        line
    }

    /// The agent's environment plus the launch's, as `CreateProcessW` takes it (sorted, UTF-16).
    fn environment(extra: &[(String, String)]) -> Vec<u16> {
        let mut vars: Vec<(String, String)> = std::env::vars_os()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        for (k, v) in extra {
            vars.retain(|(old, _)| !old.eq_ignore_ascii_case(k));
            vars.push((k.clone(), v.clone()));
        }
        vars.sort_by_key(|(k, _)| k.to_uppercase());
        let mut block = Vec::new();
        for (k, v) in vars {
            block.extend(format!("{k}={v}").encode_utf16());
            block.push(0);
        }
        block.push(0);
        block
    }

    pub(super) fn spawn(launch: &Launch, size: PtySize) -> Result<Spawned, SpawnError> {
        let fail = |e: &dyn std::fmt::Display| SpawnError(format!("{}: {e}", launch.program));
        let mut in_read = HANDLE::default();
        let mut in_write = HANDLE::default();
        let mut out_read = HANDLE::default();
        let mut out_write = HANDLE::default();
        // SAFETY: four out parameters; non-inheritable pipes with the default size.
        unsafe {
            CreatePipe(&mut in_read, &mut in_write, None, 0).map_err(|e| fail(&e))?;
            if let Err(e) = CreatePipe(&mut out_read, &mut out_write, None, 0) {
                let _ = CloseHandle(in_read);
                let _ = CloseHandle(in_write);
                return Err(fail(&e));
            }
        }
        // SAFETY: the four handles were just created and are owned only here.
        let (in_read, in_write, out_read, out_write) = unsafe {
            (
                OwnedHandle::from_raw_handle(in_read.0),
                OwnedHandle::from_raw_handle(in_write.0),
                OwnedHandle::from_raw_handle(out_read.0),
                OwnedHandle::from_raw_handle(out_write.0),
            )
        };
        use std::os::windows::io::AsRawHandle;
        let raw = |h: &OwnedHandle| HANDLE(h.as_raw_handle());
        // SAFETY: both pipe ends are open; ConPTY duplicates them, so ours close right after.
        let console =
            unsafe { CreatePseudoConsole(coord(size), raw(&in_read), raw(&out_write), 0) }
                .map_err(|e| fail(&format!("could not create the terminal: {e}")))?;
        let hpcon = console;
        let console = Arc::new(Console(std::sync::Mutex::new(Some(console))));
        drop(in_read);
        drop(out_write);

        let mut attr_size = 0usize;
        // SAFETY: the first call only asks for the size (it fails with "insufficient buffer").
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut attr_size) };
        let mut attrs = vec![0u8; attr_size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(attrs.as_mut_ptr().cast::<c_void>());
        // SAFETY: `attrs` has the size the first call asked for and outlives the list's use.
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut attr_size) }
            .map_err(|e| fail(&e))?;
        // SAFETY: the attribute value is the HPCON itself, as the documentation asks; `console`
        // outlives `CreateProcessW`.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(hpcon.0 as *const c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
        };
        if let Err(e) = updated {
            // SAFETY: initialized above.
            unsafe { DeleteProcThreadAttributeList(list) };
            return Err(fail(&e));
        }
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).unwrap_or(0);
        // No inherited standard handles: the program must talk only to the terminal.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = list;

        let mut line = wide(OsStr::new(&command_line(launch)));
        let env = environment(&with_term(&launch.env));
        let cwd = launch.cwd.as_deref().map(|c| wide(OsStr::new(c)));
        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: every pointer (command line, environment, folder, startup info) lives until
        // the call returns; born suspended so it enters the job object before running.
        let created = unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                Some(env.as_ptr().cast::<c_void>()),
                cwd.as_ref().map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr())),
                &si.StartupInfo,
                &mut pi,
            )
        };
        // SAFETY: initialized above; no longer needed once the process exists (or failed).
        unsafe { DeleteProcThreadAttributeList(list) };
        created.map_err(|e| fail(&e))?;
        let process = Process(pi.hProcess);
        let thread = pi.hThread;
        let close_thread = || {
            // SAFETY: the main thread's handle, closed once.
            let _ = unsafe { CloseHandle(thread) };
        };
        let tree = match tree::Tree::adopt(process.0) {
            Ok(t) => t,
            Err(e) => {
                close_thread();
                return Err(fail(&format!("could not group the process: {e}")));
            }
        };
        // SAFETY: the thread handle from `CreateProcessW`; `u32::MAX` is the error.
        let resumed = unsafe { ResumeThread(thread) } != u32::MAX;
        close_thread();
        if !resumed {
            tree.kill();
            return Err(fail(&"could not resume the process"));
        }

        let (tx, exit) = watch::channel(None);
        let waiter = console.clone();
        std::thread::spawn(move || {
            // The whole wrapper moves here (not just its raw handle field), with its Send.
            let process = process;
            // SAFETY: `process` is open while this thread owns it.
            let waited = unsafe { WaitForSingleObject(process.0, INFINITE) };
            let mut code = 0u32;
            // SAFETY: same handle; `code` is an out parameter.
            let ok = waited == WAIT_OBJECT_0
                && unsafe { GetExitCodeProcess(process.0, &mut code) }.is_ok();
            // The output pipe only ends when the console closes; what is left in it is
            // still read by the reader thread.
            waiter.close();
            let _ = tx.send(Some(ok.then_some(code as i32)));
        });
        Ok(Spawned {
            pid: Some(pi.dwProcessId),
            stdout: Box::new(ThreadReader::start(std::fs::File::from(out_read))),
            stderr: no_stderr(),
            stdin: Some(Box::new(ThreadWriter::start(std::fs::File::from(in_write)))),
            terminal: Some(console),
            child: Arc::new(SystemChild { exit, tree }),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn arguments_are_quoted_for_the_c_runtime() {
            let mut s = String::new();
            // Without a space or a quote nothing is quoted, trailing backslashes included; quoted,
            // a trailing run of backslashes is doubled so it does not escape the closing quote.
            for a in [
                "plain",
                "two words",
                r#"say "hi""#,
                r"C:\dir\",
                r"C:\my dir\",
                "",
            ] {
                quote(a, &mut s);
                s.push('|');
            }
            assert_eq!(
                s,
                r#"plain|"two words"|"say \"hi\""|C:\dir\|"C:\my dir\\"|""|"#
            );
        }
    }
}
