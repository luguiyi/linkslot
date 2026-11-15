//! Local/cloud play-time merge (`playtime.json`).
//!
//! `playtime.json` sits in the local unit directory
//! (`States/<platform>/<emulator>/<game>/playtime.json`) and at the *game*
//! level in the cloud (`<platform>/<rom sha256>/playtime.json`, next to
//! `state/` and `sav/`). It is deliberately **not** part of `meta.json`: it
//! never enters the file whitelist, the SHA-256 diff plan, the deletion
//! propagation, the conflict backups or the version arbitration. The save
//! transfer always runs first; `push`/`pull`/`sync` then merge playtime in a
//! best-effort step. Any playtime problem is a warning and never changes the
//! save outcome.
//!
//! Local schema (Slot increments `seconds` while the game runs):
//! `{"schema":1,"seconds":120,"total_seconds":400,"devices":{"dev-a":280,"dev-b":120}}`
//!
//! The cloud copy stores only `{"schema":1,"devices":{...},"total_seconds":400}`;
//! a `seconds` field there is ignored when reading. Merging takes the cloud
//! device table as the base, keeps `max(cloud[device], local.seconds)` for the
//! current device and writes `total_seconds` as the sum over all devices.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{self, Config};
use crate::error::{fs_error, LinkError, Result};
use crate::output::Reporter;
use crate::store;
use crate::sync::Ctx;
use crate::unit::Unit;
use crate::webdav::DavClient;

pub const SCHEMA: u32 = 1;
pub const PLAYTIME: &str = "playtime.json";

/// Content of one `playtime.json`. Every field is defaulted so partial files
/// (for example one written only by Slot) can still be read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playtime {
    #[serde(default)]
    pub schema: u32,
    /// This device's own counter; meaningless on the cloud copy.
    #[serde(default)]
    pub seconds: u64,
    #[serde(default)]
    pub total_seconds: u64,
    /// Per-device counters keyed by `config::device_id`.
    #[serde(default)]
    pub devices: BTreeMap<String, u64>,
}

impl Playtime {
    /// Parse `playtime.json`; `None` for malformed JSON. The schema is checked
    /// by the caller ([`read_local`] is lenient, the cloud side is strict).
    pub fn parse(bytes: &[u8]) -> Option<Playtime> {
        serde_json::from_slice(bytes).ok()
    }

    /// Serialize the full local file (all four fields).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|err| LinkError::fs(format!("playtime.json 生成失败：{err}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The cloud copy only records the merged device table and its sum.
#[derive(Serialize)]
struct CloudPlaytime<'a> {
    schema: u32,
    devices: &'a BTreeMap<String, u64>,
    total_seconds: u64,
}

