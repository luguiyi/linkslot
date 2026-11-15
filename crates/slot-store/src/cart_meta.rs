use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{Cart, Outline, Platform, ShellChoice, ShellFinish};

/// The metadata database is the runtime source of truth.  Per-game JSON
/// sidecars are deliberately not read by the app; migration/export is a mod
/// concern.
pub const DB_FILE: &str = "linkSlot/Metadata/library.db";
pub const SCHEMA: i64 = 4;

const COLUMNS: &str = "rom, platform, stem, title, code, size, mtime, cover, intro, video, cheat, guide, color, shell_outline, shell_finish, core, extra_json, sha256";

const CREATE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS carts (
    rom           TEXT PRIMARY KEY,
    platform      TEXT NOT NULL,
    stem          TEXT NOT NULL,
    title         TEXT,
    code          TEXT,
    size          INTEGER,
    mtime         INTEGER,
    cover         TEXT,
    intro         TEXT,
    video         TEXT,
    cheat         TEXT,
    guide         TEXT,
    color         TEXT,
    shell_outline TEXT,
    shell_finish  TEXT,
    core          TEXT,
    extra_json    TEXT,
    sha256        TEXT,
    updated_at    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS carts_platform_stem ON carts(platform, stem);
CREATE INDEX IF NOT EXISTS carts_platform_sha256 ON carts(platform, sha256);
"#;

const UPSERT_SQL: &str = r#"
INSERT INTO carts
    (rom, platform, stem, title, code, size, mtime, cover, intro,
     video, cheat, guide, color, shell_outline, shell_finish, core,
     extra_json, sha256, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
        ?16, ?17, ?18, ?19)
ON CONFLICT(rom) DO UPDATE SET
    platform = excluded.platform,
    stem = excluded.stem,
    title = excluded.title,
    code = excluded.code,
    size = excluded.size,
    mtime = excluded.mtime,
    cover = excluded.cover,
    intro = excluded.intro,
    video = excluded.video,
    cheat = excluded.cheat,
    guide = excluded.guide,
    color = excluded.color,
    shell_outline = excluded.shell_outline,
    shell_finish = excluded.shell_finish,
    core = excluded.core,
    extra_json = excluded.extra_json,
    sha256 = excluded.sha256,
    updated_at = excluded.updated_at
"#;

const INSERT_IGNORE_SQL: &str = r#"
INSERT OR IGNORE INTO carts
    (rom, platform, stem, title, code, size, mtime, cover, intro,
     video, cheat, guide, color, shell_outline, shell_finish, core,
     extra_json, sha256, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
        ?16, ?17, ?18, ?19)
"#;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CartMeta {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub rom: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    /// Full-file SHA-256, lower-case hex.  This is the logical content ID;
    /// `rom` remains the physical path identity.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub mtime: Option<u64>,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub intro: Option<String>,
    #[serde(default)]
    pub video: Option<String>,
    /// Optional per-game `.cht` path, relative to the card root.  When absent
    /// the app uses `Cheats/<platform>/<core>/<stem>.cht`.
    #[serde(default)]
    pub cheat: Option<String>,
    /// Optional TXT guide path, relative to the card root.  When absent the
    /// app looks for `Guides/<platform>/<stem>.txt`.
    #[serde(default)]
    pub guide: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub shell: Option<ShellMeta>,
    #[serde(default)]
    pub core: Option<String>,
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellMeta {
    #[serde(default)]
    pub outline: String,
    #[serde(default)]
    pub finish: String,
}

impl CartMeta {
    pub fn from_cart(root: &Path, cart: &Cart) -> CartMeta {
        let rom = relative(root, &cart.rom);
        let labels = root.join("Labels").join(cart.platform.dir_name());
        let cover = cart
            .label
            .as_deref()
            .and_then(|path| relative_option(root, path))
            .or_else(|| existing(root, labels.join(format!("{}.png", cart.stem))));
        let info = root.join("Info").join(cart.platform.dir_name());
        let intro = existing(root, info.join(format!("{}.txt", cart.stem)));
        let video = existing(root, info.join(format!("{}.mp4", cart.stem)));
        let guide_dir = root.join("Guides").join(cart.platform.dir_name());
        let guide = existing(root, guide_dir.join(format!("{}.txt", cart.stem)))
            .or_else(|| existing(root, guide_dir.join(format!("{}.epub", cart.stem))))
            .or_else(|| {
                named_file(&guide_dir, &cart.stem, "txt")
                    .and_then(|path| relative_option(root, &path))
            })
            .or_else(|| {
                named_file(&guide_dir, &cart.stem, "epub")
                    .and_then(|path| relative_option(root, &path))
            });
        let shell = cart.shell.map(|shell| ShellMeta {
            outline: outline_name(shell.outline).into(),
            finish: finish_name(shell.finish).into(),
        });
        let color = cart
            .shell
            .map(|shell| crate::cart_color::to_hex(shell.colour));
        let (size, mtime) = file_stamp(&cart.rom);
        CartMeta {
            schema: SCHEMA as u32,
            platform: cart.platform.dir_name().into(),
            rom,
            title: (!cart.title.is_empty()).then(|| cart.title.clone()),
            code: (!cart.code.is_empty()).then(|| cart.code.clone()),
            sha256: None,
            size,
            mtime,
            cover,
            intro,
            video,
            cheat: None,
            guide,
            color,
            shell,
            core: None,
            extra: HashMap::new(),
        }
    }

    pub fn shell_choice(&self) -> Option<ShellChoice> {
        let colour = self
            .color
            .as_deref()
            .and_then(crate::cart_color::parse_hex)?;
        let shell = self.shell.clone().unwrap_or_default();
        Some(ShellChoice {
            outline: outline_from(&shell.outline),
            colour,
            finish: finish_from(&shell.finish),
        })
    }

    pub fn asset(&self, root: &Path, value: Option<&String>) -> Option<PathBuf> {
        let path = value?;
        let rel = Path::new(path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return None;
        }
        let target = root.join(rel);
        target.is_file().then_some(target)
    }

    pub fn cover_path(&self, root: &Path) -> Option<PathBuf> {
        self.asset(root, self.cover.as_ref())
    }

    pub fn intro_path(&self, root: &Path) -> Option<PathBuf> {
        self.asset(root, self.intro.as_ref())
    }

    pub fn video_path(&self, root: &Path) -> Option<PathBuf> {
        self.asset(root, self.video.as_ref())
    }

    pub fn guide_path(&self, root: &Path) -> Option<PathBuf> {
        self.asset(root, self.guide.as_ref()).or_else(|| {
            let stem = Path::new(&self.rom)
                .file_stem()
                .and_then(|stem| stem.to_str())?;
            let dir = root.join("Guides").join(self.platform.as_str());
            ["txt", "epub"].into_iter().find_map(|extension| {
                let path = dir.join(format!("{stem}.{extension}"));
                path.is_file()
                    .then_some(path)
                    .or_else(|| named_file(&dir, stem, extension))
            })
        })
    }

    pub fn cheat_path(&self, root: &Path) -> Option<PathBuf> {
        self.asset(root, self.cheat.as_ref())
    }
}

fn db_path(root: &Path) -> PathBuf {
    root.join(DB_FILE)
}

fn open(root: &Path) -> rusqlite::Result<Connection> {
    let path = db_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    configure(&conn)?;
    Ok(conn)
}

fn open_read(root: &Path) -> Option<Connection> {
    let path = db_path(root);
    if !path.is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_secs(2));
    Some(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(2))?;
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='carts')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        // WAL is best on the handheld's local card.  If the filesystem rejects
        // it (for example a read-only mount), keep the default journal.
        let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0));
        conn.execute_batch(CREATE_SQL)?;
        conn.pragma_update(None, "user_version", SCHEMA)?;
    } else {
        let mut version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0);
        if version < 2 {
            // v0.4.2 and earlier stored no guide path.  Guard the ALTER as
            // well, so an interrupted migration can be retried safely.
            let has_guide: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('carts') WHERE name='guide')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !has_guide {
                conn.execute("ALTER TABLE carts ADD COLUMN guide TEXT", [])?;
            }
            version = 2;
            conn.pragma_update(None, "user_version", version)?;
        }
        if version < 3 {
            let has_sha256: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('carts') WHERE name='sha256')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !has_sha256 {
                conn.execute("ALTER TABLE carts ADD COLUMN sha256 TEXT", [])?;
            }
            conn.execute(
                "CREATE INDEX IF NOT EXISTS carts_platform_sha256 ON carts(platform, sha256)",
                [],
            )?;
        }
        if version < 4 {
            conn.pragma_update(None, "user_version", SCHEMA)?;
        }
    }
    Ok(())
}

