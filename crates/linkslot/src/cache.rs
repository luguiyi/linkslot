//! Local sync cache: `<root>/linkSlot/Config/cache/<sha256(unit key)>.json`.
//!
//! The key follows [`UnitSpec::key`] and therefore carries every identity
//! dimension — `kind`, `platform`, the core scope (or `-` for the core-free
//! battery unit) and the ROM SHA-256:
//!
//! - `state/GBA/mgba/<sha256>`
//! - `sav/GBA/-/<sha256>`
//!
//! A save state and the battery save of the same game never share a cache
//! entry, and neither can be mistaken for the other. The cache remembers the
//! last cloud meta we synchronized with, which is what makes version+hash
//! arbitration possible.
//!
//! A missing/corrupt cache, a cache written for another unit kind or scope, or
//! one whose `remote` no longer matches the configured WebDAV target is
//! treated as "no cache" (push then refuses; pull backs up local files first).
//! Pre-v0.0.10 cache files used `platform/emulator/<sha256>` names and a
//! record without `kind`/`core`; they are never read and simply age out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{fs_error, LinkError, Result};
use crate::hash;
use crate::meta::{Meta, MetaFile, MetaUnit};
use crate::store;
use crate::unit::{Kind, Unit, UnitSpec};
use crate::util;

pub const CACHE_DIR_REL: &str = "linkSlot/Config/cache";

/// Which cloud target a cache entry belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRef {
    pub url: String,
    pub remote_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    /// Unit kind the entry was written for (`state` / `sav`).
    pub kind: Kind,
    /// Core scope of the entry; `None` for the core-free battery unit.
    pub core: Option<String>,
    pub unit: MetaUnit,
    pub version: u64,
    pub aggregate_hash: String,
    pub files: BTreeMap<String, MetaFile>,
    pub remote: RemoteRef,
    pub synced_at: String,
}

impl Cache {
    pub fn from_meta(meta: &Meta, unit: &Unit, remote: RemoteRef) -> Cache {
        Cache {
            kind: meta.kind,
            core: meta.core.clone(),
            unit: MetaUnit::from_unit(unit),
            version: meta.version,
            aggregate_hash: meta.aggregate_hash.clone(),
            files: meta.files.clone(),
            remote,
            synced_at: util::now_iso8601(),
        }
    }

    pub fn remote_matches(&self, remote: &RemoteRef) -> bool {
        self.remote == *remote
    }

    /// Whether this entry describes the same unit kind and scope as `spec`.
    pub fn scope_matches(&self, spec: &UnitSpec) -> bool {
        self.kind == spec.kind && self.core.as_deref() == spec.scope.core()
    }
}

/// Outcome of loading the cache; `note` explains why a present file was
/// ignored (logged by the caller).
#[derive(Clone, Debug, Default)]
pub struct CacheLoad {
    pub cache: Option<Cache>,
    pub note: Option<String>,
}

pub fn cache_dir(root: &Path) -> PathBuf {
    root.join(CACHE_DIR_REL)
}

/// `<sha256(unit key)>.json` keeps Chinese names, case and FAT illegal
/// characters out of the file name.
///
/// A unit without a ROM SHA-256 has no cloud identity and therefore no cache
/// entry (a configuration error).
pub fn cache_path(root: &Path, spec: &UnitSpec, unit: &Unit) -> Result<PathBuf> {
    let digest = hash::sha256_hex(spec.key(unit)?.as_bytes());
    Ok(cache_dir(root).join(format!("{digest}.json")))
}

