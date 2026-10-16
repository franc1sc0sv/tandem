//! Arranque con la sesión en la clave `Run` de `HKCU`: no pide permisos de administrador y
//! solo afecta al usuario que instala (el modo servicio SYSTEM es T-027).

use std::io;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

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
