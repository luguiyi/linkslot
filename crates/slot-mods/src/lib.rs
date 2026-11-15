//! Lua mod storage shared by the Slot Web UI and the linkSlot CLI.
//!
//! Layout:
//!
//! ```text
//! mods/<mod-id>/mod.json
//! mods/<mod-id>/actions/<action-id>/action.json
//! mods/<mod-id>/actions/<action-id>/main.lua
//! ```
//!
//! A mod is a container; actions are the executable Lua scripts.  This crate is
//! pure Rust (no Lua, no network) so the host builds of `slot` and `linkslot`
//! stay free of a C toolchain.

pub mod choice;
pub mod error;
pub mod files;
pub mod meta;
pub mod path;
pub mod store;
pub mod timing;

pub use choice::{
    ChoiceKind, ChoiceOption, ChoiceRequest, ChoiceResponse, MAX_CHOICE_BODY_CHARS,
    MAX_CHOICE_KEY_CHARS, MAX_CHOICE_LABEL_CHARS, MAX_CHOICE_OPTIONS, MAX_CHOICE_TITLE_CHARS,
    MIN_CHOICE_OPTIONS,
};
pub use error::ModError;
pub use files::{
    has_web_ui, list_files, read_text_file, read_ui_asset, save_sticker_file, save_text_file, web_ui_path,
    ModFileDetail, ModFileEntry, MAX_LISTED_FILES, MAX_STICKER_BYTES, MAX_TEXT_FILE_BYTES,
    MAX_UI_ASSET_BYTES,
};
pub use meta::{
    parse_action, parse_mod, validate_capabilities, validate_name, ActionMeta, ActionUi, ModMeta,
    RawActionMeta, RawModMeta,
};
pub use path::validate_id;
pub use store::revision_of;
pub use timing::Timing;

use std::path::{Path, PathBuf};

/// Directory under the card root that holds one directory per mod.
pub const MODS_DIR: &str = "linkSlot/mods";
/// 把重构前卡根 `mods/` 里的 mod 迁到 `linkSlot/mods/`。
///
/// 只搬目标不存在的条目（同名条目保留新位置的那份）；跳过旧 README；
/// 旧目录只剩 README 时删除。返回搬移的条目数。新卡上不存在旧目录，返回 0。
pub fn migrate_legacy_mods(root: &Path) -> Result<usize, ModError> {
    let legacy = root.join("mods");
    if !legacy.is_dir() {
        return Ok(0);
    }
    let target = path::mods_dir(root);
    std::fs::create_dir_all(&target)
        .map_err(|error| ModError::io(format!("无法创建 {}：{error}", target.display())))?;
    let entries = std::fs::read_dir(&legacy)
        .map_err(|error| ModError::io(format!("无法读取 {}：{error}", legacy.display())))?;
    let mut moved = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "README.txt" {
            continue;
        }
        let destination = target.join(&name);
        if destination.exists() {
            continue;
        }
        std::fs::rename(entry.path(), &destination)
            .map_err(|error| ModError::io(format!("迁移 {} 失败：{error}", entry.path().display())))?;
        moved += 1;
    }
    let leftovers = std::fs::read_dir(&legacy)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name() != "README.txt")
                .count()
        })
        .unwrap_or(1);
    if leftovers == 0 {
        let _ = std::fs::remove_dir_all(&legacy);
    }
    Ok(moved)
}
/// Deleted mods/actions are moved here instead of being removed.
pub const TRASH_DIR: &str = ".trash";
pub const MOD_SCHEMA: u32 = 2;
pub const ACTION_SCHEMA: u32 = 1;
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
pub const MAX_MODS: usize = 64;
pub const MAX_ACTIONS_PER_MOD: usize = 16;
pub const MAX_TOTAL_ACTIONS: usize = 64;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_VERSION_CHARS: usize = 32;
pub const DEFAULT_MOD_VERSION: &str = "1.0.0";
pub const DEFAULT_ORDER: i32 = 100;

/// Web-UI grouping categories a manifest may declare. Order here is the order
/// the list renders in; anything undeclared is inferred, then falls back to
/// `other`.
pub const CATEGORIES: [&str; 4] = ["sync", "info", "tool", "theme"];

/// Capabilities an action may declare in `action.json`（v1.0.0 只解析与展示；
/// v1.1 起才按集合收紧运行时权限）。
///
/// 未声明任何能力的动作视为 `legacy`：v1.0.0 仍按旧行为全允许，但 Web/设备会提示。
pub const CAPABILITIES: [&str; 8] = [
    "fs:saves",
    "fs:states",
    "fs:mod",
    "net:base_url",
    "webdav:read",
    "webdav:write",
    "library:write",
    "ui:confirm",
];

/// 动作最多可声明多少项能力（防止清单膨胀）。
pub const MAX_CAPABILITIES: usize = 8;
/// Gap the device leaves between neighbours when it renumbers a reordered list.
pub const ORDER_STEP: i32 = 10;
pub const MIN_ORDER: i32 = -9999;
pub const MAX_ORDER: i32 = 9999;
/// Upper bound for `boot_delay_minutes` (24 hours).
pub const MAX_BOOT_DELAY_MINUTES: u32 = 1440;
/// Hard response cap for the Web list (far above `MAX_MODS`).
const MAX_LISTED: usize = 256;

