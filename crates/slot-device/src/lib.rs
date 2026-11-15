//! The actual BaseOS hardware identity.
//!
//! linkSlot used to call every machine `rg34xx-*`.  BaseOS already publishes
//! the exact `BASEOS_TARGET` and the human `BASEOS_MODEL` in
//! `/etc/baseos-release`; this small crate is shared by the Slot frontend and
//! the linkSlot CLI so both use the same model id.

use std::path::Path;
use std::sync::OnceLock;

const RELEASE_PATH: &str = "/etc/baseos-release";
const CONF_PATH: &str = "/run/baseos.conf";
const HOSTNAME_PATH: &str = "/etc/hostname";
const TARGET_ENV: &str = "LINKSLOT_DEVICE_TARGET";
const MODEL_ENV: &str = "LINKSLOT_DEVICE_MODEL";

static CURRENT: OnceLock<DeviceInfo> = OnceLock::new();

/// The exact hardware target and the name shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    target: String,
    model: String,
}

impl DeviceInfo {
    pub fn new(target: impl Into<String>, model: impl Into<String>) -> Self {
        let target = slug(&target.into());
        let model = model.into().trim().to_string();
        let model = if model.is_empty() {
            target.clone()
        } else {
            model
        };
        DeviceInfo { target, model }
    }

    /// Exact machine target, e.g. `rg34xxsp`.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Safe prefix for persistent IDs, e.g. `rg34xxsp`.
    pub fn slug(&self) -> String {
        slug(&self.target)
    }

    /// Human-readable name, e.g. `Anbernic RG34XX SP`.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Read the current machine, with environment overrides for tests and
    /// recovery on non-BaseOS hosts.  The answer is cached for the life of the
    /// process; hardware does not change at runtime.
    pub fn current() -> DeviceInfo {
        CURRENT.get_or_init(read_current).clone()
    }

    /// Parse the shell-style key/value file BaseOS installs.
    pub fn from_release_text(text: &str) -> DeviceInfo {
        let mut target = None;
        let mut model = None;
        let mut model_string = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote(value.trim());
            match key.trim() {
                "BASEOS_TARGET" => target = Some(value),
                "BASEOS_MODEL" => model = Some(value),
                "BASEOS_MODEL_STRING" => model_string = Some(value),
                _ => {}
            }
        }
        let fallback = target
            .clone()
            .or_else(|| model_string.clone())
            .or_else(|| model.clone())
            .unwrap_or_else(|| "device".into());
        let display = model
            .or(model_string)
            .or_else(|| target.clone())
            .unwrap_or_else(|| "未知设备".into());
        DeviceInfo::new(fallback, display)
    }
}

fn read_current() -> DeviceInfo {
    let env_target = std::env::var(TARGET_ENV).ok();
    let env_model = std::env::var(MODEL_ENV).ok();
    if env_target.is_some() || env_model.is_some() {
        let target = env_target.unwrap_or_else(|| "device".into());
        let model = env_model.unwrap_or_else(|| target.clone());
        return DeviceInfo::new(target, model);
    }
    if let Ok(text) = std::fs::read_to_string(RELEASE_PATH) {
        if !text.trim().is_empty() {
            return DeviceInfo::from_release_text(&text);
        }
    }
    if let Some(hostname) = read_hostname() {
        return DeviceInfo::new(hostname.clone(), hostname);
    }
    DeviceInfo::new("device", "未知设备")
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 32 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "device".into()
    } else {
        out
    }
}

fn read_hostname() -> Option<String> {
    for path in [CONF_PATH, HOSTNAME_PATH] {
        let Ok(text) = std::fs::read_to_string(Path::new(path)) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("hostname=") {
                let value = unquote(value.trim());
                if !value.is_empty() {
                    return Some(value);
                }
            } else if path == HOSTNAME_PATH && !line.is_empty() && !line.starts_with('#') {
                return Some(line.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseos_release_keeps_the_exact_target_and_human_model() {
        let info = DeviceInfo::from_release_text(
            "BASEOS_TARGET=rg34xxsp\nBASEOS_MODEL=\"Anbernic RG34XX SP\"\n\
             BASEOS_MODEL_STRING=RG34xxSP\n",
        );
        assert_eq!(info.target(), "rg34xxsp");
        assert_eq!(info.slug(), "rg34xxsp");
        assert_eq!(info.model(), "Anbernic RG34XX SP");
    }

    #[test]
    fn a_missing_model_falls_back_to_the_target() {
        let info = DeviceInfo::from_release_text("BASEOS_TARGET=rg35xxh\n");
        assert_eq!(info.slug(), "rg35xxh");
        assert_eq!(info.model(), "rg35xxh");
    }

    #[test]
    fn punctuation_is_safe_for_json_and_webdav_names() {
        let info = DeviceInfo::new("Anbernic RG34XX SP!", "Anbernic RG34XX SP");
        assert_eq!(info.slug(), "anbernic-rg34xx-sp");
        assert_eq!(info.model(), "Anbernic RG34XX SP");
    }

    #[test]
    fn empty_or_odd_values_still_yield_a_usable_identity() {
        let info = DeviceInfo::from_release_text("BASEOS_MODEL=\"\"\n");
        assert!(!info.slug().is_empty());
        assert!(!info.model().is_empty());
    }
}
