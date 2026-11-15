//! `push`, `pull`, `sync` and `status`: the version+hash arbitration and the
//! durable local replacement protocol described in project book v0.7 §3–§5.
//!
//! Push order: scan+hash -> GET meta -> detection matrix -> file-level plan
//! (`plan_upload`) -> PUT only the changed data files (tmp + MOVE when
//! supported) -> PUT `meta.json` last -> best-effort delete of cloud files that
//! are no longer in the new meta -> cache update. A forced overwrite
//! (`push --force` / `sync --resolve upload`) first backs the cloud copy up to
//! `<unit>/.conflict.<stamp>/`; `--full` re-sends every whitelist file even
//! when the detection matrix says "up to date" (repair path for a cloud copy
//! changed outside linkSlot).
//!
//! Pull order: GET meta -> file-level plan (`plan_download`) -> download only
//! the changed files to `.linkslot-tmp-*` -> verify SHA-256 -> conflict copy
//! (when needed / always for a forced overwrite) -> move the whole local
//! whitelist into `.linkslot-backup-*` -> journal the new names -> move verified
//! files into place -> move unchanged files straight back from the backup ->
//! drop backup -> cache update. Any failure before the final rename leaves the
//! local whitelist byte-identical. `--full` re-fetches every cloud file and also
//! skips the "already up to date" short-circuit.
//!
//! Sync combines both: one network round of meta GET, the v0.7 §5.1 decision,
//! an early `LINKSLOT_DIRECTION=upload|download` line, and then either the
//! upload or download phase above reusing the same client. A conflict without
//! `--resolve` changes nothing locally or remotely.
//!
//! Playtime: after a successful (`ok`/`noop`) save transfer the public
//! `push`/`pull`/`sync` entries merge `playtime.json` with the cloud copy (see
//! the [`crate::playtime`] module). That step opens its own [`DavClient`] and is
//! best-effort — playtime warnings never change the save outcome. Because
//! `playtime.json` is not part of the whitelist/meta it never takes part in
//! diffing, deletion propagation, conflicts or backups; `conflict`/`error`
//! outcomes skip the playtime step entirely.
//!
//! `<unit>` below the remote root is `<platform>/<rom sha256>/<kind>[/<core>]`
//! (v0.0.10 layout), i.e. [`UnitSpec::remote_segments`]. The remote root
//! carries a `layout.json` marker that every command validates before reading
//! unit data; the first upload creates it. Pre-v0.0.10 cloud directories
//! (`<platform>/<core>/<game>`) are never listed, read, moved or deleted, and
//! there is no automatic migration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cache::{self, Cache, RemoteRef};
use crate::config::{self, Config};
use crate::error::{fs_error, ErrorKind, LinkError, Result};
use crate::hash;
use crate::meta::{self, Meta};
use crate::mods::{self, HookContext, Timing, TransferInfo};
use crate::net::{self, NetMode};
use crate::output::{
    ConflictInfo, Direction, Outcome, Reporter, ResultKind, Stage, UnitOutcome, UnitSide,
};
use crate::playtime;
use crate::store::{self, TempDir, UnitLock};
use crate::unit::{self, Kind, Scan, Unit, UnitSpec};
use crate::util;
use crate::webdav::{DavClient, MoveOutcome};

/// Shared per-run context: where the card is mounted and whether the network
/// layer should be exercised (tests use [`NetMode::Skip`]).
#[derive(Clone, Debug)]
pub struct Ctx {
    pub root: PathBuf,
    pub net_mode: NetMode,
}

impl Ctx {
    pub fn new(root: impl Into<PathBuf>) -> Ctx {
        Ctx {
            root: root.into(),
            net_mode: NetMode::Real,
        }
    }

