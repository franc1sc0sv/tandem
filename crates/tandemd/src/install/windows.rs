//! Arranque con la sesión en la clave `Run` de `HKCU`: no pide permisos de administrador y
//! solo afecta al usuario que instala (el modo servicio SYSTEM es T-027). Y la tarea
//! programada con los privilegios más altos ([`HighestTask`], A7), que sí los pide.

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

/// El valor [`RUN_VALUE`] de `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
#[derive(Debug, Default)]
pub struct RunKey;

impl Autostart for RunKey {
    fn get(&self) -> io::Result<Option<String>> {
        let key = HSTRING::from(RUN_KEY);
        let name = HSTRING::from(RUN_VALUE);
        let mut len = 0u32;
        // SAFETY: primero solo se pide el tamaño (sin búfer); las cadenas viven hasta el final.
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
        // SAFETY: `buf` tiene `len` bytes; la API escribe como mucho eso y actualiza `len`.
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
        // RRF_RT_REG_SZ garantiza el nulo final, que no es parte del valor.
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
        // SAFETY: `key` es una clave abierta con KEY_SET_VALUE y `data` es REG_SZ terminado en nulo.
        let err = unsafe { RegSetValueExW(key.0, &name, None, REG_SZ, Some(&data)) };
        check(err)
    }

    fn remove(&mut self) -> io::Result<bool> {
        let key = HSTRING::from(RUN_KEY);
        let name = HSTRING::from(RUN_VALUE);
        // SAFETY: cadenas válidas terminadas en nulo durante la llamada.
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(err)?;
        Ok(true)
    }
}

/// La tarea programada `Tandem\tandemd`, por la API COM del Programador de tareas: el XML
/// va en memoria, sin un archivo que otro pueda cambiar antes de que lo lea un proceso elevado.
#[derive(Debug, Default)]
pub struct HighestTask;

const TASK_FOLDER: &str = "Tandem";

/// `HRESULT_FROM_WIN32` de «archivo» y «ruta» no encontrados.
const NOT_FOUND: [i32; 2] = [0x8007_0002_u32 as i32, 0x8007_0003_u32 as i32];

fn com(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("Programador de tareas: {e}"))
}

/// COM inicializado en este hilo hasta soltarlo.
struct ComScope(bool);

impl ComScope {
    fn new() -> io::Result<ComScope> {
        // SAFETY: se empareja con `CoUninitialize` en `Drop` solo si esta llamada tuvo éxito.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            // El hilo ya tiene COM en otro modo: se usa tal cual y no se desinicializa.
            return Ok(ComScope(false));
        }
        hr.ok().map_err(com)?;
        Ok(ComScope(true))
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: empareja el `CoInitializeEx` de `new`.
            unsafe { CoUninitialize() };
        }
    }
}

