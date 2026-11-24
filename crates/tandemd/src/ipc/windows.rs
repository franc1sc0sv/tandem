//! Named pipe `\\.\pipe\tandemd-<SID>`, with a DACL that only lets in the user
//! who runs the agent and without remote clients.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tandem_proto::Peer;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

use super::{IpcClient, IpcError, Listener};

/// The name carries the SID so that two users of the same PC do not clash.
pub fn default_endpoint(_data_dir: &Path) -> io::Result<PathBuf> {
    Ok(PathBuf::from(format!(
        r"\\.\pipe\tandemd-{}",
        current_user_sid()?
    )))
}

pub struct PipeListener {
    name: PathBuf,
    /// Self-relative security descriptor; `u64` so it stays aligned.
    descriptor: Vec<u64>,
    /// The instance that waits for the next client.
    next: NamedPipeServer,
}

pub async fn bind(name: &Path) -> Result<PipeListener, IpcError> {
    let descriptor = user_only_descriptor()?;
    let next = match create(name, &descriptor, true) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return Err(IpcError::AlreadyRunning(name.to_owned()));
        }
        Err(e) => return Err(e.into()),
    };
    Ok(PipeListener {
        name: name.to_owned(),
        descriptor,
        next,
    })
}

pub async fn connect(name: &Path) -> Result<IpcClient<NamedPipeClient>, IpcError> {
    connect_as(name, Peer::Ui).await
}

/// Like [`connect`], introducing itself as `peer` (`tandem mcp` and the CLI, as a client).
pub async fn connect_as(name: &Path, peer: Peer) -> Result<IpcClient<NamedPipeClient>, IpcError> {
    // All instances busy: the server creates another right away.
    let mut tries = 0;
    let client = loop {
        match ClientOptions::new().open(name) {
            Ok(c) => break c,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) && tries < 50 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };
    IpcClient::handshake_as(client, peer).await
}

impl Listener for PipeListener {
    type Stream = NamedPipeServer;

    async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        self.next.connect().await?;
        let fresh = create(&self.name, &self.descriptor, false)?;
        Ok(std::mem::replace(&mut self.next, fresh))
    }
}

fn create(name: &Path, descriptor: &[u64], first: bool) -> io::Result<NamedPipeServer> {
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.as_ptr() as *mut _,
        bInheritHandle: false.into(),
    };
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first).reject_remote_clients(true);
    // SAFETY: `attrs` is a valid SECURITY_ATTRIBUTES and its descriptor (`descriptor`)
    // outlives the call; Windows copies the DACL when creating the pipe.
    unsafe { opts.create_with_security_attributes_raw(name, (&raw mut attrs).cast()) }
}

/// Protected DACL (no inheritance) with a single permission: everything for the current user.
fn user_only_descriptor() -> io::Result<Vec<u64>> {
    let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let mut len = 0u32;
    // SAFETY: `wide` ends in 0 and lives during the call; the output pointers are local.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            Some(&mut len),
        )
    }?;
    let mut out = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: Windows reserved `len` bytes in `sd`; `out` is at least that size.
    // It is freed afterwards with LocalFree, as the function requires.
    unsafe {
        std::ptr::copy_nonoverlapping(
            sd.0.cast::<u8>(),
            out.as_mut_ptr().cast::<u8>(),
            len as usize,
        );
        LocalFree(Some(HLOCAL(sd.0)));
    }
    Ok(out)
}

/// The process user's SID, as text (`S-1-5-21-...`).
fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the current process pseudo-handle is always valid; `token` is local.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
    let token = Handle(token);

    let mut len = 0u32;
    // SAFETY: first call only to learn the size; it fails on purpose due to a null buffer.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` has `len` bytes aligned to 8, enough for a TOKEN_USER.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
    }?;
    // SAFETY: GetTokenInformation left a valid TOKEN_USER at the start of `buf`.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    let mut text = PWSTR::null();
    // SAFETY: `sid` points inside `buf`, which is still alive; `text` is freed with LocalFree.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }?;
    // SAFETY: `text` is a 0-terminated string that Windows reserved.
    let out = unsafe { text.to_string() };
    // SAFETY: `text` was reserved by ConvertSidToStringSidW with LocalAlloc.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    out.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by OpenProcessToken and is not closed anywhere else.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_looks_like_a_sid() {
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
    }

    #[test]
    fn descriptor_is_built() {
        assert!(!user_only_descriptor().unwrap().is_empty());
    }

    #[tokio::test]
    async fn second_agent_is_refused() {
        let name = PathBuf::from(format!(r"\\.\pipe\tandemd-test-dup-{}", std::process::id()));
        let _first = bind(&name).await.unwrap();
        let err = bind(&name).await.err().unwrap();
        assert!(matches!(err, IpcError::AlreadyRunning(_)), "{err}");
    }
}
