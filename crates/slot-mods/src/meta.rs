//! Tolerant `mod.json` / `action.json` parsing plus the strict metadata written
//! by the Web UI.

use serde::{Deserialize, Serialize};

use crate::error::ModError;
use crate::timing::Timing;
use crate::{
    ACTION_SCHEMA, DEFAULT_MOD_VERSION, MAX_NAME_CHARS, MAX_ORDER, MAX_VERSION_CHARS, MIN_ORDER,
    MOD_SCHEMA,
};

/// The strict mod manifest written by the Web UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMeta {
    pub schema: u32,
    pub name: String,
    #[serde(default = "default_mod_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default)]
    pub enabled: bool,
    /// Priority, shared with the action manifests' `order`: **lower runs
    /// first**, so a mod with a smaller number outranks one with a larger.
    ///
    /// Only mods that ask for the same thing compete — the shelf long-press
    /// runs one `manual` action, and this is what decides whose. Absent means
    /// [`crate::DEFAULT_ORDER`], which is what every manifest written before
    /// this field existed still reads as.
    #[serde(default = "default_order", skip_serializing_if = "order_is_default")]
    pub order: i32,
    /// Optional Web-UI grouping hint; empty means "infer from behaviour".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
}

fn default_mod_version() -> String {
    DEFAULT_MOD_VERSION.to_string()
}

fn order_is_default(order: &i32) -> bool {
    *order == crate::DEFAULT_ORDER
}

impl ModMeta {
    pub fn to_bytes(&self) -> Result<Vec<u8>, ModError> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| ModError::Io(format!("无法序列化 mod.json：{error}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.category.is_empty() && !crate::CATEGORIES.contains(&self.category.as_str()) {
            return Err(format!("category 只支持 {}", crate::CATEGORIES.join(" / ")));
        }
        validate_name(&self.name)?;
        validate_version(&self.version)?;
        if self.schema != MOD_SCHEMA {
            return Err(format!("不支持的 mod schema {}", self.schema));
        }
        if self.order < MIN_ORDER || self.order > MAX_ORDER {
            return Err(format!("order 必须在 {MIN_ORDER}..={MAX_ORDER} 之间"));
        }
        Ok(())
    }
}

/// Optional device-side animation for a manual action. It is deliberately
/// declarative: a mod can choose the existing cloud-sync grammar, but cannot
/// make the device claim a transfer direction or unit that is not real.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionUi {
    /// Only `cloud_sync` is understood today.
    pub animation: String,
    #[serde(default)]
    pub title: String,
    /// `state`, `sav`, or both. Empty means no unit nodes.
    #[serde(default)]
    pub units: Vec<String>,
    #[serde(default = "default_true")]
    pub show_cloud: bool,
    /// `upload`, `download`, or absent for no arrow.
    #[serde(default)]
    pub direction: Option<String>,
}

fn default_true() -> bool {
    true
}

impl ActionUi {
    pub fn validate(&self) -> Result<(), String> {
        if self.animation != "cloud_sync" {
            return Err(format!("不支持的 ui.animation {:?}", self.animation));
        }
        let title = self.title.trim();
        if title.chars().count() > 24 {
            return Err("ui.title 不能超过 24 个字符".to_string());
        }
        if title.chars().any(char::is_control) {
            return Err("ui.title 不能包含控制字符".to_string());
        }
        if self.units.len() > 2 {
            return Err("ui.units 最多两个".to_string());
        }
        for unit in &self.units {
            if unit != "state" && unit != "sav" {
                return Err(format!("ui.units 只支持 state / sav，收到 {unit:?}"));
            }
        }
        if self.units.iter().collect::<std::collections::HashSet<_>>().len() != self.units.len() {
            return Err("ui.units 不能重复".to_string());
        }
        if let Some(direction) = &self.direction {
            if direction != "upload" && direction != "download" {
                return Err(format!("ui.direction 只支持 upload / download，收到 {direction:?}"));
            }
        }
        Ok(())
    }
}

