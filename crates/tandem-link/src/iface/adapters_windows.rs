use std::collections::BTreeMap;

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GAA_FLAG_SKIP_UNICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::Networking::WinSock::AF_UNSPEC;

/// Description of each adapter (`IP_ADAPTER_ADDRESSES::Description`) by `IfIndex`. Empty if
/// the call fails: the kind then falls back to the friendly name.
pub(super) fn descriptions() -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    let Some(buf) = adapters() else {
        return out;
    };
    let mut cur = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !cur.is_null() {
        // SAFETY: `cur` points into `buf`, filled by `GetAdaptersAddresses` as a linked list
        // of `IP_ADAPTER_ADDRESSES_LH` that lives as long as `buf`; `Next` is null at the end.
        let a = unsafe { &*cur };
        // SAFETY: `IfIndex` is valid in every version of the struct (`Length` covers it).
        let index = unsafe { a.Anonymous1.Anonymous.IfIndex };
        if !a.Description.is_null() {
            // SAFETY: `Description` is a NUL-terminated UTF-16 string inside `buf`.
            if let Ok(d) = unsafe { a.Description.to_string() } {
                out.insert(index, d);
            }
        }
        cur = a.Next;
    }
    out
}

/// The raw `GetAdaptersAddresses` buffer (addresses skipped: only names are needed). Kept as
/// `u64` words so the structs inside are aligned.
fn adapters() -> Option<Vec<u64>> {
    let flags = GAA_FLAG_SKIP_UNICAST
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    // The list can grow between calls (an adapter plugged in): retry a few times.
    for _ in 0..4 {
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `size` bytes, aligned to 8, and outlives the call.
        let rc = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                &mut size,
            )
        };
        if rc == NO_ERROR.0 {
            return Some(buf);
        }
        if rc != ERROR_BUFFER_OVERFLOW.0 {
            return None;
        }
    }
    None
}
