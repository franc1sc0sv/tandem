//! Identificador USB: fabricante (VID) y producto (PID).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// VID/PID de un dispositivo USB. En la config se escribe como `"046d:c52b"`, el
/// mismo formato que `lsusb` y el Informe del sistema de macOS (en hexadecimal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsbId {
    pub vendor: u16,
    pub product: u16,
}

impl UsbId {
    pub const fn new(vendor: u16, product: u16) -> Self {
        UsbId { vendor, product }
    }

    /// Saca el VID/PID de un identificador de instancia de Windows, como
    /// `USB\VID_046D&PID_C52B&MI_00\7&1A2B3C&0&0000` o el nombre de un dispositivo
    /// de Raw Input (`\\?\HID#VID_046D&PID_C52B#...`). Los que no lo traen
    /// (concentradores raíz, por ejemplo) dan `None`.
    pub fn from_windows_instance_id(id: &str) -> Option<Self> {
        let upper = id.to_ascii_uppercase();
        let vendor = hex_after(&upper, "VID_")?;
        let product = hex_after(&upper, "PID_")?;
        Some(UsbId { vendor, product })
    }
}

/// Los cuatro dígitos hexadecimales que siguen a `tag`.
fn hex_after(s: &str, tag: &str) -> Option<u16> {
    let start = s.find(tag)? + tag.len();
    let digits = s.get(start..start + 4)?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

impl fmt::Display for UsbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor, self.product)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("identificador USB inválido «{0}»: se espera VID:PID en hexadecimal, como 046d:c52b")]
pub struct ParseUsbIdError(String);

impl FromStr for UsbId {
    type Err = ParseUsbIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseUsbIdError(s.to_owned());
        let (vendor, product) = s.trim().split_once(':').ok_or_else(err)?;
        let part = |p: &str| {
            let p = p.trim();
            let ok = (1..=4).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_hexdigit());
            ok.then(|| u16::from_str_radix(p, 16).ok()).flatten()
        };
        Ok(UsbId {
            vendor: part(vendor).ok_or_else(err)?,
            product: part(product).ok_or_else(err)?,
        })
    }
}

impl Serialize for UsbId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UsbId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn parses_and_prints_vid_pid() {
        let id: UsbId = "046D:c52b".parse().unwrap();
        assert_eq!(id, UsbId::new(0x046d, 0xc52b));
        assert_eq!(id.to_string(), "046d:c52b");
        assert_eq!(
            " 5ac : 24f ".parse::<UsbId>().unwrap(),
            UsbId::new(0x05ac, 0x024f)
        );
    }

    #[test]
    fn rejects_malformed_ids() {
        for bad in [
            "",
            "046d",
            "046d:",
            ":c52b",
            "046d:c52bb",
            "zz00:0000",
            "046d-c52b",
        ] {
            assert!(bad.parse::<UsbId>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn reads_windows_instance_ids() {
        assert_eq!(
            UsbId::from_windows_instance_id(r"USB\VID_046D&PID_C52B&MI_00\7&1A2B3C&0&0000"),
            Some(UsbId::new(0x046d, 0xc52b))
        );
        assert_eq!(
            UsbId::from_windows_instance_id(r"USB\VID_05ac&PID_024f\5&2C0E0B5&0&3"),
            Some(UsbId::new(0x05ac, 0x024f))
        );
        assert_eq!(
            UsbId::from_windows_instance_id(r"\\?\HID#VID_1532&PID_0084&MI_00#8&35a2#{4d1e55b2}"),
            Some(UsbId::new(0x1532, 0x0084))
        );
        assert_eq!(
            UsbId::from_windows_instance_id(r"USB\ROOT_HUB30\4&1&0"),
            None
        );
        assert_eq!(
            UsbId::from_windows_instance_id(r"USB\VID_04&PID_C52B\x"),
            None
        );
        assert_eq!(UsbId::from_windows_instance_id(r"USB\VID_046D"), None);
    }

    #[test]
    fn deserializes_from_config_string() {
        #[derive(Deserialize)]
        struct T {
            id: UsbId,
        }
        let t: T = toml::from_str(r#"id = "046d:c52b""#).unwrap();
        assert_eq!(t.id, UsbId::new(0x046d, 0xc52b));
        assert!(toml::from_str::<T>(r#"id = "teclado""#).is_err());
    }

    proptest! {
        #[test]
        fn display_round_trips(vendor: u16, product: u16) {
            let id = UsbId::new(vendor, product);
            prop_assert_eq!(id.to_string().parse::<UsbId>(), Ok(id));
            let win = format!(r"USB\VID_{vendor:04X}&PID_{product:04X}\serie");
            prop_assert_eq!(UsbId::from_windows_instance_id(&win), Some(id));
        }
    }
}
