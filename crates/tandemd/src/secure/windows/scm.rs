//! `tandemd --service`: the SCM entry point, the control handler and the supervisor loop.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR};
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_SHUTDOWN,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SESSIONCHANGE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
    StartServiceCtrlDispatcherW,
};
use windows::core::{PCWSTR, PWSTR};

use super::host::WinSessionHost;
use super::{service_log_dir, wide, win_err};
use crate::secure::SERVICE_NAME;
use crate::secure::install::{SAS_POLICY_KEY, SAS_POLICY_VALUE};
use crate::secure::supervisor::{Action, Backoff, Event, Supervisor};

/// How often the supervisor looks at the helper and the console without being woken up.
const POLL: Duration = Duration::from_secs(1);

/// The status handle (`SERVICE_STATUS_HANDLE` as an integer, so it can be static).
static STATUS: AtomicUsize = AtomicUsize::new(0);
/// Where the control handler leaves its events for the supervisor.
static EVENTS: OnceLock<Mutex<Option<Sender<Event>>>> = OnceLock::new();

/// Started by the SCM: hands the thread to the dispatcher until the service stops.
pub fn run_service() -> io::Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: the table ends in a null entry and lives until the dispatcher returns, which
    // is when the service stopped.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(win_err)
}

/// Called by the SCM on its own thread. It cannot unwind into the SCM: a panic is caught and
/// reported as a failure, which triggers the recovery actions.
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let code = match std::panic::catch_unwind(service_body) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "the service failed");
            1
        }
        Err(_) => 2,
    };
    set_status(SERVICE_STOPPED, 0, code);
}

/// Control requests from the SCM; they only queue an event.
unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    let event = match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            set_status(SERVICE_STOP_PENDING, 0, 0);
            Event::Stop
        }
        SERVICE_CONTROL_SESSIONCHANGE => Event::SessionChange,
        SERVICE_CONTROL_INTERROGATE => return NO_ERROR.0,
        _ => return ERROR_CALL_NOT_IMPLEMENTED.0,
    };
    if let Some(tx) = EVENTS.get().and_then(|m| m.lock().ok()?.clone()) {
        let _ = tx.send(event);
    }
    NO_ERROR.0
}

fn set_status(state: SERVICE_STATUS_CURRENT_STATE, accepted: u32, exit_code: u32) {
    let handle = STATUS.load(Ordering::Relaxed);
    if handle == 0 {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_RUNNING || state == SERVICE_STOPPED {
            0
        } else {
            10_000
        },
    };
    // SAFETY: the handle came from RegisterServiceCtrlHandlerExW and is valid for the whole
    // life of the service; `status` lives during the call.
    let _ = unsafe { SetServiceStatus(SERVICE_STATUS_HANDLE(handle as *mut _), &status) };
}

fn register_handler() -> io::Result<()> {
    let name = wide(SERVICE_NAME);
    // SAFETY: the name is null-terminated; the handler is a static function with the
    // expected signature and needs no context.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(control_handler), None)
    }
    .map_err(win_err)?;
    STATUS.store(handle.0 as usize, Ordering::Relaxed);
    Ok(())
}

fn service_body() -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let slot = EVENTS.get_or_init(|| Mutex::new(None));
    if let Ok(mut s) = slot.lock() {
        *s = Some(tx.clone());
    }
    register_handler()?;
    set_status(SERVICE_START_PENDING, 0, 0);
    if let Ok(dir) = service_log_dir("service") {
        let _ = crate::logging::init(&dir);
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "secure desktop service starting"
    );
    warn_if_sas_disallowed();

    let exe = std::env::current_exe()?;
    let mut supervisor = Supervisor::new(WinSessionHost::new(exe, tx), Backoff::default());
    set_status(
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE,
        0,
    );
    let mut event = Event::Poll;
    loop {
        for action in supervisor.handle(event, Instant::now()) {
            log(&action);
        }
        if supervisor.is_stopped() {
            break;
        }
        event = match rx.recv_timeout(POLL) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => Event::Poll,
            Err(RecvTimeoutError::Disconnected) => Event::Stop,
        };
    }
    tracing::info!("secure desktop service stopped");
    Ok(())
}

fn log(action: &Action) {
    match action {
        Action::Exited { .. } | Action::SpawnFailed { .. } | Action::SasFailed(_) => {
            tracing::warn!(?action, "helper")
        }
        _ => tracing::info!(?action, "helper"),
    }
}

/// `SendSAS` reports nothing: if the policy does not allow services, at least the log says so.
fn warn_if_sas_disallowed() {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let (k, v) = (wide(SAS_POLICY_KEY), wide(SAS_POLICY_VALUE));
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
    if err.is_err() || data & 1 == 0 {
        tracing::warn!(
            policy = SAS_POLICY_VALUE,
            value = data,
            "Ctrl+Alt+Del from Tandem will not work: the policy does not allow services (reinstall the service)"
        );
    }
}