    pub fn with_net_mode(mut self, net_mode: NetMode) -> Ctx {
        self.net_mode = net_mode;
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PushOptions {
    pub force: bool,
    /// `--full`: send every whitelist file, even ones the cloud already has.
    /// The repair path for a cloud copy that was changed outside linkSlot.
    pub full: bool,
    pub net_up: bool,
    pub timeout: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PullOptions {
    pub force: bool,
    /// `--full`: fetch every cloud file, even ones the local side already has.
    pub full: bool,
    pub net_up: bool,
    pub timeout: Option<u64>,
}

/// Per-unit direction chosen with `--resolve`.
///
/// A bare `--resolve upload|download` fills both fields; `--resolve
/// state=upload,sav=download` fills one or both. A unit left as `None` keeps
/// its conflict instead of guessing a direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolveSpec {
    pub state: Option<Direction>,
    pub sav: Option<Direction>,
}

impl ResolveSpec {
    /// Bare `--resolve upload|download`: every conflicting unit gets it.
    pub fn all(direction: Direction) -> ResolveSpec {
        ResolveSpec {
            state: Some(direction),
            sav: Some(direction),
        }
    }

    pub fn get(&self, kind: Kind) -> Option<Direction> {
        match kind {
            Kind::State => self.state,
            Kind::Sav => self.sav,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.sav.is_none()
    }
}

/// Expected cloud versions (`--expect state=12,sav=7`).
///
/// Checked before anything is transferred: if the cloud moved on in the
/// meantime the whole call aborts with `changed-since` instead of committing a
/// decision made against stale information.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExpectedVersions {
    pub state: Option<u64>,
    pub sav: Option<u64>,
}

impl ExpectedVersions {
    pub fn get(&self, kind: Kind) -> Option<u64> {
        match kind {
            Kind::State => self.state,
            Kind::Sav => self.sav,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.sav.is_none()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SyncOptions {
    /// `--only state|sav`: ignore the other unit completely (no meta read, no
    /// transfer, not counted as a conflict).
    pub only: Option<Kind>,
    pub resolve: ResolveSpec,
    pub expect: ExpectedVersions,
    /// `--full`: transfer the whole unit in whichever direction `sync` takes.
    pub full: bool,
    pub net_up: bool,
    pub timeout: Option<u64>,
}

fn remote_ref(config: &Config) -> RemoteRef {
    RemoteRef {
        url: config.webdav().url.clone(),
        remote_root: config.webdav().remote_root.clone(),
    }
}

fn local_aggregate(hashed: &[(String, u64, String)]) -> String {
    hash::aggregate_hash(
        hashed
            .iter()
            .map(|(name, _, digest)| (name.as_str(), digest.as_str())),
    )
}

/// `GBA/<sha256>` + `<kind>[/<core>]` as owned strings.
fn unit_segments(spec: &UnitSpec, unit: &Unit) -> Result<Vec<String>> {
    spec.remote_segments(unit)
}

fn with_name<'a>(unit_refs: &[&'a str], name: &'a str) -> Vec<&'a str> {
    let mut path = unit_refs.to_vec();
    path.push(name);
    path
}

fn as_refs(segments: &[String]) -> Vec<&str> {
    segments.iter().map(String::as_str).collect()
}

/// Per-unit paths shared by every phase of a command.
struct UnitEnv {
    spec: UnitSpec,
    unit_dir: PathBuf,
    parent: PathBuf,
    segments: Vec<String>,
    remote: RemoteRef,
}

impl UnitEnv {
    fn new(root: &Path, unit: &Unit, config: &Config) -> Result<UnitEnv> {
        let spec = UnitSpec::state(unit, root);
        let unit_dir = spec.local.path().to_path_buf();
        let parent = unit_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());
        Ok(UnitEnv {
            segments: unit_segments(&spec, unit)?,
            spec,
            unit_dir,
            parent,
            remote: remote_ref(config),
        })
    }

    fn unit_path(&self) -> Vec<&str> {
        self.segments.iter().map(String::as_str).collect()
    }
}

// ---------------------------------------------------------------------------
// Cloud layout marker
// ---------------------------------------------------------------------------

/// `layout.json` at the remote root. All DavClient paths are relative to the
/// configured remote root, so the marker is a single segment.
fn layout_segments() -> Vec<String> {
    vec![unit::LAYOUT_NAME.to_string()]
}

/// GET + parse + validate a layout marker at `segments`; `Ok(None)` when it
/// does not exist.
fn load_layout_at(
    client: &DavClient,
    rep: &mut Reporter,
    segments: &[String],
) -> Result<Option<unit::Layout>> {
    let rel = as_refs(segments);
    let Some(bytes) = client.get(rep, &rel)? else {
        return Ok(None);
    };
    let layout = unit::Layout::parse(&bytes)?;
    layout.validate()?;
    Ok(Some(layout))
}

/// GET + parse + validate `layout.json`; `Ok(None)` when it does not exist.
fn load_layout(client: &DavClient, rep: &mut Reporter) -> Result<Option<unit::Layout>> {
    load_layout_at(client, rep, &layout_segments())
}

/// A cloud that already holds unit data must carry a compatible marker. A
/// missing marker is an error, never an implicit migration: old
/// `<platform>/<core>/<game>` directories stay untouched.
fn require_layout(client: &DavClient, rep: &mut Reporter) -> Result<()> {
    if load_layout(client, rep)?.is_some() {
        return Ok(());
    }
    Err(LinkError::new(
        ErrorKind::Verify,
        format!(
            "云端缺少 {}，无法确认云端布局（v0.0.10 不自动迁移旧布局）",
            unit::LAYOUT_NAME
        ),
    ))
}

/// Write the layout marker before the first unit upload. Never rewrites an
/// existing marker (a mismatch is already an error in `load_layout`).
fn ensure_layout(client: &DavClient, rep: &mut Reporter) -> Result<()> {
    let segments = layout_segments();
    if load_layout_at(client, rep, &segments)?.is_some() {
        return Ok(());
    }
    // `ensure_dir(&[])` creates the configured remote root itself.
    client.ensure_dir(rep, &[])?;
    let rel = as_refs(&segments);
    let bytes = unit::Layout::current().to_bytes()?;
    put_atomic(client, rep, &rel, &bytes)?;
    rep.info("已在云端写入 layout.json（layout 1）");
    Ok(())
}

/// PUT a small committed file through a temporary name + MOVE so a crashed
/// upload cannot leave a half-written `layout.json` behind.
fn put_atomic(client: &DavClient, rep: &mut Reporter, rel: &[&str], bytes: &[u8]) -> Result<()> {
    let name = rel.last().copied().unwrap_or("file");
    let tmp_name = format!("{name}.linkslot-tmp.{}", util::unique_suffix());
    let mut tmp = rel[..rel.len().saturating_sub(1)].to_vec();
    tmp.push(tmp_name.as_str());
    client.put(rep, &tmp, bytes)?;
    match client.move_to(rep, &tmp, rel)? {
        MoveOutcome::Moved => Ok(()),
        MoveOutcome::Unsupported => {
            rep.warn("WebDAV 不支持 MOVE，回退为直接 PUT（无原子覆盖）");
            let _ = client.delete(rep, &tmp);
            client.put(rep, rel, bytes)
        }
    }
}

// ---------------------------------------------------------------------------
// Shared phases (one network round per command)
// ---------------------------------------------------------------------------

fn load_cache(
    root: &Path,
    spec: &UnitSpec,
    unit: &Unit,
    remote: &RemoteRef,
    rep: &mut Reporter,
) -> Result<Option<Cache>> {
    let cache_load = cache::load(root, spec, unit, remote)?;
    if let Some(note) = &cache_load.note {
        rep.warn(note);
    }
    Ok(cache_load.cache)
}

/// The cache is keyed by the hash-based unit identity, so two game-name
/// directories of the same ROM share one entry. When *this* local directory
/// does not exist yet (new card, or the ROM was renamed and the save directory
/// still has the old name), the inherited entry must not be read as "the local
/// side deleted all its files": drop it so `sync`/`pull` restore the cloud copy
/// into the new directory. Once the directory exists the shared cache is used
/// again.
fn cache_for_local_unit_dir(
    cache: Option<Cache>,
    unit_dir: &Path,
    rep: &mut Reporter,
) -> Option<Cache> {
    if cache.is_some() && !unit_dir.exists() {
        rep.info("本地单元目录不存在，忽略缓存按首次同步处理");
        return None;
    }
    cache
}

fn scan_local(env: &UnitEnv) -> Result<Scan> {
    // Only the state unit uses the directory whitelist; the battery unit is a
    // single file and gets its own scan when sav sync lands.
    let dir =
        env.spec.local.as_dir().ok_or_else(|| {
            LinkError::new(ErrorKind::Config, "该同步单元不是目录型，不能按目录扫描")
        })?;
    unit::scan_dir(dir)
}

/// GET `<unit>/meta.json`, parse it and validate that it describes `unit` as
/// the expected unit kind (a `sav` meta found where a `state` meta belongs is
/// rejected, and the other way round).
fn fetch_cloud_meta(
    client: &DavClient,
    rep: &mut Reporter,
    unit: &Unit,
    unit_path: &[&str],
    kind: Kind,
    core: Option<&str>,
) -> Result<Option<Meta>> {
    let meta_path = with_name(unit_path, meta::META_NAME);
    match client.get(rep, &meta_path)? {
        Some(bytes) => {
            let parsed = Meta::parse(&bytes)?;
            parsed.validate_kind(unit, kind, core)?;
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}

/// Optional cloud/local details for the conflict keys of the stdout contract.
fn build_conflict_info(
    cloud: Option<&Meta>,
    cache: Option<&Cache>,
    scan: &Scan,
    device_id: &str,
) -> ConflictInfo {
    let local_updated = util::newest_mtime_iso(scan.files.iter().map(|file| file.path.as_path()));
    ConflictInfo {
        cloud_version: cloud.map(|meta| meta.version),
        cloud_device: cloud.map(|meta| meta.device_id.clone()),
        cloud_updated: cloud.map(|meta| meta.updated_at.clone()),
        local_version: cache.map(|cache| cache.version),
        local_device: Some(device_id.to_string()),
        local_updated,
    }
}

/// Recover an interrupted pull before touching the unit. Returns how many
/// leftover backup directories were restored.
fn recover_interrupted_pull(env: &UnitEnv, rep: &mut Reporter) -> Result<usize> {
    let recovered = store::recover_residual_backups(&env.unit_dir)?;
    if recovered > 0 {
        rep.warn(format!(
            "检测到 {recovered} 个上次拉取残留的备份目录，已恢复本地文件"
        ));
    }
    let cleaned = store::cleanup_residual_temps(&env.unit_dir)?;
    if cleaned > 0 {
        rep.info(format!("已清理 {cleaned} 个残留临时目录"));
    }
    Ok(recovered)
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum PushDecision {
    Upload,
    Noop { detail: String, update_cache: bool },
    Conflict { kind: ErrorKind, detail: String },
}

/// `push` detection matrix (project book v0.7 §5.1, second table). `force`
/// turns every abort into a forced upload.
fn decide_push(
    cloud: Option<&Meta>,
    cache: Option<&Cache>,
    aggregate_local: &str,
    force: bool,
) -> PushDecision {
    let Some(cloud) = cloud else {
        return PushDecision::Upload;
    };
    let Some(cache) = cache else {
        if !force {
            return PushDecision::Conflict {
                kind: ErrorKind::NoCache,
                detail: "本地缓存缺失，无法判断云端版本；请先拉取云端存档".to_string(),
            };
        }
        return PushDecision::Upload;
    };
    let cloud_unchanged = hash::digest_eq(cloud.aggregate(), &cache.aggregate_hash);
    let local_matches_cloud = hash::digest_eq(aggregate_local, cloud.aggregate());
    if cloud_unchanged {
        if local_matches_cloud {
            return PushDecision::Noop {
                detail: "内容一致，无需推送".to_string(),
                update_cache: true,
            };
        }
        return PushDecision::Upload;
    }
    if cloud.version > cache.version {
        if local_matches_cloud {
            return PushDecision::Noop {
                detail: "内容一致，无需推送".to_string(),
                update_cache: true,
            };
        }
        if !force {
            return PushDecision::Conflict {
                kind: ErrorKind::RemoteNewer,
                detail: format!(
                    "云端已被其它设备更新（云端 v{}，本地缓存 v{}）",
                    cloud.version, cache.version
                ),
            };
        }
    } else if cloud.version == cache.version {
        if !force {
            return PushDecision::Conflict {
                kind: ErrorKind::Forked,
                detail: format!(
                    "云端与本地均已修改（版本同为 v{}），无法自动合并",
                    cache.version
                ),
            };
        }
    } else if !force {
        return PushDecision::Conflict {
            kind: ErrorKind::RemoteRollback,
            detail: format!(
                "云端版本异常回退（云端 v{} < 本地缓存 v{}）",
                cloud.version, cache.version
            ),
        };
    }
    PushDecision::Upload
}

/// Per-unit decision of the v0.0.10 matrix (state and battery share it).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Decision {
    Push,
    Pull,
    Noop { detail: String, update_cache: bool },
    Conflict { kind: ErrorKind, detail: String },
}

/// The dual-unit decision matrix.
///
/// `local_present` says whether the unit has *any* local content (a save-state
/// ring file or a `.sav`/`.srm`); `local_aggregate` is its aggregate hash.
/// `cloud` is the validated cloud meta of the same unit kind, `cache` the last
/// synchronized state of that same unit. The two units never see each other's
/// meta or cache, so a state conflict can never be decided from a battery
/// version (or the other way round).
///
/// | local | cloud | cache | result |
/// |---|---|---|---|
/// | none | none | any | noop |
/// | present | none | any | push |
/// | none | present | none | pull |
/// | none | present | some | conflict(no-cache) — deleted locally, never silently restored |
/// | present | present | none | conflict(no-cache) |
/// | unchanged | unchanged | some | noop |
/// | changed | changed | some | conflict(forked / remote-newer / remote-rollback) |
/// | changed | unchanged | some | push |
/// | unchanged | changed | some | pull |
fn decide_unit(
    local_present: bool,
    local_aggregate: &str,
    cloud: Option<&Meta>,
    cache: Option<&Cache>,
) -> Decision {
    let Some(cloud) = cloud else {
        if !local_present {
            return Decision::Noop {
                detail: "云端无此游戏存档".to_string(),
                update_cache: false,
            };
        }
        return Decision::Push;
    };
    // Content already identical on both sides: nothing to decide.
    if hash::digest_eq(local_aggregate, cloud.aggregate()) {
        return Decision::Noop {
            detail: "已是最新".to_string(),
            update_cache: true,
        };
    }
    let Some(cache) = cache else {
        if !local_present {
            // First sync on this card: the cloud copy is the only copy.
            return Decision::Pull;
        }
        return Decision::Conflict {
            kind: ErrorKind::NoCache,
            detail: "本地有内容但没有同步缓存，无法判断谁更新；请选择覆盖方向".to_string(),
        };
    };
    if !local_present {
        // The unit used to be synced here but its local file is gone. Pulling
        // would silently resurrect a save the user may have deleted on
        // purpose, so this is a conflict.
        return Decision::Conflict {
            kind: ErrorKind::NoCache,
            detail: "本地存档已不存在，但云端仍有备份；请选择覆盖方向".to_string(),
        };
    }
    let cloud_unchanged = hash::digest_eq(cloud.aggregate(), &cache.aggregate_hash);
    let local_unchanged = hash::digest_eq(local_aggregate, &cache.aggregate_hash);
    match (local_unchanged, cloud_unchanged) {
        (true, true) => Decision::Noop {
            detail: "已是最新".to_string(),
            update_cache: false,
        },
        (false, true) => Decision::Push,
        (true, false) => Decision::Pull,
        (false, false) => {
            if cloud.version > cache.version {
                Decision::Conflict {
                    kind: ErrorKind::RemoteNewer,
                    detail: format!(
                        "云端与本地均已修改（云端 v{} > 本地缓存 v{}）",
                        cloud.version, cache.version
                    ),
                }
            } else if cloud.version == cache.version {
                Decision::Conflict {
                    kind: ErrorKind::Forked,
                    detail: format!(
                        "云端与本地版本同为 v{} 但内容不同，无法自动合并",
                        cache.version
                    ),
                }
            } else {
                Decision::Conflict {
                    kind: ErrorKind::RemoteRollback,
                    detail: format!(
                        "云端版本异常回退（云端 v{} < 本地缓存 v{}）",
                        cloud.version, cache.version
                    ),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer plan (file-level diff)
// ---------------------------------------------------------------------------

/// One direction's file-level plan: only the files whose recorded SHA-256 (and
/// size) does not match the other side are transferred. The plan is built from
/// the local hashes the detection matrix already needs and the cloud
/// `meta.json` file table, so it costs no extra network round or local read.
#[derive(Debug, Default, PartialEq, Eq)]
struct TransferPlan {
    /// Files to PUT (upload) or GET (download), sorted by name.
    changed: Vec<String>,
    /// Cloud files absent locally: deleted after the new meta.json commits.
    /// Always empty for a download plan (local extras are dropped by the
    /// crash-safe replacement instead).
    remote_only: Vec<String>,
    /// Files that already match on both sides and are therefore skipped.
    skipped: usize,
}

impl TransferPlan {
    fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.remote_only.is_empty()
    }
}

/// Compare the local whitelist hashes with the cloud meta table. A missing
/// cloud meta means a first push: every local file is new.
fn plan_upload(hashed: &[(String, u64, String)], cloud: Option<&Meta>, full: bool) -> TransferPlan {
    let mut plan = TransferPlan::default();
    for (name, size, digest) in hashed {
        let same = cloud
            .and_then(|meta| meta.files.get(name))
            .is_some_and(|file| file.size == *size && hash::digest_eq(&file.sha256, digest));
        if full || !same {
            plan.changed.push(name.clone());
        } else {
            plan.skipped += 1;
        }
    }
    if let Some(cloud) = cloud {
        let local: HashSet<&str> = hashed.iter().map(|(name, _, _)| name.as_str()).collect();
        plan.remote_only = cloud
            .files
            .keys()
            .filter(|name| !local.contains(name.as_str()))
            .cloned()
            .collect();
    }
    plan
}

/// Compare the cloud meta table with the local whitelist hashes. A local file
/// that is missing or records another hash/size is fetched; the rest is left
/// untouched on the card.
fn plan_download(hashed: &[(String, u64, String)], cloud: &Meta, full: bool) -> TransferPlan {
    let local: std::collections::HashMap<&str, &(String, u64, String)> = hashed
        .iter()
        .map(|entry| (entry.0.as_str(), entry))
        .collect();
    let mut plan = TransferPlan::default();
    for (name, file) in &cloud.files {
        let same = local.get(name.as_str()).is_some_and(|(_, size, digest)| {
            *size == file.size && hash::digest_eq(digest, &file.sha256)
        });
        if full || !same {
            plan.changed.push(name.clone());
        } else {
            plan.skipped += 1;
        }
    }
    plan
}

// ---------------------------------------------------------------------------
// Transfer phases
// ---------------------------------------------------------------------------

/// Back up the cloud copy of a unit before an overwrite: MKCOL
/// `<unit>/.conflict.<stamp>/`, PUT the old `meta.json` into it and copy every
/// old data file the meta lists. Missing old files are warned about and
/// skipped. Returns the backup directory URL for `LINKSLOT_CONFLICT`.
fn backup_cloud_unit(
    client: &DavClient,
    rep: &mut Reporter,
    unit_path: &[&str],
    old_meta: &Meta,
) -> Result<String> {
    let dir_name = format!("{}{}", store::CONFLICT_MARKER, util::stamp_now());
    let dir_name = dir_name.as_str();
    let backup = with_name(unit_path, dir_name);
    client.mkcol(rep, &backup)?;

    let backup_meta = with_name(&backup, meta::META_NAME);
    client.put(rep, &backup_meta, &old_meta.to_bytes()?)?;
    rep.info(format!("已备份云端旧 meta.json（v{}）", old_meta.version));

    for name in old_meta.files.keys() {
        let source = with_name(unit_path, name.as_str());
        let Some(bytes) = client.get(rep, &source)? else {
            rep.warn(format!("云端旧文件 {name} 已缺失，备份时跳过"));
            continue;
        };
        let target = with_name(&backup, name.as_str());
        client.put(rep, &target, &bytes)?;
    }
    let url = client.url_for(&backup);
    rep.warn(format!("已备份云端旧版本到 {url}"));
    Ok(url)
}

/// Upload the local whitelist and commit the new meta last.
///
/// `backup_cloud` is set by `push --force` / `sync --resolve upload`: the old
/// cloud meta and every file it lists are copied into `.conflict.<stamp>/`
/// before the overwrite starts.
///
/// The upload is incremental: only files whose hash/size differs from the cloud
/// `meta.json` are PUT (all of them with `full`, the `--full` repair path). The
/// new meta still lists the whole local whitelist, so the cloud copy stays
/// complete and older clients can keep reading it.
#[allow(clippy::too_many_arguments)]
fn execute_upload(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    env: &UnitEnv,
    client: &DavClient,
    scan: &Scan,
    hashed: &[(String, u64, String)],
    device_id: &str,
    previous: Option<&Meta>,
    backup_cloud: bool,
    full: bool,
    action: &'static str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let unit_path = env.unit_path();
    let plan = plan_upload(hashed, previous, full);
    if plan.is_empty() && !backup_cloud {
        // The detection matrix normally catches equal content first; if it did
        // not, only realign the cache instead of bumping a version for an empty
        // transfer. A forced overwrite (`backup_cloud`) still runs the normal
        // backup + meta commit path below.
        if let Some(meta) = previous {
            cache::save(
                &ctx.root,
                &env.spec,
                unit,
                &Cache::from_meta(meta, unit, env.remote.clone()),
            )?;
        }
        rep.info("内容一致，无需传输");
        return Ok(Outcome::noop(action, "内容一致，无需传输"));
    }
    let new_version = previous.map(|meta| meta.version + 1).unwrap_or(1);
    let new_meta = Meta::build(
        unit,
        new_version,
        device_id,
        hashed,
        scan.ring_count,
        previous,
    );

    rep.stage(Stage::Upload);
    if full {
        rep.info("全量模式（--full）：重传整个单元");
    }
    if plan.skipped > 0 {
        rep.info(format!("跳过 {} 个未变化文件", plan.skipped));
    }

    let mut conflict_path: Option<PathBuf> = None;
    if backup_cloud {
        if let Some(old_meta) = previous {
            let url = backup_cloud_unit(client, rep, &unit_path, old_meta)?;
            conflict_path = Some(PathBuf::from(url));
        }
    }

    ensure_layout(client, rep)?;
    client.ensure_dir(rep, &unit_path)?;

    let token = util::unique_suffix();
    let mut move_supported = true;
    let changed: HashSet<&str> = plan.changed.iter().map(String::as_str).collect();
    for (name, _size, digest) in hashed {
        if !changed.contains(name.as_str()) {
            continue;
        }
        let path = env.unit_dir.join(name);
        let bytes = std::fs::read(&path)
            .map_err(|err| fs_error(format!("无法读取 {}", path.display()), err))?;
        let actual = hash::sha256_hex(&bytes);
        if !hash::digest_eq(&actual, digest) {
            return Err(LinkError::new(
                ErrorKind::Fs,
                format!("{name} 在扫描后发生了变化，请重试"),
            ));
        }
        if move_supported {
            let tmp = format!("{name}.linkslot-tmp.{token}");
            let tmp_rel = with_name(&unit_path, tmp.as_str());
            let file_rel = with_name(&unit_path, name);
            client.put(rep, &tmp_rel, &bytes)?;
            match client.move_to(rep, &tmp_rel, &file_rel)? {
                MoveOutcome::Moved => rep.info(format!("已上传 {name}")),
                MoveOutcome::Unsupported => {
                    move_supported = false;
                    rep.warn("WebDAV 不支持 MOVE，回退为直接 PUT（无原子覆盖）");
                    let _ = client.delete(rep, &tmp_rel);
                    client.put(rep, &file_rel, &bytes)?;
                    rep.info(format!("已上传 {name}（直接 PUT）"));
                }
            }
        } else {
            client.put(rep, &with_name(&unit_path, name), &bytes)?;
            rep.info(format!("已上传 {name}（直接 PUT）"));
        }
    }

    // Commit point: meta.json goes last so a failure leaves the old meta
    // pointing at the old, still-present files.
    rep.stage(Stage::Commit);
    let meta_bytes = new_meta.to_bytes()?;
    let meta_path = with_name(&unit_path, meta::META_NAME);
    if move_supported {
        let tmp_meta = format!("meta.json.linkslot-tmp.{token}");
        let tmp_rel = with_name(&unit_path, tmp_meta.as_str());
        client.put(rep, &tmp_rel, &meta_bytes)?;
        match client.move_to(rep, &tmp_rel, &meta_path)? {
            MoveOutcome::Moved => {}
            MoveOutcome::Unsupported => {
                rep.warn("WebDAV 不支持 MOVE，回退为直接 PUT meta.json（接受风险）");
                let _ = client.delete(rep, &tmp_rel);
                client.put(rep, &meta_path, &meta_bytes)?;
            }
        }
    } else {
        client.put(rep, &meta_path, &meta_bytes)?;
    }

    // Deletion propagation: only after the new meta is committed.
    if let Some(old_meta) = previous {
        if config.sync_cfg().keep_remote {
            rep.info("sync.keep_remote=true，跳过云端旧文件清理");
        } else {
            let keep: HashSet<&str> = new_meta.files.keys().map(String::as_str).collect();
            for old_name in old_meta.files.keys() {
                if keep.contains(old_name.as_str()) {
                    continue;
                }
                let rel = with_name(&unit_path, old_name.as_str());
                match client.delete(rep, &rel) {
                    Ok(_) => rep.info(format!("已清理云端旧文件 {old_name}")),
                    Err(err) => rep.warn(format!(
                        "清理云端旧文件 {old_name} 失败（v1 不自动 GC，将残留）：{}",
                        err.message()
                    )),
                }
            }
        }
    }

    let new_cache = Cache::from_meta(&new_meta, unit, env.remote.clone());
    cache::save(&ctx.root, &env.spec, unit, &new_cache)?;
    Ok(Outcome::ok(action, Some(new_version), "推送成功").with_conflict(conflict_path))
}

/// Download + verify the cloud unit, back up the local side when required,
/// then replace the local whitelist crash-safely.
///
/// The download is incremental: only files whose recorded hash/size differs
/// from the local whitelist are fetched (all of them with `full`, the `--full`
/// repair path). Unchanged local files are moved aside into the crash-safety
/// backup and straight back, so `store::restore_dir` can still recover from a
/// crash at any point of the commit.
///
/// `conflict_copy_always` is set by `pull --force` / `sync --resolve download`:
/// a local `<game>.conflict.<stamp>/` copy is made even when the local files
/// match the cache.
#[allow(clippy::too_many_arguments)]
fn execute_download(
    ctx: &Ctx,
    unit: &Unit,
    env: &UnitEnv,
    client: &DavClient,
    cloud_meta: &Meta,
    scan: &Scan,
    hashed: &[(String, u64, String)],
    local_dirty: bool,
    conflict_copy_always: bool,
    full: bool,
    action: &'static str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let unit_path = env.unit_path();
    let plan = plan_download(hashed, cloud_meta, full);

    // Download to a same-volume temp directory first.
    std::fs::create_dir_all(&env.unit_dir)
        .map_err(|err| fs_error(format!("无法创建目录 {}", env.unit_dir.display()), err))?;
    store::probe_writable(&env.unit_dir)?;
    let temp = TempDir::create(&env.unit_dir, store::TMP_PREFIX)?;
    rep.stage(Stage::Download);
    if full {
        rep.info("全量模式（--full）：重新下载整个单元");
    }
    if plan.skipped > 0 {
        rep.info(format!("跳过 {} 个未变化文件", plan.skipped));
    }
    let mut downloaded: Vec<(String, Vec<u8>)> = Vec::with_capacity(plan.changed.len());
    for name in &plan.changed {
        let file = cloud_meta
            .files
            .get(name)
            .expect("download plan names come from the cloud meta");
        let rel = with_name(&unit_path, name.as_str());
        let bytes = client.get(rep, &rel)?.ok_or_else(|| {
            LinkError::new(
                ErrorKind::Server,
                format!(
                    "云端 meta.json 列出的文件 {name} 不存在（{} 字节）",
                    file.size
                ),
            )
        })?;
        downloaded.push((name.clone(), bytes));
    }

    rep.stage(Stage::Verify);
    for (name, bytes) in &downloaded {
        let file = cloud_meta
            .files
            .get(name)
            .expect("download list comes from meta");
        hash::verify_bytes(bytes, file.size, &file.sha256).map_err(|err| {
            LinkError::new(ErrorKind::Verify, format!("{name}: {}", err.message()))
        })?;
        temp.write(name, bytes)?;
    }
    rep.info(format!("已校验 {} 个文件", downloaded.len()));

    // Preserve the local side before replacing it: always for a forced
    // overwrite, and otherwise only when unsynced local changes would be lost.
    let mut conflict_path: Option<PathBuf> = None;
    if conflict_copy_always || (local_dirty && !scan.is_empty()) {
        let names: Vec<String> = scan.files.iter().map(|file| file.name.clone()).collect();
        let path = store::conflict_copy(&env.unit_dir, &env.parent, &unit.game, &names)?;
        rep.warn(format!("本地旧存档已备份到 {}", path.display()));
        conflict_path = Some(path);
    }

    // Crash-safe replacement: move the whole replaceable whitelist into the
    // backup, journal the names this pull may install, put the verified
    // downloads in place, then move the files the cloud did not change straight
    // back. Anything left in the backup (the replaced files, ring files the
    // cloud dropped, orphan thumbnails) is dropped on commit. A crash at any
    // point replays through `store::restore_dir`, which uses the journal to
    // tell half-installed new files from originals that never reached the
    // backup.
    rep.stage(Stage::Commit);
    let mut backup = store::BackupDir::create(&env.unit_dir)?;
    for (name, _path) in unit::local_replaceable_files(&env.unit_dir)? {
        backup.move_in(&name)?;
    }
    // Every original is safe in the backup: record what this pull may install
    // before the first new file is moved in.
    backup.write_journal(&plan.changed)?;
    let changed: HashSet<&str> = plan.changed.iter().map(String::as_str).collect();
    for (name, _bytes) in &downloaded {
        store::move_file(&temp.path().join(name), &env.unit_dir.join(name))?;
    }
    for (name, _path) in unit::local_replaceable_files(backup.path())? {
        if cloud_meta.files.contains_key(&name) && !changed.contains(name.as_str()) {
            store::move_file(&backup.path().join(&name), &env.unit_dir.join(name))?;
        }
    }
    backup.commit()?;

    let new_cache = Cache::from_meta(cloud_meta, unit, env.remote.clone());
    cache::save(&ctx.root, &env.spec, unit, &new_cache)?;
    rep.info(format!("已更新缓存到 v{}", cloud_meta.version));
    Ok(Outcome::ok(action, Some(cloud_meta.version), "拉取成功").with_conflict(conflict_path))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Best-effort `playtime.json` merge after a successful save transfer.
///
/// Only `ok`/`noop` outcomes merge playtime: a conflict aborted before touching
/// anything and an error leaves both sides in an unknown state, so neither may
/// write the playtime file. Any playtime problem is a warning and the save
/// `Outcome` returned to the caller is never altered.
fn sync_playtime(outcome: &Outcome, ctx: &Ctx, config: &Config, unit: &Unit, rep: &mut Reporter) {
    if matches!(outcome.result(), ResultKind::Ok | ResultKind::Noop) {
        playtime::sync(ctx, config, unit, rep);
    }
}

/// Push one unit to WebDAV. Never returns `Err`: failures become
/// `Outcome::error/conflict` so the CLI always prints the stdout contract.
pub fn push(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PushOptions,
    rep: &mut Reporter,
) -> Outcome {
    let outcome = match run_push(ctx, config, unit, options, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("push", &err, None, None),
    };
    sync_playtime(&outcome, ctx, config, unit, rep);
    outcome
}

/// Pull one unit from WebDAV.
pub fn pull(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PullOptions,
    rep: &mut Reporter,
) -> Outcome {
    let outcome = match run_pull(ctx, config, unit, options, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("pull", &err, None, None),
    };
    sync_playtime(&outcome, ctx, config, unit, rep);
    outcome
}

/// Smart bidirectional sync: decide once, then reuse the push or pull phase.
pub fn sync(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: SyncOptions,
    rep: &mut Reporter,
) -> Outcome {
    let outcome = match run_sync_all(ctx, config, unit, options, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("sync", &err, None, None),
    };
    // Playtime only merges when every unit succeeded (or had nothing to do).
    sync_playtime(&outcome, ctx, config, unit, rep);
    outcome
}

/// Read-only status: local whitelist + cache + cloud meta (never writes).
pub fn status(ctx: &Ctx, config: &Config, unit: &Unit, rep: &mut Reporter) -> Outcome {
    match run_status(ctx, config, unit, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("status", &err, None, None),
    }
}

fn run_push(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PushOptions,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let env = UnitEnv::new(&ctx.root, unit, config)?;
    let unit_path = env.unit_path();

    rep.stage(Stage::Net);
    net::ensure_connected(
        &ctx.root,
        config,
        ctx.net_mode,
        options.net_up,
        options.timeout,
        rep,
    )?;

    let _lock = UnitLock::acquire(&ctx.root)?;
    let mod_set = mods::load_mod_set(&ctx.root, config, rep);
    let active_storage = config.active_storage_id();

    // A leftover backup directory means an interrupted pull: pushing the
    // half-replaced directory would upload inconsistent data.
    let recovered = store::recover_residual_backups(&env.unit_dir)?;
    if recovered > 0 {
        return Err(LinkError::new(
            ErrorKind::Fs,
            "检测到上次拉取残留的备份目录，已恢复本地文件；请重新拉取后再推送",
        ));
    }

    let mut scan = scan_local(&env)?;
    if !scan.has_resume {
        return Err(unit::missing_resume_error());
    }
    let mut hashed = unit::hash_scan(&scan)?;
    let aggregate_local = local_aggregate(&hashed);
    let device_id = config::device_id(&ctx.root, config);

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);
    let cloud_meta = fetch_cloud_meta(
        &client,
        rep,
        unit,
        &unit_path,
        Kind::State,
        Some(&unit.emulator),
    )?;
    if cloud_meta.is_some() {
        require_layout(&client, rep)?;
    }
    let cache = load_cache(&ctx.root, &env.spec, unit, &env.remote, rep)?;

    let decision = decide_push(
        cloud_meta.as_ref(),
        cache.as_ref(),
        &aggregate_local,
        options.force,
    );
    if options.full && matches!(&decision, PushDecision::Noop { .. }) {
        // `--full` is the repair path: re-send the whole unit even though the
        // recorded content matches (e.g. a cloud data file was removed or
        // damaged behind meta.json's back). A concurrent `--force` still gets
        // its cloud backup before the overwrite.
        rep.info("全量模式（--full）：忽略“已是最新”，重传整个单元");
        let before = run_transfer_hooks(
            ctx,
            config,
            unit,
            Kind::State,
            Timing::BeforeUpload,
            None,
            &mod_set,
            &active_storage,
            &device_id,
            rep,
        );
        if before.ran > 0 {
            scan = scan_local(&env)?;
            if !scan.has_resume {
                return Err(unit::missing_resume_error());
            }
            hashed = unit::hash_scan(&scan)?;
        }
        let outcome = execute_upload(
            ctx,
            config,
            unit,
            &env,
            &client,
            &scan,
            &hashed,
            &device_id,
            cloud_meta.as_ref(),
            options.force,
            true,
            "push",
            rep,
        )?;
        if matches!(outcome.result(), ResultKind::Ok | ResultKind::Noop) {
            run_transfer_hooks(
                ctx,
                config,
                unit,
                Kind::State,
                Timing::AfterUpload,
                Some(TransferInfo {
                    direction: Direction::Upload,
                    result: outcome.result(),
                    version: outcome.version(),
                }),
                &mod_set,
                &active_storage,
                &device_id,
                rep,
            );
        }
        return Ok(outcome);
    }
    match decision {
        PushDecision::Noop {
            detail,
            update_cache,
        } => {
            if update_cache {
                if let Some(meta) = &cloud_meta {
                    let new_cache = Cache::from_meta(meta, unit, env.remote.clone());
                    cache::save(&ctx.root, &env.spec, unit, &new_cache)?;
                }
            }
            rep.info("内容一致，无需传输");
            Ok(Outcome::noop("push", detail))
        }
        PushDecision::Conflict { kind, detail } => {
            let info = build_conflict_info(cloud_meta.as_ref(), cache.as_ref(), &scan, &device_id);
            // Keep the actionable hint (`请先拉取` / `--force`) in the detail.
            let detail = LinkError::new(kind, detail).detail();
            Ok(Outcome::conflict("push", kind, detail, None).with_conflict_info(info))
        }
        PushDecision::Upload => {
            let before = run_transfer_hooks(
                ctx,
                config,
                unit,
                Kind::State,
                Timing::BeforeUpload,
                None,
                &mod_set,
                &active_storage,
                &device_id,
                rep,
            );
            if before.ran > 0 {
                scan = scan_local(&env)?;
                if !scan.has_resume {
                    return Err(unit::missing_resume_error());
                }
                hashed = unit::hash_scan(&scan)?;
            }
            let outcome = execute_upload(
                ctx,
                config,
                unit,
                &env,
                &client,
                &scan,
                &hashed,
                &device_id,
                cloud_meta.as_ref(),
                options.force,
                options.full,
                "push",
                rep,
            )?;
            if matches!(outcome.result(), ResultKind::Ok | ResultKind::Noop) {
                run_transfer_hooks(
                    ctx,
                    config,
                    unit,
                    Kind::State,
                    Timing::AfterUpload,
                    Some(TransferInfo {
                        direction: Direction::Upload,
                        result: outcome.result(),
                        version: outcome.version(),
                    }),
                    &mod_set,
                    &active_storage,
                    &device_id,
                    rep,
                );
            }
            Ok(outcome)
        }
    }
}

fn run_pull(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PullOptions,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let env = UnitEnv::new(&ctx.root, unit, config)?;
    let unit_path = env.unit_path();

    rep.stage(Stage::Net);
    net::ensure_connected(
        &ctx.root,
        config,
        ctx.net_mode,
        options.net_up,
        options.timeout,
        rep,
    )?;

    let _lock = UnitLock::acquire(&ctx.root)?;
    let mod_set = mods::load_mod_set(&ctx.root, config, rep);
    let active_storage = config.active_storage_id();

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);
    let Some(cloud_meta) = fetch_cloud_meta(
        &client,
        rep,
        unit,
        &unit_path,
        Kind::State,
        Some(&unit.emulator),
    )?
    else {
        rep.info("云端没有该单元的 meta.json");
        return Ok(Outcome::noop("pull", "云端无此游戏存档"));
    };
    require_layout(&client, rep)?;
    let cache = load_cache(&ctx.root, &env.spec, unit, &env.remote, rep)?;

    // Interrupted pull recovery must happen before we inspect local files.
    recover_interrupted_pull(&env, rep)?;
    let cache = cache_for_local_unit_dir(cache, &env.unit_dir, rep);
    let device_id = config::device_id(&ctx.root, config);

    let mut scan = scan_local(&env)?;
    let mut hashed = unit::hash_scan(&scan)?;
    let aggregate_local = local_aggregate(&hashed);

    let cloud_updated = match &cache {
        Some(cache) => !hash::digest_eq(cloud_meta.aggregate(), &cache.aggregate_hash),
        None => true,
    };
    let mut local_dirty = match &cache {
        Some(cache) => !hash::digest_eq(&aggregate_local, &cache.aggregate_hash),
        None => !scan.is_empty(),
    };

    // `--full` is the repair path: skip the "already up to date" short-circuit
    // and fetch the whole cloud unit, so a locally damaged or missing file is
    // restored even when its recorded hash still matches the cache.
    if !options.force && !options.full {
        if let Some(_cache) = &cache {
            if !cloud_updated {
                if local_dirty {
                    rep.info("云端无更新，本地有未推送修改，不做覆盖");
                    return Ok(Outcome::noop("pull", "本地有未推送修改，请用 push 上传"));
                }
                rep.info("云端与本地一致");
                return Ok(Outcome::noop("pull", "已是最新"));
            }
        }
    }

    let before = run_transfer_hooks(
        ctx,
        config,
        unit,
        Kind::State,
        Timing::BeforeDownload,
        None,
        &mod_set,
        &active_storage,
        &device_id,
        rep,
    );
    if before.ran > 0 {
        recover_interrupted_pull(&env, rep)?;
        scan = scan_local(&env)?;
        hashed = unit::hash_scan(&scan)?;
        local_dirty = match &cache {
            Some(cache) => !hash::digest_eq(&local_aggregate(&hashed), &cache.aggregate_hash),
            None => !scan.is_empty(),
        };
    }

    let outcome = execute_download(
        ctx,
        unit,
        &env,
        &client,
        &cloud_meta,
        &scan,
        &hashed,
        local_dirty,
        options.force,
        options.full,
        "pull",
        rep,
    )?;
    if matches!(outcome.result(), ResultKind::Ok | ResultKind::Noop) {
        run_transfer_hooks(
            ctx,
            config,
            unit,
            Kind::State,
            Timing::AfterDownload,
            Some(TransferInfo {
                direction: Direction::Download,
                result: outcome.result(),
                version: outcome.version(),
            }),
            &mod_set,
            &active_storage,
            &device_id,
            rep,
        );
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Dual-unit sync (v0.0.10)
// ---------------------------------------------------------------------------

/// Short label for the success banner (`已同步：即时 v13 / 电池 v9`).
fn kind_short(kind: Kind) -> &'static str {
    match kind {
        Kind::State => "即时",
        Kind::Sav => "电池",
    }
}

/// Full human label for detail lines.
fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::State => "即时存档",
        Kind::Sav => "电池存档",
    }
}

/// Aggregate over zero files — the "no local content" hash. A validated meta
/// always lists at least one file, so it can never equal a cloud aggregate.
fn empty_aggregate() -> String {
    hash::aggregate_hash(Vec::<(&str, &str)>::new())
}

/// Cloud/local version info of one unit for the `LINKSLOT_STATE_*`/`SAV_*`
/// block. After a successful transfer both sides equal the synced version.
fn unit_sides(
    snap: &UnitSnapshot,
    result: ResultKind,
    direction: Option<Direction>,
    version: Option<u64>,
    device_id: &str,
) -> (Option<UnitSide>, Option<UnitSide>) {
    // A unit with nothing anywhere has no side info worth reporting: just its
    // `*_RESULT=noop` line.
    if snap.cloud.is_none() && !snap.present {
        return (None, None);
    }
    let local_files = Some(snap.local_file_count());
    let local_updated = snap.local_mtime();
    let local_version = snap.cache.as_ref().map(|cache| cache.version);

    let cloud = snap.cloud.as_ref().map(|meta| UnitSide {
        version: Some(meta.version),
        device: Some(meta.device_id.clone()),
        updated: Some(meta.updated_at.clone()),
        files: Some(meta.files.len()),
    });
    let local = UnitSide {
        version: local_version,
        device: Some(device_id.to_string()),
        updated: local_updated,
        files: local_files,
    };

    match (result, direction) {
        (ResultKind::Ok, Some(Direction::Upload)) => {
            let side = UnitSide {
                version,
                device: Some(device_id.to_string()),
                updated: Some(util::now_iso8601()),
                files: local.files,
            };
            (Some(side.clone()), Some(side))
        }
        (ResultKind::Ok, Some(Direction::Download)) => {
            let side = cloud.unwrap_or_else(|| UnitSide {
                version,
                device: Some(device_id.to_string()),
                updated: Some(util::now_iso8601()),
                files: local.files,
            });
            (Some(side.clone()), Some(side))
        }
        _ => (cloud, Some(local)),
    }
}

/// Result of one unit inside a multi-unit sync (never `partial` itself; the
/// aggregate decides that).
#[derive(Clone, Debug)]
struct UnitResult {
    kind: Kind,
    result: ResultKind,
    direction: Option<Direction>,
    version: Option<u64>,
    error: ErrorKind,
    detail: String,
    conflict: Option<PathBuf>,
    info: Option<ConflictInfo>,
    cloud: Option<UnitSide>,
    local: Option<UnitSide>,
}

impl UnitResult {
    fn from_outcome(kind: Kind, direction: Direction, outcome: Outcome) -> UnitResult {
        UnitResult {
            kind,
            result: outcome.result(),
            direction: Some(direction),
            version: outcome.version(),
            error: outcome.error_kind(),
            detail: outcome.detail().to_string(),
            conflict: outcome.conflict_path().map(Path::to_path_buf),
            info: outcome.conflict_info().cloned(),
            cloud: None,
            local: None,
        }
    }

    fn noop(kind: Kind, detail: String) -> UnitResult {
        UnitResult {
            kind,
            result: ResultKind::Noop,
            direction: None,
            version: None,
            error: ErrorKind::None,
            detail,
            conflict: None,
            info: None,
            cloud: None,
            local: None,
        }
    }

    fn error(kind: Kind, err: &LinkError) -> UnitResult {
        UnitResult {
            kind,
            result: ResultKind::Error,
            direction: None,
            version: None,
            error: err.kind(),
            detail: err.detail(),
            conflict: None,
            info: None,
            cloud: None,
            local: None,
        }
    }

    fn conflict(
        kind: Kind,
        error: ErrorKind,
        detail: String,
        info: Option<ConflictInfo>,
    ) -> UnitResult {
        UnitResult {
            kind,
            result: ResultKind::Conflict,
            direction: None,
            version: None,
            error,
            detail,
            conflict: None,
            info,
            cloud: None,
            local: None,
        }
    }
}

/// Everything the orchestrator knows about one unit before executing.
struct UnitSnapshot {
    kind: Kind,
    state: Option<UnitEnv>,
    sav: Option<SavEnv>,
    remote: RemoteRef,
    scan: Option<Scan>,
    hashed: Vec<(String, u64, String)>,
    sav_local: Option<unit::SavScan>,
    sav_digest: Option<String>,
    present: bool,
    aggregate: String,
    cloud: Option<Meta>,
    cache: Option<Cache>,
    decision: Decision,
}

impl UnitSnapshot {
    fn spec(&self) -> &UnitSpec {
        match self.kind {
            Kind::State => {
                &self
                    .state
                    .as_ref()
                    .expect("state snapshot carries its env")
                    .spec
            }
            Kind::Sav => {
                &self
                    .sav
                    .as_ref()
                    .expect("sav snapshot carries its env")
                    .spec
            }
        }
    }

    fn local_dirty(&self) -> bool {
        match &self.cache {
            Some(cache) => !hash::digest_eq(&self.aggregate, &cache.aggregate_hash),
            None => self.present,
        }
    }

    /// Re-read the local unit after `before_*` hooks.
    ///
    /// The sync direction is intentionally not re-decided: the hook is an
    /// input to the already chosen transfer, exactly like any other local
    /// change between the scan and the upload.
    fn refresh_local(&mut self, ctx: &Ctx, _config: &Config, unit: &Unit) -> Result<()> {
        match self.kind {
            Kind::State => {
                let env = self.state.as_ref().expect("state snapshot carries its env");
                let scan = scan_local(env)?;
                if !scan.has_resume {
                    return Err(unit::missing_resume_error());
                }
                let hashed = unit::hash_scan(&scan)?;
                self.present = !scan.is_empty();
                self.aggregate = local_aggregate(&hashed);
                self.scan = Some(scan);
                self.hashed = hashed;
            }
            Kind::Sav => {
                let sav = unit::scan_sav(&ctx.root, unit)?;
                match &sav {
                    Some(scan) => {
                        let digest = hash::sha256_file(&scan.path)?;
                        self.hashed = vec![(unit::BATTERY.to_string(), scan.size, digest.clone())];
                        self.aggregate = sav_aggregate(&digest);
                        self.sav_local = Some(scan.clone());
                        self.sav_digest = Some(digest);
                        self.present = true;
                    }
                    None => {
                        self.hashed = Vec::new();
                        self.aggregate = empty_aggregate();
                        self.sav_local = None;
                        self.sav_digest = None;
                        self.present = false;
                    }
                }
            }
        }
        Ok(())
    }

    fn local_file_count(&self) -> usize {
        match self.kind {
            Kind::State => self.scan.as_ref().map(|scan| scan.files.len()).unwrap_or(0),
            Kind::Sav => usize::from(self.present),
        }
    }

    fn local_mtime(&self) -> Option<String> {
        let paths: Vec<&Path> = match self.kind {
            Kind::State => self
                .scan
                .as_ref()
                .map(|scan| scan.files.iter().map(|file| file.path.as_path()).collect())
                .unwrap_or_default(),
            Kind::Sav => self
                .sav_local
                .as_ref()
                .map(|scan| vec![scan.path.as_path()])
                .unwrap_or_default(),
        };
        util::newest_mtime_iso(paths)
    }

    fn local_path(&self) -> Option<&Path> {
        match self.kind {
            Kind::State => self
                .scan
                .as_ref()
                .and_then(|scan| scan.files.first())
                .map(|file| file.path.as_path()),
            Kind::Sav => self.sav_local.as_ref().map(|scan| scan.path.as_path()),
        }
    }

    fn conflict_info(&self, device_id: &str) -> Option<ConflictInfo> {
        match self.kind {
            Kind::State => Some(build_conflict_info(
                self.cloud.as_ref(),
                self.cache.as_ref(),
                self.scan.as_ref().expect("state snapshot has a scan"),
                device_id,
            )),
            Kind::Sav => Some(build_sav_conflict_info(
                self.cloud.as_ref(),
                self.cache.as_ref(),
                self.local_path(),
                device_id,
            )),
        }
    }
}

/// Read one unit's local snapshot, cloud meta and cache, then decide.
///
/// Any error here (unreadable file, corrupt meta, wrong kind) aborts the whole
/// sync before a single byte is transferred.
#[allow(clippy::too_many_arguments)]
fn prepare_unit(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    kind: Kind,
    client: &DavClient,
    layout_done: &mut bool,
    recover: bool,
    rep: &mut Reporter,
) -> Result<UnitSnapshot> {
    let spec = UnitSpec::for_kind(kind, unit, &ctx.root);
    let segments = unit_segments(&spec, unit)?;
    let unit_path = as_refs(&segments);

    let (scan, hashed, sav_local, sav_digest, present, aggregate) = match kind {
        Kind::State => {
            let env = UnitEnv::new(&ctx.root, unit, config)?;
            if recover {
                recover_interrupted_pull(&env, rep)?;
            }
            let scan = scan_local(&env)?;
            let hashed = unit::hash_scan(&scan)?;
            let present = !scan.is_empty();
            let aggregate = local_aggregate(&hashed);
            (Some(scan), hashed, None, None, present, aggregate)
        }
        Kind::Sav => {
            let sav = unit::scan_sav(&ctx.root, unit)?;
            match &sav {
                Some(scan) => {
                    let digest = hash::sha256_file(&scan.path)?;
                    let hashed = vec![(unit::BATTERY.to_string(), scan.size, digest.clone())];
                    let aggregate = sav_aggregate(&digest);
                    (
                        None,
                        hashed,
                        Some(scan.clone()),
                        Some(digest),
                        true,
                        aggregate,
                    )
                }
                None => (None, Vec::new(), None, None, false, empty_aggregate()),
            }
        }
    };

    let cloud = fetch_cloud_meta(client, rep, unit, &unit_path, kind, spec.scope.core())?;
    if cloud.is_some() && !*layout_done {
        require_layout(client, rep)?;
        *layout_done = true;
    }
    let remote = remote_ref(config);
    let cache = load_cache(&ctx.root, &spec, unit, &remote, rep)?;
    // A cache recorded under another game name is a ROM rename: the new local
    // directory simply never had content, so it pulls instead of turning into a
    // "deleted local save" conflict. A same-name cache whose local file is gone
    // stays a conflict (matrix row 4: never silently restore a deletion).
    let cache = cache.filter(|cache| cache.unit.game == unit.game);
    let decision = decide_unit(present, &aggregate, cloud.as_ref(), cache.as_ref());

    let (state, sav) = match kind {
        Kind::State => (Some(UnitEnv::new(&ctx.root, unit, config)?), None),
        Kind::Sav => (None, Some(SavEnv::new(&ctx.root, unit, config)?)),
    };
    Ok(UnitSnapshot {
        kind,
        state,
        sav,
        remote,
        scan,
        hashed,
        sav_local,
        sav_digest,
        present,
        aggregate,
        cloud,
        cache,
        decision,
    })
}

/// Execute one unit in the given direction. `forced` marks a conflict the user
/// resolved: a forced upload backs the cloud copy up first, a forced download
/// copies the local save aside first.
#[allow(clippy::too_many_arguments)]
fn execute_unit(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    snap: &UnitSnapshot,
    direction: Direction,
    forced: bool,
    full: bool,
    client: &DavClient,
    device_id: &str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    match (snap.kind, direction) {
        (Kind::State, Direction::Upload) => {
            let env = snap.state.as_ref().expect("state env");
            execute_upload(
                ctx,
                config,
                unit,
                env,
                client,
                snap.scan.as_ref().expect("state scan"),
                &snap.hashed,
                device_id,
                snap.cloud.as_ref(),
                forced,
                full,
                "sync",
                rep,
            )
        }
        (Kind::State, Direction::Download) => {
            let env = snap.state.as_ref().expect("state env");
            let cloud = snap.cloud.as_ref().ok_or_else(|| {
                LinkError::new(
                    ErrorKind::Server,
                    "同步判定需要云端 meta.json，但云端不存在（内部错误）",
                )
            })?;
            execute_download(
                ctx,
                unit,
                env,
                client,
                cloud,
                snap.scan.as_ref().expect("state scan"),
                &snap.hashed,
                snap.local_dirty(),
                forced,
                full,
                "sync",
                rep,
            )
        }
        (Kind::Sav, Direction::Upload) => {
            let env = snap.sav.as_ref().expect("sav env");
            let local = snap
                .sav_local
                .as_ref()
                .map(|scan| scan.path.clone())
                .ok_or_else(|| LinkError::new(ErrorKind::Fs, unit::missing_sav_detail()))?;
            execute_sav_upload(
                ctx,
                unit,
                env,
                client,
                &local,
                &snap.hashed,
                device_id,
                snap.cloud.as_ref(),
                forced,
                full,
                "sync",
                rep,
            )
        }
        (Kind::Sav, Direction::Download) => {
            let env = snap.sav.as_ref().expect("sav env");
            let cloud = snap.cloud.as_ref().ok_or_else(|| {
                LinkError::new(
                    ErrorKind::Server,
                    "同步判定需要云端 meta.json，但云端不存在（内部错误）",
                )
            })?;
            execute_sav_download(
                ctx,
                unit,
                env,
                client,
                cloud,
                snap.sav_local.as_ref(),
                snap.sav_digest.as_deref(),
                snap.local_dirty(),
                forced,
                full,
                "sync",
                rep,
            )
        }
    }
}

/// Run the enabled Lua mods for one transfer phase.
#[allow(clippy::too_many_arguments)]
fn run_transfer_hooks(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    kind: Kind,
    timing: Timing,
    transfer: Option<TransferInfo>,
    mod_set: &mods::ModSet,
    active_storage: &str,
    device_id: &str,
    rep: &mut Reporter,
) -> mods::HookReport {
    let hook_ctx = HookContext {
        root: &ctx.root,
        config,
        unit: Some(unit),
        kind: Some(kind),
        timing,
        transfer,
        active_storage,
        device_id,
        choices: &[],
    };
    mods::run_hooks(mod_set, &hook_ctx, rep)
}

/// Run the generic whole-sync hook once after `sync` has finished all units.
fn run_sync_hooks(
    ctx: &Ctx,
    config: &Config,
    mod_set: &mods::ModSet,
    active_storage: &str,
    device_id: &str,
    rep: &mut Reporter,
) -> mods::HookReport {
    let hook_ctx = HookContext {
        root: &ctx.root,
        config,
        unit: None,
        kind: None,
        timing: Timing::AfterSync,
        transfer: None,
        active_storage,
        device_id,
        choices: &[],
    };
    mods::run_hooks(mod_set, &hook_ctx, rep)
}

/// Execute one unit around its `before_*`/`after_*` hooks.
///
/// A before hook can change local files, so the snapshot is re-scanned before
/// the transfer. Hook failures never change the sync outcome; the transfer's
/// own result is returned unchanged.
#[allow(clippy::too_many_arguments)]
fn execute_snapshot_with_hooks(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    snapshot: &mut UnitSnapshot,
    direction: Direction,
    forced: bool,
    full: bool,
    client: &DavClient,
    device_id: &str,
    mod_set: &mods::ModSet,
    active_storage: &str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let (before, after) = match direction {
        Direction::Upload => (Timing::BeforeUpload, Timing::AfterUpload),
        Direction::Download => (Timing::BeforeDownload, Timing::AfterDownload),
    };
    let report = run_transfer_hooks(
        ctx,
        config,
        unit,
        snapshot.kind,
        before,
        None,
        mod_set,
        active_storage,
        device_id,
        rep,
    );
    if report.ran > 0 {
        snapshot.refresh_local(ctx, config, unit)?;
    }
    let outcome = execute_unit(
        ctx, config, unit, snapshot, direction, forced, full, client, device_id, rep,
    )?;
    if matches!(outcome.result(), ResultKind::Ok | ResultKind::Noop) {
        run_transfer_hooks(
            ctx,
            config,
            unit,
            snapshot.kind,
            after,
            Some(TransferInfo {
                direction,
                result: outcome.result(),
                version: outcome.version(),
            }),
            mod_set,
            active_storage,
            device_id,
            rep,
        );
    }
    Ok(outcome)
}

/// Fold the per-unit results into one aggregate outcome.
fn aggregate_unit_results(units: Vec<UnitResult>) -> Outcome {
    // The backup / conflict-copy path of the single unit that made one, so
    // `LINKSLOT_CONFLICT` keeps pointing at it even after a resolved conflict.
    let unit_conflicts: Vec<PathBuf> = units
        .iter()
        .filter_map(|unit| unit.conflict.clone())
        .collect();
    let unit_outcomes: Vec<UnitOutcome> = units
        .iter()
        .map(|unit| UnitOutcome {
            kind: unit.kind,
            result: unit.result,
            direction: unit.direction,
            version: unit.version,
            error: unit.error,
            cloud: unit.cloud.clone(),
            local: unit.local.clone(),
        })
        .collect();

    let conflicts: Vec<&UnitResult> = units
        .iter()
        .filter(|unit| unit.result == ResultKind::Conflict)
        .collect();
    if !conflicts.is_empty() {
        let labels: Vec<&str> = conflicts.iter().map(|unit| kind_label(unit.kind)).collect();
        let detail = if labels.len() == 1 {
            format!("{}存在冲突，请选择覆盖方向", labels[0])
        } else {
            format!("{}均存在冲突，请选择覆盖方向", labels.join("与"))
        };
        let mut outcome = Outcome::conflict("sync", conflicts[0].error, detail, None);
        if conflicts.len() == 1 {
            if let Some(info) = &conflicts[0].info {
                outcome = outcome.with_conflict_info(info.clone());
            }
            if let Some(path) = &conflicts[0].conflict {
                outcome = outcome.with_conflict(Some(path.clone()));
            }
        }
        return outcome.with_units(unit_outcomes);
    }

    let errors: Vec<&UnitResult> = units
        .iter()
        .filter(|unit| unit.result == ResultKind::Error)
        .collect();
    if !errors.is_empty() {
        let has_success = units
            .iter()
            .any(|unit| matches!(unit.result, ResultKind::Ok | ResultKind::Noop));
        if has_success {
            let ok_labels: Vec<&str> = units
                .iter()
                .filter(|unit| matches!(unit.result, ResultKind::Ok | ResultKind::Noop))
                .map(|unit| kind_label(unit.kind))
                .collect();
            let failed = errors[0];
            let mut partial = Outcome::partial(
                "sync",
                failed.error,
                format!(
                    "{}已同步，{}失败：{}",
                    ok_labels.join("与"),
                    kind_label(failed.kind),
                    failed.detail
                ),
            );
            if unit_conflicts.len() == 1 {
                partial = partial.with_conflict(Some(unit_conflicts[0].clone()));
            }
            return partial.with_units(unit_outcomes);
        }
        let failed = errors[0];
        return Outcome::error(
            "sync",
            &LinkError::new(failed.error, failed.detail.clone()),
            None,
            None,
        )
        .with_units(unit_outcomes);
    }

    if units.iter().all(|unit| unit.result == ResultKind::Noop) {
        let detail = units[0].detail.clone();
        return Outcome::noop("sync", detail).with_units(unit_outcomes);
    }

    let versions: Vec<String> = units
        .iter()
        .filter_map(|unit| {
            unit.version
                .map(|version| format!("{} v{}", kind_short(unit.kind), version))
        })
        .collect();
    let detail = if versions.is_empty() {
        "同步完成".to_string()
    } else {
        format!("已同步：{}", versions.join(" / "))
    };
    let mut outcome = Outcome::ok("sync", None, detail);
    if unit_conflicts.len() == 1 {
        outcome = outcome.with_conflict(Some(unit_conflicts[0].clone()));
    }
    let directions: Vec<Direction> = units
        .iter()
        .filter(|unit| unit.result == ResultKind::Ok)
        .filter_map(|unit| unit.direction)
        .collect();
    if let Some(first) = directions.first() {
        if directions.iter().all(|direction| direction == first) {
            outcome = outcome.with_direction(*first);
        }
    }
    let versions: Vec<u64> = units.iter().filter_map(|unit| unit.version).collect();
    if versions.len() == 1 {
        outcome = outcome.with_version(versions[0]);
    }
    outcome.with_units(unit_outcomes)
}

fn run_sync_all(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: SyncOptions,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let enabled: Vec<Kind> = match options.only {
        Some(kind) => vec![kind],
        None => Kind::ALL.to_vec(),
    };

    rep.stage(Stage::Net);
    net::ensure_connected(
        &ctx.root,
        config,
        ctx.net_mode,
        options.net_up,
        options.timeout,
        rep,
    )?;

    let _lock = UnitLock::acquire(&ctx.root)?;
    let mod_set = mods::load_mod_set(&ctx.root, config, rep);
    let active_storage = config.active_storage_id();

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);

    let device_id = config::device_id(&ctx.root, config);
    let mut layout_done = false;
    let mut snapshots = Vec::new();
    for kind in &enabled {
        let snapshot = prepare_unit(
            ctx,
            config,
            unit,
            *kind,
            &client,
            &mut layout_done,
            true,
            rep,
        )?;
        snapshots.push(snapshot);
    }

    for snapshot in &snapshots {
        if let Some(expected) = options.expect.get(snapshot.kind) {
            let actual = snapshot.cloud.as_ref().map(|meta| meta.version);
            if actual != Some(expected) {
                let label = kind_label(snapshot.kind);
                let detail = match actual {
                    Some(version) => {
                        format!("{label}云端版本已变化（期望 v{expected}，实际 v{version}）")
                    }
                    None => format!("{label}云端还没有存档（期望 v{expected}）"),
                };
                return Err(LinkError::new(ErrorKind::ChangedSince, detail));
            }
        }
    }

    let mut results: Vec<UnitResult> = Vec::new();
    for snapshot in &mut snapshots {
        let kind = snapshot.kind;
        let decision = snapshot.decision.clone();
        match decision {
            Decision::Noop {
                detail,
                update_cache,
            } => {
                if update_cache {
                    if let Some(meta) = &snapshot.cloud {
                        cache::save(
                            &ctx.root,
                            snapshot.spec(),
                            unit,
                            &Cache::from_meta(meta, unit, snapshot.remote.clone()),
                        )?;
                    }
                }
                let (cloud, local) = unit_sides(snapshot, ResultKind::Noop, None, None, &device_id);
                let mut unit = UnitResult::noop(kind, detail.clone());
                unit.cloud = cloud;
                unit.local = local;
                results.push(unit);
            }
            Decision::Conflict {
                kind: error,
                detail,
            } => match options.resolve.get(kind) {
                Some(direction) => {
                    rep.direction(direction);
                    match execute_snapshot_with_hooks(
                        ctx,
                        config,
                        unit,
                        snapshot,
                        direction,
                        true,
                        options.full,
                        &client,
                        &device_id,
                        &mod_set,
                        &active_storage,
                        rep,
                    ) {
                        Ok(outcome) => {
                            let (cloud, local) = unit_sides(
                                snapshot,
                                outcome.result(),
                                Some(direction),
                                outcome.version(),
                                &device_id,
                            );
                            let mut unit = UnitResult::from_outcome(kind, direction, outcome);
                            unit.cloud = cloud;
                            unit.local = local;
                            results.push(unit);
                        }
                        Err(err) => {
                            let (cloud, local) = unit_sides(
                                snapshot,
                                ResultKind::Error,
                                Some(direction),
                                None,
                                &device_id,
                            );
                            let mut unit = UnitResult::error(kind, &err);
                            unit.cloud = cloud;
                            unit.local = local;
                            results.push(unit);
                        }
                    }
                }
                None => {
                    let (cloud, local) =
                        unit_sides(snapshot, ResultKind::Conflict, None, None, &device_id);
                    let mut unit = UnitResult::conflict(
                        kind,
                        error,
                        LinkError::new(error, detail.clone()).detail(),
                        snapshot.conflict_info(&device_id),
                    );
                    unit.cloud = cloud;
                    unit.local = local;
                    results.push(unit);
                }
            },
            Decision::Push => {
                rep.direction(Direction::Upload);
                match execute_snapshot_with_hooks(
                    ctx,
                    config,
                    unit,
                    snapshot,
                    Direction::Upload,
                    false,
                    options.full,
                    &client,
                    &device_id,
                    &mod_set,
                    &active_storage,
                    rep,
                ) {
                    Ok(outcome) => {
                        let (cloud, local) = unit_sides(
                            snapshot,
                            outcome.result(),
                            Some(Direction::Upload),
                            outcome.version(),
                            &device_id,
                        );
                        let mut unit = UnitResult::from_outcome(kind, Direction::Upload, outcome);
                        unit.cloud = cloud;
                        unit.local = local;
                        results.push(unit);
                    }
                    Err(err) => {
                        let (cloud, local) = unit_sides(
                            snapshot,
                            ResultKind::Error,
                            Some(Direction::Upload),
                            None,
                            &device_id,
                        );
                        let mut unit = UnitResult::error(kind, &err);
                        unit.cloud = cloud;
                        unit.local = local;
                        results.push(unit);
                    }
                }
            }
            Decision::Pull => {
                rep.direction(Direction::Download);
                match execute_snapshot_with_hooks(
                    ctx,
                    config,
                    unit,
                    snapshot,
                    Direction::Download,
                    false,
                    options.full,
                    &client,
                    &device_id,
                    &mod_set,
                    &active_storage,
                    rep,
                ) {
                    Ok(outcome) => {
                        let (cloud, local) = unit_sides(
                            snapshot,
                            outcome.result(),
                            Some(Direction::Download),
                            outcome.version(),
                            &device_id,
                        );
                        let mut unit = UnitResult::from_outcome(kind, Direction::Download, outcome);
                        unit.cloud = cloud;
                        unit.local = local;
                        results.push(unit);
                    }
                    Err(err) => {
                        let (cloud, local) = unit_sides(
                            snapshot,
                            ResultKind::Error,
                            Some(Direction::Download),
                            None,
                            &device_id,
                        );
                        let mut unit = UnitResult::error(kind, &err);
                        unit.cloud = cloud;
                        unit.local = local;
                        results.push(unit);
                    }
                }
            }
        }
    }
    let aggregate = aggregate_unit_results(results);
    run_sync_hooks(ctx, config, &mod_set, &active_storage, &device_id, rep);
    Ok(aggregate)
}

fn run_status(ctx: &Ctx, config: &Config, unit: &Unit, rep: &mut Reporter) -> Result<Outcome> {
    // Cloud side: probe only (status never brings Wi-Fi up, never locks).
    if ctx.net_mode == NetMode::Skip {
        rep.info("已跳过网络检查（测试/离线模式）");
    } else {
        let net_status = net::status(&ctx.root, config)?;
        if !net_status.connected {
            return Err(LinkError::new(
                ErrorKind::Net,
                format!("网络未连接（{}）", net_status.detail),
            ));
        }
    }

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);
    let device_id = config::device_id(&ctx.root, config);
    let mut layout_done = false;
    let mut units = Vec::new();
    let mut state_dirty = false;
    for kind in Kind::ALL {
        let snap = prepare_unit(
            ctx,
            config,
            unit,
            kind,
            &client,
            &mut layout_done,
            false,
            rep,
        )?;
        let (result, version) = if snap.cloud.is_none() && !snap.present {
            (ResultKind::Noop, None)
        } else {
            (ResultKind::Ok, snap.cloud.as_ref().map(|meta| meta.version))
        };
        if kind == Kind::State {
            state_dirty = snap.local_dirty();
        }
        let (cloud, local) = unit_sides(&snap, result, None, version, &device_id);
        units.push(UnitOutcome {
            kind,
            result,
            direction: None,
            version,
            error: ErrorKind::None,
            cloud,
            local,
        });
    }

    // Keep the single-unit wording for the state side (backwards compatible).
    let state = &units[0];
    let detail = if state.result == ResultKind::Noop {
        "云端无此游戏存档".to_string()
    } else {
        let local = match &state.local {
            Some(side) if side.version.is_some() => format!("v{}", side.version.unwrap()),
            _ => "无缓存".to_string(),
        };
        let cloud = state
            .cloud
            .as_ref()
            .and_then(|side| side.version)
            .map(|version| format!("v{version}"))
            .unwrap_or_else(|| "无".to_string());
        let mut detail = format!("本地 {local} / 云端 {cloud}");
        if state_dirty {
            detail.push_str("（本地有未推送修改）");
        }
        detail
    };
    let outcome = if units.iter().any(|unit| unit.result == ResultKind::Ok) {
        Outcome::ok("status", state.version, detail)
    } else {
        Outcome::noop("status", detail)
    };
    Ok(outcome.with_units(units))
}

// ---------------------------------------------------------------------------
// Battery save (.sav) transfer
// ---------------------------------------------------------------------------
//
// The battery unit is a single file in the shared `Saves/<platform>/`
// directory, so it must never use the directory-replacement protocol above:
// no journal, no unit-directory backup, no whole-directory moves. Downloads go
// to a private `linkSlot/Config/work/sav-<sha>/` file and are installed with
// one rename; the only other local write is an explicit conflict copy.

/// Paths of one battery unit.
struct SavEnv {
    spec: UnitSpec,
    /// `Saves/<platform>/<game>.sav` — the only local file a pull may replace.
    target: PathBuf,
    segments: Vec<String>,
    remote: RemoteRef,
}

impl SavEnv {
    fn new(root: &Path, unit: &Unit, config: &Config) -> Result<SavEnv> {
        let spec = UnitSpec::sav(unit, root);
        let target = spec
            .local
            .as_file()
            .expect("the battery unit always targets a file")
            .to_path_buf();
        Ok(SavEnv {
            segments: unit_segments(&spec, unit)?,
            spec,
            target,
            remote: remote_ref(config),
        })
    }

    fn unit_path(&self) -> Vec<&str> {
        self.segments.iter().map(String::as_str).collect()
    }
}

/// Optional cloud/local details for the battery conflict keys.
fn build_sav_conflict_info(
    cloud: Option<&Meta>,
    cache: Option<&Cache>,
    local: Option<&Path>,
    device_id: &str,
) -> ConflictInfo {
    ConflictInfo {
        cloud_version: cloud.map(|meta| meta.version),
        cloud_device: cloud.map(|meta| meta.device_id.clone()),
        cloud_updated: cloud.map(|meta| meta.updated_at.clone()),
        local_version: cache.map(|cache| cache.version),
        local_device: Some(device_id.to_string()),
        local_updated: util::newest_mtime_iso(local),
    }
}

/// Upload the local battery save (`battery.sav` on the cloud).
///
/// Only the battery unit is touched: the save-state unit, its cache and its
/// cloud directory are left exactly as they are. A card without either
/// `.sav` or `.srm` is a `noop` ("本地没有电池存档"), never an empty upload.
pub fn push_sav(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PushOptions,
    rep: &mut Reporter,
) -> Outcome {
    match run_push_sav(ctx, config, unit, options, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("push", &err, None, None),
    }
}

fn run_push_sav(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PushOptions,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let env = SavEnv::new(&ctx.root, unit, config)?;

    rep.stage(Stage::Net);
    net::ensure_connected(
        &ctx.root,
        config,
        ctx.net_mode,
        options.net_up,
        options.timeout,
        rep,
    )?;

    let _lock = UnitLock::acquire(&ctx.root)?;

    // Locate the single battery file before any network round: a card without
    // one has nothing to push.
    let Some(sav) = unit::scan_sav(&ctx.root, unit)? else {
        rep.info("本地没有 .sav/.srm，跳过电池存档推送");
        return Ok(Outcome::noop("push", unit::missing_sav_detail()));
    };
    let digest = hash::sha256_file(&sav.path)?;
    let hashed = vec![(unit::BATTERY.to_string(), sav.size, digest)];
    let aggregate_local = local_aggregate(&hashed);
    let device_id = config::device_id(&ctx.root, config);

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);
    let unit_path = env.unit_path();
    let cloud_meta = fetch_cloud_meta(&client, rep, unit, &unit_path, Kind::Sav, None)?;
    if cloud_meta.is_some() {
        require_layout(&client, rep)?;
    }
    let cache = load_cache(&ctx.root, &env.spec, unit, &env.remote, rep)?;

    let decision = decide_push(
        cloud_meta.as_ref(),
        cache.as_ref(),
        &aggregate_local,
        options.force,
    );
    if options.full && matches!(&decision, PushDecision::Noop { .. }) {
        rep.info("全量模式（--full）：忽略“已是最新”，重传电池存档");
        return execute_sav_upload(
            ctx,
            unit,
            &env,
            &client,
            &sav.path,
            &hashed,
            &device_id,
            cloud_meta.as_ref(),
            options.force,
            true,
            "push",
            rep,
        );
    }
    match decision {
        PushDecision::Noop {
            detail,
            update_cache,
        } => {
            if update_cache {
                if let Some(meta) = &cloud_meta {
                    cache::save(
                        &ctx.root,
                        &env.spec,
                        unit,
                        &Cache::from_meta(meta, unit, env.remote.clone()),
                    )?;
                }
            }
            rep.info("内容一致，无需传输");
            Ok(Outcome::noop("push", detail))
        }
        PushDecision::Conflict { kind, detail } => {
            let info = build_sav_conflict_info(
                cloud_meta.as_ref(),
                cache.as_ref(),
                Some(&sav.path),
                &device_id,
            );
            let detail = LinkError::new(kind, detail).detail();
            Ok(Outcome::conflict("push", kind, detail, None).with_conflict_info(info))
        }
        PushDecision::Upload => execute_sav_upload(
            ctx,
            unit,
            &env,
            &client,
            &sav.path,
            &hashed,
            &device_id,
            cloud_meta.as_ref(),
            options.force,
            options.full,
            "push",
            rep,
        ),
    }
}

/// Cloud aggregate of a single battery file.
fn sav_aggregate(digest: &str) -> String {
    hash::aggregate_hash([(unit::BATTERY, digest)])
}

/// Download the cloud battery save and install it atomically.
///
/// Only the battery unit is touched: no state file, no state cache, no state
/// cloud directory. The download is verified before anything local changes,
/// the previous local save is copied to `<game>.conflict.<stamp>.sav` when it
/// holds unsynced changes (or on `--force`), and the install is one rename.
pub fn pull_sav(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PullOptions,
    rep: &mut Reporter,
) -> Outcome {
    match run_pull_sav(ctx, config, unit, options, rep) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::error("pull", &err, None, None),
    }
}

fn run_pull_sav(
    ctx: &Ctx,
    config: &Config,
    unit: &Unit,
    options: PullOptions,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let env = SavEnv::new(&ctx.root, unit, config)?;

    rep.stage(Stage::Net);
    net::ensure_connected(
        &ctx.root,
        config,
        ctx.net_mode,
        options.net_up,
        options.timeout,
        rep,
    )?;

    let _lock = UnitLock::acquire(&ctx.root)?;

    let client = DavClient::new(config, rep)?;
    rep.stage(Stage::Connect);
    let unit_path = env.unit_path();
    let Some(cloud_meta) = fetch_cloud_meta(&client, rep, unit, &unit_path, Kind::Sav, None)?
    else {
        rep.info("云端没有电池存档的 meta.json");
        return Ok(Outcome::noop("pull", "云端无此游戏存档"));
    };
    require_layout(&client, rep)?;
    let cache = load_cache(&ctx.root, &env.spec, unit, &env.remote, rep)?;

    // A cache entry whose local file vanished is still "no local content", but
    // the cached version is what the dirty check compares against.
    let local = unit::scan_sav(&ctx.root, unit)?;
    let local_digest = match &local {
        Some(scan) => Some(hash::sha256_file(&scan.path)?),
        None => None,
    };
    let cloud_updated = match &cache {
        Some(cache) => !hash::digest_eq(cloud_meta.aggregate(), &cache.aggregate_hash),
        None => true,
    };
    let local_dirty = match (&cache, &local_digest) {
        (Some(cache), Some(digest)) => {
            !hash::digest_eq(&sav_aggregate(digest), &cache.aggregate_hash)
        }
        (Some(_), None) => true,
        (None, Some(_)) => true,
        (None, None) => false,
    };

    if !options.force && !options.full {
        if let Some(_cache) = &cache {
            if !cloud_updated {
                if local_dirty {
                    rep.info("云端无更新，本地有未推送修改，不做覆盖");
                    return Ok(Outcome::noop("pull", "本地有未推送修改，请用 push 上传"));
                }
                rep.info("云端与本地一致");
                return Ok(Outcome::noop("pull", "已是最新"));
            }
        }
    }

    execute_sav_download(
        ctx,
        unit,
        &env,
        &client,
        &cloud_meta,
        local.as_ref(),
        local_digest.as_deref(),
        local_dirty,
        options.force,
        options.full,
        "pull",
        rep,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_sav_download(
    ctx: &Ctx,
    unit: &Unit,
    env: &SavEnv,
    client: &DavClient,
    cloud_meta: &Meta,
    local: Option<&unit::SavScan>,
    local_digest: Option<&str>,
    local_dirty: bool,
    conflict_copy_always: bool,
    full: bool,
    action: &'static str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let unit_path = env.unit_path();
    let file = cloud_meta
        .files
        .get(unit::BATTERY)
        .expect("a validated sav meta always lists battery.sav");
    let up_to_date = match (local, local_digest) {
        (Some(scan), Some(digest)) => {
            scan.size == file.size && hash::digest_eq(digest, &file.sha256)
        }
        _ => false,
    };

    if up_to_date && !full && !conflict_copy_always {
        cache::save(
            &ctx.root,
            &env.spec,
            unit,
            &Cache::from_meta(cloud_meta, unit, env.remote.clone()),
        )?;
        rep.info(format!("已是最新（v{}）", cloud_meta.version));
        return Ok(Outcome::noop(action, "已是最新"));
    }

    // Fail before downloading: a read-only card or a full disk must not leave
    // a half-installed save behind.
    let parent = env
        .target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ctx.root.clone());
    store::probe_writable(&parent)?;

    let digest = unit.require_rom_sha256()?;
    let work = store::prepare_sav_work(&ctx.root, digest)?;

    rep.stage(Stage::Download);
    let rel = with_name(&unit_path, unit::BATTERY);
    let bytes = client.get(rep, &rel)?.ok_or_else(|| {
        LinkError::new(
            ErrorKind::Server,
            format!(
                "云端 meta.json 列出的 {} 不存在（{} 字节）",
                unit::BATTERY,
                file.size
            ),
        )
    })?;

    rep.stage(Stage::Verify);
    hash::verify_bytes(&bytes, file.size, &file.sha256).map_err(|err| {
        LinkError::new(ErrorKind::Verify, format!("battery.sav: {}", err.message()))
    })?;
    let incoming = work.join(store::SAV_INCOMING);
    store::write_file_sync(&incoming, &bytes)?;
    rep.info(format!("已校验电池存档（{} 字节）", bytes.len()));

    // Preserve the local side before replacing it: always for a forced
    // overwrite, and otherwise only when unsynced local changes would be lost.
    let mut conflict_path: Option<PathBuf> = None;
    if let Some(scan) = local {
        if conflict_copy_always || (local_dirty && !up_to_date) {
            let backup = if scan.path == env.target {
                store::backup_local_sav(&env.target, &unit.game)?
            } else {
                // `.srm` fallback: the backup is still a `.sav` conflict copy.
                let path = store::sav_conflict_path(&env.target, &unit.game);
                store::copy_file_sync(&scan.path, &path)?;
                path
            };
            rep.warn(format!("本地旧电池存档已备份到 {}", backup.display()));
            conflict_path = Some(backup);
        }
    }

    rep.stage(Stage::Commit);
    store::install_sav(&incoming, &env.target)?;
    rep.info("已落位电池存档");

    cache::save(
        &ctx.root,
        &env.spec,
        unit,
        &Cache::from_meta(cloud_meta, unit, env.remote.clone()),
    )?;
    Ok(Outcome::ok(action, Some(cloud_meta.version), "拉取成功").with_conflict(conflict_path))
}

/// PUT a committed cloud file through a temporary name + MOVE, cleaning the
/// temporary file up when the MOVE fails (a failed commit must not leave
/// `*.linkslot-tmp.*` behind).
fn put_verified_atomic(
    client: &DavClient,
    rep: &mut Reporter,
    unit_path: &[&str],
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    let token = util::unique_suffix();
    let tmp_name = format!("{name}.linkslot-tmp.{token}");
    let tmp_rel = with_name(unit_path, tmp_name.as_str());
    let file_rel = with_name(unit_path, name);
    client.put(rep, &tmp_rel, bytes)?;
    match client.move_to(rep, &tmp_rel, &file_rel) {
        Ok(MoveOutcome::Moved) => Ok(()),
        Ok(MoveOutcome::Unsupported) => {
            rep.warn("WebDAV 不支持 MOVE，回退为直接 PUT（无原子覆盖）");
            let _ = client.delete(rep, &tmp_rel);
            client.put(rep, &file_rel, bytes)
        }
        Err(err) => {
            // Best-effort cleanup: the old data file is still in place.
            let _ = client.delete(rep, &tmp_rel);
            Err(err)
        }
    }
}

/// Upload `battery.sav` and commit `meta.json` last.
///
/// `backup_cloud` (`--force`) first copies the old cloud meta and its data file
/// into `<unit>/.conflict.<stamp>/`. A failure before the meta commit leaves the
/// old meta pointing at the old, still-present file.
#[allow(clippy::too_many_arguments)]
fn execute_sav_upload(
    ctx: &Ctx,
    unit: &Unit,
    env: &SavEnv,
    client: &DavClient,
    local: &Path,
    hashed: &[(String, u64, String)],
    device_id: &str,
    previous: Option<&Meta>,
    backup_cloud: bool,
    full: bool,
    action: &'static str,
    rep: &mut Reporter,
) -> Result<Outcome> {
    let unit_path = env.unit_path();
    let (_, size, digest) = hashed
        .first()
        .expect("the battery unit always has exactly one file");
    let unchanged = previous
        .and_then(|meta| meta.files.get(unit::BATTERY))
        .is_some_and(|file| file.size == *size && hash::digest_eq(&file.sha256, digest));
    if unchanged && !full && !backup_cloud {
        if let Some(meta) = previous {
            cache::save(
                &ctx.root,
                &env.spec,
                unit,
                &Cache::from_meta(meta, unit, env.remote.clone()),
            )?;
        }
        rep.info("内容一致，无需传输");
        return Ok(Outcome::noop(action, "内容一致，无需传输"));
    }

    let new_version = previous.map(|meta| meta.version + 1).unwrap_or(1);
    let battery = (unit::BATTERY.to_string(), *size, digest.clone());
    let new_meta = Meta::build_sav(unit, new_version, device_id, &battery, previous);

    rep.stage(Stage::Upload);
    if full {
        rep.info("全量模式（--full）：重传电池存档");
    }

    let mut conflict_path: Option<PathBuf> = None;
    if backup_cloud {
        if let Some(old_meta) = previous {
            let url = backup_cloud_unit(client, rep, &unit_path, old_meta)?;
            conflict_path = Some(PathBuf::from(url));
        }
    }

    ensure_layout(client, rep)?;
    client.ensure_dir(rep, &unit_path)?;

    // Read once more right before the upload: the file may have changed after
    // the scan (the size/hash in meta must describe the uploaded bytes).
    let bytes = std::fs::read(local)
        .map_err(|err| fs_error(format!("无法读取 {}", local.display()), err))?;
    hash::verify_bytes(&bytes, *size, digest).map_err(|err| {
        LinkError::new(
            ErrorKind::Fs,
            format!("{} 在扫描后发生了变化：{}", local.display(), err.message()),
        )
    })?;
    put_verified_atomic(client, rep, &unit_path, unit::BATTERY, &bytes)?;
    rep.info("已上传 battery.sav");

    // Commit point: meta.json last, so a failure leaves the old meta pointing
    // at the old file.
    rep.stage(Stage::Commit);
    put_verified_atomic(
        client,
        rep,
        &unit_path,
        meta::META_NAME,
        &new_meta.to_bytes()?,
    )?;

    let new_cache = Cache::from_meta(&new_meta, unit, env.remote.clone());
    cache::save(&ctx.root, &env.spec, unit, &new_cache)?;
    Ok(Outcome::ok(action, Some(new_version), "推送成功").with_conflict(conflict_path))
}

/// Directories on the card that linkSlot maintains, exposed for diagnostics.
pub fn managed_dirs(root: &Path) -> Vec<PathBuf> {
    vec![root.join(config::CONFIG_DIR_REL)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::RemoteRef;
    use crate::unit::ScanFile;

    fn unit() -> Unit {
        Unit::new("GBA", "mgba", "合金弹头").unwrap()
    }

    fn virtual_remote() -> RemoteRef {
        RemoteRef {
            url: "http://127.0.0.1:9/dav".to_string(),
            remote_root: "/linkSlot".to_string(),
        }
    }

    fn digest(content: &[u8]) -> String {
        hash::sha256_hex(content)
    }

    fn aggregate(content: &[u8]) -> String {
        hash::aggregate_hash([("resume.state", digest(content).as_str())])
    }

    fn meta(version: u64, content: &[u8]) -> Meta {
        let hashed = vec![(
            "resume.state".to_string(),
            content.len() as u64,
            digest(content),
        )];
        Meta::build(&unit(), version, "other-dev", &hashed, 0, None)
    }

    fn cache(meta: &Meta) -> Cache {
        Cache::from_meta(meta, &unit(), virtual_remote())
    }

    #[allow(dead_code)]
    fn non_empty_scan() -> Scan {
        Scan {
            files: vec![ScanFile {
                name: "resume.state".to_string(),
                path: PathBuf::from("/tmp/resume.state"),
                size: 1,
            }],
            has_resume: true,
            ring_count: 0,
        }
    }

    /// Local `(name, size, sha256)` triples in scan order (name bytes).
    fn hashed_files(files: &[(&str, &[u8])]) -> Vec<(String, u64, String)> {
        let mut hashed: Vec<(String, u64, String)> = files
            .iter()
            .map(|(name, bytes)| ((*name).to_string(), bytes.len() as u64, digest(bytes)))
            .collect();
        hashed.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        hashed
    }

    fn meta_files(files: &[(&str, &[u8])]) -> Meta {
        let hashed = hashed_files(files);
        let ring = files
            .iter()
            .filter(|(name, _)| unit::state_stamp(name).is_some())
            .count();
        Meta::build(&unit(), 1, "dev", &hashed, ring, None)
    }

    #[test]
    fn upload_plan_skips_unchanged_files() {
        let cloud = meta_files(&[
            ("resume.state", b"cloud-resume"),
            ("2026-10-20_10-00-00.state", b"state"),
            ("2026-10-20_10-00-00.png", b"png"),
        ]);
        let local = hashed_files(&[
            ("resume.state", b"local-resume"),
            ("2026-10-20_10-00-00.state", b"state"),
            ("2026-10-20_10-00-00.png", b"png"),
        ]);
        let plan = plan_upload(&local, Some(&cloud), false);
        assert_eq!(plan.changed, ["resume.state"]);
        assert!(plan.remote_only.is_empty());
        assert_eq!(plan.skipped, 2);
        assert!(!plan.is_empty());
    }

    #[test]
    fn upload_plan_lists_remote_only_files_for_deletion() {
        let cloud = meta_files(&[
            ("resume.state", b"r"),
            ("2026-10-20_10-00-00.state", b"s"),
            ("2026-10-20_10-00-00.png", b"p"),
        ]);
        let local = hashed_files(&[("resume.state", b"r")]);
        let plan = plan_upload(&local, Some(&cloud), false);
        assert!(plan.changed.is_empty());
        assert_eq!(
            plan.remote_only,
            ["2026-10-20_10-00-00.png", "2026-10-20_10-00-00.state"]
        );
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn upload_plan_full_sends_every_file() {
        let cloud = meta_files(&[("resume.state", b"r")]);
        let local = hashed_files(&[("resume.state", b"r"), ("2026-10-20_10-00-00.state", b"s")]);
        let plan = plan_upload(&local, Some(&cloud), true);
        assert_eq!(plan.changed, ["2026-10-20_10-00-00.state", "resume.state"]);
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn upload_plan_first_push_sends_everything() {
        let local = hashed_files(&[("resume.state", b"r"), ("2026-10-20_10-00-00.state", b"s")]);
        let plan = plan_upload(&local, None, false);
        assert_eq!(plan.changed, ["2026-10-20_10-00-00.state", "resume.state"]);
        assert!(plan.remote_only.is_empty());
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn download_plan_only_fetches_changed_files() {
        let cloud = meta_files(&[
            ("resume.state", b"cloud-resume"),
            ("2026-10-20_10-00-00.state", b"state"),
            ("2026-10-20_10-00-00.png", b"png"),
        ]);
        let local = hashed_files(&[
            ("resume.state", b"local-resume"),
            ("2026-10-20_10-00-00.state", b"state"),
        ]);
        let plan = plan_download(&local, &cloud, false);
        assert_eq!(plan.changed, ["2026-10-20_10-00-00.png", "resume.state"]);
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn download_plan_full_fetches_every_file() {
        let cloud = meta_files(&[("resume.state", b"r"), ("2026-10-20_10-00-00.state", b"s")]);
        let local = hashed_files(&[("resume.state", b"r")]);
        let plan = plan_download(&local, &cloud, true);
        assert_eq!(plan.changed, ["2026-10-20_10-00-00.state", "resume.state"]);
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn download_plan_empty_when_local_already_matches() {
        let cloud = meta_files(&[("resume.state", b"same")]);
        let local = hashed_files(&[("resume.state", b"same")]);
        let plan = plan_download(&local, &cloud, false);
        assert!(plan.changed.is_empty());
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn plan_compares_size_as_well_as_digest() {
        let cloud = meta_files(&[("resume.state", b"same")]);
        // Same digest, different size: an inconsistent/partial meta must not
        // make the file look synchronized.
        let local = vec![("resume.state".to_string(), 99u64, digest(b"same"))];
        let upload = plan_upload(&local, Some(&cloud), false);
        assert_eq!(upload.changed, ["resume.state"]);
        assert_eq!(upload.skipped, 0);
        let download = plan_download(&local, &cloud, false);
        assert_eq!(download.changed, ["resume.state"]);
        assert_eq!(download.skipped, 0);
    }

    #[test]
    fn aggregate_prefers_local_hashes() {
        let hashed = vec![
            ("b.state".to_string(), 1, hash::sha256_hex(b"b")),
            ("a.state".to_string(), 1, hash::sha256_hex(b"a")),
        ];
        let aggregate = local_aggregate(&hashed);
        assert!(aggregate.starts_with("sha256:"));
        let digest_a = hash::sha256_hex(b"a");
        let digest_b = hash::sha256_hex(b"b");
        assert!(hash::digest_eq(
            &aggregate,
            &hash::aggregate_hash([
                ("a.state", digest_a.as_str()),
                ("b.state", digest_b.as_str())
            ])
        ));
    }

    #[test]
    fn path_building_helpers() {
        let root = Path::new("/mnt/sdcard");
        // A hashless unit is local-only: no cloud segments at all.
        let plain = Unit::new("GBA", "mgba", "合金弹头").unwrap();
        assert!(UnitSpec::state(&plain, root)
            .remote_segments(&plain)
            .is_err());

        // Hash units put the ROM SHA-256 at the game level; the game name only
        // stays in the local directory.
        let hash = "a".repeat(64);
        let hashed = plain.clone().with_rom_sha256(&hash).unwrap();
        let state = UnitSpec::state(&hashed, root);
        let segments = unit_segments(&state, &hashed).unwrap();
        assert_eq!(segments, ["GBA", hash.as_str(), "state", "mgba"]);
        let refs = as_refs(&segments);
        assert_eq!(
            with_name(&refs, "meta.json"),
            ["GBA", hash.as_str(), "state", "mgba", "meta.json"]
        );
        assert_eq!(
            hashed.local_dir(root),
            PathBuf::from("/mnt/sdcard/States/GBA/mgba/合金弹头")
        );
        // The cloud path never carries the game name.
        let remote = segments.join("/");
        assert!(!remote.contains("合金弹头"));
        assert_eq!(hashed.id().unwrap(), format!("GBA/mgba/{hash}"));

        // The battery unit has no core segment.
        let sav = UnitSpec::sav(&hashed, root);
        let segments = unit_segments(&sav, &hashed).unwrap();
        assert_eq!(segments, ["GBA", hash.as_str(), "sav"]);
        let refs = as_refs(&segments);
        assert_eq!(
            with_name(&refs, "battery.sav"),
            ["GBA", hash.as_str(), "sav", "battery.sav"]
        );
        // Game-level (playtime) path stays `<platform>/<sha>`.
        assert_eq!(hashed.remote_segments(), ["GBA", hash.as_str()]);
    }

    #[test]
    fn managed_dirs_are_under_config() {
        let dirs = managed_dirs(Path::new("/mnt/sdcard"));
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("linkSlot/Config"));
    }

    #[test]
    fn push_matrix_matches_project_book() {
        // No cloud meta -> always upload.
        assert!(matches!(
            decide_push(None, None, &aggregate(b"local"), false),
            PushDecision::Upload
        ));

        let cloud = meta(2, b"cloud");
        // Cloud exists, no cache -> conflict unless forced.
        assert!(matches!(
            decide_push(Some(&cloud), None, &aggregate(b"local"), false),
            PushDecision::Conflict {
                kind: ErrorKind::NoCache,
                ..
            }
        ));
        assert!(matches!(
            decide_push(Some(&cloud), None, &aggregate(b"local"), true),
            PushDecision::Upload
        ));

        // Cloud unchanged: identical content noop, local change uploads.
        let cached = cache(&cloud);
        assert!(matches!(
            decide_push(Some(&cloud), Some(&cached), cloud.aggregate(), false),
            PushDecision::Noop {
                update_cache: true,
                ..
            }
        ));
        assert!(matches!(
            decide_push(Some(&cloud), Some(&cached), &aggregate(b"local"), false),
            PushDecision::Upload
        ));

        // Cloud newer: content identical noop, local changes conflict.
        let newer = meta(3, b"cloud-newer");
        assert!(matches!(
            decide_push(Some(&newer), Some(&cached), newer.aggregate(), false),
            PushDecision::Noop {
                update_cache: true,
                ..
            }
        ));
        assert!(matches!(
            decide_push(Some(&newer), Some(&cached), &aggregate(b"local"), false),
            PushDecision::Conflict {
                kind: ErrorKind::RemoteNewer,
                ..
            }
        ));
        assert!(matches!(
            decide_push(Some(&newer), Some(&cached), &aggregate(b"local"), true),
            PushDecision::Upload
        ));

        // Forked (same version, different content) and rollback.
        let forked = meta(2, b"forked");
        assert!(matches!(
            decide_push(Some(&forked), Some(&cached), &aggregate(b"local"), false),
            PushDecision::Conflict {
                kind: ErrorKind::Forked,
                ..
            }
        ));
        let older = meta(1, b"older");
        assert!(matches!(
            decide_push(Some(&older), Some(&cached), &aggregate(b"local"), false),
            PushDecision::Conflict {
                kind: ErrorKind::RemoteRollback,
                ..
            }
        ));
        assert!(matches!(
            decide_push(Some(&older), Some(&cached), &aggregate(b"local"), true),
            PushDecision::Upload
        ));
    }

    // ---- v0.0.10 dual-unit decision matrix ------------------------------

    fn matrix_unit() -> Unit {
        unit().with_rom_sha256(&"a".repeat(64)).unwrap()
    }

    fn cloud_for(kind: Kind, version: u64, content: &[u8]) -> Meta {
        match kind {
            Kind::State => {
                let hashed = vec![(
                    "resume.state".to_string(),
                    content.len() as u64,
                    digest(content),
                )];
                Meta::build(&matrix_unit(), version, "other-dev", &hashed, 0, None)
            }
            Kind::Sav => {
                let battery = (
                    "battery.sav".to_string(),
                    content.len() as u64,
                    digest(content),
                );
                Meta::build_sav(&matrix_unit(), version, "other-dev", &battery, None)
            }
        }
    }

    fn cache_for(meta: &Meta) -> Cache {
        Cache::from_meta(meta, &matrix_unit(), virtual_remote())
    }

    fn aggregate_for(kind: Kind, content: &[u8]) -> String {
        let name = match kind {
            Kind::State => "resume.state",
            Kind::Sav => "battery.sav",
        };
        hash::aggregate_hash([(name, digest(content).as_str())])
    }

    /// Row 1: nothing anywhere is a noop, with or without a cache.
    #[test]
    fn matrix_no_local_no_cloud_is_noop() {
        for kind in Kind::ALL {
            let empty = aggregate_for(kind, b"");
            assert_eq!(
                decide_unit(false, &empty, None, None),
                Decision::Noop {
                    detail: "云端无此游戏存档".to_string(),
                    update_cache: false,
                },
                "{kind}"
            );
            let stale = cache_for(&cloud_for(kind, 3, b"stale"));
            assert!(matches!(
                decide_unit(false, &empty, None, Some(&stale)),
                Decision::Noop { .. }
            ));
        }
    }

    /// Row 2: local content and no cloud copy is a first push.
    #[test]
    fn matrix_local_without_cloud_is_push() {
        for kind in Kind::ALL {
            let local = aggregate_for(kind, b"local");
            assert_eq!(
                decide_unit(true, &local, None, None),
                Decision::Push,
                "{kind}"
            );
        }
    }

    /// Row 3: cloud copy, no local content, no cache -> pull (new device).
    #[test]
    fn matrix_cloud_without_local_or_cache_is_pull() {
        for kind in Kind::ALL {
            let cloud = cloud_for(kind, 2, b"cloud");
            let empty = aggregate_for(kind, b"");
            assert_eq!(
                decide_unit(false, &empty, Some(&cloud), None),
                Decision::Pull,
                "{kind}"
            );
        }
    }

    /// Row 4: cloud copy, no local file, but a cache exists -> conflict. A
    /// deleted local save must never be restored silently.
    #[test]
    fn matrix_cloud_with_cache_but_no_local_is_conflict() {
        for kind in Kind::ALL {
            let cloud = cloud_for(kind, 2, b"cloud");
            let cached = cache_for(&cloud);
            let empty = aggregate_for(kind, b"");
            assert!(
                matches!(
                    decide_unit(false, &empty, Some(&cloud), Some(&cached)),
                    Decision::Conflict {
                        kind: ErrorKind::NoCache,
                        ..
                    }
                ),
                "{kind}"
            );
        }
    }

    /// Row 5: both sides have content but no cache -> conflict, no guess.
    #[test]
    fn matrix_both_sides_without_cache_is_conflict() {
        for kind in Kind::ALL {
            let cloud = cloud_for(kind, 2, b"cloud");
            let local = aggregate_for(kind, b"local");
            assert!(
                matches!(
                    decide_unit(true, &local, Some(&cloud), None),
                    Decision::Conflict {
                        kind: ErrorKind::NoCache,
                        ..
                    }
                ),
                "{kind}"
            );
        }
    }

    /// Row 6: content equal to the cloud (and therefore to the cache).
    #[test]
    fn matrix_identical_content_is_noop() {
        for kind in Kind::ALL {
            let cloud = cloud_for(kind, 2, b"same");
            let cached = cache_for(&cloud);
            let local = aggregate_for(kind, b"same");
            assert_eq!(
                decide_unit(true, &local, Some(&cloud), Some(&cached)),
                Decision::Noop {
                    detail: "已是最新".to_string(),
                    update_cache: true,
                },
                "{kind}"
            );
        }
    }

    /// Row 7: both sides changed since the cache -> conflict, classified by
    /// the version relation.
    #[test]
    fn matrix_both_changed_is_conflict() {
        for kind in Kind::ALL {
            let cached_meta = cloud_for(kind, 2, b"cached");
            let cached = cache_for(&cached_meta);
            let local = aggregate_for(kind, b"local-new");

            let forked = cloud_for(kind, 2, b"cloud-new");
            assert!(
                matches!(
                    decide_unit(true, &local, Some(&forked), Some(&cached)),
                    Decision::Conflict {
                        kind: ErrorKind::Forked,
                        ..
                    }
                ),
                "{kind}: same version must be forked"
            );

            let newer = cloud_for(kind, 3, b"cloud-new");
            assert!(
                matches!(
                    decide_unit(true, &local, Some(&newer), Some(&cached)),
                    Decision::Conflict {
                        kind: ErrorKind::RemoteNewer,
                        ..
                    }
                ),
                "{kind}: a higher cloud version must be remote-newer"
            );

            let older = cloud_for(kind, 1, b"cloud-new");
            assert!(
                matches!(
                    decide_unit(true, &local, Some(&older), Some(&cached)),
                    Decision::Conflict {
                        kind: ErrorKind::RemoteRollback,
                        ..
                    }
                ),
                "{kind}: a lower cloud version must be remote-rollback"
            );
        }
    }

    /// Row 8: only the local side changed -> push.
    #[test]
    fn matrix_local_changed_cloud_unchanged_is_push() {
        for kind in Kind::ALL {
            let cloud = cloud_for(kind, 2, b"cloud");
            let cached = cache_for(&cloud);
            let local = aggregate_for(kind, b"local-new");
            assert_eq!(
                decide_unit(true, &local, Some(&cloud), Some(&cached)),
                Decision::Push,
                "{kind}"
            );
        }
    }

    /// Row 9: only the cloud side changed -> pull.
    #[test]
    fn matrix_local_unchanged_cloud_changed_is_pull() {
        for kind in Kind::ALL {
            let cached_meta = cloud_for(kind, 1, b"cached");
            let cached = cache_for(&cached_meta);
            let cloud = cloud_for(kind, 2, b"cloud-new");
            let local = aggregate_for(kind, b"cached");
            assert_eq!(
                decide_unit(true, &local, Some(&cloud), Some(&cached)),
                Decision::Pull,
                "{kind}"
            );
        }
    }

    /// The two units never share an aggregate: a battery meta can never look
    /// like the state's content, so one unit's version can never decide the
    /// other's direction.
    #[test]
    fn state_and_battery_aggregates_are_independent() {
        let state = cloud_for(Kind::State, 5, b"same-bytes");
        let sav = cloud_for(Kind::Sav, 9, b"same-bytes");
        assert_ne!(state.aggregate(), sav.aggregate());
        assert_ne!(state.kind, sav.kind);
        let state_cache = cache_for(&state);
        let sav_cache = cache_for(&sav);
        assert_eq!(state_cache.kind, Kind::State);
        assert_eq!(sav_cache.kind, Kind::Sav);
        // A state decision must not become a noop just because the battery
        // happens to be in sync.
        assert!(!matches!(
            decide_unit(
                true,
                &aggregate_for(Kind::State, b"state-local"),
                Some(&state),
                Some(&state_cache)
            ),
            Decision::Noop { .. }
        ));
    }
}
