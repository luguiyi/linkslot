//! Browser play: the device is a file server for a WebAssembly emulator that
//! runs in the visitor's browser.  Nothing here emulates anything; the two
//! endpoints move the ROM in and the battery save in and out, through exactly
//! the same paths and backups the device itself uses.
//!
//! This is the only part of the web UI that carries game data, so every write
//! goes through [`guard_write`] (device writable, page header present) and a
//! conflict check against the save the browser actually loaded.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::web::api::{guard_write, localized_error, localized_error_args};
use crate::web::http::{self, Request, Response};
use crate::web::Shared;

/// A GBA ROM is at most 32 MiB, and the loader may hand us a padded one.
const ROM_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Battery saves are 512 B .. 128 KiB in practice; 4 MiB is only a stop sign.
const SAVE_MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 512;

/// The header the page sends back with the mtime of the save it opened.
pub const SAVE_STAMP_HEADER: &str = "x-save-stamp";

struct Target {
    rom: PathBuf,
    save: PathBuf,
}

/// Resolve `rom=Games/<dir>/<file>` without scanning the library: the page is
/// opened from a shelf entry, so the path is already canonical.
fn resolve(shared: &Shared, request: &Request) -> Result<Target, Response> {
    let raw = http::query_param(request, "rom")
        .map_err(|error| Response::error(error.status, error.message))?
        .ok_or_else(|| localized_error(shared, 400, "web_api.error.rom_missing"))?;
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES {
        return Err(localized_error(shared, 400, "web_api.error.rom_invalid"));
    }
    if raw.starts_with('/') || raw.contains('\\') || raw.contains('\0') {
        return Err(localized_error(shared, 400, "web_api.error.rom_invalid"));
    }
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() < 3 || parts[0] != "Games" {
        return Err(localized_error(shared, 400, "web_api.error.rom_path"));
    }
    if parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..") {
        return Err(localized_error(shared, 400, "web_api.error.rom_invalid"));
    }
    let platform = slot_store::Platform::ALL
        .into_iter()
        .find(|platform| platform.dir_name().eq_ignore_ascii_case(parts[1]))
        .ok_or_else(|| localized_error(shared, 400, "web_api.error.rom_platform"))?;
    // A library may keep games in subfolders (Games/GBA/整合包/xxx.gba); the
    // save name still follows the flat device convention (Saves/GBA/xxx.sav).
    let rom = parts[2..]
        .iter()
        .fold(shared.root.join("Games").join(platform.dir_name()), |path, part| {
            path.join(part)
        });
    if !platform.accepts(&rom) {
        return Err(localized_error(shared, 400, "web_api.error.rom_extension"));
    }
    if !rom.is_file() {
        return Err(localized_error(shared, 404, "web_api.error.rom_not_found"));
    }
    let stem = rom
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| localized_error(shared, 400, "web_api.error.rom_invalid"))?;
    let save = shared
        .root
        .join("Saves")
        .join(platform.dir_name())
        .join(format!("{stem}.sav"));
    Ok(Target { rom, save })
}

/// The save's identity is its content, not its timestamp: two writes inside
/// one filesystem tick share an mtime, and a conflict check that cannot tell
/// them apart is useless.  A battery save is 512 B .. 128 KiB, so hashing it
/// on every read/write is cheap.
fn stamp(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    crate::rom_id::sha256_file(path).ok()
}

/// `GET /api/play/rom?rom=` — the cartridge image, straight from the card.
pub(super) fn rom(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let target = resolve(shared, request)?;
        let size = std::fs::metadata(&target.rom)
            .map_err(|_| localized_error(shared, 404, "web_api.error.rom_not_found"))?
            .len();
        if size > ROM_MAX_BYTES {
            return Err(localized_error(shared, 413, "web_api.error.rom_too_large"));
        }
        let bytes = std::fs::read(&target.rom)
            .map_err(|_| localized_error(shared, 500, "web_api.error.rom_read"))?;
        Ok(Response::new(200, "application/octet-stream", bytes)
            .with_header("Cache-Control", "no-store")
            .with_header("X-Rom-Bytes", size.to_string()))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/play/save?rom=` — the battery save, or 204 when the cart has none.
pub(super) fn save(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let target = resolve(shared, request)?;
        if !target.save.is_file() {
            return Ok(Response::new(204, "application/octet-stream", Vec::new())
                .with_header("Cache-Control", "no-store"));
        }
        let bytes = std::fs::read(&target.save)
            .map_err(|_| localized_error(shared, 500, "web_api.error.save_read"))?;
        let stamp = stamp(&target.save).unwrap_or_default();
        Ok(Response::new(200, "application/octet-stream", bytes)
            .with_header("Cache-Control", "no-store")
            .with_header(SAVE_STAMP_HEADER, stamp))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/play/save?rom=` — write the browser's save back to the card.
///
/// The browser tells us the stamp of the save it opened; if the file on the
/// card changed in the meantime (the handheld played the same game), the write
/// is refused so neither side loses progress by accident.
pub(super) fn save_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let target = resolve(shared, request)?;
        if request.body.is_empty() {
            return Err(localized_error(shared, 400, "web_api.error.save_empty"));
        }
        if request.body.len() > SAVE_MAX_BYTES {
            return Err(localized_error(shared, 413, "web_api.error.save_too_large"));
        }
        let current = stamp(&target.save);
        if let Some(expect) = request.header(SAVE_STAMP_HEADER) {
            let actual = current.unwrap_or_default();
            if !expect.eq_ignore_ascii_case(&actual) {
                return Err(localized_error(shared, 409, "web_api.error.save_changed"));
            }
        }
        if target.save.is_file() {
            let previous = std::fs::read(&target.save).unwrap_or_default();
            if previous != request.body {
                let backup = target.save.with_extension("sav.bak");
                std::fs::write(&backup, &previous)
                    .map_err(|_| localized_error(shared, 500, "web_api.error.save_backup"))?;
            }
        }
        if let Some(parent) = target.save.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        slot_store::atomic_write(&target.save, &request.body)
            .map_err(|error| localized_error_args(
                shared,
                500,
                "web_api.error.save_write",
                &[("message", error.to_string().as_str())],
            ))?;
        let stamp = stamp(&target.save).unwrap_or_default();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "size": request.body.len(),
                "stamp": stamp,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}