pub fn load(root: &Path, spec: &UnitSpec, unit: &Unit, remote: &RemoteRef) -> Result<CacheLoad> {
    let path = cache_path(root, spec, unit)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(CacheLoad::default()),
        Err(err) => {
            return Err(fs_error(format!("无法读取缓存 {}", path.display()), err));
        }
    };
    let cache: Cache = match serde_json::from_slice(&bytes) {
        Ok(cache) => cache,
        Err(_) => {
            return Ok(CacheLoad {
                cache: None,
                note: Some(format!("缓存文件 {} 已损坏，按缺失处理", path.display())),
            });
        }
    };
    if !cache.scope_matches(spec) {
        return Ok(CacheLoad {
            cache: None,
            note: Some("缓存文件属于其它同步单元，按缺失处理".to_string()),
        });
    }
    if !cache.unit.matches(unit) {
        return Ok(CacheLoad {
            cache: None,
            note: Some("缓存文件属于其它同步单元，按缺失处理".to_string()),
        });
    }
    if !cache.remote_matches(remote) {
        return Ok(CacheLoad {
            cache: None,
            note: Some("配置的 WebDAV 地址已变化，本地缓存按缺失处理".to_string()),
        });
    }
    Ok(CacheLoad {
        cache: Some(cache),
        note: None,
    })
}

pub fn save(root: &Path, spec: &UnitSpec, unit: &Unit, cache: &Cache) -> Result<()> {
    let path = cache_path(root, spec, unit)?;
    let dir = cache_dir(root);
    std::fs::create_dir_all(&dir)
        .map_err(|err| fs_error(format!("无法创建缓存目录 {}", dir.display()), err))?;
    let bytes = serde_json::to_vec_pretty(cache)
        .map_err(|err| LinkError::fs(format!("缓存序列化失败：{err}")))?;
    store::atomic_write(&path, &bytes)
}

