//! [`SessionHost`] on Windows: the console session, the helper launched with the service's
//! own LocalSystem token moved to that session with `TokenUIAccess`, and `SendSAS`.

use std::io::{self, BufRead, BufReader};
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::Security::{
    DuplicateTokenEx, SECURITY_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
    TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary, TokenSessionId, TokenUIAccess,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessAsUserW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use super::{Owned, token_is_system, token_session, wide, win_err};
use crate::secure::supervisor::{Event, SessionHost, TokenSource, helper_token};

/// A running helper. Dropping it ends the process: if the service unwinds (an error or a
/// panic in its loop) the helper does not stay behind holding the pipe and the executable.
pub struct Helper {
    process: Owned,
    /// `KILL_ON_JOB_CLOSE`: also covers the service dying without unwinding. `None` if
    /// Windows refused to put the helper in the job; then only `Drop` ends it.
    _job: Option<Owned>,
    pub pid: u32,
}

impl Drop for Helper {
    fn drop(&mut self) {
        // SAFETY: the handle has PROCESS_TERMINATE (CreateProcessAsUser grants all rights).
        // Terminating a process that already ended only fails.
        let _ = unsafe { TerminateProcess(self.process.0, 0) };
    }
}

pub struct WinSessionHost {
    exe: PathBuf,
    /// Where the helper's Ctrl+Alt+Del requests go.
    events: Sender<Event>,
}

impl WinSessionHost {
    pub fn new(exe: PathBuf, events: Sender<Event>) -> Self {
        WinSessionHost { exe, events }
    }
}

impl SessionHost for WinSessionHost {
    type Helper = Helper;

    fn console_session(&mut self) -> Option<u32> {
        // SAFETY: no arguments; it only reads a kernel value.
        Some(unsafe { WTSGetActiveConsoleSessionId() })
    }

    fn spawn(&mut self, session: u32) -> io::Result<Helper> {
        let token = helper_token(&mut WinTokens, session)?;
        let (read, write) = control_pipe()?;
        let helper = launch(&token, &self.exe, &write)?;
        drop(write);
        tracing::info!(session, pid = helper.pid, "helper launched");
        let events = self.events.clone();
        let pid = helper.pid;
        std::thread::Builder::new()
            .name("helper-control".into())
            .spawn(move || read_control(read, pid, &events))?;
        Ok(helper)
    }

    fn exited(&mut self, helper: &mut Helper) -> Option<u32> {
        // SAFETY: the process handle is live while `helper` exists.
        if unsafe { WaitForSingleObject(helper.process.0, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: as above; `code` is a valid out pointer.
        unsafe { GetExitCodeProcess(helper.process.0, &mut code) }.ok()?;
        Some(code)
    }

    fn kill(&mut self, helper: Helper) {
        // SAFETY: the handle has PROCESS_TERMINATE (CreateProcessAsUser grants all rights).
        let _ = unsafe { TerminateProcess(helper.process.0, 0) };
        // SAFETY: live handle; bounded wait so a stuck process cannot hang the service.
        let _ = unsafe { WaitForSingleObject(helper.process.0, 5_000) };
        drop(helper);
    }

    fn send_sas(&mut self) -> io::Result<()> {
        // SAFETY: no pointers. `FALSE`: the caller is a service. It returns nothing; if the
        // `SoftwareSASGeneration` policy does not allow services it does nothing, which the
        // install prevents by setting it.
        unsafe { SendSAS(false) };
        Ok(())
    }
}

/// Reads the helper's control lines (its stderr) until it exits.
fn read_control(read: Owned, pid: u32, events: &Sender<Event>) {
    // SAFETY: `read` is the read end of an anonymous pipe owned here; the File takes it over.
    let file = unsafe { std::fs::File::from_raw_handle(read.0.0) };
    std::mem::forget(read);
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim() == "sas" {
            if events.send(Event::Sas).is_err() {
                break;
            }
        } else if !line.trim().is_empty() {
            tracing::warn!(pid, "helper: {line}");
        }
    }
}

/// [`TokenSource`] over the service process's own token.
pub(super) struct WinTokens;

impl TokenSource for WinTokens {
    type Token = Owned;

    fn duplicate_own(&mut self) -> io::Result<Owned> {
        let mut own = HANDLE::default();
        // SAFETY: GetCurrentProcess is a pseudo-handle that needs no closing; `own` is an out
        // pointer owned below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut own) }
            .map_err(win_err)?;
        let own = Owned(own);
        let mut primary = HANDLE::default();
        // SAFETY: live token handle; `primary` is an out pointer owned below.
        unsafe {
            DuplicateTokenEx(
                own.0,
                TOKEN_ALL_ACCESS,
                None,
                SecurityImpersonation,
                TokenPrimary,
                &mut primary,
            )
        }
        .map_err(win_err)?;
        Ok(Owned(primary))
    }

    fn is_local_system(&mut self, token: &Owned) -> io::Result<bool> {
        token_is_system(token)
    }

    fn set_session(&mut self, token: &Owned, session: u32) -> io::Result<()> {
        set_dword(token, TokenSessionId, session)
    }

    fn session(&mut self, token: &Owned) -> io::Result<u32> {
        token_session(token)
    }

    fn enable_ui_access(&mut self, token: &Owned) -> io::Result<()> {
        set_dword(token, TokenUIAccess, 1)
    }
}

fn set_dword(
    token: &Owned,
    class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
    value: u32,
) -> io::Result<()> {
    // SAFETY: TokenSessionId and TokenUIAccess take a DWORD; the token has TOKEN_ALL_ACCESS
    // and the service holds SeTcbPrivilege as LocalSystem.
    unsafe {
        SetTokenInformation(
            token.0,
            class,
            (&raw const value).cast(),
            size_of::<u32>() as u32,
        )
    }
    .map_err(win_err)
}

/// An anonymous pipe whose write end the helper inherits as stderr.
fn control_pipe() -> io::Result<(Owned, Owned)> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    // SAFETY: valid out pointers and attributes that live during the call.
    unsafe { CreatePipe(&mut read, &mut write, Some(&attrs), 0) }.map_err(win_err)?;
    let (read, write) = (Owned(read), Owned(write));
    // SAFETY: live handle; only the write end may reach the child.
    unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
        .map_err(win_err)?;
    Ok((read, write))
}

/// A job that kills its processes when its last handle closes (the service exiting in any way).
fn kill_on_close_job() -> io::Result<Owned> {
    // SAFETY: no security attributes (not inheritable) and no name; owned below.
    let job = Owned(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(win_err)?);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `limits` is the struct JobObjectExtendedLimitInformation takes, with its size.
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(win_err)?;
    Ok(job)
}

/// A `PROC_THREAD_ATTRIBUTE_LIST` with a handle list, deleted on drop. The list and the
/// handles it points to must outlive `CreateProcessAsUserW`.
struct InheritList {
    buf: Vec<u64>,
    handles: Box<[HANDLE]>,
}

impl InheritList {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: first call only learns the size; it fails on purpose with no list.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
        let mut list = InheritList {
            buf: vec![0u64; size.div_ceil(8)],
            handles: handles.into(),
        };
        // SAFETY: `buf` has `size` bytes, aligned to 8.
        unsafe { InitializeProcThreadAttributeList(Some(list.ptr()), 1, None, &mut size) }
            .map_err(win_err)?;
        // SAFETY: the list was initialized for one attribute; `handles` is boxed, so its
        // address does not move while the list lives.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(list.handles.as_ptr().cast()),
                size_of_val(&*list.handles),
                None,
                None,
            )
        };
        if let Err(e) = updated {
            // SAFETY: initialized above and not used afterwards.
            unsafe { DeleteProcThreadAttributeList(list.ptr()) };
            list.buf.clear();
            return Err(win_err(e));
        }
        Ok(list)
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.buf.as_mut_ptr().cast())
    }
}

