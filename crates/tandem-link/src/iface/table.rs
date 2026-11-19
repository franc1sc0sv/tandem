use std::collections::BTreeMap;
use std::net::IpAddr;

use super::{InterfaceKind, NetInterface, classify};

/// One address row as the OS lists it (`getifaddrs` / `GetAdaptersAddresses`): an interface
/// with several addresses shows up once per address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrRow {
    pub name: String,
    pub index: u32,
    /// Hardware description (Windows only). The friendly name of the USB4 P2P adapter is a
    /// user-editable "Ethernet 3"; only the description says "USB4(TM) P2P Network Adapter".
    pub description: Option<String>,
    pub ip: IpAddr,
    pub up: bool,
}

/// Kind of an adapter from its name and, if there is one, its hardware description. The
/// description wins when it recognizes something, since the name can be anything.
pub fn classify_adapter(name: &str, description: Option<&str>) -> InterfaceKind {
    match description.map(classify) {
        Some(kind) if kind != InterfaceKind::Other => kind,
        _ => classify(name),
    }
}

/// The interface list out of the OS address rows: one entry per name with all its addresses,
/// up if any of its rows is.
pub fn interfaces_from_rows(rows: impl IntoIterator<Item = AddrRow>) -> Vec<NetInterface> {
    let mut by_name: BTreeMap<String, NetInterface> = BTreeMap::new();
    for row in rows {
        let entry = by_name
            .entry(row.name.clone())
            .or_insert_with(|| NetInterface {
                index: row.index,
                kind: classify_adapter(&row.name, row.description.as_deref()),
                name: row.name.clone(),
                addrs: Vec::new(),
                mac: None,
                speed_mbps: None,
                up: false,
            });
        entry.addrs.push(row.ip);
        entry.up |= row.up;
    }
    by_name.into_values().collect()
}