pub fn remove(root: &Path, spec: &UnitSpec, unit: &Unit) -> Result<()> {
    let path = cache_path(root, spec, unit)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(fs_error(format!("无法删除缓存 {}", path.display()), err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::meta::Meta;
    use crate::util::unique_suffix;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("linkslot-cache-{tag}-{}", unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn unit() -> Unit {
        Unit::new("GBA", "mgba", "合金弹头").unwrap()
    }

    fn hashed_unit() -> Unit {
        unit().with_rom_sha256(&"a".repeat(64)).unwrap()
    }

    fn state_spec(unit: &Unit, root: &Path) -> UnitSpec {
        UnitSpec::state(unit, root)
    }

    fn sav_spec(unit: &Unit, root: &Path) -> UnitSpec {
        UnitSpec::sav(unit, root)
    }

    fn state_meta(unit: &Unit) -> Meta {
        let hashed = vec![("resume.state".to_string(), 3, hash::sha256_hex(b"abc"))];
        Meta::build(unit, 3, "dev", &hashed, 0, None)
    }

    fn sav_meta(unit: &Unit) -> Meta {
        let battery = ("battery.sav".to_string(), 4, hash::sha256_hex(b"SRAM"));
        Meta::build_sav(unit, 7, "dev", &battery, None)
    }

    fn remote() -> RemoteRef {
        RemoteRef {
            url: "http://127.0.0.1:9/dav".to_string(),
            remote_root: "/linkSlot".to_string(),
        }
    }

    #[test]
    fn cache_path_is_hashed_and_chinese_free() {
        let root = Path::new("/mnt/sdcard");
        let unit = hashed_unit();
        let path = cache_path(root, &state_spec(&unit, root), &unit).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let stem = name.strip_suffix(".json").unwrap();
        assert_eq!(stem.len(), 64);
        assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn save_load_roundtrip_and_remote_mismatch() {
        let root = temp_root("roundtrip");
        let unit = hashed_unit();
        let spec = state_spec(&unit, &root);
        let meta = state_meta(&unit);
        let cache = Cache::from_meta(&meta, &unit, remote());
        save(&root, &spec, &unit, &cache).unwrap();
        let loaded = load(&root, &spec, &unit, &remote()).unwrap();
        assert!(loaded.note.is_none());
        let loaded = loaded.cache.unwrap();
        assert_eq!(loaded.version, 3);
        assert_eq!(loaded.kind, Kind::State);
        assert_eq!(loaded.core.as_deref(), Some("mgba"));
        assert_eq!(loaded.aggregate_hash, meta.aggregate_hash);
        assert!(!loaded.synced_at.is_empty());

        let mut other = remote();
        other.url = "http://other/dav".to_string();
        let loaded = load(&root, &spec, &unit, &other).unwrap();
        assert!(loaded.cache.is_none());
        assert!(loaded.note.unwrap().contains("WebDAV"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_cache_is_treated_as_missing() {
        let root = temp_root("corrupt");
        let unit = hashed_unit();
        let spec = state_spec(&unit, &root);
        let path = cache_path(&root, &spec, &unit).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        let loaded = load(&root, &spec, &unit, &remote()).unwrap();
        assert!(loaded.cache.is_none());
        assert!(loaded.note.unwrap().contains("损坏"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_cache_is_silent() {
        let root = temp_root("missing");
        let unit = hashed_unit();
        let loaded = load(&root, &state_spec(&unit, &root), &unit, &remote()).unwrap();
        assert!(loaded.cache.is_none());
        assert!(loaded.note.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_is_idempotent() {
        let root = temp_root("remove");
        let unit = hashed_unit();
        let spec = state_spec(&unit, &root);
        let meta = state_meta(&unit);
        let cache = Cache::from_meta(&meta, &unit, remote());
        save(&root, &spec, &unit, &cache).unwrap();
        remove(&root, &spec, &unit).unwrap();
        remove(&root, &spec, &unit).unwrap();
        assert!(load(&root, &spec, &unit, &remote())
            .unwrap()
            .cache
            .is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hashed_cache_key_ignores_the_game_name() {
        let root = Path::new("/mnt/sdcard");
        let unit = hashed_unit();
        let renamed = Unit::new("GBA", "mgba", "换个名字")
            .unwrap()
            .with_rom_sha256(&"A".repeat(64))
            .unwrap();
        // Same ROM hash -> same key -> same cache file, even after a rename.
        assert_eq!(
            cache_path(root, &state_spec(&unit, root), &unit).unwrap(),
            cache_path(root, &state_spec(&renamed, root), &renamed).unwrap()
        );
        // A different ROM is a different unit with a different cache file.
        let other_rom = unit.clone().with_rom_sha256(&"b".repeat(64)).unwrap();
        assert_ne!(
            cache_path(root, &state_spec(&unit, root), &unit).unwrap(),
            cache_path(root, &state_spec(&other_rom, root), &other_rom).unwrap()
        );
        // A hashless unit has no cloud identity, so it has no cache entry.
        let plain = super::tests::unit();
        assert_eq!(
            cache_path(root, &state_spec(&plain, root), &plain)
                .unwrap_err()
                .kind(),
            ErrorKind::Config
        );
    }

    // ---- v0.0.10: kind/scope isolation ---------------------------------

    #[test]
    fn state_and_sav_caches_never_collide() {
        let root = temp_root("isolation");
        let unit = hashed_unit();
        let state = state_spec(&unit, &root);
        let sav = sav_spec(&unit, &root);
        assert_ne!(
            cache_path(&root, &state, &unit).unwrap(),
            cache_path(&root, &sav, &unit).unwrap()
        );
        // Same platform + same ROM SHA-256, two independent entries.
        let state_cache = Cache::from_meta(&state_meta(&unit), &unit, remote());
        let sav_cache = Cache::from_meta(&sav_meta(&unit), &unit, remote());
        save(&root, &state, &unit, &state_cache).unwrap();
        save(&root, &sav, &unit, &sav_cache).unwrap();

        let loaded_state = load(&root, &state, &unit, &remote())
            .unwrap()
            .cache
            .unwrap();
        let loaded_sav = load(&root, &sav, &unit, &remote()).unwrap().cache.unwrap();
        assert_eq!(loaded_state.kind, Kind::State);
        assert_eq!(loaded_state.version, 3);
        assert_eq!(loaded_state.core.as_deref(), Some("mgba"));
        assert_eq!(loaded_sav.kind, Kind::Sav);
        assert_eq!(loaded_sav.version, 7);
        assert!(loaded_sav.core.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cache_of_another_kind_or_core_is_not_used() {
        let root = temp_root("kind-mismatch");
        let unit = hashed_unit();
        let sav = sav_spec(&unit, &root);
        // A state cache stored under the sav path must be rejected.
        let state_cache = Cache::from_meta(&state_meta(&unit), &unit, remote());
        let sav_path = cache_path(&root, &sav, &unit).unwrap();
        std::fs::create_dir_all(sav_path.parent().unwrap()).unwrap();
        std::fs::write(&sav_path, serde_json::to_vec_pretty(&state_cache).unwrap()).unwrap();
        let loaded = load(&root, &sav, &unit, &remote()).unwrap();
        assert!(loaded.cache.is_none());
        assert!(loaded.note.unwrap().contains("其它同步单元"));

        // A state cache recorded for another core is rejected too.
        let gpsp = Unit::new("GBA", "gpsp", "合金弹头")
            .unwrap()
            .with_rom_sha256(&"a".repeat(64))
            .unwrap();
        let gpsp_spec = state_spec(&gpsp, &root);
        let mut cache = Cache::from_meta(&state_meta(&gpsp), &gpsp, remote());
        cache.core = Some("mgba".to_string());
        let path = cache_path(&root, &gpsp_spec, &gpsp).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&cache).unwrap()).unwrap();
        let loaded = load(&root, &gpsp_spec, &gpsp, &remote()).unwrap();
        assert!(loaded.cache.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_cache_without_schema_2_identity_is_not_read() {
        // A v0.0.9 cache record: the unit has no `rom_sha256` (game-name
        // identity) and the entry has no `kind`/`core`. Schema 2 has no such
        // identity, so the entry must be ignored instead of being matched.
        let root = temp_root("legacy-json");
        let unit = hashed_unit();
        let spec = state_spec(&unit, &root);
        let text = format!(
            r#"{{
  "unit": {{"platform":"GBA","emulator":"mgba","game":"合金弹头"}},
  "version": 3,
  "aggregate_hash": "sha256:{}",
  "files": {{"resume.state": {{"sha256":"{}","size":3}}}},
  "remote": {{"url":"http://127.0.0.1:9/dav","remote_root":"/linkSlot"}},
  "synced_at": "2026-10-20T10:00:00Z"
}}"#,
            "0".repeat(64),
            hash::sha256_hex(b"abc")
        );
        let path = cache_path(&root, &spec, &unit).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        let loaded = load(&root, &spec, &unit, &remote()).unwrap();
        assert!(loaded.cache.is_none());
        assert!(
            loaded.note.unwrap().contains("损坏"),
            "an unreadable old cache must be treated as missing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_cache_file_name_is_never_looked_up() {
        // v0.0.9 keyed the cache by `GBA/mgba/<sha256>`; v0.0.10 keys it by
        // `state/GBA/mgba/<sha256>`, so an old file cannot even be found.
        let root = temp_root("legacy-path");
        let unit = hashed_unit();
        let old_name = format!(
            "{}.json",
            hash::sha256_hex(format!("GBA/mgba/{}", "a".repeat(64)).as_bytes())
        );
        let dir = cache_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let old_path = dir.join(&old_name);
        std::fs::write(&old_path, b"{}").unwrap();
        let loaded = load(&root, &state_spec(&unit, &root), &unit, &remote()).unwrap();
        assert!(loaded.cache.is_none(), "old cache must not be read");
        assert!(loaded.note.is_none(), "no file = silent miss");
        assert!(old_path.exists(), "old cache is not deleted either");
        let _ = std::fs::remove_dir_all(&root);
    }
}
