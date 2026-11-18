//! La MAC de la tarjeta Ethernet de esta máquina, para anunciarla a la otra y que pueda
//! despertarla por Wake-on-LAN (T-159). Los analizadores son puros y se prueban en Linux; la
//! lectura real del sistema es lo único que depende de la plataforma.

use crate::MacAddr;

/// Linux: de `/sys/class/net`, cada interfaz con su `address`, si tiene dispositivo físico y
/// si es inalámbrica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysNic {
    pub name: String,
    pub address: String,
    pub physical: bool,
    pub wireless: bool,
    pub up: bool,
}

/// Primero la que tiene cable (`operstate` `up`), luego por nombre.
pub fn pick_linux(nics: &[SysNic]) -> Option<MacAddr> {
    let mut found: Vec<(&SysNic, MacAddr)> = nics
        .iter()
        .filter(|n| n.physical && !n.wireless)
        .filter_map(|n| usable(&n.address).map(|m| (n, m)))
        .collect();
    found.sort_by(|(a, _), (b, _)| b.up.cmp(&a.up).then_with(|| a.name.cmp(&b.name)));
    found.first().map(|(_, m)| *m)
}

/// macOS: la salida de `networksetup -listallhardwareports`. Se toma el primer puerto cuyo
/// nombre no es Wi-Fi/AirPort, puente ni de otro tipo que no sea Ethernet.
pub fn pick_macos(listing: &str) -> Option<MacAddr> {
    let mut port = String::new();
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("Hardware Port:") {
            port = p.trim().to_owned();
        } else if let Some(a) = line.strip_prefix("Ethernet Address:") {
            let lower = port.to_ascii_lowercase();
            let skip = ["wi-fi", "airport", "bridge", "bluetooth", "iphone", "ipad"];
            if !skip.iter().any(|s| lower.contains(s))
                && let Some(mac) = usable(a.trim())
            {
                return Some(mac);
            }
        }
    }
    None
}

/// Windows: `Get-NetAdapter -Physical` como `Nombre,Mac,Estado,Medio` por línea. Primero los
/// 802.3 con cable (`Up`).
pub fn pick_windows(csv: &str) -> Option<MacAddr> {
    let mut rows: Vec<(bool, MacAddr)> = csv
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(|f| f.trim().trim_matches('"')).collect();
            let [_, mac, status, media] = f[..] else {
                return None;
            };
            (media == "802.3")
                .then(|| usable(mac))
                .flatten()
                .map(|m| (status.eq_ignore_ascii_case("up"), m))
        })
        .collect();
    rows.sort_by_key(|(up, _)| !*up);
    rows.first().map(|(_, m)| *m)
}

/// Una MAC que sirve para despertar: bien formada y no toda ceros.
fn usable(text: &str) -> Option<MacAddr> {
    let mac: MacAddr = text.parse().ok()?;
    (mac.0 != [0; 6]).then_some(mac)
}

/// La MAC Ethernet de esta máquina según su sistema; `None` si no se pudo saber.
pub fn detect() -> Option<MacAddr> {
    system()
}

#[cfg(target_os = "linux")]
fn system() -> Option<MacAddr> {
    let read = |dir: &std::path::Path, f: &str| {
        std::fs::read_to_string(dir.join(f))
            .map(|s| s.trim().to_owned())
            .ok()
    };
    let nics: Vec<SysNic> = std::fs::read_dir("/sys/class/net")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let dir = e.path();
            Some(SysNic {
                name: e.file_name().to_string_lossy().into_owned(),
                address: read(&dir, "address")?,
                physical: dir.join("device").exists(),
                wireless: dir.join("wireless").exists(),
                up: read(&dir, "operstate").as_deref() == Some("up"),
            })
        })
        .collect();
    pick_linux(&nics)
}

#[cfg(target_os = "macos")]
fn system() -> Option<MacAddr> {
    let out = std::process::Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
        .ok()?;
    pick_macos(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn system() -> Option<MacAddr> {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-NetAdapter -Physical | ForEach-Object { '{0},{1},{2},{3}' -f $_.Name,$_.MacAddress,$_.Status,$_.MediaType }",
        ])
        .output()
        .ok()?;
    pick_windows(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn system() -> Option<MacAddr> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nic(name: &str, address: &str, physical: bool, wireless: bool, up: bool) -> SysNic {
        SysNic {
            name: name.into(),
            address: address.into(),
            physical,
            wireless,
            up,
        }
    }

    #[test]
    fn linux_prefers_the_wired_physical_nic_with_link() {
        let nics = [
            nic("lo", "00:00:00:00:00:00", false, false, true),
            nic("docker0", "02:42:aa:bb:cc:dd", false, false, true),
            nic("wlan0", "aa:aa:aa:aa:aa:aa", true, true, true),
            nic("enp3s0", "11:22:33:44:55:66", true, false, false),
            nic("enp4s0", "66:55:44:33:22:11", true, false, true),
        ];
        assert_eq!(pick_linux(&nics).unwrap().to_string(), "66:55:44:33:22:11");
        assert_eq!(pick_linux(&nics[..3]), None);
    }

    #[test]
    fn macos_skips_wifi_and_bridges() {
        let out = "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb:cc:00:00:01\n\n\
                   Hardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: aa:bb:cc:00:00:02\n\n\
                   Hardware Port: USB 10/100/1000 LAN\nDevice: en5\nEthernet Address: aa:bb:cc:00:00:03\n";
        assert_eq!(pick_macos(out).unwrap().to_string(), "aa:bb:cc:00:00:03");
        assert_eq!(
            pick_macos("Hardware Port: Wi-Fi\nEthernet Address: aa:bb:cc:00:00:01"),
            None
        );
    }

    #[test]
    fn windows_takes_the_ethernet_with_link_first() {
        let csv = "Wi-Fi,AA-BB-CC-00-00-01,Up,Native 802.11\n\
                   Ethernet 2,AA-BB-CC-00-00-02,Disconnected,802.3\n\
                   Ethernet,AA-BB-CC-00-00-03,Up,802.3\n";
        assert_eq!(pick_windows(csv).unwrap().to_string(), "aa:bb:cc:00:00:03");
        assert_eq!(pick_windows("basura\n"), None);
    }
}