fn row_meta(row: &Row<'_>) -> rusqlite::Result<CartMeta> {
    let size: Option<i64> = row.get(5)?;
    let mtime: Option<i64> = row.get(6)?;
    let outline: Option<String> = row.get(13)?;
    let finish: Option<String> = row.get(14)?;
    let extra_json: Option<String> = row.get(16)?;
    let sha256: Option<String> = row.get(17)?;
    let shell = outline
        .zip(finish)
        .map(|(outline, finish)| ShellMeta { outline, finish });
    Ok(CartMeta {
        schema: SCHEMA as u32,
        rom: row.get(0)?,
        platform: row.get(1)?,
        title: row.get(3)?,
        code: row.get(4)?,
        sha256,
        size: size.and_then(|value| u64::try_from(value).ok()),
        mtime: mtime.and_then(|value| u64::try_from(value).ok()),
        cover: row.get(7)?,
        intro: row.get(8)?,
        video: row.get(9)?,
        cheat: row.get(10)?,
        guide: row.get(11)?,
        color: row.get(12)?,
        shell,
        core: row.get(15)?,
        extra: extra_json
            .and_then(|text: String| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
    })
}

fn row_values(
    meta: &CartMeta,
) -> (
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        meta.size.and_then(|value| i64::try_from(value).ok()),
        meta.mtime.and_then(|value| i64::try_from(value).ok()),
        meta.shell.as_ref().map(|shell| shell.outline.clone()),
        meta.shell.as_ref().map(|shell| shell.finish.clone()),
        meta.sha256.clone(),
    )
}

