//! Per-game custom front covers.
//!
//! A front face is a full-size RGBA overlay for one cartridge.  Faces are kept
//! next to the card instead of inside the metadata database so a face can be
//! copied, exported and inspected without the database.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cart_meta::CartMeta;
use crate::scan::Cart;
use crate::Platform;

pub const MAX_FRONTS: usize = 5;
pub const MAX_FRONT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PROJECT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_LAYERS: usize = 40;
pub const MAX_TEXT_CHARS: usize = 160;
pub const FRONTS_DIR: &str = "linkSlot/Fronts";
const SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FrontSource {
    #[default]
    Upload,
    Project,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrontFace {
    pub id: String,
    pub name: String,
    pub file: String,
    #[serde(default = "any_model")]
    pub model: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub source: FrontSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

fn any_model() -> String {
    "any".to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FrontManifest {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub rom: String,
    #[serde(default)]
    pub faces: Vec<FrontFace>,
}

pub fn face_dir(root: &Path, platform: Platform, stem: &str) -> PathBuf {
    root.join(FRONTS_DIR).join(platform.dir_name()).join(stem)
}

pub fn manifest_path(root: &Path, platform: Platform, stem: &str) -> PathBuf {
    face_dir(root, platform, stem).join("index.json")
}

pub fn load(root: &Path, platform: Platform, stem: &str) -> Option<FrontManifest> {
    let raw = std::fs::read(manifest_path(root, platform, stem)).ok()?;
    let mut manifest: FrontManifest = serde_json::from_slice(&raw).ok()?;
    manifest.faces.retain(|face| {
        valid_id(&face.id)
            && valid_file(&face.file)
            && face_dir(root, platform, stem).join(&face.file).is_file()
    });
    manifest.faces.sort_by_key(|face| face.order);
    manifest.faces.truncate(MAX_FRONTS);
    Some(manifest)
}

pub fn save(
    root: &Path,
    platform: Platform,
    stem: &str,
    rom: &Path,
    manifest: &FrontManifest,
) -> std::io::Result<()> {
    let dir = face_dir(root, platform, stem);
    std::fs::create_dir_all(&dir)?;
    let mut manifest = manifest.clone();
    manifest.schema = SCHEMA;
    manifest.platform = platform.dir_name().to_string();
    manifest.rom = crate::cart_meta::relative(root, rom);
    manifest.faces.sort_by_key(|face| face.order);
    manifest.faces.truncate(MAX_FRONTS);
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    crate::atomic_write(&dir.join("index.json"), &bytes)
}

pub fn selected_id(meta: &CartMeta) -> Option<&str> {
    meta.extra
        .get("front_face")
        .and_then(serde_json::Value::as_str)
        .filter(|id| valid_id(id))
}

pub fn selected_path(root: &Path, cart: &Cart, meta: Option<&CartMeta>) -> Option<PathBuf> {
    let id = selected_id(meta?)?;
    let manifest = load(root, cart.platform, &cart.stem)?;
    let face = manifest
        .faces
        .iter()
        .find(|face| face.id == id && compatible(&face.model, cart.model))?;
    let path = face_dir(root, cart.platform, &cart.stem).join(&face.file);
    path.is_file().then_some(path)
}

pub fn compatible(face_model: &str, cart_model: crate::CartModel) -> bool {
    face_model == "any" || face_model == cart_model.to_value()
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn valid_file(file: &str) -> bool {
    !file.is_empty()
        && file.len() <= 96
        && !file.contains('/')
        && !file.contains('\\')
        && !file.contains("..")
        && file.ends_with(".png")
}

pub fn valid_project_file(file: &str) -> bool {
    !file.is_empty()
        && file.len() <= 96
        && !file.contains('/')
        && !file.contains('\\')
        && !file.contains("..")
        && file.ends_with(".json")
}

pub fn project_path(root: &Path, platform: Platform, stem: &str, id: &str) -> PathBuf {
    face_dir(root, platform, stem).join(format!("{id}.json"))
}

pub fn load_project(
    root: &Path,
    platform: Platform,
    stem: &str,
    id: &str,
) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let raw = std::fs::read(project_path(root, platform, stem, id)).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn save_project(
    root: &Path,
    platform: Platform,
    stem: &str,
    id: &str,
    project: &Value,
) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid front id",
        ));
    }
    let dir = face_dir(root, platform, stem);
    std::fs::create_dir_all(&dir)?;
    let bytes = serde_json::to_vec_pretty(project)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    crate::atomic_write(&dir.join(format!("{id}.json")), &bytes)
}

