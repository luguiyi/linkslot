//! ROM library and the two Web editing operations: replace a cover and
//! replace an existing ROM in place.
//!
//! The browser never supplies a filesystem path for a write.  It supplies the
//! relative ROM path returned by `/api/roms`; the server scans the library,
//! finds that exact cart, and writes only its existing ROM (or its label), so
//! renaming and path changes are impossible from this API.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slot_store::{Cart, Core};

use super::api::guard_write;
use super::http::{self, Request, Response};
use super::{util, Shared};

/// A cover can be replaced without its URL changing (the tile cache-buster is
/// the ROM's mtime, not the label's), so the browser must revalidate.  The
/// revalidation is a stat plus an ETag compare, and a 304 avoids re-reading
/// the PNG from the card.
const COVER_CACHE_CONTROL: &str = "private, no-cache";
/// Manual intro files may be larger than the 256 KB upload cap; bound the read
/// anyway so a stray file cannot fill memory.
const INTRO_MAX_BYTES: u64 = 1024 * 1024;
const COVER_MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROM_PATH_BYTES: usize = 1024;

static COVER_TEMP: AtomicU64 = AtomicU64::new(0);

struct Entry {
    cart: Cart,
    rom: String,
    size: u64,
    mtime: u64,
    cover_width: u32,
    cover_height: u32,
    has_cover: bool,
    has_intro: bool,
    has_video: bool,
    color: Option<String>,
    cheat: Option<String>,
    guide: Option<String>,
    has_guide: bool,
}

/// `PUT /api/rom/model?rom=Games/FC/Game.nes` — JSON `{"model":"famicom"|"nes"}`.
/// Only the FC platform has two physical shell models.
pub(super) fn model_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let raw = value
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 model 参数"))?;
        let model = slot_store::CartModel::parse(raw)
            .ok_or_else(|| Response::error(400, "模型必须是 famicom 或 nes"))?;
        let entry = find(shared, request)?;
        if entry.cart.platform != slot_store::Platform::Fc {
            return Err(Response::error(400, "只有 FC 机种可以切换 FC/NES 卡带模型"));
        }
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            if model == slot_store::CartModel::Famicom {
                meta.extra.remove("cart_model");
            } else {
                meta.extra.insert(
                    "cart_model".to_string(),
                    Value::String(model.to_value().to_string()),
                );
            }
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "model": model.to_value(),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/rom/fronts/project?rom=...&id=...`
pub(super) fn front_project_get(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        if !slot_store::fronts::valid_id(&id) {
            return Err(Response::error(400, "前盖 id 非法"));
        }
        let project = slot_store::fronts::load_project(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &id,
        )
        .ok_or_else(|| Response::error(404, "前盖项目不存在"))?;
        Ok(Response::json(200, json!({ "project": project })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/fronts/project?rom=...&id=...` — save a layered project and
/// its flattened PNG.  The PNG is the only thing the device reads.
pub(super) fn front_project_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.len() > 24 * 1024 * 1024 {
            return Err(Response::error(413, "前盖项目过大"));
        }
        let entry = find(shared, request)?;
        let body: Value = serde_json::from_slice(&request.body)
            .map_err(|_| Response::error(400, "前盖项目不是合法 JSON"))?;
        let project = body
            .get("project")
            .ok_or_else(|| Response::error(400, "缺少 project"))?;
        let render = body
            .get("render")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 render PNG"))?;
        let png = decode_png_data_url(render)?;
        let (width, height) = png_size(&png)?;
        let (want_w, want_h) = slot_ui::front_size(entry.cart.platform, entry.cart.model);
        if (width, height) != (want_w, want_h) {
            return Err(Response::error(
                400,
                format!("前盖 PNG 必须是 {want_w}x{want_h}，收到 {width}x{height}"),
            ));
        }
        if png.len() > slot_store::fronts::MAX_FRONT_BYTES {
            return Err(Response::error(413, "前盖 PNG 过大"));
        }
        slot_store::fronts::validate_project(project, want_w, want_h)
            .map_err(|error| Response::error(400, error))?;
        let requested_id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .filter(|id| !id.is_empty());
        let id = requested_id.clone().unwrap_or_else(slot_store::fronts::new_id);
        if !slot_store::fronts::valid_id(&id) {
            return Err(Response::error(400, "前盖 id 非法"));
        }
        let mut manifest =
            slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
                .unwrap_or_else(|| slot_store::fronts::FrontManifest {
                    schema: 1,
                    ..Default::default()
                });
        let position = manifest.faces.iter().position(|face| face.id == id);
        if position.is_none() && manifest.faces.len() >= slot_store::fronts::MAX_FRONTS {
            return Err(Response::error(409, "每张卡带最多 5 个前盖"));
        }
        let current_revision = position
            .and_then(|_| {
                slot_store::fronts::load_project(
                    &shared.root,
                    entry.cart.platform,
                    &entry.cart.stem,
                    &id,
                )
            })
            .and_then(|project| project.get("revision").and_then(Value::as_u64))
            .unwrap_or(0);
        let expected_revision = body.get("revision").and_then(Value::as_u64).unwrap_or(0);
        if position.is_some() && expected_revision != current_revision {
            return Err(Response::error(409, "前盖已在另一个页面修改，请刷新后重试"));
        }
        let revision = current_revision.saturating_add(1);
        let mut stored_project = project.clone();
        if let Some(object) = stored_project.as_object_mut() {
            object.insert("revision".to_string(), Value::from(revision));
            object.remove("render");
        }
        let platform = entry.cart.platform.dir_name();
        if let Some(value) = stored_project.get("platform").and_then(Value::as_str) {
            if value != platform {
                return Err(Response::error(400, "前盖项目的平台不匹配"));
            }
        }
        let requested_model = http::query_param(request, "model")
            .map_err(|error| Response::error(error.status, error.message))?
            .or_else(|| {
                stored_project
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "any".to_string());
        let model = slot_store::fronts::model_from_str(&requested_model)
            .ok_or_else(|| Response::error(400, "前盖模型非法"))?;
        if entry.cart.platform != slot_store::Platform::Fc && model != "any" {
            return Err(Response::error(400, "只有 FC 机种可以指定 FC/NES 前盖模型"));
        }
        let name = http::query_param(request, "name")
            .map_err(|error| Response::error(error.status, error.message))?
            .or_else(|| {
                stored_project
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "自定义前盖".to_string());
        let name = clean_front_name(&name)?;
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Response::error(500, format!("创建前盖目录失败：{e}")))?;
        slot_store::atomic_write(&dir.join(format!("{id}.png")), &png)
            .map_err(|e| Response::error(500, format!("写入前盖 PNG 失败：{e}")))?;
        slot_store::fronts::save_project(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &id,
            &stored_project,
        )
        .map_err(|e| Response::error(500, format!("写入前盖项目失败：{e}")))?;
        let file = format!("{id}.png");
        let project_file = format!("{id}.json");
        if let Some(position) = position {
            if let Some(face) = manifest.faces.get_mut(position) {
                face.name = name;
                face.model = model.to_string();
                face.source = slot_store::fronts::FrontSource::Project;
                face.project = Some(project_file);
            }
        } else {
            let order = manifest.faces.len() as u32;
            manifest.faces.push(slot_store::fronts::FrontFace {
                id: id.clone(),
                name,
                file,
                model: model.to_string(),
                order,
                created: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                source: slot_store::fronts::FrontSource::Project,
                project: Some(project_file),
            });
        }
        slot_store::fronts::save(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &entry.cart.rom,
            &manifest,
        )
        .map_err(|e| Response::error(500, format!("写入前盖清单失败：{e}")))?;
        if position.is_none()
            && slot_store::fronts::compatible(model, entry.cart.model)
            && slot_store::cart_meta::read(
                &shared.root,
                entry.cart.platform,
                &entry.cart.rom,
                &entry.cart.stem,
            )
            .and_then(|meta| slot_store::fronts::selected_id(&meta).map(str::to_string))
            .is_none()
        {
            select_front(shared, &entry, &id)?;
        }
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "id": id, "revision": revision, "width": width, "height": height }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn decode_png_data_url(value: &str) -> Result<Vec<u8>, Response> {
    let encoded = value
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| Response::error(400, "render 必须是 PNG data URL"))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| Response::error(400, "render PNG base64 非法"))
}

