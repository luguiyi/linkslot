//! One-way import of GBA ZIP archives.
//!
//! The feature deliberately lives behind the Web API (and a mod-only bridge):
//! it extracts a single `.gba` next to the archive and removes the archive
//! only after the extracted file has been committed.  It does not teach the
//! scanner or emulator to read archives.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::web::api::{guard_write, localized_error};
use crate::web::http::{self, Request, Response};
use crate::web::Shared;

const GBA_DIR: &str = "Games/GBA";
const MAX_ROM_BYTES: u64 = 64 * 1024 * 1024;

struct Inspection {
    inner: String,
    target: PathBuf,
    status: &'static str,
    title: String,
}

pub(super) fn list(shared: &Shared, _request: &Request) -> Response {
    let dir = shared.root.join(GBA_DIR);
    let mut zips = Vec::new();
    collect_zips(&dir, &mut zips);
    zips.sort();
    let rows: Vec<Value> = zips
        .iter()
        .filter_map(|path| inspect(path).ok().map(|inspection| row(shared, path, &inspection)))
        .collect();
    let ready = rows
        .iter()
        .filter(|row| row["status"] == "ready")
        .count();
    Response::json(
        200,
        json!({
            "ok": true,
            "total": rows.len(),
            "ready": ready,
            "zips": rows,
        }),
    )
}

pub(super) fn extract(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request).map_err(|error| Response::error(error.status, error.message))?;
        let path = http::json_string(&value, "path")
            .ok_or_else(|| localized_error(shared, 400, "web_api.error.zip_path_missing"))?;
        let zip = resolve_zip(shared, &path)?;
        let inspection = inspect(&zip).map_err(|message| Response::error(400, message))?;
        match inspection.status {
            "ready" => {}
            "conflict" => {
                return Err(Response::error(
                    409,
                    format!(
                        "目标已存在，未删除 ZIP：{}",
                        relative(shared, &inspection.target)
                    ),
                ));
            }
            "multiple" => return Err(Response::error(400, "ZIP 内包含多个 .gba，无法自动选择")),
            "too_large" => return Err(Response::error(413, "ZIP 内 .gba 超过 64 MB")),
            _ => return Err(Response::error(400, "ZIP 内没有找到 .gba")),
        }

        let mut archive = open_zip(&zip).map_err(|e| Response::error(400, e))?;
        let index = single_gba_index(&mut archive)
            .ok_or_else(|| Response::error(400, "ZIP 内没有唯一 .gba"))?;
        let mut entry = archive
            .by_index(index)
            .map_err(|_| Response::error(400, "无法读取 ZIP 内的 .gba"))?;
        if entry.size() > MAX_ROM_BYTES {
            return Err(Response::error(413, "ZIP 内 .gba 超过 64 MB"));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| Response::error(400, "解压 .gba 失败"))?;
        slot_store::atomic_write(&inspection.target, &bytes)
            .map_err(|e| Response::error(500, format!("写入 .gba 失败：{e}")))?;
        std::fs::remove_file(&zip).map_err(|e| Response::error(500, format!("删除 ZIP 失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "path": relative(shared, &zip),
                "target": relative(shared, &inspection.target),
                "inner": inspection.inner,
                "title": inspection.title,
                "size": bytes.len(),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn row(shared: &Shared, zip: &Path, inspection: &Inspection) -> Value {
    let size = std::fs::metadata(zip).map(|meta| meta.len()).unwrap_or(0);
    json!({
        "path": relative(shared, zip),
        "name": zip.file_name().and_then(|name| name.to_str()).unwrap_or("archive.zip"),
        "size": size,
        "inner": inspection.inner,
        "target": relative(shared, &inspection.target),
        "status": inspection.status,
        "title": inspection.title,
    })
}

fn inspect(zip: &Path) -> Result<Inspection, String> {
    let mut archive = open_zip(zip)?;
    if archive.len() == 0 {
        return Err("ZIP 是空的".to_string());
    }
    let indices = gba_indices(&mut archive);
    let (status, inner, target, title) = match indices.len() {
        0 => ( "none", String::new(), PathBuf::new(), String::new()),
        1 => {
            let index = indices[0];
            let (inner, entry_size) = {
                let entry = archive.by_index(index).map_err(|_| "无法读取 ZIP 条目".to_string())?;
                (entry.name().replace('\\', "/"), entry.size())
            };
            let file_name = Path::new(&inner)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "ZIP 内 .gba 名称无效".to_string())?;
            let target = zip
                .parent()
                .ok_or_else(|| "ZIP 路径无效".to_string())?
                .join(file_name);
            let status = if entry_size > MAX_ROM_BYTES {
                "too_large"
            } else if target.exists() {
                "conflict"
            } else {
                "ready"
            };
            let mut head = [0u8; 0xC0];
            let mut read = 0usize;
            if entry_size >= head.len() as u64 {
                let mut entry = archive.by_index(index).map_err(|_| "无法读取 ZIP 条目".to_string())?;
                read = entry.read(&mut head).unwrap_or(0);
            }
            let title = if read == head.len() {
                clean_title(&head[0xA0..0xAC])
            } else {
                String::new()
            };
            (status, inner, target, title)
        }
        _ => ("multiple", String::new(), PathBuf::new(), String::new()),
    };
    Ok(Inspection {
        inner,
        target,
        status,
        title,
    })
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开 ZIP 失败：{e}"))?;
    zip::ZipArchive::new(file).map_err(|e| format!("ZIP 格式无效：{e}"))
}

fn gba_indices(archive: &mut zip::ZipArchive<std::fs::File>) -> Vec<usize> {
    (0..archive.len())
        .filter(|index| {
            archive
                .by_index(*index)
                .map(|entry| {
                    !entry.is_dir()
                        && Path::new(&entry.name().replace('\\', "/"))
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("gba"))
                })
                .unwrap_or(false)
        })
        .collect()
}

fn single_gba_index(archive: &mut zip::ZipArchive<std::fs::File>) -> Option<usize> {
    let indices = gba_indices(archive);
    (indices.len() == 1).then_some(indices[0])
}

fn collect_zips(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_zips(&path, out);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            out.push(path);
        }
    }
}

