//! Which monitor each screen is (T-340): the system gives, for each screen, the identity of
//! its EDID ([`DisplayKey`]), and the agent links it to the monitor it sees over DDC.
//!
//! The platform readers live next to the bounds they share an order with
//! (`macos::emulate::displays`, `windows::emulate::displays`); here is what is portable.

use tandem_core::DisplayKey;

use crate::geometry::Rect;

/// A screen as the system lists it: its bounds and, if the system says, its EDID identity.
pub type Display = (Rect, Option<DisplayKey>);

/// The registry key that holds the EDID of a monitor on Windows, from the device interface
/// name `EnumDisplayDevicesW` gives with `EDD_GET_DEVICE_INTERFACE_NAME`
/// (`\\?\DISPLAY#SAM7105#5&2b2a3f2&0&UID4352#{e6f07b5f-…}`):
/// `SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7105\5&2b2a3f2&0&UID4352\Device Parameters`
/// under `HKEY_LOCAL_MACHINE`, value `EDID`.
pub fn windows_edid_key(interface: &str) -> Option<String> {
    let rest = interface
        .strip_prefix(r"\\?\")
        .or_else(|| interface.strip_prefix(r"\\.\"))?;
    let mut parts = rest.split('#');
    let (class, hardware, instance) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = |s: &str| !s.is_empty() && !s.contains(['\\', '/']);
    if !class.eq_ignore_ascii_case("DISPLAY") || !ok(hardware) || !ok(instance) {
        return None;
    }
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{hardware}\{instance}\Device Parameters"
    ))
}

/// The CoreGraphics numbers of a screen as an EDID identity: `CGDisplayVendorNumber`,
/// `CGDisplayModelNumber` and `CGDisplaySerialNumber` are the EDID's manufacturer, product and
/// serial. A vendor that does not fit in 16 bits (`kDisplayVendorIDUnknown`) or is 0 says nothing.
pub fn mac_display_key(vendor: u32, model: u32, serial: u32) -> Option<DisplayKey> {
    let vendor = u16::try_from(vendor).ok().filter(|v| *v != 0)?;
    Some(DisplayKey {
        vendor,
        product: u16::try_from(model).ok()?,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edid_key_comes_from_the_interface_name() {
        assert_eq!(
            windows_edid_key(
                r"\\?\DISPLAY#SAM7105#5&2b2a3f2&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7105\5&2b2a3f2&0&UID4352\Device Parameters"
            )
        );
        assert_eq!(windows_edid_key(r"\\?\MONITOR#SAM7105#1#{x}"), None);
        assert_eq!(windows_edid_key(r"\\?\DISPLAY#SAM7105"), None);
        assert_eq!(windows_edid_key(r"\\?\DISPLAY#..\..#1#{x}"), None);
        assert_eq!(windows_edid_key("DISPLAY#SAM7105#1#{x}"), None);
    }

    #[test]
    fn the_coregraphics_numbers_are_the_edid_identity() {
        assert_eq!(
            mac_display_key(0x4C2D, 0x7105, 7),
            Some(DisplayKey {
                vendor: 0x4C2D,
                product: 0x7105,
                serial: 7
            })
        );
        assert_eq!(mac_display_key(0x756E_6B6E, 1, 1), None, "unknown vendor");
        assert_eq!(mac_display_key(0, 1, 1), None);
        assert_eq!(mac_display_key(0x610, 0x1_0000, 1), None);
    }
}