fn png_size(bytes: &[u8]) -> Result<(u32, u32), Response> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let reader = decoder
        .read_info()
        .map_err(|_| Response::error(400, "render 不是合法 PNG"))?;
    Ok((reader.info().width, reader.info().height))
}

/// `GET /api/rom/front/template?rom=...` — the real cartridge front without a
/// label.  The Web editor uses its alpha channel as the fixed silhouette mask
/// and draws it as the design base.
pub(super) fn front_template(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let entry = find(shared, request)?;
        let mut cart = entry.cart.clone();
        cart.front = None;
        cart.label = None;
        let (label_w, label_h) = slot_ui::label_size(&cart);
        let label = vec![0u8; (label_w * label_h * 4) as usize];
        let face = slot_ui::cart_face_with(&cart, Some(label));
        let png = encode_png(&face.rgba, face.w, face.h)?;
        Ok(Response::new(200, "image/png", png).with_header("Cache-Control", "no-store"))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/rom/fronts/index` — front names grouped by GBA game stem.
///
/// The cover mod needs one cheap round trip to hide already-imported fronts;
/// querying `/api/rom/fronts` once per game would mean hundreds of requests.
pub(super) fn fronts_index(shared: &Shared, _request: &Request) -> Response {
    let platform = slot_store::Platform::Gba;
    let dir = shared
        .root
        .join(slot_store::fronts::face_dir(&shared.root, platform, ""));
    let mut games = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Some(stem) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(manifest) = slot_store::fronts::load(&shared.root, platform, &stem) else {
                continue;
            };
            let names: Vec<&str> = manifest.faces.iter().map(|face| face.name.as_str()).collect();
            games.push(json!({ "stem": stem, "names": names }));
        }
    }
    games.sort_by(|a, b| a["stem"].as_str().cmp(&b["stem"].as_str()));
    Response::json(
        200,
        json!({
            "ok": true,
            "platform": platform.dir_name(),
            "games": games,
        }),
    )
}

/// `GET /api/rom/fronts?rom=...` — list the custom front faces for this cart.
pub(super) fn fronts_get(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let entry = find(shared, request)?;
        let manifest = slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem);
        let selected = slot_store::cart_meta::read(
            &shared.root,
            entry.cart.platform,
            &entry.cart.rom,
            &entry.cart.stem,
        )
        .and_then(|meta| slot_store::fronts::selected_id(&meta).map(str::to_string));
        let faces: Vec<Value> = manifest
            .map(|manifest| {
                manifest
                    .faces
                    .into_iter()
                    .map(|face| {
                        json!({
                            "id": face.id,
                            "name": face.name,
                            "model": face.model,
                            "order": face.order,
                            "source": match face.source {
                                slot_store::fronts::FrontSource::Project => "project",
                                slot_store::fronts::FrontSource::Upload => "upload",
                            },
                            "project": face.project,
                            "url": format!(
                                "/api/rom/front?rom={}&id={}",
                                util::encode_query(&entry.rom),
                                util::encode_query(&face.id)
                            ),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Response::json(
            200,
            json!({
                "faces": faces,
                "selected": selected,
                "max": slot_store::fronts::MAX_FRONTS,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/rom/front?rom=...&id=...` — raw PNG for a front face.
pub(super) fn front_get(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        let manifest = slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
            .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let face = manifest
            .faces
            .iter()
            .find(|face| face.id == id)
            .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let path = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem)
            .join(&face.file);
        let bytes = std::fs::read(path).map_err(|_| Response::error(404, "前盖文件不存在"))?;
        Ok(Response::new(200, "image/png", bytes).with_header("Cache-Control", "no-store"))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/rom/fronts?rom=...&name=...&model=...` — upload a PNG and fit it
/// to the fixed front canvas.  Phase A stores a flat image; the layered editor
/// can replace this endpoint's body later.
pub(super) fn front_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.len() > slot_store::fronts::MAX_FRONT_BYTES {
            return Err(Response::error(413, "前盖文件过大"));
        }
        let entry = find(shared, request)?;
        let name = http::query_param(request, "name")
            .map_err(|error| Response::error(error.status, error.message))?
            .unwrap_or_else(|| "自定义前盖".to_string());
        let name = clean_front_name(&name)?;
        let model = http::query_param(request, "model")
            .map_err(|error| Response::error(error.status, error.message))?
            .unwrap_or_else(|| "any".to_string());
        let model = slot_store::fronts::model_from_str(&model)
            .ok_or_else(|| Response::error(400, "model 必须是 famicom、nes 或 any"))?;
        if entry.cart.platform != slot_store::Platform::Fc && model != "any" {
            return Err(Response::error(400, "只有 FC 机种可以指定 FC/NES 前盖模型"));
        }
        let (rgba, width, height) = slot_ui::fit_front(&request.body, entry.cart.platform, entry.cart.model)
            .ok_or_else(|| Response::error(400, "前盖图片无法解码或格式不支持"))?;
        let png = encode_png(&rgba, width, height)?;
        let mut manifest =
            slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
                .unwrap_or_else(|| slot_store::fronts::FrontManifest {
                    schema: 1,
                    ..Default::default()
                });
        if manifest.faces.len() >= slot_store::fronts::MAX_FRONTS {
            return Err(Response::error(409, "每张卡带最多 5 个前盖"));
        }
        let id = slot_store::fronts::new_id();
        let file = format!("{id}.png");
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        std::fs::create_dir_all(&dir).map_err(|e| Response::error(500, format!("创建前盖目录失败：{e}")))?;
        slot_store::atomic_write(&dir.join(&file), &png)
            .map_err(|e| Response::error(500, format!("写入前盖失败：{e}")))?;
        let order = manifest.faces.len() as u32;
        manifest.faces.push(slot_store::fronts::FrontFace {
            id: id.clone(),
            name,
            file,
            model: model.to_string(),
            order,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            source: slot_store::fronts::FrontSource::Upload,
            project: None,
        });
        slot_store::fronts::save(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &entry.cart.rom,
            &manifest,
        )
        .map_err(|e| Response::error(500, format!("写入前盖清单失败：{e}")))?;
        // First upload becomes the selected face when it fits the current model.
        let already_selected = slot_store::cart_meta::read(
            &shared.root,
            entry.cart.platform,
            &entry.cart.rom,
            &entry.cart.stem,
        )
        .and_then(|meta| slot_store::fronts::selected_id(&meta).map(str::to_string))
        .is_some();
        let auto_selected = slot_store::fronts::compatible(model, entry.cart.model) && !already_selected;
        if auto_selected {
            select_front(shared, &entry, &id)?;
        }
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "id": id, "width": width, "height": height, "selected": auto_selected }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/fronts/select?rom=...&id=...`
pub(super) fn front_select(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        select_front(shared, &entry, &id)?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true, "id": id })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `DELETE /api/rom/fronts?rom=...&id=...`
pub(super) fn front_delete(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        let mut manifest =
            slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
                .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let Some(position) = manifest.faces.iter().position(|face| face.id == id) else {
            return Err(Response::error(404, "前盖不存在"));
        };
        let face = manifest.faces.remove(position);
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        let path = dir.join(&face.file);
        let _ = std::fs::remove_file(path);
        if let Some(project) = &face.project {
            if slot_store::fronts::valid_project_file(project) {
                let _ = std::fs::remove_file(dir.join(project));
            }
        }
        for (index, face) in manifest.faces.iter_mut().enumerate() {
            face.order = index as u32;
        }
        slot_store::fronts::save(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &entry.cart.rom,
            &manifest,
        )
        .map_err(|e| Response::error(500, format!("写入前盖清单失败：{e}")))?;
        let next = manifest
            .faces
            .iter()
            .find(|face| slot_store::fronts::compatible(&face.model, entry.cart.model))
            .map(|face| face.id.clone());
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            if meta.extra.get("front_face").and_then(Value::as_str) == Some(id.as_str()) {
                match &next {
                    Some(next) => {
                        meta.extra.insert(
                            "front_face".to_string(),
                            Value::String(next.clone()),
                        );
                    }
                    None => {
                        meta.extra.remove("front_face");
                    }
                }
            }
        })
        .map_err(|e| Response::error(500, format!("更新前盖选择失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/rom/fronts/copy?rom=...&id=...` — duplicate a front face.
pub(super) fn front_copy(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        let mut manifest =
            slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
                .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        if manifest.faces.len() >= slot_store::fronts::MAX_FRONTS {
            return Err(Response::error(409, "每张卡带最多 5 个前盖"));
        }
        let face = manifest
            .faces
            .iter()
            .find(|face| face.id == id)
            .cloned()
            .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let name = http::query_param(request, "name")
            .map_err(|error| Response::error(error.status, error.message))?
            .map(|name| clean_front_name(&name))
            .transpose()?
            .unwrap_or_else(|| format!("{} 副本", face.name));
        let new_id = slot_store::fronts::new_id();
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        let source_png = dir.join(&face.file);
        let png = std::fs::read(&source_png)
            .map_err(|_| Response::error(404, "前盖文件不存在"))?;
        let new_file = format!("{new_id}.png");
        slot_store::atomic_write(&dir.join(&new_file), &png)
            .map_err(|e| Response::error(500, format!("复制前盖 PNG 失败：{e}")))?;
        let mut source = slot_store::fronts::FrontSource::Upload;
        let mut project = None;
        if let Some(project_file) = &face.project {
            if slot_store::fronts::valid_project_file(project_file) {
                if let Some(mut value) = slot_store::fronts::load_project(
                    &shared.root,
                    entry.cart.platform,
                    &entry.cart.stem,
                    &id,
                ) {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("revision".to_string(), Value::from(1));
                    }
                    slot_store::fronts::save_project(
                        &shared.root,
                        entry.cart.platform,
                        &entry.cart.stem,
                        &new_id,
                        &value,
                    )
                    .map_err(|e| Response::error(500, format!("复制前盖项目失败：{e}")))?;
                    source = slot_store::fronts::FrontSource::Project;
                    project = Some(format!("{new_id}.json"));
                }
            }
        }
        let order = manifest.faces.len() as u32;
        manifest.faces.push(slot_store::fronts::FrontFace {
            id: new_id.clone(),
            name,
            file: new_file,
            model: face.model,
            order,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            source,
            project,
        });
        slot_store::fronts::save(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &entry.cart.rom,
            &manifest,
        )
        .map_err(|e| Response::error(500, format!("写入前盖清单失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true, "id": new_id })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/rom/fronts/export?rom=...&id=...` — portable JSON front package.
pub(super) fn front_export(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let entry = find(shared, request)?;
        let id = http::query_param(request, "id")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 id 参数"))?;
        let manifest = slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
            .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let face = manifest
            .faces
            .iter()
            .find(|face| face.id == id)
            .cloned()
            .ok_or_else(|| Response::error(404, "前盖不存在"))?;
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        let png = std::fs::read(dir.join(&face.file))
            .map_err(|_| Response::error(404, "前盖文件不存在"))?;
        let project = if face.source == slot_store::fronts::FrontSource::Project {
            slot_store::fronts::load_project(
                &shared.root,
                entry.cart.platform,
                &entry.cart.stem,
                &id,
            )
        } else {
            None
        };
        let render = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        Ok(Response::json(
            200,
            json!({
                "schema": 1,
                "name": face.name,
                "platform": entry.cart.platform.dir_name(),
                "model": face.model,
                "project": project,
                "render": render,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/rom/fronts/import?rom=...` — import a portable front package.
pub(super) fn front_import(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.len() > 24 * 1024 * 1024 {
            return Err(Response::error(413, "前盖包过大"));
        }
        let entry = find(shared, request)?;
        let package: Value = serde_json::from_slice(&request.body)
            .map_err(|_| Response::error(400, "前盖包不是合法 JSON"))?;
        let render = package
            .get("render")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "前盖包缺少 render PNG"))?;
        let png = decode_png_data_url(render)?;
        let (width, height) = png_size(&png)?;
        let (want_w, want_h) = slot_ui::front_size(entry.cart.platform, entry.cart.model);
        if (width, height) != (want_w, want_h) {
            return Err(Response::error(400, "前盖包画布尺寸不匹配"));
        }
        if png.len() > slot_store::fronts::MAX_FRONT_BYTES {
            return Err(Response::error(413, "前盖 PNG 过大"));
        }
        let project = package.get("project").filter(|value| !value.is_null());
        if let Some(project) = project {
            slot_store::fronts::validate_project(project, want_w, want_h)
                .map_err(|error| Response::error(400, error))?;
        }
        let name = http::query_param(request, "name")
            .map_err(|error| Response::error(error.status, error.message))?
            .or_else(|| package.get("name").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "导入前盖".to_string());
        let name = clean_front_name(&name)?;
        let model = http::query_param(request, "model")
            .map_err(|error| Response::error(error.status, error.message))?
            .or_else(|| package.get("model").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "any".to_string());
        let model = slot_store::fronts::model_from_str(&model)
            .ok_or_else(|| Response::error(400, "前盖模型非法"))?;
        if entry.cart.platform != slot_store::Platform::Fc && model != "any" {
            return Err(Response::error(400, "只有 FC 机种可以指定 FC/NES 前盖模型"));
        }
        let mut manifest =
            slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
                .unwrap_or_else(|| slot_store::fronts::FrontManifest {
                    schema: 1,
                    ..Default::default()
                });
        if manifest.faces.len() >= slot_store::fronts::MAX_FRONTS {
            return Err(Response::error(409, "每张卡带最多 5 个前盖"));
        }
        let id = slot_store::fronts::new_id();
        let dir = slot_store::fronts::face_dir(&shared.root, entry.cart.platform, &entry.cart.stem);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Response::error(500, format!("创建前盖目录失败：{e}")))?;
        slot_store::atomic_write(&dir.join(format!("{id}.png")), &png)
            .map_err(|e| Response::error(500, format!("写入前盖 PNG 失败：{e}")))?;
        let (source, project) = if let Some(project) = project {
            let mut project = project.clone();
            if let Some(object) = project.as_object_mut() {
                object.insert("revision".to_string(), Value::from(1));
            }
            slot_store::fronts::save_project(
                &shared.root,
                entry.cart.platform,
                &entry.cart.stem,
                &id,
                &project,
            )
            .map_err(|e| Response::error(500, format!("写入前盖项目失败：{e}")))?;
            (
                slot_store::fronts::FrontSource::Project,
                Some(format!("{id}.json")),
            )
        } else {
            (slot_store::fronts::FrontSource::Upload, None)
        };
        let order = manifest.faces.len() as u32;
        manifest.faces.push(slot_store::fronts::FrontFace {
            id: id.clone(),
            name,
            file: format!("{id}.png"),
            model: model.to_string(),
            order,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            source,
            project,
        });
        slot_store::fronts::save(
            &shared.root,
            entry.cart.platform,
            &entry.cart.stem,
            &entry.cart.rom,
            &manifest,
        )
        .map_err(|e| Response::error(500, format!("写入前盖清单失败：{e}")))?;
        if slot_store::fronts::compatible(model, entry.cart.model)
            && slot_store::cart_meta::read(
                &shared.root,
                entry.cart.platform,
                &entry.cart.rom,
                &entry.cart.stem,
            )
            .and_then(|meta| slot_store::fronts::selected_id(&meta).map(str::to_string))
            .is_none()
        {
            select_front(shared, &entry, &id)?;
        }
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true, "id": id })))
    })();
    handled.unwrap_or_else(|response| response)
}

fn select_front(
    shared: &Shared,
    entry: &Entry,
    id: &str,
) -> Result<(), Response> {
    let manifest = slot_store::fronts::load(&shared.root, entry.cart.platform, &entry.cart.stem)
        .ok_or_else(|| Response::error(404, "前盖不存在"))?;
    let face = manifest
        .faces
        .iter()
        .find(|face| face.id == id)
        .ok_or_else(|| Response::error(404, "前盖不存在"))?;
    if !slot_store::fronts::compatible(&face.model, entry.cart.model) {
        return Err(Response::error(400, "该前盖不适用于当前卡带模型"));
    }
    slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
        meta.extra
            .insert("front_face".to_string(), Value::String(id.to_string()));
    })
    .map_err(|e| Response::error(500, format!("更新前盖选择失败：{e}")))?;
    Ok(())
}

fn clean_front_name(raw: &str) -> Result<String, Response> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Response::error(400, "前盖名称不能为空"));
    }
    let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    Ok(name)
}

/// A library scan costs a directory walk plus a metadata join; the page asks
/// for the list on every visit and after every write, so keep the answer for a
/// few seconds and drop it as soon as the page itself changes something.
const LIST_CACHE: Duration = Duration::from_secs(5);

pub(super) fn list(shared: &Shared) -> Response {
    let generation = shared.library_generation.load(Ordering::Acquire);
    if let Ok(cache) = shared.library_cache.lock() {
        if let Some((cached_generation, built_at, body)) = cache.as_ref() {
            if *cached_generation == generation && built_at.elapsed() < LIST_CACHE {
                return Response::new(200, "application/json; charset=utf-8", body.clone())
                    .with_header("Cache-Control", "no-store");
            }
        }
    }
    match collect(&shared.root) {
        Ok(entries) => {
            let games: Vec<Value> = entries.iter().map(entry_value).collect();
            let body = match serde_json::to_vec(&json!({ "games": games })) {
                Ok(body) => body,
                Err(e) => return Response::error(500, format!("序列化游戏库失败：{e}")),
            };
            if let Ok(mut cache) = shared.library_cache.lock() {
                *cache = Some((generation, Instant::now(), body.clone()));
            }
            Response::new(200, "application/json; charset=utf-8", body)
                .with_header("Cache-Control", "no-store")
        }
        Err(response) => response,
    }
}


/// A `Games/<平台>/<…>/<文件>` reference, parsed without touching the library.
struct RomRef {
    platform: slot_store::Platform,
    /// Segments between the platform directory and the file (may be empty).
    folders: Vec<String>,
    /// File name including its extension.
    file: String,
    stem: String,
}

fn parse_rom(request: &Request) -> Result<RomRef, Response> {
    let raw = http::query_param(request, "rom")
        .map_err(|e| Response::error(e.status, e.message))?
        .ok_or_else(|| Response::error(400, "缺少 rom 参数"))?;
    parse_rom_value(&raw)
}

fn parse_rom_value(raw: &str) -> Result<RomRef, Response> {
    if raw.is_empty() || raw.len() > MAX_ROM_PATH_BYTES {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    if raw.starts_with('/') || raw.contains('\\') || raw.contains('\0') {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() < 3 || parts[0] != "Games" {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    if parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..") {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    let Some(platform) = slot_store::Platform::ALL
        .into_iter()
        .find(|platform| platform.dir_name().eq_ignore_ascii_case(parts[1]))
    else {
        return Err(Response::error(400, "未知机种"));
    };
    let file = parts[parts.len() - 1];
    let Some(stem) = Path::new(file).file_stem().and_then(|stem| stem.to_str()) else {
        return Err(Response::error(400, "rom 参数不合法"));
    };
    Ok(RomRef {
        platform,
        folders: parts[2..parts.len() - 1].iter().map(|part| part.to_string()).collect(),
        file: file.to_string(),
        stem: stem.to_string(),
    })
}

impl RomRef {
    fn rom(&self, root: &Path) -> PathBuf {
        let mut path = root.join("Games").join(self.platform.dir_name());
        for folder in &self.folders {
            path = path.join(folder);
        }
        path.join(&self.file)
    }

    fn label(&self, root: &Path, extension: &str) -> PathBuf {
        let mut path = root
            .join(slot_store::cart_meta::asset_dir(extension))
            .join(self.platform.dir_name());
        for folder in &self.folders {
            path = path.join(folder);
        }
        path.join(format!("{}.{extension}", self.stem))
    }
}

/// `GET /api/rom/cover?rom=Games/GBA/Game.gba`
///
/// Resolving the label used to run a full library scan per tile, so a page
/// with forty covers scanned the card forty times.  A metadata row plus two
/// conventional paths is enough.
pub(super) fn cover(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let rom = parse_rom(request)?;
        let rom_path = rom.rom(&shared.root);
        let Some(label) = slot_store::cart_meta::read(
            &shared.root,
            rom.platform,
            &rom_path,
            &rom.stem,
        )
        .and_then(|meta| meta.cover_path(&shared.root))
        .or_else(|| {
            [
                rom.label(&shared.root, "png"),
                shared
                    .root
                    .join("Labels")
                    .join(rom.platform.dir_name())
                    .join(format!("{}.png", rom.stem)),
            ]
            .into_iter()
            .find(|path| path.is_file())
        }) else {
            return Err(Response::error(404, "该游戏还没有封面"));
        };
        serve_label(shared, request, &label, &rom.stem)
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/rom/intro?rom=Games/GBA/Game.gba`
///
/// The intro is a plain UTF-8 text file beside the cover; the detail dialog
/// shows it so the player can read it without booting the game.
pub(super) fn intro_get(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let rom = parse_rom(request)?;
        let rom_path = rom.rom(&shared.root);
        let Some(path) = slot_store::cart_meta::read(
            &shared.root,
            rom.platform,
            &rom_path,
            &rom.stem,
        )
        .and_then(|meta| meta.intro_path(&shared.root))
        .or_else(|| {
            [
                rom.label(&shared.root, "txt"),
                shared
                    .root
                    .join("Info")
                    .join(rom.platform.dir_name())
                    .join(format!("{}.txt", rom.stem)),
            ]
            .into_iter()
            .find(|path| path.is_file())
        }) else {
            return Err(Response::error(404, "该游戏还没有介绍"));
        };
        if !under_root(&shared.root, &path) {
            return Err(Response::error(403, "介绍路径越界"));
        }
        let metadata = std::fs::metadata(&path).map_err(|_| Response::error(404, "介绍不存在"))?;
        if metadata.len() > INTRO_MAX_BYTES {
            return Err(Response::error(413, "介绍文件过大"));
        }
        let bytes = std::fs::read(&path).map_err(|_| Response::error(404, "介绍不存在"))?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(Response::json(
            200,
            json!({
                "text": text,
                "size": bytes.len(),
            }),
        )
        .with_header("Cache-Control", "no-store"))
    })();
    handled.unwrap_or_else(|response| response)
}

/// Shared tail of the cover endpoint: ETag revalidation plus a bounded body.
fn serve_label(shared: &Shared, request: &Request, label: &Path, stem: &str) -> Result<Response, Response> {
    if !under_root(&shared.root, label) {
        return Err(Response::error(403, "封面路径越界"));
    }
    let metadata = std::fs::metadata(label).map_err(|_| Response::error(404, "封面不存在"))?;
    if metadata.len() > COVER_MAX_BYTES as u64 {
        return Err(Response::error(413, "封面文件过大"));
    }
    let etag = format!(
        "\"{}-{}\"",
        metadata.len(),
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    );
    if request
        .header("if-none-match")
        .is_some_and(|value| value == etag)
    {
        return Ok(Response::new(304, "image/png", Vec::new())
            .with_header("ETag", etag)
            .with_header("Cache-Control", COVER_CACHE_CONTROL));
    }
    let bytes = std::fs::read(label).map_err(|_| Response::error(404, "封面不存在"))?;
    let name = format!("{stem}.png");
    Ok(Response::new(200, "image/png", bytes)
        .with_header(
            "Content-Disposition",
            http::content_disposition("inline", &name),
        )
        .with_header("ETag", etag)
        .with_header("Cache-Control", COVER_CACHE_CONTROL))
}

/// `PUT /api/rom/cover?rom=Games/GBA/Game.gba`
pub(super) fn cover_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.len() > COVER_MAX_BYTES {
            return Err(Response::error(413, "封面超过 8MB 上限"));
        }
        let entry = find(shared, request)?;
        let (label_w, label_h) = slot_ui::label_size(&entry.cart);
        let (width, height) = (label_w.saturating_mul(2), label_h.saturating_mul(2));
        let png = normalize_cover(&shared.root, &request.body, width, height)?;
        let target = entry.cart.label.clone().unwrap_or_else(|| {
            shared
                .root
                .join("Labels")
                .join(entry.cart.platform.dir_name())
                .join(format!("{}.png", entry.cart.stem))
        });
        let target = checked_target(&shared.root, &target)?;
        slot_store::atomic_write(&target, &png)
            .map_err(|e| Response::error(500, format!("写入封面失败：{e}")))?;
        // The label cache key uses the source file's size and second-level
        // mtime; replacing a cover twice in one second can otherwise keep the
        // old cache.  Deleting the entry forces a rebuild on the next shelf
        // face.
        let cache = crate::label_cache::cache_path(&shared.root, &entry.cart);
        let _ = std::fs::remove_file(cache);
        let cover = slot_store::cart_meta::relative(&shared.root, &target);
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.cover = Some(cover.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "width": width,
                "height": height,
                "size": png.len(),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/intro?rom=Games/GBA/Game.gba` — raw UTF-8 text.
pub(super) fn intro_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.is_empty() {
            return Err(Response::error(400, "介绍内容为空"));
        }
        if request.body.len() > 256 * 1024 {
            return Err(Response::error(413, "介绍超过 256KB 上限"));
        }
        if std::str::from_utf8(&request.body).is_err() {
            return Err(Response::error(400, "介绍必须是 UTF-8 文本"));
        }
        if request.body.contains(&0) {
            return Err(Response::error(400, "介绍不能包含二进制内容"));
        }
        let entry = find(shared, request)?;
        let target = labels_file(&shared.root, &entry.cart, "txt");
        let target = checked_target(&shared.root, &target)?;
        slot_store::atomic_write(&target, &request.body)
            .map_err(|e| Response::error(500, format!("写入介绍失败：{e}")))?;
        let intro = slot_store::cart_meta::relative(&shared.root, &target);
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.intro = Some(intro.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "kind": "intro", "size": request.body.len() }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/video?rom=Games/GBA/Game.gba` — raw MP4 bytes.
pub(super) fn video_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.is_empty() {
            return Err(Response::error(400, "视频内容为空"));
        }
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("视频超过 {mb}MB 上限")));
        }
        if request.body.len() < 12 || &request.body[4..8] != b"ftyp" {
            return Err(Response::error(400, "视频必须是 MP4 文件"));
        }
        let entry = find(shared, request)?;
        let target = labels_file(&shared.root, &entry.cart, "mp4");
        let target = checked_target(&shared.root, &target)?;
        slot_store::atomic_write(&target, &request.body)
            .map_err(|e| Response::error(500, format!("写入视频失败：{e}")))?;
        let video = slot_store::cart_meta::relative(&shared.root, &target);
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.video = Some(video.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "kind": "video", "size": request.body.len() }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/color?rom=Games/GBA/Game.gba` — JSON `{"color":"#rrggbb"}`.
pub(super) fn color_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let raw = value
            .get("color")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 color 参数"))?;
        let colour = slot_store::cart_color::parse_hex(raw)
            .ok_or_else(|| Response::error(400, "颜色必须是 #rrggbb 或 rrggbb"))?;
        let entry = find(shared, request)?;
        let hex = slot_store::cart_color::to_hex(colour);
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.color = Some(hex.clone());
            if meta.shell.is_none() {
                meta.shell = Some(slot_store::cart_meta::ShellMeta {
                    outline: "auto".into(),
                    finish: "solid".into(),
                });
            }
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "color": hex,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `PUT /api/rom/cheat?rom=Games/GBA/Game.gba` — JSON
/// `{"cheat":"Cheats/GBA/mgba/Game.cht"}`.  An empty string clears the override
/// and restores the conventional `Cheats/<platform>/<core>/<stem>.cht` lookup.
pub(super) fn cheat_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let raw = value
            .get("cheat")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 cheat 参数"))?
            .trim()
            .to_string();
        let entry = find(shared, request)?;
        if raw.is_empty() {
            slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
                meta.cheat = None;
            })
            .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
            return Ok(Response::json(200, json!({"ok": true, "cheat": null})));
        }
        if raw.len() > MAX_ROM_PATH_BYTES || raw.contains('\\') || raw.contains('\0') {
            return Err(Response::error(400, "金手指路径不合法"));
        }
        if !raw.to_ascii_lowercase().ends_with(".cht") {
            return Err(Response::error(400, "金手指必须是 .cht 文件"));
        }
        let relative = Path::new(&raw);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(Response::error(400, "金手指路径不合法"));
        }
        let target = shared.root.join(relative);
        if !target.is_file() {
            return Err(Response::error(404, "金手指文件不存在"));
        }
        if !under_root(&shared.root, &target) {
            return Err(Response::error(403, "金手指路径越界"));
        }
        let stored = relative_path(&shared.root, &target)
            .ok_or_else(|| Response::error(400, "金手指路径不合法"))?;
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.cheat = Some(stored.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        Ok(Response::json(200, json!({"ok": true, "cheat": stored})))
    })();
    handled.unwrap_or_else(|response| response)
}

fn labels_file(root: &Path, cart: &Cart, extension: &str) -> PathBuf {
    root.join(slot_store::cart_meta::asset_dir(extension))
        .join(cart.platform.dir_name())
        .join(format!("{}.{extension}", cart.stem))
}

/// `PUT /api/rom/guide?rom=Games/GBA/Game.gba&ext=txt|epub` — raw guide bytes.
pub(super) fn guide_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.is_empty() {
            return Err(Response::error(400, "攻略内容为空"));
        }
        if request.body.len() > crate::epub::MAX_BYTES {
            return Err(Response::error(
                413,
                format!(
                    "攻略超过 {}MB 上限",
                    crate::epub::MAX_BYTES / (1024 * 1024)
                ),
            ));
        }
        let extension = http::query_param(request, "ext")
            .map_err(|error| Response::error(error.status, error.message))?
            .unwrap_or_else(|| "txt".into())
            .to_ascii_lowercase();
        let extension = match extension.as_str() {
            "txt" => "txt",
            "epub" => "epub",
            _ => return Err(Response::error(400, "攻略扩展名必须是 txt 或 epub")),
        };
        let entry = find(shared, request)?;
        let target = guides_file(&shared.root, &entry.cart, extension);
        let target = checked_target(&shared.root, &target)?;
        slot_store::atomic_write(&target, &request.body)
            .map_err(|e| Response::error(500, format!("写入攻略失败：{e}")))?;
        let stored = slot_store::cart_meta::relative(&shared.root, &target);
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.guide = Some(stored.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({"ok": true, "guide": stored, "size": request.body.len()}),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/rom/guide?rom=Games/GBA/Game.gba` — JSON `{"guide":"Guides/GBA/Game.txt"}`.
/// An empty string clears the override; the conventional `Guides/` lookup remains.
pub(super) fn guide_set(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let raw = value
            .get("guide")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 guide 参数"))?
            .trim()
            .to_string();
        let entry = find(shared, request)?;
        if raw.is_empty() {
            slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
                meta.guide = None;
            })
            .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
            shared.touch_library_changed();
            return Ok(Response::json(200, json!({"ok": true, "guide": null})));
        }
        if raw.len() > MAX_ROM_PATH_BYTES || raw.contains('\\') || raw.contains('\0') {
            return Err(Response::error(400, "攻略路径不合法"));
        }
        let extension = raw
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or_default();
        if !matches!(extension.to_ascii_lowercase().as_str(), "txt" | "epub") {
            return Err(Response::error(400, "攻略必须是 .txt 或 .epub 文件"));
        }
        let relative = Path::new(&raw);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(Response::error(400, "攻略路径不合法"));
        }
        let target = shared.root.join(relative);
        if !target.is_file() {
            return Err(Response::error(404, "攻略文件不存在"));
        }
        if !under_root(&shared.root, &target) {
            return Err(Response::error(403, "攻略路径越界"));
        }
        let stored = relative_path(&shared.root, &target)
            .ok_or_else(|| Response::error(400, "攻略路径不合法"))?;
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.guide = Some(stored.clone());
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({"ok": true, "guide": stored})))
    })();
    handled.unwrap_or_else(|response| response)
}