/// The strict action manifest written by the Web UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionMeta {
    pub schema: u32,
    pub name: String,
    pub timing: Timing,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_order")]
    pub order: i32,
    /// Minutes after system boot for `timing=after_boot`; ignored otherwise.
    #[serde(default)]
    pub boot_delay_minutes: u32,
    /// Optional device animation for a manual action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<ActionUi>,
    /// Capabilities the action declares.
    ///
    /// `None` = 旧版清单（没写这个字段，v1.0.0 视为 legacy、全允许）；
    /// `Some([])` = 新版清单且明确声明"不需要任何能力"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// 校验能力列表：每项必须在白名单内、不得重复、不得超过上限。
pub fn validate_capabilities(list: &[String]) -> Result<(), String> {
    if list.len() > crate::MAX_CAPABILITIES {
        return Err(format!(
            "capabilities 最多 {} 项",
            crate::MAX_CAPABILITIES
        ));
    }
    for (index, item) in list.iter().enumerate() {
        if !crate::CAPABILITIES.contains(&item.as_str()) {
            return Err(format!(
                "未知能力 {item:?}；可用：{}",
                crate::CAPABILITIES.join(", ")
            ));
        }
        if list[..index].contains(item) {
            return Err(format!("能力 {item:?} 重复声明"));
        }
    }
    Ok(())
}

fn default_order() -> i32 {
    crate::DEFAULT_ORDER
}

impl ActionMeta {
    pub fn to_bytes(&self) -> Result<Vec<u8>, ModError> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| ModError::Io(format!("无法序列化 action.json：{error}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        if self.schema != ACTION_SCHEMA {
            return Err(format!("不支持的 action schema {}", self.schema));
        }
        if self.order < MIN_ORDER || self.order > MAX_ORDER {
            return Err(format!("order 必须在 {MIN_ORDER}..={MAX_ORDER} 之间"));
        }
        if self.boot_delay_minutes > crate::MAX_BOOT_DELAY_MINUTES {
            return Err(format!(
                "boot_delay_minutes 必须在 0..={} 之间",
                crate::MAX_BOOT_DELAY_MINUTES
            ));
        }
        if let Some(ui) = &self.ui {
            ui.validate()?;
        }
        if let Some(list) = &self.capabilities {
            validate_capabilities(list)?;
        }
        Ok(())
    }
}

/// Tolerant mod parse result: invalid fields keep a usable fallback and an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawModMeta {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub author: String,
    pub enabled: bool,
    pub order: i32,
    pub category: String,
    pub error: Option<String>,
}

impl RawModMeta {
    pub fn validated(&self) -> Result<ModMeta, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let meta = ModMeta {
            schema: self.schema,
            name: self.name.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            enabled: self.enabled,
            order: self.order,
            category: self.category.clone(),
        };
        meta.validate()?;
        Ok(meta)
    }
}

/// Tolerant action parse result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawActionMeta {
    pub schema: u32,
    pub name: String,
    pub timing: Option<Timing>,
    pub enabled: bool,
    pub order: i32,
    pub boot_delay_minutes: u32,
    pub ui: Option<ActionUi>,
    pub capabilities: Option<Vec<String>>,
    pub error: Option<String>,
}

impl RawActionMeta {
    pub fn validated(&self) -> Result<ActionMeta, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let timing = self.timing.ok_or_else(|| "缺少 timing".to_string())?;
        let meta = ActionMeta {
            schema: self.schema,
            name: self.name.clone(),
            timing,
            enabled: self.enabled,
            order: self.order,
            boot_delay_minutes: self.boot_delay_minutes,
            ui: self.ui.clone(),
            capabilities: self.capabilities.clone(),
        };
        meta.validate()?;
        Ok(meta)
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    let chars = name.chars().count();
    if chars == 0 {
        return Err("mod/动作名称不能为空".to_string());
    }
    if chars > MAX_NAME_CHARS {
        return Err(format!("mod/动作名称不能超过 {MAX_NAME_CHARS} 个字符"));
    }
    if name.chars().any(char::is_control) {
        return Err("名称不能包含控制字符".to_string());
    }
    Ok(())
}