/// One mod as shown in the Web list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub enabled: bool,
    pub action_count: usize,
    /// 动作里没有声明 capabilities 的个数（>0 时 Web 会提示"旧版权限"）。
    pub legacy_actions: usize,
    /// True when `web/index.html` exists and can be embedded in the Web page.
    pub has_ui: bool,
    /// Non-empty when the mod or one of its actions is invalid.
    pub error: Option<String>,
    /// Priority from `mod.json`, lower first. See [`set_order`].
    pub order: i32,
    /// `mod.json`'s declared category, or the inferred one.
    pub category: String,
    pub updated_at: u64,
}

/// One action summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionEntry {
    pub id: String,
    pub name: String,
    /// `None` for an invalid action manifest.
    pub timing: Option<Timing>,
    pub enabled: bool,
    pub order: i32,
    /// Minutes after boot for `after_boot`; 0 for other timings.
    pub boot_delay_minutes: u32,
    /// Optional declarative animation for a manual action.
    pub ui: Option<ActionUi>,
    /// Capabilities the action declares; empty = legacy（未声明，v1.0.0 全允许）。
    pub capabilities: Vec<String>,
    /// `true` = 清单里写了 `capabilities` 字段（哪怕是空数组）；
    /// `false` = 旧版清单没写该字段。Web 用它区分"新格式/旧版"。
    pub capabilities_declared: bool,
    pub error: Option<String>,
    pub script_bytes: u64,
    pub updated_at: u64,
}

/// Mod including its action summaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModDetail {
    pub entry: ModEntry,
    pub actions: Vec<ActionEntry>,
    pub revision: String,
    pub meta_present: bool,
}

/// One action including its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionDetail {
    pub entry: ActionEntry,
    pub content: String,
    /// Changes when either `action.json` or `main.lua` changes.
    pub revision: String,
    pub meta_present: bool,
    pub script_present: bool,
}

/// A create/update request for a mod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEdit {
    /// `None` creates a new mod with a generated id.  `Some` creates at that
    /// exact id when `allow_create` is set, otherwise updates an existing mod.
    pub id: Option<String>,
    pub name: String,
    pub version: String,
    pub author: String,
    pub enabled: bool,
    /// Allow a package-provided id to create a new directory.  Normal Web
    /// edits leave this false so a missing mod is still a 404.
    pub allow_create: bool,
    /// Optimistic concurrency for updates; ignored for creation.
    pub revision: Option<String>,
}

/// A create/update request for an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionEdit {
    pub mod_id: String,
    /// `None` creates a new action.
    pub action_id: Option<String>,
    pub name: String,
    pub timing: Timing,
    pub enabled: bool,
    pub order: i32,
    /// Minutes after boot for `after_boot`; ignored for other timings.
    pub boot_delay_minutes: u32,
    pub content: String,
    /// New declarative animation; `None` keeps the existing one on updates.
    pub ui: Option<ActionUi>,
    /// Capabilities from the uploaded manifest; `None` keeps the existing ones
    /// (a legacy action stays legacy, a declared one keeps its list).
    pub capabilities: Option<Vec<String>>,
    /// Optimistic concurrency for updates; ignored for creation.
    pub revision: Option<String>,
}

/// List every valid mod directory, including broken entries.
pub fn list(root: &Path) -> Result<Vec<ModEntry>, ModError> {
    let dir = path::mods_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ModError::Io(format!("无法读取 {}：{error}", dir.display()))),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() || validate_id(&name).is_err() {
            continue;
        }
        out.push(read_parts(root, &name).0);
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    // Priority first: the shelf long-press runs the highest-priority mod that
    // asks for it, and the device list shows that same order, so a player who
    // moves a mod to the top of the list moves it to the front of that queue.
    out.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(out)
}

/// Move one mod's priority and keep the rest of the numbers sane.
///
/// `order` is a plain integer shared with the action manifests (lower runs
/// first), so two mods can carry the same number. Rather than forbid ties —
/// which would make moving one mod rewrite half the list — the caller rewrites
/// the whole queue in steps of [`ORDER_STEP`] once a move lands.
pub fn set_order(root: &Path, id: &str, order: i32) -> Result<ModDetail, ModError> {
    validate_id(id).map_err(ModError::Invalid)?;
    if order < MIN_ORDER || order > MAX_ORDER {
        return Err(ModError::Invalid(format!(
            "order 必须在 {MIN_ORDER}..={MAX_ORDER} 之间"
        )));
    }
    if !path::mod_dir(root, id).is_dir() {
        return Err(ModError::NotFound(format!("mod {id} 不存在")));
    }
    let meta_raw = store::read_optional(&path::meta_path(root, id))?;
    let raw = parse_mod(meta_raw.as_deref());
    let mut meta = raw
        .validated()
        .map_err(|error| ModError::Invalid(format!("mod {id} 的 mod.json 无法修改：{error}")))?;
    if meta.order == order {
        return read_mod(root, id);
    }
    meta.order = order;
    store::atomic_write(&path::meta_path(root, id), &meta.to_bytes()?)?;
    read_mod(root, id)
}

/// Read one mod including its action summaries. A missing directory is
/// `NotFound`; missing/invalid files are reported inside the returned detail.
pub fn read_mod(root: &Path, id: &str) -> Result<ModDetail, ModError> {
    validate_id(id).map_err(ModError::Invalid)?;
    if !path::mod_dir(root, id).is_dir() {
        return Err(ModError::NotFound(format!("mod {id} 不存在")));
    }
    let (entry, actions) = read_parts(root, id);
    let meta_raw = store::read_optional(&path::meta_path(root, id))?;
    let revision = revision_of(meta_raw.as_deref().unwrap_or(&[]), b"");
    Ok(ModDetail {
        entry,
        actions,
        revision,
        meta_present: meta_raw.is_some(),
    })
}