/// Serialize the cloud shape: exactly `schema`/`devices`/`total_seconds`.
pub fn cloud_bytes(playtime: &Playtime) -> Result<Vec<u8>> {
    let cloud = CloudPlaytime {
        schema: SCHEMA,
        devices: &playtime.devices,
        total_seconds: playtime.total_seconds,
    };
    let mut bytes = serde_json::to_vec_pretty(&cloud)
        .map_err(|err| LinkError::fs(format!("playtime.json 生成失败：{err}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// One merge of a local counter with the cloud device table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// Local file content to write back: `seconds` is this device's counter.
    pub local: Playtime,
    /// Cloud file content (`seconds` is always zero and is not serialized).
    pub cloud: Playtime,
    pub local_changed: bool,
    pub cloud_changed: bool,
}

/// Merge one device's local counter into the cloud device table.
///
/// - `devices` starts as a copy of the cloud table (the cloud is authoritative
///   for every other device);
/// - `devices[device_id] = max(cloud[device_id], local.seconds)`, so a zeroed
///   or reset local file never lowers the cloud value;
/// - `total_seconds` is the sum over all device counters;
/// - the local copy gets `seconds = devices[device_id]` so the next merge starts
///   from the merged value.
pub fn merge(local: &Playtime, cloud: &Playtime, device_id: &str) -> Merged {
    let mut devices = cloud.devices.clone();
    let seconds = local
        .seconds
        .max(cloud.devices.get(device_id).copied().unwrap_or(0));
    devices.insert(device_id.to_string(), seconds);
    let total_seconds = devices
        .values()
        .fold(0u64, |total, seconds| total.saturating_add(*seconds));
    let local_out = Playtime {
        schema: SCHEMA,
        seconds,
        total_seconds,
        devices: devices.clone(),
    };
    let cloud_out = Playtime {
        schema: SCHEMA,
        seconds: 0,
        total_seconds,
        devices,
    };
    let local_changed = local_out.seconds != local.seconds
        || local_out.total_seconds != local.total_seconds
        || local_out.devices != local.devices;
    let cloud_changed =
        cloud_out.total_seconds != cloud.total_seconds || cloud_out.devices != cloud.devices;
    Merged {
        local: local_out,
        cloud: cloud_out,
        local_changed,
        cloud_changed,
    }
}

/// Whether any side actually recorded play time. A card/cloud pair with no
/// counters at all is left untouched so a plain save sync never creates a
/// zeroed `playtime.json` (nor a cloud file) out of nowhere.
fn has_any_playtime(local: &Playtime, cloud: &Playtime) -> bool {
    local.seconds > 0
        || local.total_seconds > 0
        || !local.devices.is_empty()
        || cloud.total_seconds > 0
        || !cloud.devices.is_empty()
}

/// `<root>/States/<platform>/<emulator>/<game>/playtime.json`.
pub fn local_path(root: &Path, unit: &Unit) -> PathBuf {
    unit.local_dir(root).join(PLAYTIME)
}

/// Read the local playtime file. A missing, unreadable or malformed file is
/// treated as "no local counter" (`0`) so a bad file can never abort a sync.
pub fn read_local(root: &Path, unit: &Unit, rep: &mut Reporter) -> Playtime {
    let path = local_path(root, unit);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Playtime::default(),
        Err(err) => {
            rep.warn(format!(
                "无法读取本地 playtime.json（{}）：{err}",
                path.display()
            ));
            return Playtime::default();
        }
    };
    match Playtime::parse(&bytes) {
        // A file without a schema (0) is accepted as well: only the fields we
        // know about participate in the merge.
        Some(playtime) if playtime.schema <= SCHEMA => playtime,
        Some(_) => {
            rep.warn("本地 playtime.json 版本不支持，按 0 处理");
            Playtime::default()
        }
        None => {
            rep.warn("本地 playtime.json 已损坏，按 0 处理");
            Playtime::default()
        }
    }
}

/// Atomically write the local playtime file (creating the unit directory when
/// needed).
pub fn write_local(root: &Path, unit: &Unit, playtime: &Playtime) -> Result<()> {
    let path = local_path(root, unit);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| fs_error(format!("无法创建目录 {}", dir.display()), err))?;
    }
    store::atomic_write(&path, &playtime.to_bytes()?)
}

/// `<platform>/<rom sha256>/playtime.json` segments below the remote root.
///
/// A unit without a ROM SHA-256 has no cloud identity at all; it must never
/// degrade to `<remote_root>/playtime.json`, so this is a configuration error.
fn remote_rel(unit: &Unit) -> Result<Vec<String>> {
    let mut segments = unit.try_remote_segments()?;
    segments.push(PLAYTIME.to_string());
    Ok(segments)
}

/// Merge and synchronize `playtime.json` for a unit after a successful save
/// transfer. Best-effort: every failure is only a warning.
pub fn sync(ctx: &Ctx, config: &Config, unit: &Unit, rep: &mut Reporter) {
    if let Err(err) = try_sync(ctx, config, unit, rep) {
        rep.warn(format!("游戏时长同步失败（不影响存档结果）：{err}"));
    }
}

