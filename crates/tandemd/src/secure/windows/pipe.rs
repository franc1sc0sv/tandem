//! The helper's named pipe: the server side (the helper, SYSTEM) and the client side (the
//! session agent).
//!
//! The pipe carries keystrokes, and whoever writes to it can click "Yes" on a UAC prompt, so
//! both ends check the other:
//!
//! - the helper creates it with `FILE_FLAG_FIRST_PIPE_INSTANCE` (a squatter makes it fail
//!   instead of sharing the name), no remote clients, and a DACL for SYSTEM and
//!   administrators only, at high integrity: an unelevated process of the user (whose
//!   administrators group is deny-only) cannot open it;
//! - the agent opens it at identification level (the server cannot impersonate it) and
//!   refuses to talk unless the server process runs as LocalSystem.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PCWSTR;

use super::desktop::WinDesktops;
use super::{Owned, current_session, token_is_system, wide, win_err};
use crate::secure::desktop::{CHECK_INTERVAL, DesktopTracker};
use crate::secure::helper::Helper;
use crate::secure::helper_pipe_name;
use crate::secure::protocol::MAX_FRAME;
use crate::secure::routed::{ConnectError, FramedLink, HelperConnector, HelperLink};

/// SYSTEM and administrators, everything; nobody below high integrity may write.
const HELPER_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;HI)";

/// A self-relative security descriptor built from SDDL, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let w = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `w` is null-terminated and lives during the call; `sd` is an out pointer
        // freed in Drop with LocalFree, as the API requires.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(w.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(win_err)?;
        Ok(Descriptor(sd))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

fn create_pipe(name: &str, sd: &Descriptor) -> io::Result<Owned> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    };
    let w = wide(name);
    // SAFETY: the name and the attributes live during the call; Windows copies the descriptor.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(w.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            MAX_FRAME as u32,
            MAX_FRAME as u32,
            0,
            Some(&attrs),
        )
    };
    if pipe.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    Ok(Owned(pipe))
}

fn wait_client(pipe: &Owned) -> io::Result<()> {
    // SAFETY: live pipe handle, synchronous call (no OVERLAPPED).
    match unsafe { ConnectNamedPipe(pipe.0, None) } {
        Ok(()) => Ok(()),
        // The client connected between CreateNamedPipe and ConnectNamedPipe: that is fine.
        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(()),
        Err(e) => Err(win_err(e)),
    }
}

/// `tandemd --input-helper`: serves this session's agent until the service ends it.
pub fn run_helper() -> io::Result<()> {
    let session = current_session()?;
    let name = helper_pipe_name(session);
    let sd = Descriptor::from_sddl(HELPER_SDDL)?;
    let pipe = create_pipe(&name, &sd)?;
    tracing::info!(session, pipe = %name, "helper serving");
    let sink = tandem_input::windows::emulate::SendInputSink::new()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut helper = Helper::new(
        sink,
        DesktopTracker::new(WinDesktops::default(), CHECK_INTERVAL),
        // stderr is the control channel the service reads.
        Box::new(|| {
            let mut err = io::stderr().lock();
            writeln!(err, "sas")?;
            err.flush()
        }),
    );
    loop {
        if let Err(e) = wait_client(&pipe) {
            tracing::warn!(error = %e, "waiting for the agent failed");
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        // SAFETY: the File borrows the pipe handle for this client only: ManuallyDrop keeps it
        // from closing it, so the same instance serves the next client.
        let file = ManuallyDrop::new(unsafe { File::from_raw_handle(pipe.0.0) });
        let mut stream: &File = &file;
        match helper.serve(&mut stream) {
            Ok(()) => tracing::info!("agent disconnected"),
            Err(e) => tracing::info!(error = %e, "agent connection ended"),
        }
        // SAFETY: live pipe handle; it drops the client so the next one can connect.
        let _ = unsafe { DisconnectNamedPipe(pipe.0) };
    }
}

/// Opens this session's helper pipe from the agent.
#[derive(Debug, Default)]
pub struct PipeConnector {
    session: Option<u32>,
}

impl PipeConnector {
    pub fn new() -> Self {
        PipeConnector {
            session: current_session().ok(),
        }
    }
}

impl HelperConnector for PipeConnector {
    fn connect(&mut self) -> Result<Box<dyn HelperLink>, ConnectError> {
        let session = self.session.ok_or(ConnectError::NotRunning)?;
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0)
            .open(helper_pipe_name(session));
        let file = match opened {
            Ok(f) => f,
            Err(e) => {
                return Err(match e.raw_os_error().map(|c| c as u32) {
                    Some(c) if c == ERROR_FILE_NOT_FOUND.0 => match super::service_state() {
                        Ok(None) => ConnectError::NotInstalled,
                        _ => ConnectError::NotRunning,
                    },
                    Some(c) if c == ERROR_ACCESS_DENIED.0 => ConnectError::NoAccess,
                    Some(c) if c == ERROR_PIPE_BUSY.0 => ConnectError::NotRunning,
                    _ => ConnectError::Io(e),
                });
            }
        };
        if !server_is_system(&file)? {
            return Err(ConnectError::Untrusted(
                "its process is not LocalSystem".into(),
            ));
        }
        Ok(Box::new(FramedLink(file)))
    }
}

/// The pipe's server process runs as LocalSystem.
fn server_is_system(file: &File) -> io::Result<bool> {
    let mut pid = 0u32;
    // SAFETY: the handle is the client end of a pipe, live while `file` is.
    unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid) }
        .map_err(win_err)?;
    // SAFETY: no pointers; the handle is owned below.
    let process = Owned(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(win_err)?,
    );
    let mut token = HANDLE::default();
    // SAFETY: live process handle; `token` is owned below.
    unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) }.map_err(win_err)?;
    token_is_system(&Owned(token))
}

/// The helper's pipe, created by the current process (a test that plays squatter).
#[cfg(test)]
pub(super) fn squat_for_tests(name: &str) -> io::Result<Owned> {
    create_pipe(name, &Descriptor::from_sddl(HELPER_SDDL)?)
}