fn guides_file(root: &Path, cart: &Cart, extension: &str) -> PathBuf {
    root.join("Guides")
        .join(cart.platform.dir_name())
        .join(format!("{}.{extension}", cart.stem))
}

fn duplicate_rom_by_sha(
    root: &Path,
    platform: slot_store::Platform,
    digest: &str,
    size: u64,
    current: &Path,
) -> Option<slot_store::cart_meta::CartMeta> {
    let current = relative_path(root, current);
    if let Some(meta) = slot_store::cart_meta::read_by_sha(root, platform, digest) {
        let mut matches = slot_store::cart_meta::stamp_matches(root, &meta);
        if !matches {
            let path = root.join(&meta.rom);
            if let Ok(actual) = crate::rom_id::sha256_file(&path) {
                if let (Some(size), Some(mtime)) = (meta.size, meta.mtime) {
                    let _ = slot_store::cart_meta::set_sha256(
                        root, &path, &actual, size, mtime,
                    );
                }
                matches = actual.eq_ignore_ascii_case(digest);
            }
        }
        if matches && current.as_deref() != Some(meta.rom.as_str()) {
            return Some(meta);
        }
    }
    for candidate in slot_store::cart_meta::candidates_by_size(root, platform, size) {
        if current.as_deref() == Some(candidate.rom.as_str()) {
            continue;
        }
        let path = root.join(&candidate.rom);
        let Ok(actual) = crate::rom_id::sha256_file(&path) else {
            continue;
        };
        if let (Some(candidate_size), Some(candidate_mtime)) = (candidate.size, candidate.mtime) {
            let _ = slot_store::cart_meta::set_sha256(
                root,
                &path,
                &actual,
                candidate_size,
                candidate_mtime,
            );
        }
        if actual.eq_ignore_ascii_case(digest) {
            return Some(candidate);
        }
    }
    None
}

