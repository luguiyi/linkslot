//! `meta.json` schema 2: parsing, building and validation.
//!
//! The cloud copy of `meta.json` is the authority for a unit. It is uploaded
//! last so a failed push never leaves the cloud pointing at partial data.
//! `updated_at` is informational only and never takes part in arbitration.
//!
//! Schema 2 describes both sync units of v0.0.10:
//!
//! - `state`: `kind: "state"`, `core`, the `<platform>/<sha>/state/<core>/`
//!   file ring and its `ring` summary;
//! - `sav`: `kind: "sav"`, no `core` and no `ring`, exactly one file
//!   (`battery.sav`).
//!
//! `kind` decides which file names are legal, so a state meta can never be
//! read as a battery meta (or the other way round). `unit` carries the game
//! identity: `platform` + the full-file SHA-256 of the ROM. `game` is
//! informational (the local directory name), so renaming a ROM keeps the same
//! cloud unit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorKind, LinkError, Result};
use crate::hash;
use crate::unit::{self, Kind, Unit, BATTERY, RESUME, RING_MAX};
use crate::util;

pub const SCHEMA: u32 = 2;
pub const META_NAME: &str = "meta.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaFile {
    pub sha256: String,
    pub size: u64,
}

/// Game identity recorded in `meta.json`.
///
/// The emulator is not here: save states are scoped by `core` (a top-level
/// field), and the battery save is shared by every core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaUnit {
    pub platform: String,
    /// Full-file SHA-256 of the ROM, lowercase bare hex.
    pub rom_sha256: String,
    /// Game name; informational only (the local directory name).
    pub game: String,
}

impl MetaUnit {
    pub fn from_unit(unit: &Unit) -> Self {
        MetaUnit {
            platform: unit.platform.clone(),
            rom_sha256: unit.rom_sha256.clone().unwrap_or_default(),
            game: unit.game.clone(),
        }
    }