fn execute_upsert(conn: &Connection, meta: &CartMeta) -> rusqlite::Result<()> {
    let (size, mtime, outline, finish, sha256) = row_values(meta);
    let extra = serde_json::to_string(&meta.extra).unwrap_or_else(|_| "{}".into());
    conn.execute(
        UPSERT_SQL,
        params![
            &meta.rom,
            &meta.platform,
            &stem_of(&meta.rom),
            &meta.title,
            &meta.code,
            &size,
            &mtime,
            &meta.cover,
            &meta.intro,
            &meta.video,
            &meta.cheat,
            &meta.guide,
            &meta.color,
            &outline,
            &finish,
            &meta.core,
            &extra,
            &sha256,
            unix_now(),
        ],
    )?;
    Ok(())
}

fn execute_insert_ignore(conn: &Connection, meta: &CartMeta) -> rusqlite::Result<usize> {
    let (size, mtime, outline, finish, sha256) = row_values(meta);
    let extra = serde_json::to_string(&meta.extra).unwrap_or_else(|_| "{}".into());
    conn.execute(
        INSERT_IGNORE_SQL,
        params![
            &meta.rom,
            &meta.platform,
            &stem_of(&meta.rom),
            &meta.title,
            &meta.code,
            &size,
            &mtime,
            &meta.cover,
            &meta.intro,
            &meta.video,
            &meta.cheat,
            &meta.guide,
            &meta.color,
            &outline,
            &finish,
            &meta.core,
            &extra,
            &sha256,
            unix_now(),
        ],
    )
}

pub fn read(root: &Path, _platform: Platform, rom: &Path, _stem: &str) -> Option<CartMeta> {
    let conn = open(root).ok().or_else(|| open_read(root))?;
    let rel = relative(root, rom);
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM carts WHERE rom = ?1"),
        [&rel],
        |row| row_meta(row),
    )
    .optional()
    .ok()
    .flatten()
}

/// The first cached row for a content ID.  A path-based library may contain
/// multiple physical copies with the same SHA-256; callers that only need to
/// know whether the content exists can use this query.
pub fn read_by_sha(root: &Path, platform: Platform, sha256: &str) -> Option<CartMeta> {
    let conn = open(root).ok().or_else(|| open_read(root))?;
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM carts WHERE platform = ?1 AND sha256 = ?2 ORDER BY rom LIMIT 1"
        ),
        rusqlite::params![platform.dir_name(), sha256],
        |row| row_meta(row),
    )
    .optional()
    .ok()
    .flatten()
}