fn resolve_zip(shared: &Shared, relative_path: &str) -> Result<PathBuf, Response> {
    let dir = shared.root.join(GBA_DIR);
    let base = std::fs::canonicalize(&dir)
        .map_err(|_| localized_error(shared, 500, "web_api.error.rom_missing"))?;
    let candidate = shared.root.join(relative_path);
    let candidate = std::fs::canonicalize(&candidate)
        .map_err(|_| localized_error(shared, 404, "web_api.error.rom_missing"))?;
    if !candidate.starts_with(&base) {
        return Err(localized_error(shared, 403, "web_api.error.zip_outside"));
    }
    if !candidate.is_file()
        || !candidate
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return Err(Response::error(400, "只能解压 Games/GBA 下的 .zip"));
    }
    Ok(candidate)
}

fn relative(shared: &Shared, path: &Path) -> String {
    path.strip_prefix(&shared.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn clean_title(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.trim_matches(|ch: char| ch == '\0' || ch.is_whitespace())
        .to_string()
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gba_zip(dir: &Path, name: &str, inner: &str) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut rom = vec![0u8; 0xC0];
        rom[0xA0..0xA4].copy_from_slice(b"TEST");
        zip.start_file(inner, options).unwrap();
        zip.write_all(&rom).unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn a_single_gba_zip_is_ready_with_a_neighbouring_target() {
        let dir = tempfile::tempdir().unwrap();
        let zip = gba_zip(dir.path(), "Dragon Ball.zip", "nested/Dragon Ball.gba");
        let inspection = inspect(&zip).unwrap();
        assert_eq!(inspection.status, "ready");
        assert_eq!(inspection.inner, "nested/Dragon Ball.gba");
        assert_eq!(inspection.target, dir.path().join("Dragon Ball.gba"));
        assert_eq!(inspection.title, "TEST");
    }

    #[test]
    fn an_existing_target_is_a_conflict_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let zip = gba_zip(dir.path(), "Dragon Ball.zip", "Dragon Ball.gba");
        std::fs::write(dir.path().join("Dragon Ball.gba"), b"existing").unwrap();
        let inspection = inspect(&zip).unwrap();
        assert_eq!(inspection.status, "conflict");
        assert_eq!(std::fs::read(inspection.target).unwrap(), b"existing");
    }
}
