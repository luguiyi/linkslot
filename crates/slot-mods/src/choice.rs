//! Generic manual-mod choice requests shared by the CLI, the UI and Lua.
//!
//! A `manual` action may ask the user to pick one of 2–4 labelled options.
//! The first `linkSlot mods run` that reaches the question exits with a
//! `LINKSLOT_RESULT=choice` block; the device shows a modal and relaunches the
//! same action with `--choice <key>=<option>`.

use serde::{Deserialize, Serialize};

pub const MAX_CHOICE_OPTIONS: usize = 4;
pub const MIN_CHOICE_OPTIONS: usize = 2;
pub const MAX_CHOICE_KEY_CHARS: usize = 32;
pub const MAX_CHOICE_TITLE_CHARS: usize = 40;
pub const MAX_CHOICE_BODY_CHARS: usize = 200;
pub const MAX_CHOICE_LABEL_CHARS: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceKind {
    /// `mods.choose`: normal 2–4 option picker.
    #[default]
    Choice,
    /// `ui.confirm`: a yes/no confirmation. B is a real "no".
    Confirm,
    /// `ui.message`: a text popup with one close button.
    Message,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceRequest {
    pub key: String,
    #[serde(default)]
    pub kind: ChoiceKind,
    pub title: String,
    pub body: String,
    pub options: Vec<ChoiceOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceResponse {
    pub key: String,
    pub option: String,
}

impl ChoiceRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_key(&self.key, "key")?;
        validate_text(&self.title, "title", MAX_CHOICE_TITLE_CHARS)?;
        validate_text(&self.body, "body", MAX_CHOICE_BODY_CHARS)?;
        let (min, max) = match self.kind {
            ChoiceKind::Choice => (MIN_CHOICE_OPTIONS, MAX_CHOICE_OPTIONS),
            ChoiceKind::Confirm => (2, 2),
            ChoiceKind::Message => (1, 1),
        };
        if !(min..=max).contains(&self.options.len()) {
            return Err(format!("options 必须是 {min}–{max} 个"));
        }
        let mut seen = Vec::new();
        for option in &self.options {
            validate_key(&option.id, "option.id")?;
            validate_text(&option.label, "option.label", MAX_CHOICE_LABEL_CHARS)?;
            if seen.contains(&option.id) {
                return Err(format!("option.id 重复：{}", option.id));
            }
            seen.push(option.id.clone());
        }
        if let Some(default) = &self.default {
            if !seen.iter().any(|id| id == default) {
                return Err(format!("default {default:?} 不在 options 中"));
            }
        }
        Ok(())
    }

    pub fn default_index(&self) -> usize {
        self.default
            .as_ref()
            .and_then(|default| self.options.iter().position(|option| &option.id == default))
            .unwrap_or(0)
    }

    pub fn option_label(&self, id: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|option| option.id == id)
            .map(|option| option.label.as_str())
    }
}

impl ChoiceResponse {
    pub fn validate(&self, request: &ChoiceRequest) -> Result<(), String> {
        if self.key != request.key {
            return Err(format!(
                "choice key 不匹配：期望 {}，收到 {}",
                request.key, self.key
            ));
        }
        if request.option_label(&self.option).is_none() {
            return Err(format!(
                "choice option {:?} 不在 {} 的选项中",
                self.option, request.key
            ));
        }
        Ok(())
    }
}

fn validate_key(value: &str, field: &str) -> Result<(), String> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(format!("{field} 不能为空"));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!("{field} 必须以小写字母或数字开头"));
    }
    if value.chars().count() > MAX_CHOICE_KEY_CHARS
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "{field} 只能使用小写字母、数字、_ 和 -（最长 {MAX_CHOICE_KEY_CHARS}）"
        ));
    }
    Ok(())
}

fn validate_text(value: &str, field: &str, max_chars: usize) -> Result<(), String> {
    let count = value.chars().count();
    if count == 0 {
        return Err(format!("{field} 不能为空"));
    }
    if count > max_chars {
        return Err(format!("{field} 最长 {max_chars} 字符"));
    }
    if value.chars().any(|ch| ch.is_control()) {
        return Err(format!("{field} 不能包含控制字符"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ChoiceRequest {
        ChoiceRequest {
            key: "conflict".to_string(),
            kind: ChoiceKind::Choice,
            title: "存档冲突".to_string(),
            body: "本地和远端都改过".to_string(),
            options: vec![
                ChoiceOption {
                    id: "remote".to_string(),
                    label: "用远端覆盖本地".to_string(),
                },
                ChoiceOption {
                    id: "local".to_string(),
                    label: "用本地覆盖远端".to_string(),
                },
            ],
            default: Some("remote".to_string()),
        }
    }

    #[test]
    fn valid_requests_round_trip() {
        let value = request();
        value.validate().unwrap();
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.contains('\n'));
        let decoded: ChoiceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(value.default_index(), 0);
        assert_eq!(value.option_label("local"), Some("用本地覆盖远端"));
    }

    #[test]
    fn rejects_bad_keys_texts_options_and_defaults() {
        let mut bad = request();
        bad.key = "Bad Key".to_string();
        assert!(bad.validate().is_err());

        let mut bad = request();
        bad.title.clear();
        assert!(bad.validate().is_err());

        let mut bad = request();
        bad.options.pop();
        assert!(bad.validate().is_err());

        let mut bad = request();
        bad.options.push(bad.options[0].clone());
        assert!(bad.validate().is_err());

        let mut bad = request();
        bad.default = Some("missing".to_string());
        assert!(bad.validate().is_err());

        let mut bad = request();
        bad.options[0].label = "line\nbreak".to_string();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn responses_require_the_same_key_and_a_known_option() {
        let value = request();
        let ok = ChoiceResponse {
            key: "conflict".to_string(),
            option: "local".to_string(),
        };
        ok.validate(&value).unwrap();
        assert!(ChoiceResponse {
            key: "other".to_string(),
            option: "local".to_string(),
        }
        .validate(&value)
        .is_err());
        assert!(ChoiceResponse {
            key: "conflict".to_string(),
            option: "nope".to_string(),
        }
        .validate(&value)
        .is_err());
    }
}
