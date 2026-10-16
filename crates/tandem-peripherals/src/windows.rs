//! Windows: dispositivos USB presentes, por SetupAPI.
//!
//! Se enumeran los nodos del enumerador `USB` (dispositivos e interfaces de los
//! compuestos) y se saca el VID/PID de su identificador de instancia, del tipo
//! `USB\VID_046D&PID_C52B&MI_00\...`. Así un receptor inalámbrico o un teclado
//! con varias interfaces cuentan igual que en macOS.

use ::windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
};
use ::windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use ::windows::core::w;

use crate::id::UsbId;
use crate::watcher::DeviceScanner;

#[derive(Debug, thiserror::Error)]
#[error("{op}: {source}")]
pub struct ScanError {
    op: &'static str,
    #[source]
    source: ::windows::core::Error,
}

/// Escáner real de Windows.
#[derive(Debug, Default, Clone, Copy)]
pub struct SetupApiScanner;

impl SetupApiScanner {
    pub fn new() -> Self {
        SetupApiScanner
    }
}

impl DeviceScanner for SetupApiScanner {
    type Error = ScanError;

    fn scan(&mut self) -> Result<Vec<UsbId>, ScanError> {
        let set = DeviceSet::usb_present()?;
        let mut ids = Vec::new();
        for index in 0.. {
            let Some(info) = set.device(index)? else {
                break;
            };
            if let Some(id) = set
                .instance_id(&info)
                .and_then(|s| UsbId::from_windows_instance_id(&s))
            {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
}

/// Lista de dispositivos de SetupAPI; se libera al soltarla.
struct DeviceSet(HDEVINFO);

impl DeviceSet {
    fn usb_present() -> Result<Self, ScanError> {
        // SAFETY: sin GUID de clase ni ventana; el enumerador es una cadena
        // estática terminada en cero.
        let set = unsafe {
            SetupDiGetClassDevsW(None, w!("USB"), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
        }
        .map_err(|source| ScanError {
            op: "SetupDiGetClassDevsW",
            source,
        })?;
        Ok(DeviceSet(set))
    }

    /// El dispositivo `index`, o `None` al pasar el último.
    fn device(&self, index: u32) -> Result<Option<SP_DEVINFO_DATA>, ScanError> {
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: la lista es válida mientras viva `self` y `info` lleva su tamaño.
        match unsafe { SetupDiEnumDeviceInfo(self.0, index, &mut info) } {
            Ok(()) => Ok(Some(info)),
            Err(e) if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() => Ok(None),
            Err(source) => Err(ScanError {
                op: "SetupDiEnumDeviceInfo",
                source,
            }),
        }
    }

    /// Identificador de instancia. Uno que no se pueda leer (un dispositivo que
    /// se fue a mitad de la lista) se salta.
    fn instance_id(&self, info: &SP_DEVINFO_DATA) -> Option<String> {
        // Los identificadores de instancia miden como mucho 200 caracteres
        // (MAX_DEVICE_ID_LEN), más el cero final.
        let mut buf = [0u16; 256];
        // SAFETY: `info` salió de esta misma lista; el búfer y su largo van juntos
        // en el slice.
        unsafe { SetupDiGetDeviceInstanceIdW(self.0, info, Some(&mut buf), None) }.ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: la lista la creó `SetupDiGetClassDevsW` y se libera una sola vez.
        if let Err(e) = unsafe { SetupDiDestroyDeviceInfoList(self.0) } {
            tracing::warn!("SetupDiDestroyDeviceInfoList: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corre en el runner de Windows: la enumeración funciona aunque no haya
    /// ningún dispositivo USB (las máquinas virtuales a veces no tienen).
    #[test]
    fn scan_runs() {
        let ids = SetupApiScanner::new().scan().unwrap();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
