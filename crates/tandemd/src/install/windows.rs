//! Startup with the session in the `HKCU` `Run` key: it does not ask for administrator permissions and
//! only affects the installing user (the SYSTEM service mode is T-027). And the task
//! scheduled with the highest privileges ([`HighestTask`], A7), which does ask for them.

use std::ffi::c_void;
use std::io;
use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, HLOCAL, LocalFree, RPC_E_CHANGED_MODE, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    TOKEN_QUERY,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::TaskScheduler::{
    ITaskService, TASK_CREATE_OR_UPDATE, TASK_LOGON_NONE, TaskScheduler,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, HSTRING, PCWSTR, PWSTR};

use super::acl::{Ace, RawAce, Rigor, classify, verdict};
use super::task::{PathAcl, Scheduler, TASK_NAME};
use super::{Autostart, RUN_VALUE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The [`RUN_VALUE`] value of `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
#[derive(Debug, Default)]
pub struct RunKey;

impl Autostart for RunKey {
    fn get(&self) -> io::Result<Option<String>> {
        let key = HSTRING::from(RUN_KEY);
        let name = HSTRING::from(RUN_VALUE);
        let mut len = 0u32;
        // SAFETY: first only the size is requested (without a buffer); the strings live until the end.
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut len),
            )
        };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(err)?;
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        // SAFETY: `buf` has `len` bytes; the API writes at most that and updates `len`.
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(err)?;
        buf.truncate((len as usize) / 2);
        // RRF_RT_REG_SZ guarantees the trailing null, which is not part of the value.
        while buf.last() == Some(&0) {
            buf.pop();
        }
        Ok(Some(String::from_utf16_lossy(&buf)))
    }

    fn set(&mut self, command: &str) -> io::Result<()> {
        let key = open_run_key()?;
        let data: Vec<u8> = command
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let name = HSTRING::from(RUN_VALUE);
        // SAFETY: `key` is a key opened with KEY_SET_VALUE and `data` is a null-terminated REG_SZ.
        let err = unsafe { RegSetValueExW(key.0, &name, None, REG_SZ, Some(&data)) };
        check(err)
    }

    fn remove(&mut self) -> io::Result<bool> {
        let key = HSTRING::from(RUN_KEY);
        let name = HSTRING::from(RUN_VALUE);
        // SAFETY: valid null-terminated strings during the call.
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(err)?;
        Ok(true)
    }
}

/// The `Tandem\tandemd` scheduled task, through the Task Scheduler COM API: the XML
/// goes in memory, without a file someone else could change before an elevated process reads it.
#[derive(Debug, Default)]
pub struct HighestTask;

const TASK_FOLDER: &str = "Tandem";

/// `HRESULT_FROM_WIN32` of "file" and "path" not found.
const NOT_FOUND: [i32; 2] = [0x8007_0002_u32 as i32, 0x8007_0003_u32 as i32];

fn com(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("Task Scheduler: {e}"))
}

/// COM initialized on this thread until it is dropped.
struct ComScope(bool);

impl ComScope {
    fn new() -> io::Result<ComScope> {
        // SAFETY: paired with `CoUninitialize` in `Drop` only if this call succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            // The thread already has COM in another mode: it is used as is and not uninitialized.
            return Ok(ComScope(false));
        }
        hr.ok().map_err(com)?;
        Ok(ComScope(true))
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: pairs with the `CoInitializeEx` of `new`.
            unsafe { CoUninitialize() };
        }
    }
}

fn service() -> io::Result<ITaskService> {
    // SAFETY: creates the Task Scheduler COM object; COM is initialized by the caller.
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }.map_err(com)?;
    let none = VARIANT::default();
    // SAFETY: empty variants = the local machine and the current account.
    unsafe { service.Connect(&none, &none, &none, &none) }.map_err(com)?;
    Ok(service)
}

fn name(path: &str) -> BSTR {
    BSTR::from(path)
}

impl Scheduler for HighestTask {
    fn is_elevated(&self) -> bool {
        is_elevated()
    }

    fn exists(&self) -> io::Result<bool> {
        let _com = ComScope::new()?;
        let service = service()?;
        // SAFETY: COM calls on live objects and valid strings.
        let found = unsafe {
            let root = service.GetFolder(&name(r"\")).map_err(com)?;
            root.GetTask(&name(TASK_NAME))
        };
        match found {
            Ok(_) => Ok(true),
            Err(e) if NOT_FOUND.contains(&e.code().0) => Ok(false),
            Err(e) => Err(com(e)),
        }
    }

    fn register(&mut self, xml: &str) -> io::Result<()> {
        let _com = ComScope::new()?;
        let service = service()?;
        let none = VARIANT::default();
        // SAFETY: COM calls on live objects and valid strings.
        unsafe {
            let root = service.GetFolder(&name(r"\")).map_err(com)?;
            if root.GetFolder(&name(TASK_FOLDER)).is_err() {
                root.CreateFolder(&name(TASK_FOLDER), &none).map_err(com)?;
            }
            // The account data goes in the XML (the user's `InteractiveToken`).
            root.RegisterTask(
                &name(TASK_NAME),
                &name(xml),
                TASK_CREATE_OR_UPDATE.0,
                &none,
                &none,
                TASK_LOGON_NONE,
                &none,
            )
            .map_err(com)?;
        }
        Ok(())
    }

    fn remove(&mut self) -> io::Result<bool> {
        let _com = ComScope::new()?;
        let service = service()?;
        // SAFETY: COM calls on live objects and valid strings.
        let gone = unsafe {
            let root = service.GetFolder(&name(r"\")).map_err(com)?;
            root.DeleteTask(&name(TASK_NAME), 0)
        };
        match gone {
            Ok(()) => Ok(true),
            Err(e) if NOT_FOUND.contains(&e.code().0) => Ok(false),
            Err(e) => Err(com(e)),
        }
    }
}

/// A path's ACL: reads who can write it ([`PathAcl`]). The policy (SID allowlist,
/// owner, inherit-only ACEs) is in [`super::acl`]; here the system is only read.
#[derive(Debug, Default)]
pub struct WinAcl;

/// Frees with `LocalFree` what a security API returned.
struct LocalMem(*mut c_void);

impl Drop for LocalMem {
    fn drop(&mut self) {
        // SAFETY: it was allocated by an API that is freed with LocalFree and is only freed here.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
    }
}

fn win32(err: WIN32_ERROR, what: &str) -> io::Result<()> {
    check(err).map_err(|e| io::Error::other(format!("{what}: {e}")))
}

/// The SID as text (`S-1-5-...`).
///
/// # Safety
/// `sid` must point to a valid SID during the call.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid (required of the caller); `text` stays allocated with LocalAlloc.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(com)?;
    let _free = LocalMem(text.0.cast());
    // SAFETY: Windows returns a zero-terminated UTF-16 string.
    unsafe { text.to_string() }.map_err(|e| io::Error::other(e.to_string()))
}

/// The DACL's allow ACEs, already classified.
///
/// # Safety
/// `dacl` must point to a valid ACL during the call.
unsafe fn read_aces(dacl: *const ACL) -> io::Result<Vec<Ace>> {
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: the buffer is `size` and its exact size is passed.
    unsafe {
        GetAclInformation(
            dacl,
            std::ptr::from_mut(&mut size).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    }
    .map_err(com)?;
    let mut aces = Vec::new();
    for i in 0..size.AceCount {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: `i` is in range; `raw` points inside the live ACL.
        unsafe { GetAce(dacl, i, &mut raw) }.map_err(com)?;
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header = unsafe { *raw.cast::<ACE_HEADER>() };
        // In allow ACEs (`ACCESS_ALLOWED_ACE` and the callback one) the mask is after the
        // header (4 bytes) and the SID after the mask (offset 8). The other types are not read
        // beyond the header.
        let (mask, sid) = if matches!(header.AceType, 0 | 9) {
            // SAFETY: the allow ACE measures at least header + mask + SID.
            let mask = unsafe { raw.cast::<u8>().add(4).cast::<u32>().read_unaligned() };
            // SAFETY: the SID starts at byte 8 of the ACE.
            let sid = PSID(unsafe { raw.cast::<u8>().add(8) }.cast());
            // SAFETY: the SID is inside the ACE, alive with the ACL.
            (mask, unsafe { sid_text(sid) }?)
        } else {
            (0, String::new())
        };
        match classify(header.AceType, header.AceFlags, mask, sid).map_err(io::Error::other)? {
            RawAce::Allow(ace) => aces.push(ace),
            RawAce::Ignored => {}
        }
    }
    Ok(aces)
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: junction or symbolic link.
const REPARSE_POINT: u32 = 0x400;

impl PathAcl for WinAcl {
    fn admin_only(&self, path: &Path, rigor: Rigor) -> io::Result<bool> {
        use std::os::windows::fs::MetadataExt;

        // `GetNamedSecurityInfoW` follows links and the XML's path does not: they are rejected.
        if std::fs::symlink_metadata(path)?.file_attributes() & REPARSE_POINT != 0 {
            return Ok(false);
        }
        let wide = HSTRING::from(path.as_os_str());
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the outputs are local pointers; `sd` is freed below and owner and DACL live
        // inside it.
        let err = unsafe {
            GetNamedSecurityInfoW(
                &wide,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut sd,
            )
        };
        win32(err, "GetNamedSecurityInfo")?;
        let _sd = LocalMem(sd.0);
        // Without a DACL everyone has full access; without an owner it cannot be trusted.
        if dacl.is_null() || owner.0.is_null() {
            return Ok(false);
        }
        // SAFETY: `owner` and `dacl` point inside the descriptor `_sd`, alive until the end.
        let (owner, aces) = unsafe { (sid_text(owner)?, read_aces(dacl)?) };
        match verdict(&owner, &aces, rigor) {
            Ok(()) => Ok(true),
            Err(why) => {
                tracing::debug!(path = %path.display(), %why, "ACL is not administrators-only");
                Ok(false)
            }
        }
    }
}

/// The process token is elevated (it runs as administrator).
fn is_elevated() -> bool {
    use windows::Win32::Security::{TOKEN_ELEVATION, TokenElevation};

    let mut token = HANDLE::default();
    // SAFETY: `token` receives a new handle that is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the buffer is `elevation` and its exact size is passed; `token` is valid.
    let got = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    // SAFETY: `token` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(token) };
    got.is_ok() && elevation.TokenIsElevated != 0
}

/// Closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: it was opened by `open_run_key` and nobody else closes it.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open_run_key() -> io::Result<Key> {
    let mut key = HKEY::default();
    let path = HSTRING::from(RUN_KEY);
    // SAFETY: `key` receives the opened key; the key exists on all Windows but is created if missing.
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &path,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    check(err)?;
    Ok(Key(key))
}

fn check(err: WIN32_ERROR) -> io::Result<()> {
    if err.is_ok() {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(err.0 as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Read only: the real ACLs of `C:\` and `C:\Program Files` of a normal Windows pass
    /// as ancestors (on `C:\` the Authenticated Users `(AD)` does not count).
    #[test]
    fn root_and_program_files_pass_as_ancestors() {
        for p in ["C:\\", "C:\\Program Files"] {
            assert!(
                WinAcl.admin_only(Path::new(p), Rigor::Ancestor).unwrap(),
                "{p}"
            );
        }
    }

    /// Touches the user's real registry, so it leaves the value as it was.
    #[test]
    fn round_trip_in_hkcu() {
        let mut run = RunKey;
        let before = run.get().unwrap();
        run.set(r#""C:\Path with spaces\tandemw.exe""#).unwrap();
        assert_eq!(
            run.get().unwrap().as_deref(),
            Some(r#""C:\Path with spaces\tandemw.exe""#)
        );
        assert!(run.remove().unwrap());
        assert_eq!(run.get().unwrap(), None);
        assert!(!run.remove().unwrap());
        if let Some(cmd) = before {
            run.set(&cmd).unwrap();
        }
    }
}