/// `PUT /api/rom?rom=Games/GBA/Game.gba`
/// `POST /api/rom/add?platform=GBA&name=Game.gba`: put a new ROM into the
/// library.  The browser chooses a platform and a file; the server owns the
/// directory, the extension rules and the duplicate checks, so a card can never
/// be handed a ROM outside `Games/<platform>/`.
pub(super) fn add(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.is_empty() {
            return Err(Response::error(400, "ROM 内容为空"));
        }
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("ROM 超过 {mb}MB 上限")));
        }
        let platform_raw = http::query_param(request, "platform")
            .map_err(|e| Response::error(e.status, e.message))?
            .ok_or_else(|| Response::error(400, "缺少机种"))?;
        let platform = crate::collection::platform(&platform_raw)
            .ok_or_else(|| Response::error(400, "不支持的机种"))?;
        let raw_name = http::query_param(request, "name")
            .map_err(|e| Response::error(e.status, e.message))?
            .ok_or_else(|| Response::error(400, "缺少文件名"))?;
        let name = rom_file_name(&raw_name, platform)?;
        let dir = shared.root.join("Games").join(platform.dir_name());
        std::fs::create_dir_all(&dir)
            .map_err(|e| Response::error(500, format!("无法创建平台目录：{e}")))?;
        let desired = dir.join(&name);
        let rom = relative_path(&shared.root, &desired)
            .ok_or_else(|| Response::error(400, "ROM 路径不合法"))?;
        if desired.exists() {
            return Err(Response::error(409, format!("同名 ROM 已存在：{rom}")));
        }
        let digest = crate::rom_id::sha256_hex(&request.body);
        if let Some(existing) = duplicate_content(
            &shared.root,
            platform,
            &digest,
            request.body.len() as u64,
        ) {
            return Err(Response::error(
                409,
                format!("相同内容的 ROM 已存在：{existing}"),
            ));
        }
        let target = checked_target(&shared.root, &desired)?;
        slot_store::atomic_write(&target, &request.body)
            .map_err(|e| Response::error(500, format!("写入 ROM 失败：{e}")))?;
        // Fill the card metadata straight from what we just wrote.  Scanning
        // the whole library here cost seconds on a full card and made the
        // upload look stuck at 100%.
        let stem = name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(&name)
            .to_string();
        let cart = slot_store::Cart {
            platform,
            stem: stem.clone(),
            rom: target.clone(),
            label: None,
            code: String::new(),
            shell: None,
            title: String::new(),
        model: Default::default(),
        front: None,
};
        let _ = slot_store::cart_meta::ensure(&shared.root, &cart);
        let _ = slot_store::cart_meta::update(&shared.root, &cart, |meta| {
            meta.sha256 = Some(digest.clone());
            meta.size = Some(request.body.len() as u64);
            meta.mtime = std::fs::metadata(&target)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos() as u64);
        });
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "rom": rom,
                "platform": platform.dir_name(),
                "stem": stem,
                "size": request.body.len(),
                "sha256": digest,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}
