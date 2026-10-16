use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Dirección MAC de la tarjeta Ethernet de la PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddr(pub [u8; 6]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "dirección MAC inválida `{0}`: se esperan 6 bytes en hexadecimal, como `aa:bb:cc:dd:ee:ff`"
)]
pub struct ParseMacError(pub String);

impl FromStr for MacAddr {
    type Err = ParseMacError;

    /// Acepta `aa:bb:cc:dd:ee:ff` y `aa-bb-cc-dd-ee-ff` (la forma de `ipconfig /all`),
    /// en mayúsculas o minúsculas.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseMacError(s.to_owned());
        let sep = if s.contains('-') { '-' } else { ':' };
        let mut out = [0u8; 6];
        let mut parts = s.split(sep);
        for byte in &mut out {
            let part = parts.next().ok_or_else(err)?;
            // `from_str_radix` aceptaría un `+` delante.
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(err());
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| err())?;
        }
        if parts.next().is_some() {
            return Err(err());
        }
        Ok(MacAddr(out))
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

impl Serialize for MacAddr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MacAddr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const PC: MacAddr = MacAddr([0x9c, 0x6b, 0x00, 0x1a, 0x2b, 0xff]);

    #[test]
    fn parses_colon_and_dash_forms() {
        assert_eq!("9c:6b:00:1a:2b:ff".parse::<MacAddr>().unwrap(), PC);
        assert_eq!("9C-6B-00-1A-2B-FF".parse::<MacAddr>().unwrap(), PC);
        assert_eq!(PC.to_string(), "9c:6b:00:1a:2b:ff");
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "9c:6b:00:1a:2b",
            "9c:6b:00:1a:2b:ff:00",
            "9c:6b:00:1a:2b:f",
            "9c:6b:00:1a:2b:fff",
            "9c:6b:00:1a:2b:zz",
            "9c6b001a2bff",
            "9c:6b-00:1a:2b:ff",
            "+c:6b:00:1a:2b:ff",
        ] {
            assert!(
                bad.parse::<MacAddr>().is_err(),
                "`{bad}` no debería ser válida"
            );
        }
    }

    #[test]
    fn deserializes_from_toml_string() {
        #[derive(serde::Deserialize)]
        struct T {
            mac: MacAddr,
        }
        let t: T = toml::from_str("mac = \"9c-6b-00-1a-2b-ff\"").unwrap();
        assert_eq!(t.mac, PC);
        assert!(toml::from_str::<T>("mac = \"nope\"").is_err());
    }

    proptest! {
        #[test]
        fn display_roundtrips(bytes in any::<[u8; 6]>()) {
            let mac = MacAddr(bytes);
            prop_assert_eq!(mac.to_string().parse::<MacAddr>().unwrap(), mac);
            prop_assert_eq!(mac.to_string().replace(':', "-").to_uppercase().parse::<MacAddr>().unwrap(), mac);
        }
    }
}