impl Drop for InheritList {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            // SAFETY: initialized in `new` (an empty buffer means it was already deleted).
            unsafe { DeleteProcThreadAttributeList(self.ptr()) };
        }
    }
}

fn launch(token: &Owned, exe: &std::path::Path, stderr: &Owned) -> io::Result<Helper> {
    let mut command = wide(&format!("\"{}\" --input-helper", exe.display()));
    let mut desktop = wide(r"winsta0\default");
    // Only the control pipe reaches the child, whatever else the service has inheritable.
    let mut inherit = InheritList::new(&[stderr.0])?;
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: HANDLE::default(),
            hStdOutput: HANDLE::default(),
            hStdError: stderr.0,
            ..Default::default()
        },
        lpAttributeList: inherit.ptr(),
    };
    let job = kill_on_close_job()?;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer (command line, desktop, startup info and its attribute list)
    // lives during the call; the command line buffer is writable, as CreateProcessAsUserW
    // requires. EXTENDED_STARTUPINFO_PRESENT says `startup` is a STARTUPINFOEXW.
    unsafe {
        CreateProcessAsUserW(
            Some(token.0),
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut info,
        )
    }
    .map_err(win_err)?;
    drop(inherit);
    let thread = Owned(info.hThread);
    let mut helper = Helper {
        process: Owned(info.hProcess),
        _job: None,
        pid: info.dwProcessId,
    };
    // Suspended until it is in the job, so it cannot start anything outside it first.
    // SAFETY: both handles are live; the job was created by this process.
    match unsafe { AssignProcessToJobObject(job.0, helper.process.0) } {
        Ok(()) => helper._job = Some(job),
        Err(e) => tracing::warn!(
            error = %win_err(e),
            "the helper is not in a job: it only ends with the service if the service unwinds"
        ),
    }
    // SAFETY: live thread handle of the suspended main thread. On failure `helper` drops and
    // ends the process.
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(helper)
}