/// Read one action including its Lua source.
pub fn read_action(root: &Path, mod_id: &str, action_id: &str) -> Result<ActionDetail, ModError> {
    validate_id(mod_id).map_err(ModError::Invalid)?;
    validate_id(action_id).map_err(ModError::Invalid)?;
    if !path::mod_dir(root, mod_id).is_dir() {
        return Err(ModError::NotFound(format!("mod {mod_id} 不存在")));
    }
    if !path::action_dir(root, mod_id, action_id).is_dir() {
        return Err(ModError::NotFound(format!(
            "mod {mod_id} 的动作 {action_id} 不存在"
        )));
    }
    let meta_raw = store::read_optional(&path::action_meta_path(root, mod_id, action_id))?;
    let script_raw = store::read_optional(&path::action_script_path(root, mod_id, action_id))?;
    let content = match &script_raw {
        Some(bytes) => {
            if bytes.len() > MAX_SCRIPT_BYTES {
                return Err(ModError::TooLarge(format!(
                    "main.lua 超过 {} KiB 限制",
                    MAX_SCRIPT_BYTES / 1024
                )));
            }
            String::from_utf8(bytes.clone())
                .map_err(|_| ModError::Invalid("main.lua 不是 UTF-8".to_string()))?
        }
        None => String::new(),
    };
    let entry = read_action_entry(root, mod_id, action_id);
    let revision = revision_of(
        meta_raw.as_deref().unwrap_or(&[]),
        script_raw.as_deref().unwrap_or(&[]),
    );
    Ok(ActionDetail {
        entry,
        content,
        revision,
        meta_present: meta_raw.is_some(),
        script_present: script_raw.is_some(),
    })
}

/// Create or update one mod. Metadata is written last.
pub fn save_mod(root: &Path, edit: ModEdit) -> Result<ModDetail, ModError> {
    let name = edit.name.trim().to_string();
    validate_name(&name).map_err(ModError::Invalid)?;
    let version = edit.version.trim().to_string();
    meta::validate_version(&version).map_err(ModError::Invalid)?;
    let author = edit.author.trim().to_string();
    let (id, creating) = match &edit.id {
        Some(id) => {
            validate_id(id).map_err(ModError::Invalid)?;
            let exists = path::mod_dir(root, id).is_dir();
            if !exists && !edit.allow_create {
                return Err(ModError::NotFound(format!("mod {id} 不存在")));
            }
            (id.clone(), !exists)
        }
        None => {
            path::ensure_mods_dir(root)?;
            if list(root)?.len() >= MAX_MODS {
                return Err(ModError::TooLarge(format!("最多只能有 {MAX_MODS} 个 mod")));
            }
            (store::new_mod_id(root), true)
        }
    };

    if creating {
        path::ensure_mods_dir(root)?;
        if list(root)?.len() >= MAX_MODS {
            return Err(ModError::TooLarge(format!("最多只能有 {MAX_MODS} 个 mod")));
        }
    }

    if creating {
        if edit.revision.is_some() {
            return Err(ModError::Conflict("新 mod 不能携带 revision".to_string()));
        }
    } else if let Some(expected) = &edit.revision {
        let meta_raw = store::read_optional(&path::meta_path(root, &id))?;
        let current = revision_of(meta_raw.as_deref().unwrap_or(&[]), b"");
        if &current != expected {
            return Err(ModError::Conflict(
                "mod 已被其他页面修改，请刷新".to_string(),
            ));
        }
    }

    // Editing a manifest in the Web UI must not silently reset the priority
    // the player set on the device, so an update keeps whatever order the
    // existing mod.json carries.
    let (order, category) = if creating {
        (DEFAULT_ORDER, String::new())
    } else {
        store::read_optional(&path::meta_path(root, &id))
            .ok()
            .flatten()
            .map_or((DEFAULT_ORDER, String::new()), |bytes| {
                let raw = parse_mod(Some(&bytes));
                (raw.order, raw.category)
            })
    };
    let meta = ModMeta {
        schema: MOD_SCHEMA,
        name,
        version,
        author,
        enabled: edit.enabled,
        order,
        category,
    };
    meta.validate().map_err(ModError::Invalid)?;
    let dir = path::mod_dir(root, &id);
    std::fs::create_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", dir.display())))?;
    store::atomic_write(&path::meta_path(root, &id), &meta.to_bytes()?)?;
    read_mod(root, &id)
}

