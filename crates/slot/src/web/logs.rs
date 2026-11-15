//! `GET /api/logs/export`: one stored zip with the card's logs, so a crash can
//! be handed over in a single file. The name carries the machine and the local
//! time, e.g. `rg34xx_20261029223000_log.zip`.

use std::path::Path;

use crate::web::http::{self, Response};
use crate::web::Shared;

/// One huge log is still worth its tail, but not its whole size on a card that
/// is also the game library.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Whole-archive cap; the browser has to hold this in memory.
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) fn export(shared: &Shared) -> Response {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    collect(&shared.root.join("linkSlot/Logs"), "linkSlot/Logs", &mut files, &mut total);
    // The version manifest places the logs (which build reported the crash).
    if let Ok(bytes) = std::fs::read(shared.root.join("linkSlot/version.json")) {
        if bytes.len() as u64 <= MAX_FILE_BYTES {
            files.push(("linkSlot/version.json".to_string(), bytes));
        }
    }
    if files.is_empty() {
        return Response::error(404, "没有可导出的日志");
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let body = zip_stored(&files);
    let name = format!(
        "{}_{}_log.zip",
        slot_device::DeviceInfo::current().slug(),
        stamp(shared)
    );
    Response::new(200, "application/zip", body)
        .with_header(
            "Content-Disposition",
            http::content_disposition("attachment", &name),
        )
        .with_header("Cache-Control", "no-store")
}

fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, Vec<u8>)>, total: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let relative = format!("{prefix}/{name}");
        if meta.is_dir() {
            collect(&path, &relative, files, total);
            continue;
        }
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES || *total + meta.len() > MAX_TOTAL_BYTES
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if *total + bytes.len() as u64 > MAX_TOTAL_BYTES {
            continue;
        }
        *total += bytes.len() as u64;
        files.push((relative, bytes));
    }
}

/// `YYYYMMDDHHMMSS` in the card's local time (`linkSlot/Config/slot.state` carries the
/// offset), the same convention the log file names use.
fn stamp(shared: &Shared) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let offset = i64::from(slot_store::read_slot_state(&shared.root).utc_offset_min);
    slot_store::format_stamp(secs + offset * 60)
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// A zip without compression: logs are text and the archive is stored once, so
/// a tiny writer beats pulling in a compressor for this one button.
fn zip_stored(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();
        let offset = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        // extra length, comment length, disk start, internal attributes
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        // External attributes are four bytes; the old two-byte field shifted
        // every following central header by two bytes and made the archive
        // unreadable for standard zip tools.
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::{request, test_shared};
    use crate::web::http::Method;

    #[test]
    fn the_exported_archive_parses_with_a_standard_zip_reader() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Logs/crash")).unwrap();
        std::fs::write(dir.path().join("linkSlot/Logs/linkSlot.log"), b"link line\n").unwrap();
        std::fs::write(dir.path().join("linkSlot/Logs/crash/cores.log"), b"signal 11\n").unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot")).unwrap();
        std::fs::write(dir.path().join("linkSlot/version.json"), b"{\"version\":\"0.1.25\"}").unwrap();
        let shared = test_shared(dir.path().to_path_buf());
        let response = super::export(&shared);
        assert_eq!(response.status, 200);

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(response.body))
            .expect("the log archive is not a valid zip");
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "linkSlot/Logs/crash/cores.log".to_string(),
                "linkSlot/Logs/linkSlot.log".to_string(),
                "linkSlot/version.json".to_string(),
            ]
        );
        let mut text = String::new();
        use std::io::Read;
        archive
            .by_name("linkSlot/Logs/linkSlot.log")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "link line\n");
    }

    #[test]
    fn the_archive_is_a_zip_named_after_the_machine_and_the_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Logs/crash")).unwrap();
        std::fs::write(dir.path().join("linkSlot/Logs/linkSlot.log"), b"link line\n").unwrap();
        std::fs::write(dir.path().join("linkSlot/Logs/crash/cores.log"), b"signal 11\n").unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot")).unwrap();
        std::fs::write(dir.path().join("linkSlot/version.json"), b"{\"version\":\"0.1.25\"}").unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        std::fs::write(dir.path().join("linkSlot/Config/config.toml"), b"secret").unwrap();
        let shared = test_shared(dir.path().to_path_buf());

        let response = super::export(&shared);
        assert_eq!(response.status, 200);
        assert_eq!(&response.body[..4], b"PK\x03\x04", "not a zip");
        let disposition = response
            .headers
            .iter()
            .find(|(name, _)| name == "Content-Disposition")
            .map(|(_, value)| value.clone())
            .expect("no Content-Disposition");
        assert!(
            disposition.contains("_log.zip"),
            "the name lost the suffix: {disposition}"
        );
        assert!(
            disposition.contains(&format!("_{}_", super::stamp(&shared))),
            "the name lost the timestamp: {disposition}"
        );
        for name in ["linkSlot/Logs/linkSlot.log", "linkSlot/Logs/crash/cores.log", "linkSlot/version.json"] {
            assert!(
                response
                    .body
                    .windows(name.len())
                    .any(|window| window == name.as_bytes()),
                "the archive is missing {name}"
            );
        }
        assert!(
            !response
                .body
                .windows(b"secret".len())
                .any(|window| window == b"secret"),
            "the WebDAV config leaked into the archive"
        );

        // And it really is a readable zip: count the central directory entries.
        let entries = u16::from_le_bytes([
            response.body[response.body.len() - 12],
            response.body[response.body.len() - 11],
        ]);
        assert_eq!(entries, 3, "unexpected entry count");
        let _ = request(Method::Get, "/api/logs/export", "");
    }
}