/// Rows with the same size but no SHA-256 yet.  Import uses this when the
/// background index has not finished, so it can hash only plausible matches.
pub fn candidates_by_size(root: &Path, platform: Platform, size: u64) -> Vec<CartMeta> {
    let Some(conn) = open(root).ok().or_else(|| open_read(root)) else {
        return Vec::new();
    };
    let Ok(size) = i64::try_from(size) else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM carts WHERE platform = ?1 AND size = ?2 AND sha256 IS NULL"
    )) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(rusqlite::params![platform.dir_name(), size], |row| {
        row_meta(row)
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// Store a verified SHA-256 only while the file stamp still matches the row.
/// A background hasher must never attach a digest to a ROM that changed while
/// it was being read.
pub fn set_sha256(
    root: &Path,
    rom: &Path,
    digest: &str,
    size: u64,
    mtime: u64,
) -> std::io::Result<bool> {
    set_sha256_many(root, &[(rom.to_path_buf(), digest.to_string(), size, mtime)])
        .map(|changed| changed > 0)
}

/// Batch form used by the background index.  Opening one connection and one
/// transaction per frame avoids thousands of SQLite opens during a cold scan.
pub fn set_sha256_many(
    root: &Path,
    updates: &[(PathBuf, String, u64, u64)],
) -> std::io::Result<usize> {
    if updates.is_empty() {
        return Ok(0);
    }
    let mut conn = open(root).map_err(io_from_sql)?;
    let tx = conn.transaction().map_err(io_from_sql)?;
    let mut changed = 0;
    {
        let mut statement = tx
            .prepare(
                "UPDATE carts SET sha256 = ?1, updated_at = ?2
                 WHERE rom = ?3 AND size IS ?4 AND mtime IS ?5",
            )
            .map_err(io_from_sql)?;
        for (rom, digest, size, mtime) in updates {
            changed += statement
                .execute(rusqlite::params![
                    digest,
                    unix_now(),
                    relative(root, rom),
                    i64::try_from(*size).ok(),
                    i64::try_from(*mtime).ok()
                ])
                .map_err(io_from_sql)?;
        }
    }
    tx.commit().map_err(io_from_sql)?;
    Ok(changed)
}

/// Whether a database row still describes the file on disk.  SHA-256 callers
/// must check this before trusting `sha256`, because a file can be replaced
/// outside the Web UI without the row being updated immediately.
pub fn stamp_matches(root: &Path, meta: &CartMeta) -> bool {
    let (Some(size), Some(mtime)) = (meta.size, meta.mtime) else {
        return false;
    };
    let (actual_size, actual_mtime) = file_stamp(&root.join(&meta.rom));
    actual_size == Some(size) && actual_mtime == Some(mtime)
}

/// Read all metadata in one query.  Used by the scanner so a 5000-cart library
/// does not open the database once per cart.
pub fn read_all(root: &Path) -> HashMap<String, CartMeta> {
    let Some(conn) = open(root).ok().or_else(|| open_read(root)) else {
        return HashMap::new();
    };
    let Ok(mut statement) = conn.prepare(&format!("SELECT {COLUMNS} FROM carts")) else {
        return HashMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| row_meta(row)) else {
        return HashMap::new();
    };
    rows.flatten().map(|meta| (meta.rom.clone(), meta)).collect()
}

pub fn write(root: &Path, _platform: Platform, _rom: &Path, meta: &CartMeta) -> std::io::Result<()> {
    let conn = open(root).map_err(io_from_sql)?;
    let mut meta = meta.clone();
    if meta.rom.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "metadata has no ROM path",
        ));
    }
    meta.rom = relative(root, Path::new(&meta.rom));
    execute_upsert(&conn, &meta).map_err(io_from_sql)
}

pub fn ensure(root: &Path, cart: &Cart) -> std::io::Result<bool> {
    let conn = open(root).map_err(io_from_sql)?;
    let rel = relative(root, &cart.rom);
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM carts WHERE rom = ?1)", [&rel], |row| row.get(0))
        .map_err(io_from_sql)?;
    if exists {
        return Ok(false);
    }
    let mut meta = CartMeta::from_cart(root, cart);
    meta.rom = rel;
    let _ = execute_insert_ignore(&conn, &meta).map_err(io_from_sql)?;
    Ok(true)
}

