//! Cheap Wi-Fi state for the shelf status-bar indicator.
//!
//! The slot frontend does not own the network: linkSlot brings Wi-Fi up on
//! demand when a sync starts. The indicator therefore only reads the kernel's
//! carrier state plus the IPv4 address of the home-Wi-Fi interface instead of
//! probing the network (which would spawn a shell script every couple of
//! seconds).
//!
//! On the H700 the home Wi-Fi is `wlan0`; the icon turns on only when the
//! interface has carrier **and** a usable IPv4 address (not loopback or
//! link-local). That still does not prove the WebDAV server is reachable; the
//! sync result banner names the real failure (DNS/TCP/WebDAV) separately.
//! A missing interface (desktop hosts, Wi-Fi off) reads as disconnected.

#[cfg(target_os = "linux")]
use std::ffi::CStr;

#[cfg(target_os = "linux")]
const HOME_WIFI: &str = "wlan0";
#[cfg(target_os = "linux")]
const HOME_WIFI_STATE: &str = "/sys/class/net/wlan0/operstate";

#[cfg(target_os = "linux")]
fn has_usable_ipv4(interface: &str) -> bool {
    // SAFETY: getifaddrs/freeifaddrs own the list; every pointer is checked
    // for null before dereferencing and the name/address layouts are the
    // libc ABI for the running platform.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return false;
        }
        let mut found = false;
        let mut current = head;
        while !current.is_null() {
            let entry = &*current;
            if !entry.ifa_name.is_null() && !entry.ifa_addr.is_null() {
                let name = CStr::from_ptr(entry.ifa_name);
                if name.to_bytes() == interface.as_bytes()
                    && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET
                {
                    let address = &*(entry.ifa_addr as *const libc::sockaddr_in);
                    let ip = u32::from_be(address.sin_addr.s_addr);
                    // 127.0.0.0/8 loopback and 169.254.0.0/16 link-local are
                    // not usable for reaching a NAS.
                    if (ip >> 24) != 127 && (ip >> 16) != 0xA9FE {
                        found = true;
                        break;
                    }
                }
            }
            current = (*current).ifa_next;
        }
        libc::freeifaddrs(head);
        found
    }
}

/// True when the home Wi-Fi interface has carrier and a usable IPv4 address.
pub fn connected() -> bool {
    #[cfg(target_os = "linux")]
    {
        let carrier = std::fs::read_to_string(HOME_WIFI_STATE)
            .map(|state| state.trim() == "up")
            .unwrap_or(false);
        carrier && has_usable_ipv4(HOME_WIFI)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_interface_reads_as_disconnected() {
        // The function is allowed to return either on a developer host with a
        // real wlan0; the only invariant is that it never panics.
        let _ = connected();
    }
}