/// `POST /api/rom/delete?rom=Games/GBA/Game.gba` — delete the ROM file only.
///
/// Saves, states, covers, guides, cheats and fronts are intentionally left in
/// place: deleting a ROM must not silently destroy user data.
pub(super) fn delete(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let rom = parse_rom(request)?;
        let path = rom.rom(&shared.root);
        if !path.is_file() {
            return Err(Response::error(404, "游戏不存在"));
        }
        if !rom.platform.accepts(&path) {
            return Err(Response::error(400, "只能删除当前机种的 ROM"));
        }
        std::fs::remove_file(&path)
            .map_err(|error| Response::error(500, format!("删除 ROM 失败：{error}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "rom": format!("Games/{}/{}", rom.platform.dir_name(), rom.file),
                "stem": rom.stem,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}


/// `POST /api/rom/rename` — rename the ROM and its conventional sidecars.
///
/// The display name is the file stem everywhere on the shelf, so this is a real
/// file rename rather than a display-only alias.  Saves, states, covers, guides
/// and cheats are moved with it; otherwise a rename would silently orphan them.
pub(super) fn rename(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let body = http::json_body(request).map_err(|e| Response::error(e.status, e.message))?;
        let raw = body
            .get("rom")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 rom 参数"))?;
        let requested = body
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 name 参数"))?;
        let old = parse_rom_value(raw)?;
        let old_path = old.rom(&shared.root);
        if !old_path.is_file() {
            return Err(Response::error(404, "游戏不存在"));
        }
        let new_stem = validated_stem(requested, &old)?;
        if new_stem.eq_ignore_ascii_case(&old.stem) {
            return Err(Response::error(400, "新游戏名与原名称相同"));
        }
        let extension = old
            .file
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or("");
        let new_file = if extension.is_empty() {
            new_stem.clone()
        } else {
            format!("{new_stem}.{extension}")
        };
        let new = RomRef {
            platform: old.platform,
            folders: old.folders.clone(),
            file: new_file.clone(),
            stem: new_stem.clone(),
        };
        let new_path = new.rom(&shared.root);
        if new_path.exists() {
            return Err(Response::error(409, "目标游戏名已存在"));
        }

        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        moves.push((old_path.clone(), new_path.clone()));
        add_sidecar_moves(&shared.root, &old, &new, &mut moves)?;
        // Preflight every destination before a single rename: a half-renamed
        // library is worse than a clear 409.
        for (_, destination) in &moves {
            if destination.exists() {
                return Err(Response::error(409, "目标游戏名已存在"));
            }
        }
        let mut completed = Vec::new();
        for (source, destination) in &moves {
            if let Some(parent) = destination.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::rename(source, destination) {
                Ok(()) => completed.push((source.clone(), destination.clone())),
                Err(error) => {
                    for (from, to) in completed.into_iter().rev() {
                        let _ = std::fs::rename(&to, &from);
                    }
                    return Err(Response::error(
                        500,
                        format!("重命名游戏失败：{error}"),
                    ));
                }
            }
        }

        // The metadata row is keyed by ROM path.  Keep the content fields, but
        // force the conventional paths so they point at the moved sidecars.
        let old_cart = Cart {
            platform: old.platform,
            stem: old.stem.clone(),
            rom: old_path.clone(),
            label: None,
            title: String::new(),
            code: String::new(),
            shell: None,
        model: Default::default(),
        front: None,
};
        if let Some(mut meta) = slot_store::cart_meta::read(
            &shared.root,
            old.platform,
            &old_path,
            &old.stem,
        ) {
            meta.rom = relative_to(&shared.root, &new_path);
            meta.cover = None;
            meta.intro = None;
            meta.video = None;
            meta.cheat = None;
            meta.guide = None;
            let _ = slot_store::cart_meta::remove(&shared.root, &old_cart);
            let _ = slot_store::cart_meta::write(
                &shared.root,
                new.platform,
                &new_path,
                &meta,
            );
        } else {
            let new_cart = Cart {
                platform: new.platform,
                stem: new.stem.clone(),
                rom: new_path.clone(),
                label: None,
                title: String::new(),
                code: String::new(),
                shell: None,
            model: Default::default(),
            front: None,
};
            let _ = slot_store::cart_meta::ensure(&shared.root, &new_cart);
        }

        // Recent/favourite entries use the stem; keep the shelf history looking
        // at the renamed file.
        let state_path = shared.root.join("linkSlot/Config").join("slot.state");
        if state_path.is_file() {
            let mut state = slot_store::read_slot_state(&shared.root);
            let mut changed = false;
            if state.cart.as_deref() == Some(old.stem.as_str()) {
                state.cart = Some(new_stem.clone());
                changed = true;
            }
            if let Some(cart) = state.shelf_cart.as_mut() {
                if cart.platform == old.platform && cart.stem == old.stem {
                    cart.stem = new_stem.clone();
                    changed = true;
                }
            }
            for recent in state.recents.iter_mut().chain(state.favorites.iter_mut()) {
                if recent.platform == old.platform && recent.stem == old.stem {
                    recent.stem = new_stem.clone();
                    changed = true;
                }
            }
            if changed {
                let _ = slot_store::write_slot_state(&shared.root, &state);
            }
        }

        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "rom": relative_to(&shared.root, &new_path),
                "stem": new_stem,
                "file": new_file,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn validated_stem(raw: &str, old: &RomRef) -> Result<String, Response> {
    let mut value = raw.trim();
    let extension = old.file.rsplit_once('.').map(|(_, extension)| extension);
    if let Some(extension) = extension {
        let suffix = format!(".{}", extension.to_ascii_lowercase());
        if value.to_ascii_lowercase().ends_with(&suffix) {
            value = &value[..value.len() - suffix.len()];
        }
    }
    let value = value.trim();
    if value.is_empty() || value.len() > 200 {
        return Err(Response::error(400, "游戏名不能为空或过长"));
    }
    if value == "." || value == ".." || value.starts_with('.') || value.ends_with('.') {
        return Err(Response::error(400, "游戏名不能以点开头或结尾"));
    }
    if value.ends_with(' ') {
        return Err(Response::error(400, "游戏名不能以空格结尾"));
    }
    if value.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err(Response::error(400, "游戏名包含非法字符"));
    }
    Ok(value.to_string())
}

fn add_sidecar_moves(
    root: &Path,
    old: &RomRef,
    new: &RomRef,
    moves: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), Response> {
    let platform = old.platform.dir_name();
    let old_states = root.join("States").join(platform);
    if let Ok(cores) = std::fs::read_dir(&old_states) {
        for core in cores.flatten() {
            let source = core.path().join(&old.stem);
            if source.is_dir() {
                moves.push((source, core.path().join(&new.stem)));
            }
        }
    }
    for extension in ["sav", "srm"] {
        let source = root
            .join("Saves")
            .join(platform)
            .join(format!("{}.{extension}", old.stem));
        if source.is_file() {
            moves.push((
                source,
                root.join("Saves")
                    .join(platform)
                    .join(format!("{}.{extension}", new.stem)),
            ));
        }
    }
    for extension in ["png", "txt", "mp4"] {
        let dir = root.join(slot_store::cart_meta::asset_dir(extension));
        let flat = dir
            .join(platform)
            .join(format!("{}.{extension}", old.stem));
        if flat.is_file() {
            moves.push((
                flat.clone(),
                dir.join(platform)
                    .join(format!("{}.{extension}", new.stem)),
            ));
        }
        let nested = old.label(root, extension);
        if nested.is_file() && nested != flat {
            moves.push((nested, new.label(root, extension)));
        }
    }
    for extension in ["txt", "epub"] {
        let source = root
            .join("Guides")
            .join(platform)
            .join(format!("{}.{extension}", old.stem));
        if source.is_file() {
            moves.push((
                source,
                root.join("Guides")
                    .join(platform)
                    .join(format!("{}.{extension}", new.stem)),
            ));
        }
    }
    let old_cheats = root.join("Cheats").join(platform);
    if let Ok(cores) = std::fs::read_dir(&old_cheats) {
        for core in cores.flatten() {
            let source = core.path().join(format!("{}.cht", old.stem));
            if source.is_file() {
                moves.push((source, core.path().join(format!("{}.cht", new.stem))));
            }
        }
    }
    let old_fronts = slot_store::fronts::face_dir(root, old.platform, &old.stem);
    if old_fronts.is_dir() {
        moves.push((
            old_fronts,
            slot_store::fronts::face_dir(root, new.platform, &new.stem),
        ));
    }
    Ok(())
}


///
/// The cached SHA is the cheap answer.  Only rows that are missing a hash are
/// read back, and only while the bill stays small: a GBA library can hold
/// dozens of 16-32 MiB ROMs of identical size, and hashing all of them made
/// "add game" look frozen for half a minute after the upload finished.
fn duplicate_content(
    root: &Path,
    platform: slot_store::Platform,
    digest: &str,
    size: u64,
) -> Option<String> {
    if let Some(meta) = slot_store::cart_meta::read_by_sha(root, platform, digest) {
        return Some(meta.rom);
    }
    const HASH_BUDGET: u64 = 32 * 1024 * 1024;
    const HASH_LIMIT: usize = 4;
    let mut spent = 0;
    for candidate in slot_store::cart_meta::candidates_by_size(root, platform, size) {
        if let Some(sha) = candidate.sha256.as_deref() {
            if sha.eq_ignore_ascii_case(digest) {
                return Some(candidate.rom);
            }
            continue;
        }
        if spent + size > HASH_BUDGET || HASH_LIMIT == 0 {
            crate::link_log::warn(format!(
                "rom add: skipped checking {} for duplicate content",
                candidate.rom
            ));
            continue;
        }
        let path = root.join(&candidate.rom);
        if let Ok(actual) = crate::rom_id::sha256_file(&path) {
            spent += size;
            if actual.eq_ignore_ascii_case(digest) {
                return Some(candidate.rom);
            }
        }
    }
    None
}

/// The name the card will store: the browser's file name, never a path, and
/// only an extension the chosen platform actually boots.
fn rom_file_name(raw: &str, platform: slot_store::Platform) -> Result<String, Response> {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err(Response::error(400, "文件名不合法"));
    }
    if base.chars().any(char::is_control) || base.chars().count() > 120 {
        return Err(Response::error(400, "文件名不合法"));
    }
    if !platform.accepts(Path::new(base)) {
        let extensions: Vec<String> = platform
            .extensions()
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect();
        return Err(Response::error(
            400,
            format!("{} 只接受 {}", platform.dir_name(), extensions.join(" / ")),
        ));
    }
    Ok(base.to_string())
}

pub(super) fn rom_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if request.body.is_empty() {
            return Err(Response::error(400, "ROM 内容为空"));
        }
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("ROM 超过 {mb}MB 上限")));
        }
        let entry = find(shared, request)?;
        if !entry.cart.platform.accepts(&entry.cart.rom) {
            return Err(Response::error(400, "ROM 扩展名与平台不匹配"));
        }
        let digest = crate::rom_id::sha256_hex(&request.body);
        if let Some(existing) = duplicate_rom_by_sha(
            &shared.root,
            entry.cart.platform,
            &digest,
            request.body.len() as u64,
            &entry.cart.rom,
        ) {
            return Err(Response::error(
                409,
                format!("相同内容的 ROM 已存在：{}", existing.rom),
            ));
        }
        let target = checked_target(&shared.root, &entry.cart.rom)?;
        slot_store::atomic_write(&target, &request.body)
            .map_err(|e| Response::error(500, format!("替换 ROM 失败：{e}")))?;
        slot_store::cart_meta::update(&shared.root, &entry.cart, |meta| {
            meta.sha256 = Some(digest.clone());
            meta.size = Some(request.body.len() as u64);
            meta.mtime = std::fs::metadata(&target)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos() as u64);
        })
        .map_err(|e| Response::error(500, format!("写入卡带元数据失败：{e}")))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "rom": &entry.rom,
                "size": request.body.len(),
                "sha256": digest,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn entry_value(entry: &Entry) -> Value {
    let cover = format!("/api/rom/cover?rom={}", util::encode_query(&entry.rom));
    json!({
        "rom": &entry.rom,
        "platform": entry.cart.platform.dir_name(),
        "stem": &entry.cart.stem,
        "model": entry.cart.model.to_value(),
        "file": entry.cart.rom.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        "size": entry.size,
        "mtime": entry.mtime,
        "has_cover": entry.has_cover,
        "has_intro": entry.has_intro,
        "has_video": entry.has_video,
        "cheat": entry.cheat,
        "has_cheat": entry.cheat.is_some(),
        "guide": entry.guide,
        "has_guide": entry.has_guide,
        "cover": cover,
        "cover_width": entry.cover_width,
        "cover_height": entry.cover_height,
        "color": entry.color,
    })
}

