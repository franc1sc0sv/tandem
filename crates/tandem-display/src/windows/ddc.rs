//! DDC/CI real por `dxva2` (Monitor Configuration API).
//!
//! Enumeración: cada `HMONITOR` del escritorio da uno o más monitores físicos
//! (`GetPhysicalMonitorsFromHMONITOR`). La identidad sale del EDID que Windows guarda en
//! el registro, encontrado a partir de la ruta de interfaz del monitor
//! (`EnumDisplayDevicesW` con `EDD_GET_DEVICE_INTERFACE_NAME`).

use ::windows::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitors, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, SetVCPFeature,
};
use ::windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use ::windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors,
    GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use ::windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use ::windows::core::{BOOL, HSTRING, w};

use crate::ddc::{DdcBus, DdcError, MonitorInfo, VcpValue, edid};

/// `EDD_GET_DEVICE_INTERFACE_NAME` (winuser.h); definido aquí para no arrastrar toda la
/// feature `Win32_UI_WindowsAndMessaging` por una constante.
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
/// Un EDID con extensiones no pasa de unos pocos bloques de 128 bytes.
const EDID_MAX: usize = 4096;

fn os_error(op: &'static str) -> DdcError {
    let e = ::windows::core::Error::from_thread();
    DdcError::Os {
        op,
        // Se guarda el HRESULT tal cual (bits), como lo muestran las herramientas de Windows.
        code: e.code().0 as u32,
        message: e.message(),
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Monitores físicos abiertos. Los handles se liberan al soltar el valor.
pub struct WindowsDdc {
    infos: Vec<MonitorInfo>,
    handles: Vec<PHYSICAL_MONITOR>,
}

impl WindowsDdc {
    pub fn open() -> Result<Self, DdcError> {
        let mut infos = Vec::new();
        let mut handles = Vec::new();
        for hmon in desktop_monitors()? {
            let device = device_name(hmon).unwrap_or_default();
            let interfaces = if device.is_empty() {
                Vec::new()
            } else {
                active_interfaces(&device)
            };
            // Un HMONITOR sin DDC (por ejemplo, una pantalla virtual) no es un error.
            let Ok(physical) = physical_monitors(hmon) else {
                continue;
            };
            for (k, pm) in physical.into_iter().enumerate() {
                // Se emparejan por orden: con un monitor por salida (lo normal) es exacto.
                let edid = interfaces
                    .get(k)
                    .and_then(|path| edid::registry_key_from_interface(path))
                    .and_then(|key| read_edid(&key))
                    .and_then(|bytes| edid::parse(&bytes).ok());
                infos.push(MonitorInfo {
                    index: infos.len(),
                    // PHYSICAL_MONITOR es `packed`: se copia el arreglo antes de tomar referencias.
                    description: wide_to_string(&{ pm.szPhysicalMonitorDescription }),
                    device: device.clone(),
                    edid,
                });
                handles.push(pm);
            }
        }
        Ok(Self { infos, handles })
    }

    fn handle(&self, monitor: usize) -> Result<&PHYSICAL_MONITOR, DdcError> {
        self.handles
            .get(monitor)
            .ok_or(DdcError::NoSuchMonitor(monitor))
    }
}

impl Drop for WindowsDdc {
    fn drop(&mut self) {
        if !self.handles.is_empty() {
            // SAFETY: los handles salieron de GetPhysicalMonitorsFromHMONITOR y no se liberaron antes.
            let _ = unsafe { DestroyPhysicalMonitors(&self.handles) };
        }
    }
}

impl DdcBus for WindowsDdc {
    fn monitors(&self) -> &[MonitorInfo] {
        &self.infos
    }

    fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError> {
        let h = self.handle(monitor)?.hPhysicalMonitor;
        let mut len = 0u32;
        // SAFETY: `h` es un handle físico vivo y `len` es un u32 válido para escribir.
        if unsafe { GetCapabilitiesStringLength(h, &mut len) } == 0 {
            return Err(os_error("GetCapabilitiesStringLength"));
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` mide exactamente lo que pide la API (largo en caracteres ASCII).
        if unsafe { CapabilitiesRequestAndCapabilitiesReply(h, &mut buf) } == 0 {
            return Err(os_error("CapabilitiesRequestAndCapabilitiesReply"));
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<VcpValue, DdcError> {
        let h = self.handle(monitor)?.hPhysicalMonitor;
        let (mut current, mut maximum) = (0u32, 0u32);
        // SAFETY: `h` es un handle físico vivo; los punteros apuntan a u32 locales.
        let ok = unsafe {
            GetVCPFeatureAndVCPFeatureReply(h, code, None, &mut current, Some(&mut maximum))
        };
        if ok == 0 {
            return Err(os_error("GetVCPFeatureAndVCPFeatureReply"));
        }
        Ok(VcpValue { current, maximum })
    }

    fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError> {
        let h = self.handle(monitor)?.hPhysicalMonitor;
        // SAFETY: `h` es un handle físico vivo.
        if unsafe { SetVCPFeature(h, code, value) } == 0 {
            return Err(os_error("SetVCPFeature"));
        }
        Ok(())
    }
}

unsafe extern "system" fn collect_monitor(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` es el `&mut Vec<HMONITOR>` que pasa `desktop_monitors`, vivo durante
    // toda la llamada síncrona a EnumDisplayMonitors.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(hmon);
    true.into()
}

fn desktop_monitors() -> Result<Vec<HMONITOR>, DdcError> {
    let mut list: Vec<HMONITOR> = Vec::new();
    // SAFETY: el callback solo usa `list` mientras dura esta llamada.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut list as *mut Vec<HMONITOR> as isize),
        )
    };
    if !ok.as_bool() {
        return Err(os_error("EnumDisplayMonitors"));
    }
    Ok(list)
}

/// `\\.\DISPLAY1`: la salida del escritorio que corresponde al HMONITOR.
fn device_name(hmon: HMONITOR) -> Option<String> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: MONITORINFOEXW empieza con MONITORINFO y `cbSize` indica la versión extendida.
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    ok.as_bool().then(|| wide_to_string(&info.szDevice))
}

/// Rutas de interfaz de los monitores activos en una salida.
fn active_interfaces(device: &str) -> Vec<String> {
    let device = HSTRING::from(device);
    let mut out = Vec::new();
    for i in 0.. {
        let mut dd = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `dd` está inicializado con su tamaño; `device` vive durante la llamada.
        let ok = unsafe { EnumDisplayDevicesW(&device, i, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) };
        if !ok.as_bool() {
            break;
        }
        if dd.StateFlags.0 & DISPLAY_DEVICE_ACTIVE.0 != 0 {
            out.push(wide_to_string(&dd.DeviceID));
        }
    }
    out
}

fn physical_monitors(hmon: HMONITOR) -> Result<Vec<PHYSICAL_MONITOR>, DdcError> {
    let mut n = 0u32;
    // SAFETY: `n` es un u32 local válido para escribir.
    unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n) }
        .map_err(|_| os_error("GetNumberOfPhysicalMonitorsFromHMONITOR"))?;
    let mut list = vec![PHYSICAL_MONITOR::default(); n as usize];
    if list.is_empty() {
        return Ok(list);
    }
    // SAFETY: `list` tiene exactamente `n` elementos, como pide la API.
    unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, &mut list) }
        .map_err(|_| os_error("GetPhysicalMonitorsFromHMONITOR"))?;
    Ok(list)
}

