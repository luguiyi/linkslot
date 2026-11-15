//! Lightweight SNTP time synchronisation.
//!
//! The handheld has Wi-Fi but no reliable battery-backed clock, and the
//! on-screen clock is also used for timestamps and save ordering.  Once Wi-Fi
//! is connected we ask a public SNTP server for UTC in a background thread;
//! the UI thread never blocks on DNS or UDP.

use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const NTP_PORT: u16 = 123;
const NTP_UNIX_OFFSET: i64 = 2_208_988_800;
const TIMEOUT: Duration = Duration::from_secs(2);
const HOSTS: &[&str] = &["pool.ntp.org", "time.cloudflare.com", "ntp.aliyun.com"];

/// Spawn one best-effort SNTP lookup.  The receiver yields Unix UTC seconds.
pub fn spawn() -> Receiver<Result<i64, String>> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("slot-ntp".into())
        .spawn(move || {
            let _ = tx.send(sync());
        });
    rx
}

/// Query the first reachable SNTP host.
pub fn sync() -> Result<i64, String> {
    let mut errors = Vec::new();
    for host in HOSTS {
        match query(host) {
            Ok(seconds) => return Ok(seconds),
            Err(error) => errors.push(format!("{host}: {error}")),
        }
    }
    Err(errors.join("; "))
}

fn query(host: &str) -> Result<i64, String> {
    let address = (host, NTP_PORT)
        .to_socket_addrs()
        .map_err(|error| format!("DNS: {error}"))?
        .find(|address| address.is_ipv4())
        .ok_or_else(|| "没有 IPv4 地址".to_string())?;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|error| error.to_string())?;
    socket
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|error| error.to_string())?;
    socket
        .connect(address)
        .map_err(|error| error.to_string())?;

    // LI=0, VN=3, mode=3 (client).
    let mut request = [0u8; 48];
    request[0] = 0x1b;
    socket.send(&request).map_err(|error| error.to_string())?;

    let mut reply = [0u8; 48];
    let length = socket.recv(&mut reply).map_err(|error| error.to_string())?;
    if length < 48 {
        return Err("NTP 响应过短".into());
    }
    parse_reply(&reply).ok_or_else(|| "NTP 响应无效".into())
}

fn parse_reply(reply: &[u8; 48]) -> Option<i64> {
    if reply[0] & 0x07 != 4 || reply[1] == 0 || reply[1] >= 16 {
        return None;
    }
    let seconds = u32::from_be_bytes(reply[40..44].try_into().ok()?) as i64;
    let unix = seconds - NTP_UNIX_OFFSET;
    // Guard against an unset server timestamp or a spoofed/foreign packet.
    (1_577_836_800..=4_102_444_800).contains(&unix).then_some(unix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_sntp_reply_becomes_unix_seconds() {
        let mut reply = [0u8; 48];
        reply[0] = 0x24; // LI=0, VN=4, mode=4 (server)
        reply[1] = 2;
        reply[40..44].copy_from_slice(&(NTP_UNIX_OFFSET as u32 + 1_700_000_000).to_be_bytes());
        assert_eq!(parse_reply(&reply), Some(1_700_000_000));
    }

    #[test]
    fn a_client_packet_is_not_accepted_as_a_reply() {
        let reply = [0x1b; 48];
        assert_eq!(parse_reply(&reply), None);
    }
}