/// Mod versions are deliberately filename-safe: letters, digits, `.`, `_`, `+`
/// and `-`. This lets the mod ZIP filename carry the version verbatim.
pub fn validate_version(version: &str) -> Result<(), String> {
    let version = version.trim();
    if version.is_empty() {
        return Err("mod 版本不能为空".to_string());
    }
    let count = version.chars().count();
    if count > MAX_VERSION_CHARS {
        return Err(format!("mod 版本不能超过 {MAX_VERSION_CHARS} 个字符"));
    }
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
    {
        return Err("mod 版本只能包含字母、数字、. _ + -，且不能含空格或路径分隔符".to_string());
    }
    Ok(())
}

/// Parse `mod.json` bytes. `None` means the file is missing.
pub fn parse_mod(bytes: Option<&[u8]>) -> RawModMeta {
    let mut raw = RawModMeta {
        schema: 0,
        name: String::new(),
        version: DEFAULT_MOD_VERSION.to_string(),
        author: String::new(),
        enabled: false,
        order: crate::DEFAULT_ORDER,
        category: String::new(),
        error: None,
    };
    let Some(bytes) = bytes else {
        raw.error = Some("缺少 mod.json".to_string());
        return raw;
    };
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => {
            raw.error = Some(format!("mod.json 解析失败：{error}"));
            return raw;
        }
    };
    let Some(object) = value.as_object() else {
        raw.error = Some("mod.json 必须是 JSON 对象".to_string());
        return raw;
    };
    let mut errors = Vec::new();
    match object.get("schema") {
        Some(value) => match value.as_u64() {
            Some(schema) if schema <= u32::MAX as u64 => raw.schema = schema as u32,
            _ => errors.push("schema 必须是整数".to_string()),
        },
        None => errors.push("缺少 schema".to_string()),
    }
    match object.get("name") {
        Some(value) => match value.as_str() {
            Some(name) => raw.name = name.trim().to_string(),
            None => errors.push("name 必须是字符串".to_string()),
        },
        None => errors.push("缺少 name".to_string()),
    }
    if let Some(value) = object.get("version") {
        match value.as_str() {
            Some(version) => raw.version = version.trim().to_string(),
            None => errors.push("version 必须是字符串".to_string()),
        }
    }
    if let Some(value) = object.get("author") {
        if let Some(author) = value.as_str() {
            raw.author = author.trim().to_string();
        }
    }
    if let Some(value) = object.get("enabled") {
        match value.as_bool() {
            Some(enabled) => raw.enabled = enabled,
            None => errors.push("enabled 必须是布尔值".to_string()),
        }
    }
    if let Some(value) = object.get("order") {
        match value.as_i64() {
            Some(order) if order >= MIN_ORDER as i64 && order <= MAX_ORDER as i64 => {
                raw.order = order as i32;
            }
            Some(_) => errors.push(format!("order 必须在 {MIN_ORDER}..={MAX_ORDER} 之间")),
            None => errors.push("order 必须是整数".to_string()),
        }
    }
    if let Some(value) = object.get("category") {
        match value.as_str() {
            Some(category) if category.trim().is_empty() => {}
            Some(category) if crate::CATEGORIES.contains(&category.trim()) => {
                raw.category = category.trim().to_string();
            }
            Some(_) => errors.push(format!("category 只支持 {}", crate::CATEGORIES.join(" / "))),
            None => errors.push("category 必须是字符串".to_string()),
        }
    }
    if raw.schema == 1 {
        errors.push("旧版单脚本 mod 结构，不支持；请重新安装新结构 mod".to_string());
    } else if raw.schema != MOD_SCHEMA && raw.schema != 0 {
        errors.push(format!("不支持的 mod schema {}", raw.schema));
    }
    if let Err(error) = validate_name(&raw.name) {
        if !raw.name.is_empty() {
            errors.push(error);
        }
    }
    if let Err(error) = validate_version(&raw.version) {
        if !raw.version.is_empty() {
            errors.push(error);
        }
    }
    if !errors.is_empty() {
        raw.error = Some(errors.join("；"));
    }
    raw
}

