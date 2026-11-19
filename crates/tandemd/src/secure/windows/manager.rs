//! Carries out the install plan's steps against the SCM, the files and the registry, and
//! reads the service's state.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
    ERROR_SERVICE_NOT_ACTIVE, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegDeleteKeyW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
    CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW,
    QueryServiceConfigW, QueryServiceStatus, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE,
    SC_MANAGER_ALL_ACCESS, SC_MANAGER_CONNECT, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONFIG_DESCRIPTION, SERVICE_CONFIG_FAILURE_ACTIONS,
    SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, SERVICE_CONTROL_STOP, SERVICE_DESCRIPTIONW,
    SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONS_FLAG, SERVICE_FAILURE_ACTIONSW,
    SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS, SERVICE_STOPPED,
    SERVICE_WIN32_OWN_PROCESS, StartServiceW,
};
use windows::core::{PCWSTR, PWSTR};

use super::{wide, win_err};
use crate::install::{Scheduler, WinAcl};
use crate::secure::SERVICE_NAME;
use crate::secure::install::{
    Current, Installed, POLICY_ABSENT, SAS_POLICY_KEY, SAS_POLICY_VALUE, STATE_KEY,
    STATE_PREVIOUS_POLICY, ServiceSpec, Step, install_plan, uninstall_plan,
};

/// An SCM handle closed on drop.
struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: opened by OpenSCManagerW/OpenServiceW/CreateServiceW and closed only here.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> io::Result<Sc> {
    // SAFETY: null machine and database: the local SCM.
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }
        .map(Sc)
        .map_err(win_err)
}

/// `Ok(None)` if the service does not exist.
fn open(scm: &Sc, access: u32) -> io::Result<Option<Sc>> {
    let name = wide(SERVICE_NAME);
    // SAFETY: live SCM handle and a null-terminated name.
    match unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), access) } {
        Ok(h) => Ok(Some(Sc(h))),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
        Err(e) => Err(win_err(e)),
    }
}

fn status(service: &Sc) -> io::Result<SERVICE_STATUS> {
    let mut st = SERVICE_STATUS::default();
    // SAFETY: live service handle with SERVICE_QUERY_STATUS; `st` is an out pointer.
    unsafe { QueryServiceStatus(service.0, &mut st) }.map_err(win_err)?;
    Ok(st)
}

fn command_line(service: &Sc) -> io::Result<String> {
    let mut needed = 0u32;
    // SAFETY: first call only learns the size.
    let _ = unsafe { QueryServiceConfigW(service.0, None, 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(8)];
    let len = (buf.len() * 8) as u32;
    let config = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: `buf` is aligned and as large as the SCM asked.
    unsafe { QueryServiceConfigW(service.0, Some(config), len, &mut needed) }.map_err(win_err)?;
    // SAFETY: the SCM filled a QUERY_SERVICE_CONFIGW whose strings point inside `buf`.
    let path = unsafe { (*config).lpBinaryPathName };
    if path.is_null() {
        return Ok(String::new());
    }
    // SAFETY: a null-terminated string inside `buf`, still alive.
    Ok(unsafe { path.to_string() }.unwrap_or_default())
}

/// The installed service and whether it runs; `None` if it is not installed. It only needs
/// the rights every user has.
pub fn service_state() -> io::Result<Option<Installed>> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let Some(service) = open(&scm, SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG)? else {
        return Ok(None);
    };
    Ok(Some(Installed {
        command_line: command_line(&service).unwrap_or_default(),
        running: status(&service)?.dwCurrentState != SERVICE_STOPPED,
    }))
}

fn check(err: WIN32_ERROR) -> io::Result<()> {
    if err.is_ok() {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(err.0 as i32))
    }
}

fn reg_dword(key: &str, value: &str) -> io::Result<Option<u32>> {
    let (k, v) = (wide(key), wide(value));
    let mut data = 0u32;
    let mut len = size_of::<u32>() as u32;
    // SAFETY: `data` is a DWORD and `len` its size; the strings live during the call.
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&mut len),
        )
    };
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(err)?;
    Ok(Some(data))
}

fn set_reg_dword(key: &str, value: &str, data: u32) -> io::Result<()> {
    let (k, v) = (wide(key), wide(value));
    // SAFETY: `data` is a DWORD that lives during the call; RegSetKeyValueW creates the key.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            REG_DWORD.0,
            Some((&raw const data).cast()),
            size_of::<u32>() as u32,
        )
    })
}