pub fn ensure_all(root: &Path, carts: &[Cart]) -> std::io::Result<usize> {
    let mut conn = open(root).map_err(io_from_sql)?;
    let mut existing = HashSet::new();
    {
        let mut statement = conn
            .prepare("SELECT rom FROM carts")
            .map_err(io_from_sql)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(io_from_sql)?;
        for rom in rows.flatten() {
            existing.insert(rom);
        }
    }

    let tx = conn.transaction().map_err(io_from_sql)?;
    let mut written = 0;
    for cart in carts {
        let rom = relative(root, &cart.rom);
        if existing.contains(&rom) {
            continue;
        }
        let mut meta = CartMeta::from_cart(root, cart);
        meta.rom = rom;
        written += execute_insert_ignore(&tx, &meta).map_err(io_from_sql)?;
    }
    tx.commit().map_err(io_from_sql)?;
    Ok(written)
}

pub fn update<F>(root: &Path, cart: &Cart, edit: F) -> std::io::Result<CartMeta>
where
    F: FnOnce(&mut CartMeta),
{
    let conn = open(root).map_err(io_from_sql)?;
    let rel = relative(root, &cart.rom);
    let mut meta = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM carts WHERE rom = ?1"),
            [&rel],
            |row| row_meta(row),
        )
        .optional()
        .map_err(io_from_sql)?
        .unwrap_or_else(|| CartMeta::from_cart(root, cart));
    meta.rom = rel;
    edit(&mut meta);
    execute_upsert(&conn, &meta).map_err(io_from_sql)?;
    Ok(meta)
}

pub fn remove(root: &Path, cart: &Cart) -> std::io::Result<()> {
    let conn = open(root).map_err(io_from_sql)?;
    conn.execute("DELETE FROM carts WHERE rom = ?1", [relative(root, &cart.rom)])
        .map_err(io_from_sql)?;
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MigrationReport {
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Import legacy `Metadata/**/*.json` sidecars into the SQLite store.
///
/// This is deliberately an explicit operation, not a boot migration: a mod's
/// Web UI calls it once.  Sidecars are left in place as a backup.
pub fn migrate_sidecars(root: &Path) -> std::io::Result<MigrationReport> {
    let mut files = Vec::new();
    collect_sidecars(&root.join("linkSlot/Metadata"), &mut files, 0);
    files.sort();
    let mut report = MigrationReport::default();
    let mut conn = open(root).map_err(io_from_sql)?;
    let tx = conn.transaction().map_err(io_from_sql)?;
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.display().to_string());
        let result = (|| -> Result<CartMeta, String> {
            let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let mut meta: CartMeta =
                serde_json::from_str(&text).map_err(|error| format!("JSON: {error}"))?;
            if meta.rom.is_empty() || !valid_sidecar_rom(&meta.rom) {
                return Err("rom 路径不在 Games/ 下或包含 ..".into());
            }
            if meta.platform.is_empty() {
                return Err("缺少 platform".into());
            }
            meta.rom = meta.rom.replace('\\', "/");
            Ok(meta)
        })();
        match result {
            Ok(meta) => match execute_upsert(&tx, &meta) {
                Ok(()) => report.imported += 1,
                Err(error) => {
                    report.skipped += 1;
                    push_error(&mut report, format!("{relative}: {error}"));
                }
            },
            Err(error) => {
                report.skipped += 1;
                push_error(&mut report, format!("{relative}: {error}"));
            }
        }
    }
    tx.commit().map_err(io_from_sql)?;
    // A mod just changed the database; make the Web/App refresh path notice.
    Ok(report)
}

fn collect_sidecars(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_sidecars(&path, out, depth + 1);
        } else if kind.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            out.push(path);
        }
    }
}