    /// Whether this cloud/local unit record describes `unit`.
    ///
    /// platform and the ROM hash must match; the game name never does. A
    /// record without a ROM hash (written by a pre-v0.0.10 client) matches
    /// nothing: schema 2 has no game-name identity.
    pub fn matches(&self, unit: &Unit) -> bool {
        if self.platform != unit.platform {
            return false;
        }
        match &unit.rom_sha256 {
            Some(digest) => hash::digest_eq(digest, &self.rom_sha256),
            None => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RingInfo {
    pub count: usize,
    pub max: usize,
}

impl Default for RingInfo {
    fn default() -> Self {
        RingInfo {
            count: 0,
            max: RING_MAX,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub build: String,
    pub fork: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub schema: u32,
    pub kind: Kind,
    /// Save-state core (`state` only); absent for `sav`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<String>,
    pub unit: MetaUnit,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<u64>,
    pub device_id: String,
    pub updated_at: String,
    #[serde(default)]
    pub files: BTreeMap<String, MetaFile>,
    #[serde(default)]
    pub aggregate_hash: String,
    /// Ring summary (`state` only); absent for `sav`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<RingInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotInfo>,
}

impl Meta {
    /// Build a `state` meta. Kept under the old name so the push/pull phases
    /// (which only move save states in this milestone) need no change.
    ///
    /// `hashed` is `(name, size, sha256_hex)` from [`unit::hash_scan`].
    pub fn build(
        unit: &Unit,
        version: u64,
        device_id: &str,
        hashed: &[(String, u64, String)],
        ring_count: usize,
        previous: Option<&Meta>,
    ) -> Meta {
        Meta::build_kind(
            Kind::State,
            unit,
            version,
            device_id,
            hashed,
            ring_count,
            previous,
        )
    }

    /// Build a `sav` meta: exactly one `battery.sav`, no `core`, no `ring`.
    pub fn build_sav(
        unit: &Unit,
        version: u64,
        device_id: &str,
        battery: &(String, u64, String),
        previous: Option<&Meta>,
    ) -> Meta {
        Meta::build_kind(
            Kind::Sav,
            unit,
            version,
            device_id,
            std::slice::from_ref(battery),
            0,
            previous,
        )
    }

    fn build_kind(
        kind: Kind,
        unit: &Unit,
        version: u64,
        device_id: &str,
        hashed: &[(String, u64, String)],
        ring_count: usize,
        previous: Option<&Meta>,
    ) -> Meta {
        let mut files = BTreeMap::new();
        for (name, size, digest) in hashed {
            files.insert(
                name.clone(),
                MetaFile {
                    sha256: digest.clone(),
                    size: *size,
                },
            );
        }
        let aggregate_hash = hash::aggregate_hash(
            files
                .iter()
                .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
        );
        Meta {
            schema: SCHEMA,
            kind,
            core: match kind {
                Kind::State => Some(unit.emulator.clone()),
                Kind::Sav => None,
            },
            unit: MetaUnit::from_unit(unit),
            version,
            previous_version: previous.map(|meta| meta.version),
            device_id: device_id.to_string(),
            updated_at: util::now_iso8601(),
            files,
            aggregate_hash,
            ring: match kind {
                Kind::State => Some(RingInfo {
                    count: ring_count,
                    max: RING_MAX,
                }),
                Kind::Sav => None,
            },
            slot: slot_info(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Meta> {
        serde_json::from_slice::<Meta>(bytes).map_err(|err| {
            LinkError::new(ErrorKind::Server, format!("云端 meta.json 格式无效：{err}"))
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|err| {
            LinkError::new(ErrorKind::Config, format!("meta.json 生成失败：{err}"))
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Validate a save-state meta (the only kind the push/pull phases read in
    /// this milestone).
    pub fn validate(&self, unit: &Unit) -> Result<()> {
        self.validate_kind(unit, Kind::State, Some(&unit.emulator))
    }

    /// Validate a battery-save meta: no core, no ring, exactly `battery.sav`.
    pub fn validate_sav(&self, unit: &Unit) -> Result<()> {
        self.validate_kind(unit, Kind::Sav, None)
    }

    /// Validate a meta as `kind` scoped to `core`.
    ///
    /// The caller states what it expects, so a battery meta found where a
    /// state meta belongs (or vice versa) is always rejected instead of being
    /// silently accepted.
    pub fn validate_kind(&self, unit: &Unit, kind: Kind, core: Option<&str>) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(LinkError::new(
                ErrorKind::Server,
                format!(
                    "云端 meta.json schema 不支持（{}），本机支持 {SCHEMA}",
                    self.schema
                ),
            ));
        }
        if self.kind != kind {
            return Err(LinkError::new(
                ErrorKind::Server,
                format!(
                    "云端 meta.json 的单元类型不匹配（{}，期望 {}）",
                    self.kind, kind
                ),
            ));
        }
        if !self.unit.rom_sha256.is_empty() && !hash::is_valid_digest(&self.unit.rom_sha256) {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 的 rom_sha256 无效",
            ));
        }
        if !self.unit.matches(unit) {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 的同步单元与请求不一致",
            ));
        }
        if self.version == 0 {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 的 version 无效",
            ));
        }
        if !hash::is_valid_digest(&self.aggregate_hash) {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 的 aggregate_hash 无效",
            ));
        }
        self.validate_scope(kind, core)?;
        self.validate_ring(kind)?;
        self.validate_files(kind)?;
        let expected = hash::aggregate_hash(
            self.files
                .iter()
                .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
        );
        if !hash::digest_eq(&self.aggregate_hash, &expected) {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 的 aggregate_hash 与文件表不一致",
            ));
        }
        Ok(())
    }

    /// `core` belongs to `state`; `sav` is core-free by design.
    fn validate_scope(&self, kind: Kind, core: Option<&str>) -> Result<()> {
        match kind {
            Kind::State => {
                let expected = core.ok_or_else(|| {
                    LinkError::new(ErrorKind::Config, "save state 单元缺少 core 信息")
                })?;
                unit::validate_core(expected)?;
                match self.core.as_deref() {
                    Some(actual) if actual == expected => Ok(()),
                    Some(actual) => Err(LinkError::new(
                        ErrorKind::Server,
                        format!("云端 meta.json 的 core 不匹配（{actual}，期望 {expected}）"),
                    )),
                    None => Err(LinkError::new(
                        ErrorKind::Server,
                        "云端 meta.json 缺少 core 字段",
                    )),
                }
            }
            Kind::Sav => {
                if self.core.is_some() {
                    return Err(LinkError::new(
                        ErrorKind::Server,
                        "电池存档 meta.json 不应包含 core 字段",
                    ));
                }
                if core.is_some() {
                    return Err(LinkError::new(ErrorKind::Config, "电池存档单元不应带 core"));
                }
                Ok(())
            }
        }
    }

    fn validate_ring(&self, kind: Kind) -> Result<()> {
        match kind {
            Kind::State => {
                if self.ring.is_none() {
                    return Err(LinkError::new(
                        ErrorKind::Server,
                        "云端 meta.json 缺少 ring 字段",
                    ));
                }
                Ok(())
            }
            Kind::Sav => {
                if self.ring.is_some() {
                    return Err(LinkError::new(
                        ErrorKind::Server,
                        "电池存档 meta.json 不应包含 ring 字段",
                    ));
                }
                Ok(())
            }
        }
    }

    fn validate_files(&self, kind: Kind) -> Result<()> {
        if self.files.is_empty() {
            return Err(LinkError::new(
                ErrorKind::Server,
                "云端 meta.json 没有文件条目",
            ));
        }
        for (name, file) in &self.files {
            if !kind_whitelist(kind).allows(name) {
                return Err(LinkError::new(
                    ErrorKind::Server,
                    format!("云端 meta.json 包含非白名单文件名 {name:?}（{kind}）"),
                ));
            }
            if name.contains('/') || name.contains('\\') {
                return Err(LinkError::new(
                    ErrorKind::Server,
                    format!("云端 meta.json 文件名非法 {name:?}"),
                ));
            }
            if !hash::is_valid_digest(&file.sha256) {
                return Err(LinkError::new(
                    ErrorKind::Server,
                    format!("云端 meta.json 文件 {name} 的 sha256 无效"),
                ));
            }
        }
        match kind {
            Kind::State => {
                if !self.files.contains_key(RESUME) {
                    return Err(LinkError::new(
                        ErrorKind::Server,
                        "云端 meta.json 缺少 resume.state",
                    ));
                }
                // A thumbnail without its save state is not a legal ring entry.
                for name in self.files.keys() {
                    if let Some(stamp) = unit::png_stamp(name) {
                        let state = format!("{stamp}.state");
                        if !self.files.contains_key(&state) {
                            return Err(LinkError::new(
                                ErrorKind::Server,
                                format!("云端 meta.json 的缩略图 {name} 没有对应的 {state}"),
                            ));
                        }
                    }
                }
                Ok(())
            }
            Kind::Sav => {
                if self.files.len() != 1 || !self.files.contains_key(BATTERY) {
                    return Err(LinkError::new(
                        ErrorKind::Server,
                        format!("电池存档 meta.json 只能包含 {BATTERY}"),
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn is_state(&self) -> bool {
        self.kind == Kind::State
    }

    pub fn is_sav(&self) -> bool {
        self.kind == Kind::Sav
    }

    pub fn digest_of(&self, name: &str) -> Option<&str> {
        self.files.get(name).map(|file| file.sha256.as_str())
    }

    pub fn aggregate(&self) -> &str {
        &self.aggregate_hash
    }

    pub fn has_same_content(&self, other: &Meta) -> bool {
        hash::digest_eq(&self.aggregate_hash, &other.aggregate_hash)
    }
}

/// File-name whitelist of a unit kind.
pub fn kind_whitelist(kind: Kind) -> unit::Whitelist {
    match kind {
        Kind::State => unit::Whitelist::StateRing,
        Kind::Sav => unit::Whitelist::SingleBattery,
    }
}

/// Optional build/fork marker from the environment; not part of arbitration.
fn slot_info() -> Option<SlotInfo> {
    let build = std::env::var("LINKSLOT_SLOT_BUILD").ok()?;
    let fork = std::env::var("LINKSLOT_SLOT_FORK").unwrap_or_default();
    if build.is_empty() && fork.is_empty() {
        return None;
    }
    Some(SlotInfo { build, fork })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Unit {
        Unit::new("GBA", "mgba", "合金弹头").unwrap()
    }

    fn digest_a() -> String {
        "a".repeat(64)
    }

    fn hashed_unit() -> Unit {
        unit().with_rom_sha256(&digest_a()).unwrap()
    }

    fn sample_meta() -> Meta {
        let hashed = vec![
            ("resume.state".to_string(), 3, hash::sha256_hex(b"abc")),
            (
                "2026-10-20_10-00-00.state".to_string(),
                4,
                hash::sha256_hex(b"data"),
            ),
            (
                "2026-10-20_10-00-00.png".to_string(),
                5,
                hash::sha256_hex(b"thumb"),
            ),
        ];
        Meta::build(&hashed_unit(), 6, "rg34xx-test", &hashed, 1, None)
    }

    #[test]
    fn builds_the_schema_2_state_document() {
        let meta = sample_meta();
        assert_eq!(meta.schema, 2);
        assert_eq!(meta.kind, Kind::State);
        assert_eq!(meta.core.as_deref(), Some("mgba"));
        assert_eq!(meta.unit.platform, "GBA");
        assert_eq!(meta.unit.rom_sha256, digest_a());
        assert_eq!(meta.unit.game, "合金弹头");
        assert_eq!(meta.version, 6);
        assert!(meta.previous_version.is_none());
        assert_eq!(meta.ring.as_ref().unwrap().count, 1);
        assert_eq!(meta.ring.as_ref().unwrap().max, RING_MAX);
        assert_eq!(meta.files.len(), 3);
        assert!(meta.aggregate_hash.starts_with("sha256:"));
        assert!(meta.updated_at.ends_with('Z'));
        meta.validate(&hashed_unit()).unwrap();

        let text = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        assert!(text.contains("\"schema\": 2"), "{text}");
        assert!(text.contains("\"kind\": \"state\""), "{text}");
        assert!(text.contains("\"core\": \"mgba\""), "{text}");
        assert!(!text.contains("previous_aggregate_hash"));
    }

    #[test]
    fn roundtrips_through_json() {
        let meta = sample_meta();
        let bytes = meta.to_bytes().unwrap();
        let parsed = Meta::parse(&bytes).unwrap();
        assert_eq!(parsed, meta);
        parsed.validate(&hashed_unit()).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("合金弹头"));
    }

    #[test]
    fn previous_version_tracks_the_prior_meta() {
        let old = sample_meta();
        let mut hashed = Vec::new();
        for (name, file) in &old.files {
            hashed.push((name.clone(), file.size, file.sha256.clone()));
        }
        let new = Meta::build(&hashed_unit(), 7, "dev", &hashed, 1, Some(&old));
        assert_eq!(new.previous_version, Some(6));
        assert_eq!(new.aggregate_hash, old.aggregate_hash);
    }

    #[test]
    fn rejects_bad_schema_kind_and_unit() {
        let meta = sample_meta();

        let mut bad = meta.clone();
        bad.schema = 1;
        let err = bad.validate(&hashed_unit()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Server);

        let mut bad = meta.clone();
        bad.core = Some("gpsp".to_string());
        assert!(bad.validate(&hashed_unit()).is_err());

        let mut bad = meta.clone();
        bad.core = None;
        assert!(bad.validate(&hashed_unit()).is_err());

        let mut bad = meta.clone();
        bad.kind = Kind::Sav;
        let err = bad.validate(&hashed_unit()).unwrap_err();
        assert!(err.message().contains("单元类型不匹配"), "{err}");

        let mut bad = meta.clone();
        bad.unit.game = "另一个游戏".to_string();
        // The game name is informational: still the same unit.
        bad.validate(&hashed_unit()).unwrap();

        let mut bad = meta.clone();
        bad.unit.rom_sha256 = "b".repeat(64);
        assert!(bad.validate(&hashed_unit()).is_err());

        let mut bad = meta.clone();
        bad.aggregate_hash = "not-a-digest".to_string();
        assert!(bad.validate(&hashed_unit()).is_err());

        let mut bad = meta.clone();
        bad.version = 0;
        assert!(bad.validate(&hashed_unit()).is_err());
    }

    #[test]
    fn rejects_a_tampered_file_table() {
        let mut meta = sample_meta();
        meta.files.get_mut("resume.state").unwrap().sha256 = "f".repeat(64);
        let err = meta.validate(&hashed_unit()).unwrap_err();
        assert!(err.message().contains("不一致"), "{err}");
    }

    #[test]
    fn rejects_non_whitelisted_or_traversing_files() {
        let mut meta = sample_meta();
        meta.files.insert(
            "evil/../../resume.state".to_string(),
            MetaFile {
                sha256: "0".repeat(64),
                size: 1,
            },
        );
        assert!(meta.validate(&hashed_unit()).is_err());

        let mut meta = sample_meta();
        meta.files.insert(
            "resume-refused-2026-10-20_10-00-00.state".to_string(),
            MetaFile {
                sha256: "0".repeat(64),
                size: 1,
            },
        );
        assert!(meta.validate(&hashed_unit()).is_err());

        let mut meta = sample_meta();
        meta.files.insert(
            "battery.sav".to_string(),
            MetaFile {
                sha256: "0".repeat(64),
                size: 1,
            },
        );
        let err = meta.validate(&hashed_unit()).unwrap_err();
        assert!(err.message().contains("非白名单"), "{err}");

        let mut meta = sample_meta();
        meta.files.insert(
            "resume.state".to_string(),
            MetaFile {
                sha256: "zz".to_string(),
                size: 1,
            },
        );
        assert!(meta.validate(&hashed_unit()).is_err());
    }

    #[test]
    fn rejects_state_meta_without_resume_or_orphan_thumbnail() {
        let hashed = vec![(
            "2026-10-20_10-00-00.state".to_string(),
            4,
            hash::sha256_hex(b"data"),
        )];
        let meta = Meta::build(&hashed_unit(), 1, "dev", &hashed, 1, None);
        assert!(meta.validate(&hashed_unit()).is_err());

        let hashed = vec![
            ("resume.state".to_string(), 3, hash::sha256_hex(b"abc")),
            (
                "2026-10-20_10-00-00.png".to_string(),
                5,
                hash::sha256_hex(b"thumb"),
            ),
        ];
        let meta = Meta::build(&hashed_unit(), 1, "dev", &hashed, 0, None);
        let err = meta.validate(&hashed_unit()).unwrap_err();
        assert!(err.message().contains("缩略图"), "{err}");
    }

    #[test]
    fn parse_reports_server_error() {
        let err = Meta::parse(b"{not json").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Server);
    }

    #[test]
    fn serialization_is_sorted_and_pretty() {
        let meta = sample_meta();
        let text = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        let state_pos = text.find("2026-10-20_10-00-00.state").unwrap();
        let resume_pos = text.find("resume.state").unwrap();
        assert!(state_pos < resume_pos);
        assert!(text.contains("\n  \"schema\": 2,"));
        // Field order follows the documented schema.
        let schema = text.find("\"schema\"").unwrap();
        let kind = text.find("\"kind\"").unwrap();
        let core = text.find("\"core\"").unwrap();
        let unit = text.find("\"unit\"").unwrap();
        let version = text.find("\"version\"").unwrap();
        assert!(schema < kind && kind < core && core < unit && unit < version);
    }

    #[test]
    fn matches_uses_the_rom_hash_instead_of_the_game_name() {
        let meta = sample_meta();
        let unit = hashed_unit();
        // Same bytes under another name -> same unit.
        let renamed = Unit::new("GBA", "mgba", "另一个名字")
            .unwrap()
            .with_rom_sha256(&digest_a())
            .unwrap();
        assert!(meta.unit.matches(&renamed));
        meta.validate(&renamed).unwrap();
        // Different ROM bytes -> rejected.
        let other_rom = unit.clone().with_rom_sha256(&"b".repeat(64)).unwrap();
        assert!(!meta.unit.matches(&other_rom));
        assert!(meta.validate(&other_rom).is_err());
        // Different platform is always rejected.
        let other_platform = Unit::new("GB", "mgba", "合金弹头")
            .unwrap()
            .with_rom_sha256(&digest_a())
            .unwrap();
        assert!(!meta.unit.matches(&other_platform));
        // A unit without a ROM hash has no identity and never matches.
        let plain = Unit::new("GBA", "mgba", "合金弹头").unwrap();
        assert!(!meta.unit.matches(&plain));
    }

    #[test]
    fn rejects_invalid_rom_sha256_in_cloud_meta() {
        let mut meta = sample_meta();
        meta.unit.rom_sha256 = "not-a-digest".to_string();
        let err = meta.validate(&hashed_unit()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Server);

        let mut meta = sample_meta();
        meta.unit.rom_sha256 = format!("sha256:{}", "A".repeat(64));
        // A prefixed/upper-case digest is still a valid digest.
        meta.validate(&hashed_unit()).unwrap();
        assert!(meta.unit.matches(&hashed_unit()));
    }

    #[test]
    fn old_schema_1_meta_is_rejected_not_migrated() {
        // A v0.0.9 meta: schema 1, `emulator` instead of `core`, no `kind`.
        let legacy = format!(
            r#"{{
  "schema": 1,
  "unit": {{"platform":"GBA","emulator":"mgba","game":"合金弹头","rom_sha256":"{}"}},
  "version": 3,
  "device_id": "rg34xx-old",
  "updated_at": "2026-10-20T10:00:00Z",
  "files": {{"resume.state": {{"sha256":"{}","size":3}}}},
  "aggregate_hash": "{}",
  "ring": {{"count":0,"max":10}}
}}"#,
            digest_a(),
            hash::sha256_hex(b"abc"),
            hash::aggregate_hash([("resume.state", hash::sha256_hex(b"abc").as_str())])
        );
        let err = Meta::parse(legacy.as_bytes()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Server);
        assert!(err.message().contains("格式无效"), "{err}");
    }

    // ---- battery (sav) meta ---------------------------------------------

    fn sav_hashed(bytes: &[u8]) -> (String, u64, String) {
        (
            BATTERY.to_string(),
            bytes.len() as u64,
            hash::sha256_hex(bytes),
        )
    }

    fn sample_sav_meta() -> Meta {
        Meta::build_sav(
            &hashed_unit(),
            8,
            "rg34xx-test",
            &sav_hashed(b"SRAM"),
            Some(&sample_meta()),
        )
    }

    #[test]
    fn builds_the_schema_2_sav_document() {
        let meta = sample_sav_meta();
        assert_eq!(meta.schema, 2);
        assert_eq!(meta.kind, Kind::Sav);
        assert!(meta.is_sav());
        assert!(meta.core.is_none());
        assert!(meta.ring.is_none());
        assert_eq!(meta.unit.platform, "GBA");
        assert_eq!(meta.unit.rom_sha256, digest_a());
        assert_eq!(meta.version, 8);
        assert_eq!(meta.previous_version, Some(6));
        assert_eq!(meta.files.len(), 1);
        assert_eq!(meta.files.get(BATTERY).unwrap().size, 4);
        assert!(meta.aggregate_hash.starts_with("sha256:"));
        meta.validate_sav(&hashed_unit()).unwrap();

        let text = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        assert!(text.contains("\"kind\": \"sav\""), "{text}");
        assert!(!text.contains("\"core\""), "{text}");
        assert!(!text.contains("\"ring\""), "{text}");
        assert!(text.contains("\"battery.sav\""), "{text}");
        assert_eq!(Meta::parse(text.as_bytes()).unwrap(), meta);
    }

    #[test]
    fn sav_accepts_a_zero_byte_battery_file() {
        let meta = Meta::build_sav(&hashed_unit(), 1, "dev", &sav_hashed(b""), None);
        assert_eq!(meta.files.get(BATTERY).unwrap().size, 0);
        meta.validate_sav(&hashed_unit()).unwrap();
        let parsed = Meta::parse(&meta.to_bytes().unwrap()).unwrap();
        parsed.validate_sav(&hashed_unit()).unwrap();
    }

    #[test]
    fn state_and_sav_metas_never_validate_as_each_other() {
        let state = sample_meta();
        let sav = sample_sav_meta();
        let unit = hashed_unit();

        // Right kind, right validator.
        state.validate(&unit).unwrap();
        sav.validate_sav(&unit).unwrap();

        // Wrong validator -> rejected, not silently accepted.
        assert!(state.validate_sav(&unit).is_err());
        assert!(sav.validate(&unit).is_err());

        let err = sav.validate(&unit).unwrap_err();
        assert!(err.message().contains("单元类型不匹配"), "{err}");
        let err = state.validate_sav(&unit).unwrap_err();
        assert!(err.message().contains("单元类型不匹配"), "{err}");
    }

    #[test]
    fn sav_rejects_illegal_files_and_state_fields() {
        let unit = hashed_unit();

        // A state file name is not legal in a battery meta.
        let meta = Meta::build_sav(&hashed_unit(), 1, "dev", &sav_hashed(b"x"), None);
        let mut bad = meta.clone();
        bad.files.insert(
            RESUME.to_string(),
            MetaFile {
                sha256: hash::sha256_hex(b"resume"),
                size: 6,
            },
        );
        bad.aggregate_hash = hash::aggregate_hash(
            bad.files
                .iter()
                .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
        );
        let err = bad.validate_sav(&unit).unwrap_err();
        assert!(err.message().contains("非白名单"), "{err}");

        // Only one file is allowed.
        let mut bad = meta.clone();
        bad.files.insert(
            "battery.sav.bak".to_string(),
            MetaFile {
                sha256: hash::sha256_hex(b"bak"),
                size: 3,
            },
        );
        bad.aggregate_hash = hash::aggregate_hash(
            bad.files
                .iter()
                .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
        );
        assert!(bad.validate_sav(&unit).is_err());

        // No core, no ring: both are state-only fields.
        let mut bad = meta.clone();
        bad.core = Some("mgba".to_string());
        assert!(bad.validate_sav(&unit).is_err());

        let mut bad = meta.clone();
        bad.ring = Some(RingInfo::default());
        assert!(bad.validate_sav(&unit).is_err());

        // Asking for a core-scoped battery unit is a caller error.
        assert!(meta.validate_kind(&unit, Kind::Sav, Some("mgba")).is_err());

        // A meta without its battery file is invalid.
        let mut bad = meta;
        bad.files.clear();
        bad.aggregate_hash = hash::aggregate_hash(std::iter::empty());
        assert!(bad.validate_sav(&unit).is_err());
    }

    #[test]
    fn sav_meta_with_a_damaged_hash_is_rejected() {
        let mut meta = sample_sav_meta();
        meta.files.get_mut(BATTERY).unwrap().sha256 = "0".repeat(64);
        let err = meta.validate_sav(&hashed_unit()).unwrap_err();
        assert!(err.message().contains("不一致"), "{err}");
    }
}