fn read_edid(subkey: &str) -> Option<Vec<u8>> {
    let subkey = HSTRING::from(subkey);
    let mut buf = vec![0u8; EDID_MAX];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` mide `len` bytes; RegGetValueW escribe como mucho eso y actualiza `len`.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &subkey,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// En el runner de CI no hay monitores con DDC; abrir no debe fallar ni colgarse.
    #[test]
    fn open_without_ddc_monitors_does_not_fail() {
        let bus = WindowsDdc::open().unwrap();
        for (i, m) in bus.monitors().iter().enumerate() {
            assert_eq!(m.index, i);
        }
    }

    #[test]
    fn wide_strings_stop_at_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "ab".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wide_to_string(&buf), "ab");
        assert_eq!(wide_to_string(&[]), "");
    }

    /// Lista los monitores reales. Solo lee: no cambia nada.
    #[test]
    #[ignore = "hardware"]
    fn lists_real_monitors_with_edid() {
        let mut bus = WindowsDdc::open().unwrap();
        assert!(!bus.monitors().is_empty());
        for i in 0..bus.monitors().len() {
            let m = bus.monitors()[i].clone();
            assert!(m.edid.is_some(), "{m}: sin EDID");
            let caps = bus.capabilities(i).unwrap();
            crate::ddc::caps::parse(&caps).unwrap();
        }
    }
}