/// Create or update one action inside an existing mod.
pub fn save_action(root: &Path, edit: ActionEdit) -> Result<ActionDetail, ModError> {
    validate_id(&edit.mod_id).map_err(ModError::Invalid)?;
    let mod_dir = path::mod_dir(root, &edit.mod_id);
    if !mod_dir.is_dir() {
        return Err(ModError::NotFound(format!("mod {} 不存在", edit.mod_id)));
    }
    let mod_raw = parse_mod(store::read_optional(&path::meta_path(root, &edit.mod_id))?.as_deref());
    if let Some(error) = &mod_raw.error {
        return Err(ModError::Invalid(format!(
            "mod {} 不可编辑：{error}",
            edit.mod_id
        )));
    }
    if edit.order < MIN_ORDER || edit.order > MAX_ORDER {
        return Err(ModError::Invalid(format!(
            "执行顺序必须在 {MIN_ORDER}..={MAX_ORDER} 之间"
        )));
    }
    let name = edit.name.trim().to_string();
    validate_name(&name).map_err(ModError::Invalid)?;
    let content_bytes = edit.content.as_bytes();
    if content_bytes.len() > MAX_SCRIPT_BYTES {
        return Err(ModError::TooLarge(format!(
            "动作脚本超过 {} KiB 限制",
            MAX_SCRIPT_BYTES / 1024
        )));
    }

    let (action_id, creating) = match &edit.action_id {
        Some(action_id) => {
            validate_id(action_id).map_err(ModError::Invalid)?;
            // Importing a ZIP preserves `actions/<id>`.  A requested id that
            // does not exist yet is a create at that exact id (not an error);
            // an existing directory is an update.
            let exists = path::action_dir(root, &edit.mod_id, action_id).is_dir();
            (action_id.clone(), !exists)
        }
        None => (store::new_action_id(root, &edit.mod_id), true),
    };

    if creating {
        let mod_detail = read_mod(root, &edit.mod_id)?;
        if mod_detail.actions.len() >= MAX_ACTIONS_PER_MOD {
            return Err(ModError::TooLarge(format!(
                "每个 mod 最多 {MAX_ACTIONS_PER_MOD} 个动作"
            )));
        }
        let total: usize = list(root)?.iter().map(|entry| entry.action_count).sum();
        if total >= MAX_TOTAL_ACTIONS {
            return Err(ModError::TooLarge(format!(
                "全部 mod 最多 {MAX_TOTAL_ACTIONS} 个动作"
            )));
        }
        if edit.revision.is_some() {
            return Err(ModError::Conflict("新动作不能携带 revision".to_string()));
        }
    } else if let Some(expected) = &edit.revision {
        let meta_raw =
            store::read_optional(&path::action_meta_path(root, &edit.mod_id, &action_id))?;
        let script_raw =
            store::read_optional(&path::action_script_path(root, &edit.mod_id, &action_id))?;
        let current = revision_of(
            meta_raw.as_deref().unwrap_or(&[]),
            script_raw.as_deref().unwrap_or(&[]),
        );
        if &current != expected {
            return Err(ModError::Conflict(
                "动作已被其他页面修改，请刷新".to_string(),
            ));
        }
    }

    let previous = parse_action(
        store::read_optional(&path::action_meta_path(root, &edit.mod_id, &action_id))?.as_deref(),
    )
    .validated()
    .ok();
    let previous_ui = previous.as_ref().and_then(|meta| meta.ui.clone());
    let previous_capabilities = previous
        .map(|meta| meta.capabilities)
        .unwrap_or_default();
    let meta = ActionMeta {
        schema: ACTION_SCHEMA,
        name,
        timing: edit.timing,
        enabled: edit.enabled,
        order: edit.order,
        boot_delay_minutes: edit.boot_delay_minutes,
        ui: edit.ui.or(previous_ui),
        capabilities: edit.capabilities.or(previous_capabilities),
    };
    meta.validate().map_err(ModError::Invalid)?;
    let dir = path::action_dir(root, &edit.mod_id, &action_id);
    std::fs::create_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", dir.display())))?;
    store::atomic_write(
        &path::action_script_path(root, &edit.mod_id, &action_id),
        content_bytes,
    )?;
    store::atomic_write(
        &path::action_meta_path(root, &edit.mod_id, &action_id),
        &meta.to_bytes()?,
    )?;
    read_action(root, &edit.mod_id, &action_id)
}

/// Flip only a mod's `enabled` flag.
pub fn toggle_mod(root: &Path, id: &str, enabled: bool) -> Result<ModDetail, ModError> {
    validate_id(id).map_err(ModError::Invalid)?;
    if !path::mod_dir(root, id).is_dir() {
        return Err(ModError::NotFound(format!("mod {id} 不存在")));
    }
    let meta_raw = store::read_optional(&path::meta_path(root, id))?;
    let raw = parse_mod(meta_raw.as_deref());
    let mut meta = raw.validated().map_err(ModError::Invalid)?;
    meta.enabled = enabled;
    store::atomic_write(&path::meta_path(root, id), &meta.to_bytes()?)?;
    read_mod(root, id)
}

/// Flip only one action's `enabled` flag.
pub fn toggle_action(
    root: &Path,
    mod_id: &str,
    action_id: &str,
    enabled: bool,
) -> Result<ActionDetail, ModError> {
    validate_id(mod_id).map_err(ModError::Invalid)?;
    validate_id(action_id).map_err(ModError::Invalid)?;
    if !path::action_dir(root, mod_id, action_id).is_dir() {
        return Err(ModError::NotFound(format!(
            "mod {mod_id} 的动作 {action_id} 不存在"
        )));
    }
    let meta_raw = store::read_optional(&path::action_meta_path(root, mod_id, action_id))?;
    let raw = parse_action(meta_raw.as_deref());
    let mut meta = raw.validated().map_err(ModError::Invalid)?;
    meta.enabled = enabled;
    store::atomic_write(
        &path::action_meta_path(root, mod_id, action_id),
        &meta.to_bytes()?,
    )?;
    read_action(root, mod_id, action_id)
}

/// Move a mod (and all of its actions) into `mods/.trash/`.
pub fn delete_mod(root: &Path, id: &str) -> Result<PathBuf, ModError> {
    validate_id(id).map_err(ModError::Invalid)?;
    let dir = path::mod_dir(root, id);
    if !dir.is_dir() {
        return Err(ModError::NotFound(format!("mod {id} 不存在")));
    }
    let target = store::trash_target(root, id)?;
    std::fs::rename(&dir, &target).map_err(|error| {
        ModError::Io(format!(
            "无法移动 {} 到 {}：{error}",
            dir.display(),
            target.display()
        ))
    })?;
    Ok(target)
}