/// Parse `action.json` bytes. `None` means the file is missing.
pub fn parse_action(bytes: Option<&[u8]>) -> RawActionMeta {
    let mut raw = RawActionMeta {
        schema: 0,
        name: String::new(),
        timing: None,
        enabled: true,
        order: crate::DEFAULT_ORDER,
        boot_delay_minutes: 0,
        ui: None,
        capabilities: None,
        error: None,
    };
    let Some(bytes) = bytes else {
        raw.error = Some("缺少 action.json".to_string());
        return raw;
    };
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => {
            raw.error = Some(format!("action.json 解析失败：{error}"));
            return raw;
        }
    };
    let Some(object) = value.as_object() else {
        raw.error = Some("action.json 必须是 JSON 对象".to_string());
        return raw;
    };
    let mut errors = Vec::new();
    match object.get("schema") {
        Some(value) => match value.as_u64() {
            Some(schema) if schema <= u32::MAX as u64 => raw.schema = schema as u32,
            _ => errors.push("schema 必须是整数".to_string()),
        },
        None => errors.push("缺少 schema".to_string()),
    }
    match object.get("name") {
        Some(value) => match value.as_str() {
            Some(name) => raw.name = name.trim().to_string(),
            None => errors.push("name 必须是字符串".to_string()),
        },
        None => errors.push("缺少 name".to_string()),
    }
    match object.get("timing") {
        Some(value) => match value.as_str().and_then(Timing::parse) {
            Some(timing) => raw.timing = Some(timing),
            None => errors.push("timing 无效".to_string()),
        },
        None => errors.push("缺少 timing".to_string()),
    }
    if let Some(value) = object.get("enabled") {
        match value.as_bool() {
            Some(enabled) => raw.enabled = enabled,
            None => errors.push("enabled 必须是布尔值".to_string()),
        }
    }
    if let Some(value) = object.get("order") {
        match value.as_i64() {
            Some(order) if order >= MIN_ORDER as i64 && order <= MAX_ORDER as i64 => {
                raw.order = order as i32;
            }
            Some(_) => errors.push(format!("order 必须在 {MIN_ORDER}..={MAX_ORDER} 之间")),
            None => errors.push("order 必须是整数".to_string()),
        }
    }
    if let Some(value) = object.get("boot_delay_minutes") {
        match value.as_u64() {
            Some(minutes) if minutes <= crate::MAX_BOOT_DELAY_MINUTES as u64 => {
                raw.boot_delay_minutes = minutes as u32;
            }
            Some(_) => errors.push(format!(
                "boot_delay_minutes 必须在 0..={} 之间",
                crate::MAX_BOOT_DELAY_MINUTES
            )),
            None => errors.push("boot_delay_minutes 必须是非负整数".to_string()),
        }
    }
    if let Some(value) = object.get("capabilities") {
        match value.as_array() {
            Some(items) => {
                let mut list = Vec::new();
                let mut bad = false;
                for item in items {
                    match item.as_str() {
                        Some(text) => list.push(text.to_string()),
                        None => {
                            errors.push("capabilities 的每一项必须是字符串".to_string());
                            bad = true;
                            break;
                        }
                    }
                }
                if !bad {
                    match validate_capabilities(&list) {
                        Ok(()) => raw.capabilities = Some(list),
                        Err(error) => errors.push(error),
                    }
                }
            }
            None => errors.push("capabilities 必须是数组".to_string()),
        }
    }
    if let Some(value) = object.get("ui") {
        match serde_json::from_value::<ActionUi>(value.clone()) {
            Ok(ui) => match ui.validate() {
                Ok(()) => raw.ui = Some(ui),
                Err(error) => errors.push(error),
            },
            Err(error) => errors.push(format!("ui 解析失败：{error}")),
        }
    }
    if raw.schema != ACTION_SCHEMA && raw.schema != 0 {
        errors.push(format!("不支持的 action schema {}", raw.schema));
    }
    if let Err(error) = validate_name(&raw.name) {
        if !raw.name.is_empty() {
            errors.push(error);
        }
    }
    if !errors.is_empty() {
        raw.error = Some(errors.join("；"));
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_new_mod_manifest() {
        let raw = parse_mod(Some(
            r#"{"schema":2,"name":"测试","enabled":true}"#.as_bytes(),
        ));
        assert_eq!(raw.error, None);
        let meta = raw.validated().unwrap();
        assert_eq!(meta.name, "测试");
        assert_eq!(meta.version, DEFAULT_MOD_VERSION);
        assert!(meta.enabled);
        assert_eq!(meta.schema, MOD_SCHEMA);
        assert_eq!(meta.author, "");

        let raw = parse_mod(Some(
            r#"{"schema":2,"name":"测试","version":"2.1.0-beta","author":"作者 A","enabled":true}"#
                .as_bytes(),
        ));
        assert_eq!(raw.validated().unwrap().version, "2.1.0-beta");
        assert_eq!(raw.validated().unwrap().author, "作者 A");

        let bad = parse_mod(Some(
            r#"{"schema":2,"name":"测试","version":"../escape"}"#.as_bytes(),
        ));
        assert!(bad.error.unwrap().contains("版本"));
    }

    #[test]
    fn rejects_legacy_mod_manifest() {
        let raw = parse_mod(Some(
            r#"{"schema":1,"name":"旧 mod","timing":"after_sync","enabled":true}"#.as_bytes(),
        ));
        assert!(raw.error.as_ref().unwrap().contains("旧版"));
        assert!(raw.validated().is_err());
        assert_eq!(raw.name, "旧 mod");
    }

    #[test]
    fn parses_action_manifest_and_limits() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"动作","timing":"after_sync","enabled":false,"order":7}"#
                .as_bytes(),
        ));
        assert_eq!(raw.error, None);
        let meta = raw.validated().unwrap();
        assert_eq!(meta.timing, Timing::AfterSync);
        assert!(!meta.enabled);
        assert_eq!(meta.order, 7);

        let bad = parse_action(Some(
            br#"{"schema":1,"name":"x","timing":"sometimes","order":99999}"#,
        ));
        let error = bad.error.unwrap();
        assert!(error.contains("timing"));
        assert!(error.contains("order"));
    }

    #[test]
    fn parses_after_boot_delay_with_default_and_range() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"开机","timing":"after_boot","enabled":true,"order":7}"#
                .as_bytes(),
        ));
        assert_eq!(raw.error, None);
        assert_eq!(raw.boot_delay_minutes, 0);
        assert_eq!(raw.validated().unwrap().boot_delay_minutes, 0);

        let raw = parse_action(Some(
            r#"{"schema":1,"name":"开机","timing":"after_boot","boot_delay_minutes":15}"#
                .as_bytes(),
        ));
        assert_eq!(raw.error, None);
        assert_eq!(raw.validated().unwrap().boot_delay_minutes, 15);

        for bad in [
            r#"{"schema":1,"name":"x","timing":"after_boot","boot_delay_minutes":1441}"#,
            r#"{"schema":1,"name":"x","timing":"after_boot","boot_delay_minutes":-1}"#,
            r#"{"schema":1,"name":"x","timing":"after_boot","boot_delay_minutes":"5"}"#,
        ] {
            let raw = parse_action(Some(bad.as_bytes()));
            assert!(raw.error.is_some(), "{bad}");
        }
    }

    #[test]
    fn manual_actions_can_declare_a_cloud_sync_animation() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,
                 "ui":{"animation":"cloud_sync","title":"同步电池存档",
                       "units":["sav"],"show_cloud":true,"direction":"upload"}}"#
                .as_bytes(),
        ));
        assert_eq!(raw.error, None, "{:?}", raw.error);
        let meta = raw.validated().unwrap();
        let ui = meta.ui.clone().unwrap();
        assert_eq!(ui.title, "同步电池存档");
        assert_eq!(ui.units, vec!["sav"]);
        assert!(ui.show_cloud);
        assert_eq!(ui.direction.as_deref(), Some("upload"));
        let text = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        assert!(text.contains("cloud_sync"), "{text}");

        let bad = parse_action(Some(
            r#"{"schema":1,"name":"同步","timing":"manual",
                 "ui":{"animation":"transfer","units":["sav"]}}"#
                .as_bytes(),
        ));
        assert!(bad.error.is_some());
    }

    #[test]
    fn serializes_after_boot_delay() {
        let action = ActionMeta {
            schema: ACTION_SCHEMA,
            name: "开机".to_string(),
            timing: Timing::AfterBoot,
            enabled: true,
            order: 100,
            boot_delay_minutes: 15,
            ui: None,
            capabilities: None,
        };
        let bytes = action.to_bytes().unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("boot_delay_minutes"));
        assert_eq!(parse_action(Some(&bytes)).validated().unwrap(), action);
    }

    #[test]
    fn missing_manifests_are_errors() {
        assert!(parse_mod(None).error.unwrap().contains("mod.json"));
        assert!(parse_action(None).error.unwrap().contains("action.json"));
    }

    #[test]
    fn serialized_manifests_round_trip() {
        let mod_meta = ModMeta {
            schema: MOD_SCHEMA,
            name: "合金弹头".to_string(),
            version: "1.2.3".to_string(),
            author: "测试作者".to_string(),
            enabled: false,
            order: crate::DEFAULT_ORDER,
            category: String::new(),
        };
        let bytes = mod_meta.to_bytes().unwrap();
        assert_eq!(parse_mod(Some(&bytes)).validated().unwrap(), mod_meta);
        assert!(bytes.ends_with(b"\n"));
        assert!(
            !String::from_utf8_lossy(&bytes).contains("\"order\""),
            "the default priority should not be written into every manifest"
        );

        let mut moved = mod_meta.clone();
        moved.order = 10;
        let bytes = moved.to_bytes().unwrap();
        assert_eq!(parse_mod(Some(&bytes)).validated().unwrap(), moved);

        let action = ActionMeta {
            schema: ACTION_SCHEMA,
            name: "上传前".to_string(),
            timing: Timing::BeforeUpload,
            enabled: true,
            order: 100,
            boot_delay_minutes: 0,
            ui: None,
            capabilities: None,
        };
        let bytes = action.to_bytes().unwrap();
        assert_eq!(parse_action(Some(&bytes)).validated().unwrap(), action);
    }
}

    #[test]
    fn capabilities_round_trip_and_are_validated() {
        let mut action = ActionMeta {
            schema: ACTION_SCHEMA,
            name: "同步".to_string(),
            timing: Timing::Manual,
            enabled: true,
            order: 100,
            boot_delay_minutes: 0,
            ui: None,
            capabilities: Some(vec!["fs:saves".to_string(), "webdav:read".to_string()]),
        };
        action.validate().unwrap();
        let bytes = action.to_bytes().unwrap();
        assert_eq!(parse_action(Some(&bytes)).validated().unwrap(), action);

        action.capabilities = Some(vec!["fs:everything".to_string()]);
        assert!(action.validate().unwrap_err().contains("未知能力"));

        action.capabilities = Some(vec!["fs:saves".to_string(), "fs:saves".to_string()]);
        assert!(action.validate().unwrap_err().contains("重复"));

        action.capabilities = Some(vec!["fs:saves".to_string(); crate::MAX_CAPABILITIES + 1]);
        assert!(action.validate().unwrap_err().contains("最多"));
    }

    #[test]
    fn an_action_without_capabilities_is_legacy() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"旧动作","timing":"manual"}"#.as_bytes(),
        ));
        let meta = raw.validated().unwrap();
        assert!(meta.capabilities.is_none(), "缺省必须是 legacy（未声明）");
    }

    #[test]
    fn an_explicit_empty_list_is_not_legacy() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"新动作","timing":"manual","capabilities":[]}"#.as_bytes(),
        ));
        let meta = raw.validated().unwrap();
        assert_eq!(meta.capabilities, Some(Vec::new()), "显式空列表 = 声明不需要能力");
        let bytes = meta.to_bytes().unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"capabilities\": []"));
    }

    #[test]
    fn a_bad_capabilities_array_is_reported() {
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"坏动作","timing":"manual","capabilities":[1]}"#.as_bytes(),
        ));
        assert!(raw.error.is_some());
        let raw = parse_action(Some(
            r#"{"schema":1,"name":"坏动作","timing":"manual","capabilities":"fs:saves"}"#.as_bytes(),
        ));
        assert!(raw.error.is_some());
    }