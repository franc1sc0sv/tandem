//! Apagado limpio en Windows: lo mismo que `shutdown /s /t 0`, por API.

use windows::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Shutdown::{
    InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
    SHTDN_REASON_MINOR_OTHER, SHUTDOWN_POWEROFF,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use crate::{PowerError, Shutdown};

/// Apaga la PC con `InitiateShutdownW`: sin gracia y sin forzar (los programas que
/// piden guardar cambios pueden frenarlo, como con `shutdown /s /t 0`). Sin
/// `SHUTDOWN_HYBRID`: un arranque de Inicio rápido no sería limpio (DESIGN §6).
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsShutdown;

impl Shutdown for WindowsShutdown {
    fn power_off(&self) -> Result<(), PowerError> {
        enable_shutdown_privilege()?;
        // SAFETY: sin punteros propios; máquina y mensaje nulos significan «esta» y «ninguno».
        let code = unsafe {
            InitiateShutdownW(
                PCWSTR::null(),
                PCWSTR::null(),
                0,
                SHUTDOWN_POWEROFF,
                SHTDN_REASON_FLAG_PLANNED | SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER,
            )
        };
        if code != 0 {
            return Err(PowerError::Shutdown(format!(
                "InitiateShutdownW: {}",
                windows::core::Error::from(windows::Win32::Foundation::WIN32_ERROR(code))
            )));
        }
        Ok(())
    }
}

/// `SeShutdownPrivilege` existe en el token (SYSTEM y administradores lo tienen) pero
/// viene deshabilitado: hay que activarlo antes de pedir el apagado.
fn enable_shutdown_privilege() -> Result<(), PowerError> {
    let err = |what: &str, e: windows::core::Error| PowerError::Shutdown(format!("{what}: {e}"));
    let mut token = HANDLE::default();
    // SAFETY: `token` es un HANDLE escribible; el pseudo-handle del proceso no se cierra.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .map_err(|e| err("OpenProcessToken", e))?;
    let result = adjust(token);
    // SAFETY: `token` se abrió arriba y no se usa después.
    let _ = unsafe { CloseHandle(token) };
    result.map_err(|e| err("SeShutdownPrivilege", e))
}

fn adjust(token: HANDLE) -> windows::core::Result<()> {
    let mut luid = LUID::default();
    // SAFETY: `luid` es escribible; el nombre es una constante del sistema.
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `token` es válido y tiene TOKEN_ADJUST_PRIVILEGES; `privileges` vive durante la llamada.
    unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) }?;
    // Devuelve éxito aunque el token no tenga el privilegio: eso solo se ve en el último error.
    // SAFETY: GetLastError no tiene precondiciones.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(windows::core::Error::from(ERROR_NOT_ALL_ASSIGNED));
    }
    Ok(())
}