/// Permanently remove a mod and all of its actions.
///
/// This is the explicit Web/UI uninstall operation.  It deliberately does not
/// use the trash directory: the caller must confirm the irreversible action.
pub fn uninstall_mod(root: &Path, id: &str) -> Result<PathBuf, ModError> {
    validate_id(id).map_err(ModError::Invalid)?;
    let dir = path::mod_dir(root, id);
    let metadata = std::fs::symlink_metadata(&dir)
        .map_err(|_| ModError::NotFound(format!("mod {id} 不存在")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ModError::Invalid(format!("mod {id} 不是目录")));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法卸载 {}：{error}", dir.display())))?;
    Ok(dir)
}

/// Move one action into `mods/<mod>/.trash/actions/`.
pub fn delete_action(root: &Path, mod_id: &str, action_id: &str) -> Result<PathBuf, ModError> {
    validate_id(mod_id).map_err(ModError::Invalid)?;
    validate_id(action_id).map_err(ModError::Invalid)?;
    let dir = path::action_dir(root, mod_id, action_id);
    if !dir.is_dir() {
        return Err(ModError::NotFound(format!(
            "mod {mod_id} 的动作 {action_id} 不存在"
        )));
    }
    let target = store::action_trash_target(root, mod_id, action_id)?;
    std::fs::rename(&dir, &target).map_err(|error| {
        ModError::Io(format!(
            "无法移动 {} 到 {}：{error}",
            dir.display(),
            target.display()
        ))
    })?;
    Ok(target)
}

fn read_parts(root: &Path, id: &str) -> (ModEntry, Vec<ActionEntry>) {
    let mut errors: Vec<String> = Vec::new();
    let meta_raw = match store::read_optional(&path::meta_path(root, id)) {
        Ok(bytes) => bytes,
        Err(error) => {
            errors.push(error.message().to_string());
            None
        }
    };
    let raw = parse_mod(meta_raw.as_deref());
    if let Some(error) = &raw.error {
        errors.push(error.clone());
    }
    let actions = if raw.error.is_none() {
        scan_actions(root, id, &mut errors)
    } else {
        Vec::new()
    };
    if actions.len() > MAX_ACTIONS_PER_MOD {
        errors.push(format!(
            "动作数量超过每个 mod 最多 {MAX_ACTIONS_PER_MOD} 个的限制"
        ));
    }
    let updated_at = meta_raw
        .as_ref()
        .and_then(|_| std::fs::metadata(path::meta_path(root, id)).ok())
        .map(|_| store::mtime_secs(&path::meta_path(root, id)))
        .unwrap_or(0)
        .max(
            actions
                .iter()
                .map(|action| action.updated_at)
                .max()
                .unwrap_or(0),
        );
    let entry = ModEntry {
        id: id.to_string(),
        name: if raw.name.trim().is_empty() {
            id.to_string()
        } else {
            raw.name.clone()
        },
        version: if raw.version.trim().is_empty() {
            DEFAULT_MOD_VERSION.to_string()
        } else {
            raw.version.clone()
        },
        author: raw.author.clone(),
        enabled: raw.enabled,
        action_count: actions.len(),
        legacy_actions: actions.iter().filter(|a| !a.capabilities_declared).count(),
        order: raw.order,
        category: category_of(root, id, &raw.category, &actions),
        has_ui: files::has_web_ui(root, id),
        error: if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
        updated_at,
    };
    (entry, actions)
}

/// Declared category when the manifest has one, otherwise infer it from what
/// the mod actually does: a long-press Y action means it syncs something, a
/// mod that only ships images is cosmetic, everything else is "other".
fn category_of(root: &Path, id: &str, declared: &str, actions: &[ActionEntry]) -> String {
    if !declared.is_empty() {
        return declared.to_string();
    }
    if actions
        .iter()
        .any(|action| action.timing == Some(crate::Timing::Manual))
    {
        return "sync".to_string();
    }
    let dir = path::mod_dir(root, id);
    let has_images = dir.join("sync-chip.png").is_file()
        || std::fs::read_dir(&dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    name.starts_with("slot_back_") && name.ends_with(".png")
                })
            })
            .unwrap_or(false);
    if has_images {
        "theme".to_string()
    } else {
        "other".to_string()
    }
}