fn valid_sidecar_rom(rom: &str) -> bool {
    let path = Path::new(rom);
    rom.starts_with("Games/")
        && !rom.contains('\\')
        && !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn push_error(report: &mut MigrationReport, error: String) {
    if report.errors.len() < 20 {
        report.errors.push(error);
    }
}

pub fn intro_path(root: &Path, cart: &Cart) -> Option<PathBuf> {
    read(root, cart.platform, &cart.rom, &cart.stem)
        .and_then(|meta| meta.intro_path(root))
        .or_else(|| conventional(root, cart, "txt"))
}

pub fn video_path(root: &Path, cart: &Cart) -> Option<PathBuf> {
    read(root, cart.platform, &cart.rom, &cart.stem)
        .and_then(|meta| meta.video_path(root))
        .or_else(|| conventional(root, cart, "mp4"))
}

pub fn guide_path(root: &Path, cart: &Cart) -> Option<PathBuf> {
    read(root, cart.platform, &cart.rom, &cart.stem)
        .and_then(|meta| meta.guide_path(root))
        .or_else(|| {
            let dir = root
                .join("Guides")
                .join(cart.platform.dir_name());
            ["txt", "epub"].into_iter().find_map(|extension| {
                let path = dir.join(format!("{}.{extension}", cart.stem));
                path.is_file()
                    .then_some(path)
                    .or_else(|| named_file(&dir, &cart.stem, extension))
            })
        })
}

pub fn cover_path(root: &Path, cart: &Cart) -> Option<PathBuf> {
    read(root, cart.platform, &cart.rom, &cart.stem)
        .and_then(|meta| meta.cover_path(root))
        .or_else(|| cart.label.clone())
        .or_else(|| conventional(root, cart, "png"))
}

/// The game's cheat file, or the conventional path used by the built-in
/// cheat menu.  A custom DB path wins; a missing custom file falls back to the
/// old convention so existing cheat layouts keep working.
pub fn cheat_path(root: &Path, cart: &Cart, core: &str) -> Option<PathBuf> {
    read(root, cart.platform, &cart.rom, &cart.stem)
        .and_then(|meta| meta.cheat_path(root))
        .or_else(|| {
            let dir = root
                .join("Cheats")
                .join(cart.platform.dir_name())
                .join(core);
            let path = dir.join(format!("{}.cht", cart.stem));
            path.is_file()
                .then_some(path)
                .or_else(|| named_file(&dir, &cart.stem, "cht"))
        })
}


fn named_file(dir: &Path, stem: &str, extension: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_stem()
                    .is_some_and(|name| name.eq_ignore_ascii_case(stem))
                && path
                    .extension()
                    .is_some_and(|extension_name| extension_name.eq_ignore_ascii_case(extension))
        })
}

pub fn relative(root: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    // Windows temp paths can arrive once canonicalized (`\\?\C:\...`) and
    // once as the original root; normalize only on the fallback path so Linux
    // scans do not pay for an extra canonicalize per cart.
    if let (Ok(root), Ok(path)) = (root.canonicalize(), path.canonicalize()) {
        if let Ok(rel) = path.strip_prefix(&root) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    path.to_string_lossy().replace('\\', "/")
}


fn stem_of(rom: &str) -> String {
    Path::new(rom)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn io_from_sql(error: rusqlite::Error) -> std::io::Error {
    std::io::Error::other(error)
}

pub const LABELS_DIR: &str = "Labels";
pub const INFO_DIR: &str = "Info";

/// 卡带副产物目录：贴纸在 Labels，简介与演示在 Info。
pub fn asset_dir(extension: &str) -> &'static str {
    match extension {
        "txt" | "mp4" => INFO_DIR,
        _ => LABELS_DIR,
    }
}

fn conventional(root: &Path, cart: &Cart, extension: &str) -> Option<PathBuf> {
    let path = root
        .join(asset_dir(extension))
        .join(cart.platform.dir_name())
        .join(format!("{}.{}", cart.stem, extension));
    path.is_file().then_some(path)
}

fn existing(root: &Path, path: PathBuf) -> Option<String> {
    path.is_file().then(|| relative(root, &path))
}

fn relative_option(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|_| relative(root, path))
}

fn file_stamp(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (None, None);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos() as u64);
    (Some(meta.len()), mtime)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn outline_name(outline: Outline) -> &'static str {
    match outline {
        Outline::Auto => "auto",
        Outline::Notched => "notched",
        Outline::Rounded => "rounded",
    }
}

fn outline_from(value: &str) -> Outline {
    match value {
        "notched" => Outline::Notched,
        "rounded" => Outline::Rounded,
        _ => Outline::Auto,
    }
}

fn finish_name(finish: ShellFinish) -> &'static str {
    match finish {
        ShellFinish::Solid => "solid",
        ShellFinish::Clear => "clear",
        ShellFinish::Glitter => "glitter",
    }
}

fn finish_from(value: &str) -> ShellFinish {
    match value {
        "clear" => ShellFinish::Clear,
        "glitter" => ShellFinish::Glitter,
        _ => ShellFinish::Solid,
    }
}