fn service() -> io::Result<ITaskService> {
    // SAFETY: crea el objeto COM del Programador de tareas; COM está inicializado por el llamador.
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }.map_err(com)?;
    let none = VARIANT::default();
    // SAFETY: variantes vacías = el equipo local y la cuenta actual.
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
        // SAFETY: llamadas COM sobre objetos vivos y cadenas válidas.
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
        // SAFETY: llamadas COM sobre objetos vivos y cadenas válidas.
        unsafe {
            let root = service.GetFolder(&name(r"\")).map_err(com)?;
            if root.GetFolder(&name(TASK_FOLDER)).is_err() {
                root.CreateFolder(&name(TASK_FOLDER), &none).map_err(com)?;
            }
            // Los datos de la cuenta van en el XML (`InteractiveToken` del usuario).
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
        // SAFETY: llamadas COM sobre objetos vivos y cadenas válidas.
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

/// La ACL de una ruta: lee quién la puede escribir ([`PathAcl`]). La política (lista blanca de
/// SID, dueño, ACE de solo herencia) está en [`super::acl`]; aquí solo se lee el sistema.
#[derive(Debug, Default)]
pub struct WinAcl;

/// Libera con `LocalFree` lo que devolvió una API de seguridad.
struct LocalMem(*mut c_void);

impl Drop for LocalMem {
    fn drop(&mut self) {
        // SAFETY: lo asignó una API que se libera con LocalFree y solo se libera aquí.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
    }
}

fn win32(err: WIN32_ERROR, what: &str) -> io::Result<()> {
    check(err).map_err(|e| io::Error::other(format!("{what}: {e}")))
}

/// El SID en texto (`S-1-5-…`).
///
/// # Safety
/// `sid` debe apuntar a un SID válido durante la llamada.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` es válido (lo exige quien llama); `text` queda asignado con LocalAlloc.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(com)?;
    let _free = LocalMem(text.0.cast());
    // SAFETY: Windows devuelve una cadena UTF-16 terminada en cero.
    unsafe { text.to_string() }.map_err(|e| io::Error::other(e.to_string()))
}

/// Los ACE de permiso de la DACL, ya clasificados.
///
/// # Safety
/// `dacl` debe apuntar a una ACL válida durante la llamada.
unsafe fn read_aces(dacl: *const ACL) -> io::Result<Vec<Ace>> {
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: el búfer es `size` y se pasa su tamaño exacto.
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
        // SAFETY: `i` está en rango; `raw` apunta dentro de la ACL viva.
        unsafe { GetAce(dacl, i, &mut raw) }.map_err(com)?;
        // SAFETY: todo ACE empieza con un ACE_HEADER.
        let header = unsafe { *raw.cast::<ACE_HEADER>() };
        // En los ACE de permiso (`ACCESS_ALLOWED_ACE` y el de callback) la máscara está tras la
        // cabecera (4 bytes) y el SID tras la máscara (offset 8). Los demás tipos no se leen
        // más allá de la cabecera.
        let (mask, sid) = if matches!(header.AceType, 0 | 9) {
            // SAFETY: el ACE de permiso mide al menos cabecera + máscara + SID.
            let mask = unsafe { raw.cast::<u8>().add(4).cast::<u32>().read_unaligned() };
            // SAFETY: el SID empieza en el byte 8 del ACE.
            let sid = PSID(unsafe { raw.cast::<u8>().add(8) }.cast());
            // SAFETY: el SID está dentro del ACE, vivo con la ACL.
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

/// `FILE_ATTRIBUTE_REPARSE_POINT`: junction o enlace simbólico.
const REPARSE_POINT: u32 = 0x400;

impl PathAcl for WinAcl {
    fn admin_only(&self, path: &Path, rigor: Rigor) -> io::Result<bool> {
        use std::os::windows::fs::MetadataExt;

        // `GetNamedSecurityInfoW` sigue los enlaces y la ruta del XML no: se rechazan.
        if std::fs::symlink_metadata(path)?.file_attributes() & REPARSE_POINT != 0 {
            return Ok(false);
        }
        let wide = HSTRING::from(path.as_os_str());
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: las salidas son punteros locales; `sd` se libera abajo y dueño y DACL viven
        // dentro de él.
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
        // Sin DACL todo el mundo tiene acceso total; sin dueño no se puede confiar.
        if dacl.is_null() || owner.0.is_null() {
            return Ok(false);
        }
        // SAFETY: `owner` y `dacl` apuntan dentro del descriptor `_sd`, vivo hasta el final.
        let (owner, aces) = unsafe { (sid_text(owner)?, read_aces(dacl)?) };
        match verdict(&owner, &aces, rigor) {
            Ok(()) => Ok(true),
            Err(why) => {
                tracing::debug!(path = %path.display(), %why, "ACL no es solo de administradores");
                Ok(false)
            }
        }
    }
}

/// El token del proceso está elevado (se ejecuta como administrador).
fn is_elevated() -> bool {
    use windows::Win32::Security::{TOKEN_ELEVATION, TokenElevation};

    let mut token = HANDLE::default();
    // SAFETY: `token` recibe un identificador nuevo que se cierra abajo.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: el búfer es `elevation` y se pasa su tamaño exacto; `token` es válido.
    let got = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    // SAFETY: `token` se abrió arriba y no se usa después.
    let _ = unsafe { CloseHandle(token) };
    got.is_ok() && elevation.TokenIsElevated != 0
}

/// Se cierra al soltarla.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: la abrió `open_run_key` y nadie más la cierra.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open_run_key() -> io::Result<Key> {
    let mut key = HKEY::default();
    let path = HSTRING::from(RUN_KEY);
    // SAFETY: `key` recibe la clave abierta; la clave existe en todo Windows pero se crea si falta.
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

    /// Solo lectura: las ACL reales de `C:\` y `C:\Program Files` de un Windows normal pasan
    /// como ancestros (en `C:\` el `(AD)` de Authenticated Users no cuenta).
    #[test]
    fn root_and_program_files_pass_as_ancestors() {
        for p in ["C:\\", "C:\\Program Files"] {
            assert!(
                WinAcl.admin_only(Path::new(p), Rigor::Ancestor).unwrap(),
                "{p}"
            );
        }
    }

    /// Toca el registro real del usuario, así que deja el valor como estaba.
    #[test]
    fn round_trip_in_hkcu() {
        let mut run = RunKey;
        let before = run.get().unwrap();
        run.set(r#""C:\Ruta con espacios\tandemw.exe""#).unwrap();
        assert_eq!(
            run.get().unwrap().as_deref(),
            Some(r#""C:\Ruta con espacios\tandemw.exe""#)
        );
        assert!(run.remove().unwrap());
        assert_eq!(run.get().unwrap(), None);
        assert!(!run.remove().unwrap());
        if let Some(cmd) = before {
            run.set(&cmd).unwrap();
        }
    }
}