fn scan_actions(root: &Path, mod_id: &str, errors: &mut Vec<String>) -> Vec<ActionEntry> {
    let dir = path::actions_dir(root, mod_id);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            errors.push(format!("无法读取动作目录：{error}"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || validate_id(&name).is_err() {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let action = read_action_entry(root, mod_id, &name);
        if let Some(error) = &action.error {
            errors.push(format!("动作 {name}：{error}"));
        }
        out.push(action);
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    out.sort_by(action_order);
    out
}

fn action_order(left: &ActionEntry, right: &ActionEntry) -> std::cmp::Ordering {
    left.order
        .cmp(&right.order)
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.id.cmp(&right.id))
}

fn read_action_entry(root: &Path, mod_id: &str, action_id: &str) -> ActionEntry {
    let mut errors = Vec::new();
    let meta_raw = match store::read_optional(&path::action_meta_path(root, mod_id, action_id)) {
        Ok(bytes) => bytes,
        Err(error) => {
            errors.push(error.message().to_string());
            None
        }
    };
    let raw = parse_action(meta_raw.as_deref());
    if let Some(error) = &raw.error {
        errors.push(error.clone());
    }
    let script_path = path::action_script_path(root, mod_id, action_id);
    let (script_bytes, script_mtime) = match std::fs::metadata(&script_path) {
        Ok(metadata) if metadata.is_file() => (metadata.len(), store::mtime_secs(&script_path)),
        Ok(_) => {
            errors.push("main.lua 不是文件".to_string());
            (0, 0)
        }
        Err(_) => {
            errors.push("缺少 main.lua".to_string());
            (0, 0)
        }
    };
    if script_bytes > MAX_SCRIPT_BYTES as u64 {
        errors.push(format!(
            "main.lua 超过 {} KiB 限制",
            MAX_SCRIPT_BYTES / 1024
        ));
    }
    let updated_at = std::fs::metadata(path::action_meta_path(root, mod_id, action_id))
        .map(|_| store::mtime_secs(&path::action_meta_path(root, mod_id, action_id)))
        .unwrap_or(0)
        .max(script_mtime);
    ActionEntry {
        id: action_id.to_string(),
        name: if raw.name.trim().is_empty() {
            action_id.to_string()
        } else {
            raw.name.clone()
        },
        timing: raw.timing,
        enabled: raw.enabled,
        order: raw.order,
        boot_delay_minutes: raw.boot_delay_minutes,
        ui: raw.ui.clone(),
        capabilities: raw.capabilities.clone().unwrap_or_default(),
        capabilities_declared: raw.capabilities.is_some(),
        error: if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
        script_bytes,
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn write_mod(root: &Path, id: &str, name: &str, enabled: bool) {
        write_file(
            &path::meta_path(root, id),
            format!("{{\"schema\":{MOD_SCHEMA},\"name\":\"{name}\",\"enabled\":{enabled}}}")
                .as_bytes(),
        );
    }

    fn write_action(
        root: &Path,
        mod_id: &str,
        action_id: &str,
        name: &str,
        timing: &str,
        enabled: bool,
        order: i32,
        script: &str,
    ) {
        write_file(
            &path::action_meta_path(root, mod_id, action_id),
            format!(
                "{{\"schema\":{ACTION_SCHEMA},\"name\":\"{name}\",\"timing\":\"{timing}\",\"enabled\":{enabled},\"order\":{order}}}"
            )
            .as_bytes(),
        );
        write_file(
            &path::action_script_path(root, mod_id, action_id),
            script.as_bytes(),
        );
    }

    #[test]
    fn missing_mods_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn lists_new_structure_and_rejects_legacy() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "good", "好 mod", true);
        write_action(
            dir.path(),
            "good",
            "after",
            "下载后",
            "after_download",
            true,
            100,
            "return 1",
        );
        write_file(
            &path::meta_path(dir.path(), "legacy"),
            r#"{"schema":1,"name":"旧 mod","timing":"after_download"}"#.as_bytes(),
        );
        let entries = list(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        let good = entries.iter().find(|entry| entry.id == "good").unwrap();
        assert_eq!(good.name, "好 mod");
        assert_eq!(good.action_count, 1);
        assert!(good.error.is_none());
        let legacy = entries.iter().find(|entry| entry.id == "legacy").unwrap();
        assert!(legacy.error.as_deref().unwrap().contains("旧版"));
        assert_eq!(legacy.action_count, 0);
    }

    #[test]
    fn reads_actions_in_order() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "demo", "演示", true);
        write_action(
            dir.path(),
            "demo",
            "b",
            "后",
            "after_sync",
            true,
            200,
            "return 2",
        );
        write_action(
            dir.path(),
            "demo",
            "a",
            "前",
            "after_sync",
            true,
            100,
            "return 1",
        );
        let detail = read_mod(dir.path(), "demo").unwrap();
        assert_eq!(detail.actions.len(), 2);
        assert_eq!(detail.actions[0].id, "a");
        assert_eq!(detail.actions[1].id, "b");
        let action = read_action(dir.path(), "demo", "a").unwrap();
        assert_eq!(action.content, "return 1");
        assert_eq!(action.entry.timing, Some(Timing::AfterSync));
    }

    #[test]
    fn a_mod_without_an_order_reads_as_the_default() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "plain", "没有优先级", true);
        let entries = list(dir.path()).unwrap();
        assert_eq!(entries[0].order, DEFAULT_ORDER);
    }

    #[test]
    fn the_list_is_the_priority_queue() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "beta", "Beta", true);
        write_mod(dir.path(), "alpha", "Alpha", true);
        // Both start at the default, so the tie is broken by name.
        let ids: Vec<String> = list(dir.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, ["alpha", "beta"]);

        // A lower number outranks a higher one whatever the name is.
        set_order(dir.path(), "beta", ORDER_STEP).unwrap();
        let ids: Vec<String> = list(dir.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, ["beta", "alpha"], "the priority did not reorder the list");
        let raw = std::fs::read(path::meta_path(dir.path(), "beta")).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains(&format!("\"order\": {ORDER_STEP}")),
            "the manifest does not carry the priority: {text}"
        );
        // The default is not written out, so an untouched mod.json stays as
        // the mod author shipped it.
        let raw = std::fs::read(path::meta_path(dir.path(), "alpha")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("\"order\""));
    }

    #[test]
    fn set_order_refuses_a_number_outside_the_range() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "demo", "演示", true);
        assert!(set_order(dir.path(), "demo", MAX_ORDER + 1).is_err());
        assert!(set_order(dir.path(), "demo", MIN_ORDER - 1).is_err());
        assert!(
            set_order(dir.path(), "missing", ORDER_STEP).is_err(),
            "a missing mod accepted a priority"
        );
    }

    #[test]
    fn save_mod_creates_empty_mod_then_save_action() {
        let dir = tempfile::tempdir().unwrap();
        let created = save_mod(
            dir.path(),
            ModEdit {
                id: None,
                name: "新 mod".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            },
        )
        .unwrap();
        assert!(validate_id(&created.entry.id).is_ok());
        assert_eq!(created.entry.version, "1.0.0");
        assert!(created.actions.is_empty());
        let id = created.entry.id.clone();
        let action = save_action(
            dir.path(),
            ActionEdit {
                mod_id: id.clone(),
                action_id: None,
                name: "新动作".to_string(),
                timing: Timing::AfterSync,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "log.info('hi')".to_string(),
                ui: None,
                capabilities: None,
                revision: None,
            },
        )
        .unwrap();
        assert_eq!(action.content, "log.info('hi')");
        assert_eq!(read_mod(dir.path(), &id).unwrap().actions.len(), 1);
    }


    #[test]
    fn uploaded_action_capabilities_are_saved_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let created = save_mod(
            dir.path(),
            ModEdit {
                id: Some("caps".to_string()),
                name: "Caps".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: true,
                revision: None,
            },
        )
        .unwrap();
        let edit = |capabilities: Option<Vec<String>>| ActionEdit {
            mod_id: created.entry.id.clone(),
            action_id: Some("run".to_string()),
            name: "运行".to_string(),
            timing: Timing::Manual,
            enabled: true,
            order: 100,
            boot_delay_minutes: 0,
            content: "return".to_string(),
            ui: None,
            capabilities,
            revision: None,
        };
        // 旧包没有这个字段：保持 legacy。
        let detail = save_action(dir.path(), edit(None)).unwrap();
        assert!(!detail.entry.capabilities_declared);

        // 新包上传声明：写进卡上的 action.json。
        let detail = save_action(
            dir.path(),
            edit(Some(vec!["fs:saves".to_string(), "ui:confirm".to_string()])),
        )
        .unwrap();
        assert!(detail.entry.capabilities_declared);
        assert_eq!(
            detail.entry.capabilities,
            vec!["fs:saves".to_string(), "ui:confirm".to_string()]
        );
        let raw =
            std::fs::read(path::action_meta_path(dir.path(), &created.entry.id, "run")).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("fs:saves"));

        // 之后的普通编辑不带 capabilities：保留声明。
        let detail = save_action(dir.path(), edit(None)).unwrap();
        assert!(detail.entry.capabilities_declared);

        // 显式空列表：新版且明确"不需要能力"。
        let detail = save_action(dir.path(), edit(Some(Vec::new()))).unwrap();
        assert!(detail.entry.capabilities_declared);
        assert!(detail.entry.capabilities.is_empty());
    }    #[test]
    fn a_package_can_create_and_update_a_stable_id() {
        let dir = tempfile::tempdir().unwrap();
        let edit = || ModEdit {
            id: Some("gbastation-battery-sync".to_string()),
            name: "GBAStation".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            enabled: false,
            allow_create: true,
            revision: None,
        };
        let created = save_mod(dir.path(), edit()).unwrap();
        assert_eq!(created.entry.id, "gbastation-battery-sync");
        assert!(path::mod_dir(dir.path(), "gbastation-battery-sync").is_dir());
        let updated = save_mod(dir.path(), edit()).unwrap();
        assert_eq!(updated.entry.id, "gbastation-battery-sync");
    }

    #[test]
    fn a_normal_edit_cannot_create_a_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let error = save_mod(
            dir.path(),
            ModEdit {
                id: Some("missing".to_string()),
                name: "missing".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            },
        )
        .unwrap_err();
        assert!(matches!(error, ModError::NotFound(_)));
    }

    #[test]
    fn revision_conflict_and_toggles() {
        let dir = tempfile::tempdir().unwrap();
        let created = save_mod(
            dir.path(),
            ModEdit {
                id: None,
                name: "x".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            },
        )
        .unwrap();
        let mod_id = created.entry.id.clone();
        let action = save_action(
            dir.path(),
            ActionEdit {
                mod_id: mod_id.clone(),
                action_id: None,
                name: "a".to_string(),
                timing: Timing::AfterDownload,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return 1".to_string(),
                ui: None,
                capabilities: None,
                revision: None,
            },
        )
        .unwrap();
        assert!(toggle_mod(dir.path(), &mod_id, true).unwrap().entry.enabled);
        assert!(
            !toggle_action(dir.path(), &mod_id, &action.entry.id, false)
                .unwrap()
                .entry
                .enabled
        );
        let error = save_action(
            dir.path(),
            ActionEdit {
                mod_id: mod_id.clone(),
                action_id: Some(action.entry.id.clone()),
                name: "a".to_string(),
                timing: Timing::AfterDownload,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return 2".to_string(),
                ui: None,
                capabilities: None,
                revision: Some("deadbeef".to_string()),
            },
        )
        .unwrap_err();
        assert!(matches!(error, ModError::Conflict(_)));
    }

    #[test]
    fn delete_action_moves_to_mod_trash_and_uninstall_removes() {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "demo", "演示", true);
        write_action(
            dir.path(),
            "demo",
            "a",
            "动作",
            "before_upload",
            true,
            100,
            "return",
        );
        let trash = delete_action(dir.path(), "demo", "a").unwrap();
        assert!(trash.starts_with(path::action_trash_dir(dir.path(), "demo")));
        assert!(!path::action_dir(dir.path(), "demo", "a").exists());
        let removed = uninstall_mod(dir.path(), "demo").unwrap();
        assert!(!removed.exists());
        assert!(!path::mod_dir(dir.path(), "demo").exists());
    }

    #[test]
    fn save_action_accepts_an_explicit_id_for_a_new_action() {
        let dir = tempfile::tempdir().unwrap();
        let created = save_mod(
            dir.path(),
            ModEdit {
                id: None,
                name: "导入的 mod".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            },
        )
        .unwrap();
        let detail = save_action(
            dir.path(),
            ActionEdit {
                mod_id: created.entry.id.clone(),
                action_id: Some("sync".to_string()),
                name: "同步".to_string(),
                timing: Timing::Manual,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return 1".to_string(),
                ui: None,
                capabilities: None,
                revision: None,
            },
        )
        .unwrap();
        assert_eq!(detail.entry.id, "sync");
        assert_eq!(read_mod(dir.path(), &created.entry.id).unwrap().actions.len(), 1);
    }

    #[test]
    fn rejects_bad_inputs_and_legacy_actions() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save_mod(
            dir.path(),
            ModEdit {
                id: None,
                name: "".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            }
        )
        .is_err());
        assert!(save_mod(
            dir.path(),
            ModEdit {
                id: None,
                name: "x".to_string(),
                version: "../escape".to_string(),
                author: String::new(),
                enabled: false,
                allow_create: false,
                revision: None,
            }
        )
        .is_err());
        write_mod(dir.path(), "legacy", "旧", true);
        write_file(
            &path::meta_path(dir.path(), "legacy"),
            r#"{"schema":1,"name":"旧","timing":"after_download"}"#.as_bytes(),
        );
        let error = save_action(
            dir.path(),
            ActionEdit {
                mod_id: "legacy".to_string(),
                action_id: None,
                name: "x".to_string(),
                timing: Timing::AfterDownload,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return".to_string(),
                ui: None,
                capabilities: None,
                revision: None,
            },
        )
        .unwrap_err();
        assert!(matches!(error, ModError::Invalid(_)));
    }

    #[test]
    fn action_ui_is_applied_and_preserved_on_later_edits() {
        let dir = tempfile::tempdir().unwrap();
        let created = save_mod(
            dir.path(),
            ModEdit {
                id: Some("ui-mod".to_string()),
                name: "动画 mod".to_string(),
                version: "1.0.0".to_string(),
                author: String::new(),
                enabled: true,
                allow_create: true,
                revision: None,
            },
        )
        .unwrap();
        let id = created.entry.id.clone();
        let ui = ActionUi {
            animation: "cloud_sync".to_string(),
            title: "同步测试".to_string(),
            units: vec!["sav".to_string()],
            show_cloud: false,
            direction: Some("upload".to_string()),
        };
        let detail = save_action(
            dir.path(),
            ActionEdit {
                mod_id: id.clone(),
                action_id: Some("run".to_string()),
                name: "运行".to_string(),
                timing: Timing::Manual,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return".to_string(),
                ui: Some(ui),
                capabilities: None,
                revision: None,
            },
        )
        .unwrap();
        assert_eq!(detail.entry.ui.as_ref().unwrap().title, "同步测试");

        // A normal Web action edit does not resend `ui`; it must survive.
        save_action(
            dir.path(),
            ActionEdit {
                mod_id: id.clone(),
                action_id: Some("run".to_string()),
                name: "运行".to_string(),
                timing: Timing::Manual,
                enabled: true,
                order: 100,
                boot_delay_minutes: 0,
                content: "return 1".to_string(),
                ui: None,
                capabilities: None,
                revision: None,
            },
        )
        .unwrap();
        assert_eq!(
            read_action(dir.path(), &id, "run")
                .unwrap()
                .entry
                .ui
                .unwrap()
                .title,
            "同步测试"
        );
    }
}

    #[test]
    fn legacy_root_mods_are_migrated_into_link_slot() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("mods/demo/web")).unwrap();
        std::fs::write(root.path().join("mods/demo/mod.json"), b"{}").unwrap();
        std::fs::write(root.path().join("mods/demo/web/index.html"), b"x").unwrap();
        std::fs::write(root.path().join("mods/README.txt"), b"old").unwrap();

        let moved = migrate_legacy_mods(root.path()).unwrap();
        assert_eq!(moved, 1);
        assert!(root.path().join("linkSlot/mods/demo/mod.json").is_file());
        assert!(root.path().join("linkSlot/mods/demo/web/index.html").is_file());
        assert!(!root.path().join("mods").exists(), "旧 mods/ 目录应被删除");
    }

    #[test]
    fn migration_never_overwrites_a_newer_mod() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("mods/demo")).unwrap();
        std::fs::write(root.path().join("mods/demo/mod.json"), b"old").unwrap();
        std::fs::create_dir_all(root.path().join("linkSlot/mods/demo")).unwrap();
        std::fs::write(root.path().join("linkSlot/mods/demo/mod.json"), b"new").unwrap();

        let moved = migrate_legacy_mods(root.path()).unwrap();
        assert_eq!(moved, 0);
        assert_eq!(
            std::fs::read(root.path().join("linkSlot/mods/demo/mod.json")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn a_card_without_a_legacy_dir_is_untouched() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(migrate_legacy_mods(root.path()).unwrap(), 0);
        assert!(!root.path().join("linkSlot/mods").exists());
    }