fn collect(root: &Path) -> Result<Vec<Entry>, Response> {
    let carts = slot_store::scan_quiet(root)
        .map_err(|e| Response::error(500, format!("扫描游戏库失败：{e}")))?;
    let metas = slot_store::cart_meta::read_all(root);
    let cheat_index = asset_index(root, "Cheats", &["cht"]);
    let guide_index = asset_index(root, "Guides", &["txt", "epub"]);
    let mut entries = Vec::new();
    for cart in carts {
        let Some(rom) = relative_path(root, &cart.rom) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&cart.rom) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let (label_w, label_h) = slot_ui::label_size(&cart);
        let meta = metas.get(&rom);
        let has_cover = cart
            .label
            .as_deref()
            .is_some_and(|label| under_root(root, label) && label.is_file());
        let has_intro = meta
            .and_then(|meta| meta.intro_path(root))
            .is_some()
            || labels_file(root, &cart, "txt").is_file();
        let has_video = meta
            .and_then(|meta| meta.video_path(root))
            .is_some()
            || labels_file(root, &cart, "mp4").is_file();
        let core = slot_store::core_for_platform(root, &cart.stem, cart.platform);
        let cheat = effective_cheat(root, &cart, meta, core, &cheat_index);
        let guide = effective_guide(root, &cart, meta, &guide_index);
        let has_guide = guide.is_some();
        entries.push(Entry {
            color: cart
                .shell
                .map(|shell| slot_store::cart_color::to_hex(shell.colour)),
            cart,
            rom,
            size: metadata.len(),
            mtime: util::mtime_secs(&metadata),
            cover_width: label_w.saturating_mul(2),
            cover_height: label_h.saturating_mul(2),
            has_cover,
            has_intro,
            has_video,
            cheat,
            guide,
            has_guide,
        });
    }
    Ok(entries)
}

