//! [`SessionHost`] on Windows: the console session, the helper launched with `winlogon`'s
//! token and `TokenUIAccess`, and `SendSAS`.

use std::io::{self, BufRead, BufReader};
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::Security::{
    DuplicateTokenEx, SECURITY_ATTRIBUTES, SecurityIdentification, SetTokenInformation,
    TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary, TokenUIAccess,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessAsUserW, GetExitCodeProcess, OpenProcess, OpenProcessToken,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use super::{Owned, wide, win_err};
use crate::secure::supervisor::{Event, SessionHost, winlogon_in};

/// A running helper.
pub struct Helper {
    process: Owned,
    pub pid: u32,
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
        let token = helper_token(session)?;
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

fn processes() -> io::Result<Vec<(u32, String, Option<u32>)>> {
    // SAFETY: no pointers; the snapshot handle is owned below.
    let snap = Owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(win_err)?);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut out = Vec::new();
    // SAFETY: `entry` is initialized with its size, as the API requires.
    let mut more = unsafe { Process32FirstW(snap.0, &mut entry) }.is_ok();
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
        out.push((entry.th32ProcessID, name, session_of(entry.th32ProcessID)));
        // SAFETY: same snapshot and entry.
        more = unsafe { Process32NextW(snap.0, &mut entry) }.is_ok();
    }
    Ok(out)
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out pointer.
    unsafe { ProcessIdToSessionId(pid, &mut session) }.ok()?;
    Some(session)
}

/// SYSTEM's primary token in `session` (a copy of its `winlogon.exe`'s) with UIAccess on.
fn helper_token(session: u32) -> io::Result<Owned> {
    let pid = winlogon_in(processes()?, session)
        .ok_or_else(|| io::Error::other(format!("no winlogon.exe in session {session}")))?;
    // SAFETY: no pointers; the handle is owned below.
    let process = Owned(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(win_err)?,
    );
    let mut token = HANDLE::default();
    // SAFETY: live process handle; `token` is an out pointer owned below.
    unsafe { OpenProcessToken(process.0, TOKEN_DUPLICATE | TOKEN_QUERY, &mut token) }
        .map_err(win_err)?;
    let token = Owned(token);
    let mut primary = HANDLE::default();
    // SAFETY: live token handle; `primary` is an out pointer owned below.
    unsafe {
        DuplicateTokenEx(
            token.0,
            TOKEN_ALL_ACCESS,
            None,
            SecurityIdentification,
            TokenPrimary,
            &mut primary,
        )
    }
    .map_err(win_err)?;
    let primary = Owned(primary);
    let ui_access: u32 = 1;
    // SAFETY: a DWORD is what TokenUIAccess takes; the service has SeTcbPrivilege as SYSTEM.
    unsafe {
        SetTokenInformation(
            primary.0,
            TokenUIAccess,
            (&raw const ui_access).cast(),
            size_of::<u32>() as u32,
        )
    }
    .map_err(win_err)?;
    Ok(primary)
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

fn launch(token: &Owned, exe: &std::path::Path, stderr: &Owned) -> io::Result<Helper> {
    let mut command = wide(&format!("\"{}\" --input-helper", exe.display()));
    let mut desktop = wide(r"winsta0\default");
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        dwFlags: STARTF_USESTDHANDLES,
        hStdInput: HANDLE::default(),
        hStdOutput: HANDLE::default(),
        hStdError: stderr.0,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer (command line, desktop, startup info) lives during the call; the
    // command line buffer is writable, as CreateProcessAsUserW requires. Only the inheritable
    // write end of the control pipe is passed down.
    unsafe {
        CreateProcessAsUserW(
            Some(token.0),
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup,
            &mut info,
        )
    }
    .map_err(win_err)?;
    drop(Owned(info.hThread));
    Ok(Helper {
        process: Owned(info.hProcess),
        pid: info.dwProcessId,
    })
}