fn try_sync(ctx: &Ctx, config: &Config, unit: &Unit, rep: &mut Reporter) -> Result<()> {
    // Fail before any local read or network request: a hashless unit has no
    // cloud identity to merge against.
    let rel_owned = remote_rel(unit)?;
    let rel: Vec<&str> = rel_owned.iter().map(String::as_str).collect();
    let local = read_local(&ctx.root, unit, rep);

    let client = DavClient::new(config, rep)?;
    let cloud = match client.get(rep, &rel)? {
        None => Playtime::default(),
        Some(bytes) => match Playtime::parse(&bytes) {
            Some(playtime) if playtime.schema == SCHEMA => playtime,
            // Never overwrite a cloud table we cannot understand: it may hold
            // other devices' counters written by a newer linkSlot.
            Some(_) => {
                rep.warn("云端 playtime.json 版本不支持，跳过游戏时长同步（不覆盖云端数据）");
                return Ok(());
            }
            None => {
                rep.warn("云端 playtime.json 已损坏，跳过游戏时长同步（不覆盖云端数据）");
                return Ok(());
            }
        },
    };

    if !has_any_playtime(&local, &cloud) {
        return Ok(());
    }

    let device_id = config::device_id(&ctx.root, config);
    let merged = merge(&local, &cloud, &device_id);
    if merged.local_changed {
        write_local(&ctx.root, unit, &merged.local)?;
    }
    if merged.cloud_changed {
        client.put(rep, &rel, &cloud_bytes(&merged.cloud)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
        entries
            .iter()
            .map(|(device, seconds)| ((*device).to_string(), *seconds))
            .collect()
    }

    fn playtime(seconds: u64, devices: BTreeMap<String, u64>) -> Playtime {
        let total_seconds = devices
            .values()
            .fold(0u64, |total, seconds| total.saturating_add(*seconds));
        Playtime {
            schema: SCHEMA,
            seconds,
            total_seconds,
            devices,
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "linkslot-playtime-{tag}-{}",
            crate::util::unique_suffix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn unit() -> Unit {
        Unit::new("GBA", "mgba", "合金弹头").unwrap()
    }

    /// Run `read_local` with a throw-away reporter (warnings are just dropped).
    fn read_playtime(root: &Path, unit: &Unit) -> Playtime {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        read_local(
            root,
            unit,
            &mut Reporter::new(root, false, &mut out, &mut err),
        )
    }

    #[test]
    fn merge_sums_the_device_table_and_keeps_the_max_for_this_device() {
        let cloud = playtime(0, devices(&[("dev-a", 300), ("dev-b", 100)]));
        let local = playtime(150, devices(&[("dev-b", 100)]));
        let merged = merge(&local, &cloud, "dev-b");
        assert_eq!(merged.cloud.devices.get("dev-a"), Some(&300));
        assert_eq!(
            merged.cloud.devices.get("dev-b"),
            Some(&150),
            "local 150 > cloud 100"
        );
        assert_eq!(merged.cloud.total_seconds, 450);
        assert_eq!(merged.cloud.seconds, 0);
        assert_eq!(merged.local.seconds, 150);
        assert_eq!(merged.local.total_seconds, 450);
        assert_eq!(merged.local.devices, merged.cloud.devices);
        assert!(merged.local_changed);
        assert!(merged.cloud_changed);
    }

    #[test]
    fn local_zero_does_not_lower_the_cloud_counter() {
        let cloud = playtime(0, devices(&[("dev-a", 300)]));
        let merged = merge(&Playtime::default(), &cloud, "dev-a");
        assert_eq!(merged.cloud.devices.get("dev-a"), Some(&300));
        assert_eq!(merged.cloud.total_seconds, 300);
        assert_eq!(
            merged.local.seconds, 300,
            "the local counter is lifted to the cloud value"
        );
        assert!(!merged.cloud_changed);
        assert!(merged.local_changed);
    }

    #[test]
    fn merge_copies_the_cloud_table_and_drops_stale_local_devices() {
        let cloud = playtime(0, devices(&[("dev-a", 10)]));
        let local = playtime(5, devices(&[("stale-device", 999)]));
        let merged = merge(&local, &cloud, "dev-b");
        assert_eq!(merged.cloud.devices.get("dev-a"), Some(&10));
        assert_eq!(merged.cloud.devices.get("dev-b"), Some(&5));
        assert!(!merged.cloud.devices.contains_key("stale-device"));
        assert_eq!(merged.cloud.total_seconds, 15);
    }

    #[test]
    fn empty_units_have_no_playtime_to_sync() {
        assert!(!has_any_playtime(
            &Playtime::default(),
            &Playtime::default()
        ));
        let local = Playtime {
            schema: SCHEMA,
            seconds: 1,
            ..Playtime::default()
        };
        assert!(has_any_playtime(&local, &Playtime::default()));
        let cloud = playtime(0, devices(&[("dev-a", 1)]));
        assert!(has_any_playtime(&Playtime::default(), &cloud));
    }

    #[test]
    fn cloud_bytes_contain_only_the_cloud_fields() {
        let cloud = playtime(0, devices(&[("dev-a", 42)]));
        let bytes = cloud_bytes(&cloud).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 3, "cloud copy: {value}");
        assert_eq!(object.get("schema").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(
            object.get("total_seconds").and_then(|v| v.as_u64()),
            Some(42)
        );
        assert!(object
            .get("devices")
            .and_then(|v| v.as_object())
            .is_some_and(|devices| devices.contains_key("dev-a")));
        assert!(
            !object.contains_key("seconds"),
            "the local seconds counter must not be uploaded: {value}"
        );
    }

    #[test]
    fn parse_tolerates_partial_files_and_rejects_garbage() {
        let partial = Playtime::parse(br#"{"seconds":120}"#).unwrap();
        assert_eq!(partial.schema, 0);
        assert_eq!(partial.seconds, 120);
        assert_eq!(partial.total_seconds, 0);
        assert!(partial.devices.is_empty());
        assert!(Playtime::parse(b"not json").is_none());

        let value = playtime(120, devices(&[("dev-a", 120)]));
        let parsed = Playtime::parse(&value.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn local_file_roundtrips_and_tolerates_corruption() {
        let root = temp_root("local");
        let unit = unit();
        let path = local_path(&root, &unit);
        // Missing file -> zero.
        assert_eq!(read_playtime(&root, &unit), Playtime::default());
        // Written file round-trips through the local read path.
        let value = playtime(120, devices(&[("dev-a", 120)]));
        write_local(&root, &unit, &value).unwrap();
        assert!(path.is_file());
        assert_eq!(read_playtime(&root, &unit), value);
        // Malformed JSON is treated as zero.
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(read_playtime(&root, &unit), Playtime::default());
        // An unknown schema is treated as zero as well.
        let mut future = playtime(10, devices(&[("dev-a", 10)]));
        future.schema = SCHEMA + 1;
        std::fs::write(&path, future.to_bytes().unwrap()).unwrap();
        assert_eq!(read_playtime(&root, &unit), Playtime::default());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remote_segments_use_the_game_level_dir() {
        let hash = "a".repeat(64);
        let unit = unit().with_rom_sha256(&hash).unwrap();
        // v0.0.10: playtime.json is a game-level file, next to (not inside)
        // the state/sav unit directories.
        assert_eq!(remote_rel(&unit).unwrap(), ["GBA", hash.as_str(), PLAYTIME]);
    }

    #[test]
    fn remote_rel_rejects_a_unit_without_a_rom_hash() {
        let unit = unit();
        assert!(unit.rom_sha256.is_none());
        let err = remote_rel(&unit).unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Config);
        assert!(err.message().contains("SHA-256"), "{}", err.message());
    }
}