type AssetIndex = HashMap<(String, String), Vec<PathBuf>>;

/// Index the files under `Cheats/` or `Guides/` once per Web request.  The
/// platform is the first path component and the lookup key is the lower-cased
/// file stem, so `Game.cht`, `Game.CHT` and nested guide folders all resolve
/// without scanning a directory for every game in the library.
fn asset_index(root: &Path, top: &str, extensions: &[&str]) -> AssetIndex {
    let mut index = AssetIndex::new();
    let Ok(platforms) = std::fs::read_dir(root.join(top)) else {
        return index;
    };
    for platform in platforms.flatten() {
        if !platform
            .file_type()
            .map(|kind| kind.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let name = platform.file_name().to_string_lossy().to_ascii_lowercase();
        collect_assets(&platform.path(), &name, extensions, &mut index);
    }
    for paths in index.values_mut() {
        paths.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    }
    index
}

fn collect_assets(
    dir: &Path,
    platform: &str,
    extensions: &[&str],
    index: &mut AssetIndex,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect_assets(&path, platform, extensions, index);
            continue;
        }
        let Some(extension_name) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !extensions
            .iter()
            .any(|extension| extension_name.eq_ignore_ascii_case(extension))
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        index
            .entry((platform.to_string(), stem.to_lowercase()))
            .or_default()
            .push(path);
    }
}