pub fn validate_project(
    project: &Value,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let object = project
        .as_object()
        .ok_or_else(|| "project must be a JSON object".to_string())?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("project schema must be 1".to_string());
    }
    let canvas = object
        .get("canvas")
        .and_then(Value::as_object)
        .ok_or_else(|| "project canvas is missing".to_string())?;
    if canvas.get("width").and_then(Value::as_u64) != Some(width as u64)
        || canvas.get("height").and_then(Value::as_u64) != Some(height as u64)
    {
        return Err(format!("project canvas must be {width}x{height}"));
    }
    if let Some(name) = object.get("name").and_then(Value::as_str) {
        if name.chars().count() > 80 {
            return Err("project name is too long".to_string());
        }
    }
    if let Some(background) = object.get("background") {
        if !background.is_string() && !background.is_object() {
            return Err("project background must be a string or object".to_string());
        }
    }
    let layers = object
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(|| "project layers are missing".to_string())?;
    if layers.len() > MAX_LAYERS {
        return Err(format!("project has more than {MAX_LAYERS} layers"));
    }
    for layer in layers {
        let layer = layer
            .as_object()
            .ok_or_else(|| "each project layer must be an object".to_string())?;
        let kind = layer
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "project layer type is missing".to_string())?;
        match kind {
            "background" => {}
            "image" | "label" => {
                let src = layer
                    .get("src")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "image layer src is missing".to_string())?;
                if src.len() > 4 * 1024 * 1024 {
                    return Err("image layer is too large".to_string());
                }
                if !(src.starts_with("data:image/png;base64,")
                    || src.starts_with("data:image/jpeg;base64,")
                    || src.starts_with("data:image/webp;base64,"))
                {
                    return Err("image layer must be a PNG/JPEG/WebP data URL".to_string());
                }
                if src.contains("http://") || src.contains("https://") {
                    return Err("remote image URLs are not allowed".to_string());
                }
            }
            "text" => {
                let text = layer
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "text layer text is missing".to_string())?;
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err(format!("text layer is longer than {MAX_TEXT_CHARS} characters"));
                }
            }
            "shape" => {
                let kind = layer
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "shape layer kind is missing".to_string())?;
                if !matches!(kind, "rect" | "ellipse" | "line") {
                    return Err(format!("unsupported shape kind: {kind}"));
                }
                for key in ["colour", "stroke"] {
                    if let Some(value) = layer.get(key) {
                        if !value.is_string() {
                            return Err(format!("shape {key} must be a color string"));
                        }
                    }
                }
            }
            _ => return Err(format!("unsupported project layer type: {kind}")),
        }
    }
    Ok(())
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{stamp:x}{:x}", NEXT.fetch_add(1, Ordering::Relaxed))
}

pub fn model_from_str(value: &str) -> Option<&'static str> {
    match value {
        "famicom" => Some("famicom"),
        "nes" => Some("nes"),
        "any" => Some("any"),
        _ => None,
    }
}

pub fn model_is_compatible(face_model: &str, platform: Platform) -> bool {
    platform == Platform::Fc || face_model == "any"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_is_capped_at_five_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = FrontManifest::default();
        for index in 0..7 {
            manifest.faces.push(FrontFace {
                id: format!("f{index}"),
                name: format!("face {index}"),
                file: format!("f{index}.png"),
                model: "any".into(),
                order: 6 - index,
                created: 0,
                source: FrontSource::Upload,
                project: None,
            });
        }
        save(
            dir.path(),
            Platform::Gba,
            "Game",
            &dir.path().join("Games/GBA/Game.gba"),
            &manifest,
        )
        .unwrap();
        let face_dir = face_dir(dir.path(), Platform::Gba, "Game");
        for index in 0..7 {
            std::fs::write(face_dir.join(format!("f{index}.png")), b"png").unwrap();
        }
        let loaded = load(dir.path(), Platform::Gba, "Game").unwrap();
        assert_eq!(loaded.faces.len(), 5);
        assert_eq!(loaded.faces[0].id, "f6");
    }

    #[test]
    fn only_png_files_are_accepted() {
        assert!(valid_file("front.png"));
        assert!(!valid_file("../front.png"));
        assert!(!valid_file("front.svg"));
    }

    #[test]
    fn a_quick_cover_project_is_valid() {
        let quick = serde_json::json!({
            "schema": 1,
            "canvas": {"width": 360, "height": 202},
            "background": "rgba(0,0,0,0)",
            "layers": [{
                "type": "image",
                "name": "Cover",
                "src": "data:image/png;base64,AAAA",
                "x": 0,
                "y": 0,
                "w": 360,
                "h": 202,
                "fit": "stretch"
            }]
        });
        assert!(validate_project(&quick, 360, 202).is_ok());
    }

    #[test]
    fn a_project_accepts_shapes_and_rejects_unknown_layer_kinds() {
        let good = serde_json::json!({
            "schema": 1,
            "canvas": {"width": 360, "height": 299},
            "background": {"type": "linear", "from": "#000", "to": "#fff", "angle": 90},
            "layers": [
                {"type": "shape", "kind": "rect", "x": 1, "y": 2, "w": 30, "h": 10}
            ]
        });
        assert!(validate_project(&good, 360, 299).is_ok());
        let bad = serde_json::json!({
            "schema": 1,
            "canvas": {"width": 360, "height": 299},
            "layers": [{"type": "shape", "kind": "star"}]
        });
        assert!(validate_project(&bad, 360, 299).is_err());
    }
}
