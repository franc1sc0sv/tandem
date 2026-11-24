//! Windows: USB devices present, through SetupAPI.
//!
//! The nodes of the `USB` enumerator are listed (devices and interfaces of
//! composites) and the VID/PID is taken from their instance ID, of the form
//! `USB\VID_046D&PID_C52B&MI_00\...`. This way a wireless receiver or a keyboard
//! with several interfaces counts the same as on macOS.

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

/// Real Windows scanner.
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

/// SetupAPI device list; freed when dropped.
struct DeviceSet(HDEVINFO);

impl DeviceSet {
    fn usb_present() -> Result<Self, ScanError> {
        // SAFETY: no class GUID or window; the enumerator is a zero-terminated
        // static string.
        let set = unsafe {
            SetupDiGetClassDevsW(None, w!("USB"), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
        }
        .map_err(|source| ScanError {
            op: "SetupDiGetClassDevsW",
            source,
        })?;
        Ok(DeviceSet(set))
    }

    /// The device at `index`, or `None` once past the last one.
    fn device(&self, index: u32) -> Result<Option<SP_DEVINFO_DATA>, ScanError> {
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: the list is valid while `self` lives and `info` carries its size.
        match unsafe { SetupDiEnumDeviceInfo(self.0, index, &mut info) } {
            Ok(()) => Ok(Some(info)),
            Err(e) if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() => Ok(None),
            Err(source) => Err(ScanError {
                op: "SetupDiEnumDeviceInfo",
                source,
            }),
        }
    }

    /// Instance ID. One that cannot be read (a device that
    /// left mid-list) is skipped.
    fn instance_id(&self, info: &SP_DEVINFO_DATA) -> Option<String> {
        // Instance IDs are at most 200 characters long
        // (MAX_DEVICE_ID_LEN), plus the trailing zero.
        let mut buf = [0u16; 256];
        // SAFETY: `info` came from this same list; the buffer and its length go together
        // in the slice.
        unsafe { SetupDiGetDeviceInstanceIdW(self.0, info, Some(&mut buf), None) }.ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: the list was created by `SetupDiGetClassDevsW` and is freed only once.
        if let Err(e) = unsafe { SetupDiDestroyDeviceInfoList(self.0) } {
            tracing::warn!("SetupDiDestroyDeviceInfoList: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on the Windows runner: enumeration works even when there is
    /// no USB device (virtual machines sometimes have none).
    #[test]
    fn scan_runs() {
        let ids = SetupApiScanner::new().scan().unwrap();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