fn indexed_asset(
    root: &Path,
    index: &AssetIndex,
    cart: &Cart,
    preferred: Option<&str>,
) -> Option<String> {
    let key = (
        cart.platform.dir_name().to_ascii_lowercase(),
        cart.stem.to_lowercase(),
    );
    let paths = index.get(&key)?;
    let path = preferred
        .and_then(|core| {
            paths.iter().find(|path| {
                path.components()
                    .any(|part| part.as_os_str().to_string_lossy().eq_ignore_ascii_case(core))
            })
        })
        .or_else(|| paths.first())?;
    relative_path(root, path)
}

/// The cheat file the device would actually use.  The database may hold only
/// an override; a conventional `Cheats/<platform>/<core>/<stem>.cht` must still
/// appear in the game detail page instead of leaving the input empty.
fn effective_cheat(
    root: &Path,
    cart: &Cart,
    meta: Option<&slot_store::cart_meta::CartMeta>,
    preferred: Core,
    index: &AssetIndex,
) -> Option<String> {
    if let Some(path) = meta.and_then(|meta| meta.cheat_path(root)) {
        return relative_path(root, &path);
    }
    indexed_asset(root, index, cart, Some(preferred.as_str()))
}

/// The same fallback for TXT guides: an override wins, then a conventional
/// guide under any case/subdirectory.
fn effective_guide(
    root: &Path,
    cart: &Cart,
    meta: Option<&slot_store::cart_meta::CartMeta>,
    index: &AssetIndex,
) -> Option<String> {
    if let Some(path) = meta.and_then(|meta| meta.guide_path(root)) {
        return relative_path(root, &path);
    }
    indexed_asset(root, index, cart, None)
}

fn find(shared: &Shared, request: &Request) -> Result<Entry, Response> {
    let raw = http::query_param(request, "rom")
        .map_err(|e| Response::error(e.status, e.message))?
        .ok_or_else(|| Response::error(400, "缺少 rom 参数"))?;
    if raw.is_empty() || raw.len() > MAX_ROM_PATH_BYTES {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    if raw.starts_with('/') || raw.contains('\\') || raw.contains('\0') {
        return Err(Response::error(400, "rom 参数不合法"));
    }
    let entries = collect(&shared.root)?;
    entries
        .into_iter()
        .find(|entry| entry.rom == raw)
        .ok_or_else(|| Response::error(404, "未找到该 ROM"))
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|rel| !rel.is_empty())
}

fn under_root(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

/// Return a target that stays below the card root and is not a symlink.
/// Missing parent directories may be created for a first cover; every existing
/// component is checked before it is followed.
pub(crate) fn checked_target(root: &Path, target: &Path) -> Result<PathBuf, Response> {
    let canon_root = root
        .canonicalize()
        .map_err(|_| Response::error(500, "存储根目录不可用"))?;
    let parent = target
        .parent()
        .ok_or_else(|| Response::error(400, "目标没有父目录"))?;
    let rel = parent
        .strip_prefix(root)
        .map_err(|_| Response::error(403, "目标路径越界"))?;
    let mut current = canon_root.clone();
    for component in rel.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(Response::error(403, "目标路径越界"));
        };
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(Response::error(403, "不允许通过符号链接写入"));
                }
                if !metadata.is_dir() {
                    return Err(Response::error(400, "目标父路径不是目录"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|e| Response::error(500, format!("无法创建目标目录：{e}")))?;
            }
            Err(error) => {
                return Err(Response::error(500, format!("无法检查目标目录：{error}")));
            }
        }
    }
    let parent = current
        .canonicalize()
        .map_err(|_| Response::error(500, "无法解析目标目录"))?;
    if !parent.starts_with(&canon_root) {
        return Err(Response::error(403, "目标路径越界"));
    }
    let name = target
        .file_name()
        .ok_or_else(|| Response::error(400, "目标缺少文件名"))?;
    let target = parent.join(name);
    if let Ok(metadata) = std::fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            return Err(Response::error(403, "不允许通过符号链接写入"));
        }
    }
    Ok(target)
}

fn normalize_cover(root: &Path, body: &[u8], width: u32, height: u32) -> Result<Vec<u8>, Response> {
    if !body.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(Response::error(400, "封面必须是 PNG 图片"));
    }
    let dir = root.join("System").join("Cache").join("Covers");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Response::error(500, format!("无法创建封面缓存目录：{e}")))?;
    let name = format!(
        "cover-{}-{}.png",
        std::process::id(),
        COVER_TEMP.fetch_add(1, Ordering::Relaxed)
    );
    let temp = dir.join(name);
    let result = (|| -> Result<Vec<u8>, Response> {
        std::fs::write(&temp, body)
            .map_err(|e| Response::error(500, format!("无法读取上传封面：{e}")))?;
        let rgba = slot_ui::cover(&temp, width, height)
            .ok_or_else(|| Response::error(400, "封面图片无法解码"))?;
        encode_png(&rgba, width, height)
    })();
    let _ = std::fs::remove_file(&temp);
    result
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, Response> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| Response::error(500, format!("生成封面失败：{e}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| Response::error(500, format!("生成封面失败：{e}")))?;
    }
    Ok(out)
}