fn delete_reg_value(key: &str, value: &str) -> io::Result<()> {
    let (k, v) = (wide(key), wide(value));
    // SAFETY: valid null-terminated strings.
    let err =
        unsafe { RegDeleteKeyValueW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr())) };
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(err)
}

fn delete_reg_key(key: &str) {
    let k = wide(key);
    // SAFETY: valid null-terminated string. It fails if the key has subkeys, which is wanted.
    let _ = unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr())) };
}

/// `%ProgramFiles%` from the registry (written by Windows, only administrators change it); the
/// environment variable could come from the user.
fn program_files() -> io::Result<PathBuf> {
    let k = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion");
    let v = wide("ProgramFilesDir");
    let mut buf = vec![0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` has `len` bytes; RRF_RT_REG_SZ guarantees the trailing null.
    check(unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    })?;
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

fn current(source: &Path) -> io::Result<Current> {
    Ok(Current {
        elevated: crate::install::HighestTask.is_elevated(),
        service: service_state()?,
        source: source.to_owned(),
        program_files: program_files()?,
        sas_policy: reg_dword(SAS_POLICY_KEY, SAS_POLICY_VALUE)?,
        remembered_policy: reg_dword(STATE_KEY, STATE_PREVIOUS_POLICY)?,
    })
}

/// Installs (or refreshes) the service from `source`, this `tandemd.exe`.
pub fn install_service(source: &Path) -> io::Result<Vec<Step>> {
    let now = current(source)?;
    let steps = install_plan(&now, &WinAcl).map_err(io::Error::other)?;
    run(&steps)?;
    Ok(steps)
}

pub fn uninstall_service(source: &Path) -> io::Result<Vec<Step>> {
    let now = current(source)?;
    let steps = uninstall_plan(&now).map_err(io::Error::other)?;
    run(&steps)?;
    Ok(steps)
}

fn run(steps: &[Step]) -> io::Result<()> {
    for step in steps {
        tracing::info!(?step, "service install");
        apply(step).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", describe(step))))?;
    }
    Ok(())
}

fn describe(step: &Step) -> String {
    match step {
        Step::Stop => "stopping the service".into(),
        Step::CreateDir(p) => format!("creating {}", p.display()),
        Step::Copy { to, .. } => format!("copying to {}", to.display()),
        Step::CheckAdminOnly(p) => format!("checking the permissions of {}", p.display()),
        Step::Create(_) => "creating the service".into(),
        Step::Update(_) => "updating the service".into(),
        Step::Configure(_) => "configuring the service's recovery".into(),
        Step::SetSasPolicy { .. } => format!("setting {SAS_POLICY_VALUE}"),
        Step::RestoreSasPolicy(_) => format!("restoring {SAS_POLICY_VALUE}"),
        Step::Start => "starting the service".into(),
        Step::Delete => "deleting the service".into(),
        Step::RemoveFile(p) => format!("removing {}", p.display()),
        Step::RemoveDir(p) => format!("removing {}", p.display()),
    }
}

/// Retries `f` for a while: right after a stop, the old process can still hold the file.
fn retrying<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if Instant::now() < deadline && e.kind() != io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(e) => return Err(e),
        }
    }
}

fn wait_state(service: &Sc, running: bool, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = status(service)?.dwCurrentState;
        if (running && state == SERVICE_RUNNING) || (!running && state == SERVICE_STOPPED) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("the service stayed in state {}", state.0),
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn apply(step: &Step) -> io::Result<()> {
    match step {
        Step::Stop => {
            let scm = manager(SC_MANAGER_ALL_ACCESS)?;
            let Some(service) = open(&scm, SERVICE_ALL_ACCESS)? else {
                return Ok(());
            };
            let mut st = SERVICE_STATUS::default();
            // SAFETY: live handle with SERVICE_STOP; `st` is an out pointer.
            match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut st) } {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => return Ok(()),
                Err(e) => return Err(win_err(e)),
            }
            wait_state(&service, false, Duration::from_secs(30))
        }
        Step::CreateDir(dir) => std::fs::create_dir_all(dir),
        Step::Copy { from, to } => retrying(|| std::fs::copy(from, to)).map(drop),
        Step::CheckAdminOnly(path) => match crate::install::not_admin_only(&WinAcl, path) {
            None => Ok(()),
            Some(why) => Err(io::Error::new(io::ErrorKind::PermissionDenied, why)),
        },
        Step::Create(spec) => create(spec),
        Step::Update(spec) => update(spec),
        Step::Configure(spec) => configure(spec),
        Step::SetSasPolicy { value, previous } => {
            set_reg_dword(
                STATE_KEY,
                STATE_PREVIOUS_POLICY,
                previous.unwrap_or(POLICY_ABSENT),
            )?;
            set_reg_dword(SAS_POLICY_KEY, SAS_POLICY_VALUE, *value)
        }
        Step::RestoreSasPolicy(previous) => {
            match previous {
                Some(v) => set_reg_dword(SAS_POLICY_KEY, SAS_POLICY_VALUE, *v)?,
                None => delete_reg_value(SAS_POLICY_KEY, SAS_POLICY_VALUE)?,
            }
            delete_reg_value(STATE_KEY, STATE_PREVIOUS_POLICY)?;
            delete_reg_key(STATE_KEY);
            delete_reg_key(r"SOFTWARE\Tandem");
            Ok(())
        }
        Step::Start => {
            let scm = manager(SC_MANAGER_ALL_ACCESS)?;
            let service = open(&scm, SERVICE_ALL_ACCESS)?
                .ok_or_else(|| io::Error::other("the service does not exist"))?;
            // SAFETY: live handle with SERVICE_START; no arguments.
            match unsafe { StartServiceW(service.0, None) } {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => {}
                Err(e) => return Err(win_err(e)),
            }
            wait_state(&service, true, Duration::from_secs(15))
        }
        Step::Delete => {
            let scm = manager(SC_MANAGER_ALL_ACCESS)?;
            let Some(service) = open(&scm, SERVICE_ALL_ACCESS)? else {
                return Ok(());
            };
            // SAFETY: live handle with DELETE.
            unsafe { DeleteService(service.0) }.map_err(win_err)
        }
        Step::RemoveFile(path) => match retrying(|| std::fs::remove_file(path)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
        Step::RemoveDir(dir) => match std::fs::remove_dir_all(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
}

fn create(spec: &ServiceSpec) -> io::Result<()> {
    let scm = manager(SC_MANAGER_ALL_ACCESS)?;
    let (name, display, command) = (
        wide(&spec.name),
        wide(&spec.display_name),
        wide(&spec.command_line),
    );
    // SAFETY: every string is null-terminated and lives during the call; a null account is
    // LocalSystem.
    let service = unsafe {
        CreateServiceW(
            scm.0,
            PCWSTR(name.as_ptr()),
            PCWSTR(display.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(command.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(win_err)?;
    drop(Sc(service));
    Ok(())
}

fn update(spec: &ServiceSpec) -> io::Result<()> {
    let scm = manager(SC_MANAGER_ALL_ACCESS)?;
    let service = open(&scm, SERVICE_ALL_ACCESS)?
        .ok_or_else(|| io::Error::other("the service does not exist"))?;
    let (display, command, account) = (
        wide(&spec.display_name),
        wide(&spec.command_line),
        wide("LocalSystem"),
    );
    // SAFETY: live handle with SERVICE_CHANGE_CONFIG; strings live during the call.
    unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(command.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR(account.as_ptr()),
            PCWSTR::null(),
            PCWSTR(display.as_ptr()),
        )
    }
    .map_err(win_err)
}

fn configure(spec: &ServiceSpec) -> io::Result<()> {
    let scm = manager(SC_MANAGER_ALL_ACCESS)?;
    let service = open(&scm, SERVICE_ALL_ACCESS)?
        .ok_or_else(|| io::Error::other("the service does not exist"))?;
    let mut description = wide(&spec.description);
    let info = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(description.as_mut_ptr()),
    };
    // SAFETY: `info` and its string live during the call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some((&raw const info).cast()),
        )
    }
    .map_err(win_err)?;

    let mut actions: Vec<SC_ACTION> = spec
        .restart_delays
        .iter()
        .map(|d| SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: u32::try_from(d.as_millis()).unwrap_or(u32::MAX),
        })
        .collect();
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: u32::try_from(spec.reset_after.as_secs()).unwrap_or(u32::MAX),
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    // SAFETY: `failure` and the actions it points to live during the call. Restart actions
    // need SERVICE_START, which SERVICE_ALL_ACCESS includes.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some((&raw const failure).cast()),
        )
    }
    .map_err(win_err)?;

    // An exit with an error code (not only a crash) also triggers the recovery.
    let flag = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: true.into(),
    };
    // SAFETY: `flag` lives during the call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            Some((&raw const flag).cast()),
        )
    }
    .map_err(win_err)
}
