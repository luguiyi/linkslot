//! Durable local file operations: atomic writes, temp/backup directories,
//! conflict copies, crash recovery and the cross-process sync lock.
//!
//! FAT32 has no journaled rename, so every multi-file replacement is staged
//! the project-book way:
//!
//! 1. download + verify into `.linkslot-tmp-<uuid>/` (same volume),
//! 2. move the old whitelist into `.linkslot-backup-<uuid>/`,
//! 3. write `.linkslot-journal` inside the backup directory listing the files
//!    this pull may install (after the originals are safe, before any new file
//!    is moved in),
//! 4. move the verified files into place (and, for a file-level diff pull, move
//!    the unchanged originals straight back),
//! 5. delete the backup only after everything succeeded.
//!
//! A leftover `.linkslot-backup-*` means an interrupted pull; the next pull
//! restores it before doing anything else. The journal lets the restore tell a
//! half-installed new file (remove it) apart from an original that a crash left
//! in the unit directory before it could be moved into the backup (keep it).

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{fs_error, ErrorKind, LinkError, Result};
use crate::util;

pub const TMP_PREFIX: &str = ".linkslot-tmp-";
pub const BACKUP_PREFIX: &str = ".linkslot-backup-";
/// Journal file inside a backup directory (never a whitelist file name).
pub const JOURNAL_NAME: &str = ".linkslot-journal";
pub const CONFLICT_MARKER: &str = ".conflict.";
pub const LOCK_REL: &str = "linkSlot/Config/linkSlot.lock";
pub const LOCK_STALE_SECS: u64 = 600;

/// Private per-game work directory for the battery save (never `Saves/`).
pub const SAV_WORK_DIR_REL: &str = "linkSlot/Config/work";
pub const SAV_WORK_PREFIX: &str = "sav-";
/// Downloaded battery file before its atomic rename into `Saves/`.
pub const SAV_INCOMING: &str = "battery.sav.new";

const NAME_MAX: usize = 255;
const COPY_BUF: usize = 64 * 1024;

/// Write `bytes` to `path` atomically (same-directory temp + fsync + rename +
/// directory fsync).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = temp_path(path);
    let result = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(fs_error(format!("写入 {} 失败", path.display()), err));
    }
    sync_dir(path);
    Ok(())
}

/// Create a file or replace it wholesale, then fsync it (used for downloaded
/// files inside the temp directory).
pub fn write_file_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        File::create(path).map_err(|err| fs_error(format!("无法创建 {}", path.display()), err))?;
    file.write_all(bytes)
        .map_err(|err| fs_error(format!("无法写入 {}", path.display()), err))?;
    file.sync_all()
        .map_err(|err| fs_error(format!("无法同步 {}", path.display()), err))?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let suffix = format!(".{}.tmp", util::unique_suffix());
    let mut room = NAME_MAX.saturating_sub(suffix.len() + 1).min(name.len());
    while room > 0 && !name.is_char_boundary(room) {
        room -= 1;
    }
    let tmp = format!(".{}{suffix}", &name[..room]);
    match path.parent() {
        Some(dir) => dir.join(tmp),
        None => PathBuf::from(tmp),
    }
}

pub(crate) fn sync_dir(path: &Path) {
    if let Some(dir) = path.parent() {
        if let Ok(handle) = File::open(dir) {
            let _ = handle.sync_all();
        }
    }
}

/// A temporary directory next to its target, removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn create(parent: &Path, prefix: &str) -> Result<TempDir> {
        std::fs::create_dir_all(parent)
            .map_err(|err| fs_error(format!("无法创建目录 {}", parent.display()), err))?;
        for _ in 0..8 {
            let path = parent.join(format!("{prefix}{}", util::unique_suffix()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(fs_error(
                        format!("无法创建临时目录 {}", path.display()),
                        err,
                    ));
                }
            }
        }
        Err(LinkError::fs("无法创建唯一的临时目录"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        write_file_sync(&self.path.join(name), bytes)
    }

    pub fn discard(self) {
        drop(self);
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Crash-safety backup of the local whitelist, restored on drop unless
/// committed. Kept on disk if the process dies (next pull recovers it).
pub struct BackupDir {
    unit_dir: PathBuf,
    path: PathBuf,
    done: bool,
}

impl BackupDir {
    pub fn create(unit_dir: &Path) -> Result<BackupDir> {
        std::fs::create_dir_all(unit_dir)
            .map_err(|err| fs_error(format!("无法创建目录 {}", unit_dir.display()), err))?;
        for _ in 0..8 {
            let path = unit_dir.join(format!("{BACKUP_PREFIX}{}", util::unique_suffix()));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(BackupDir {
                        unit_dir: unit_dir.to_path_buf(),
                        path,
                        done: false,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(fs_error(
                        format!("无法创建备份目录 {}", path.display()),
                        err,
                    ));
                }
            }
        }
        Err(LinkError::fs("无法创建唯一的备份目录"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Move `unit_dir/<name>` into the backup directory.
    pub fn move_in(&mut self, name: &str) -> Result<()> {
        let from = self.unit_dir.join(name);
        if !from.exists() {
            return Ok(());
        }
        let to = self.path.join(name);
        move_file(&from, &to)
    }

    /// Record the file names this pull may install into the unit directory.
    ///
    /// Call it after the whole old whitelist has been moved into the backup and
    /// before the first new file is moved in. A crash restores every backed-up
    /// original; journal names that have no backed-up original (files that did
    /// not exist locally) are removed as half-installed new content, while
    /// whitelist files that a crash left in the unit directory *before* the
    /// backup completed are kept untouched.
    pub fn write_journal(&self, names: &[String]) -> Result<()> {
        let mut text = String::new();
        for name in names {
            text.push_str(name);
            text.push('\n');
        }
        write_file_sync(&self.path.join(JOURNAL_NAME), text.as_bytes())?;
        sync_dir(&self.path.join(JOURNAL_NAME));
        Ok(())
    }

    /// Success: everything is in place, throw the backup away.
    pub fn commit(mut self) -> Result<()> {
        let result = match std::fs::remove_dir_all(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(fs_error(
                format!("无法删除备份目录 {}", self.path.display()),
                err,
            )),
        };
        self.done = true;
        result
    }
}

impl Drop for BackupDir {
    fn drop(&mut self) {
        if !self.done {
            let _ = restore_dir(&self.unit_dir, &self.path);
            self.done = true;
        }
    }
}

/// Names the interrupted pull may have installed, from the backup journal. A
/// missing/unreadable journal means "nothing installed" (the crash happened
/// while the old whitelist was still being moved into the backup).
fn read_journal(backup: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(bytes) = std::fs::read(backup.join(JOURNAL_NAME)) else {
        return names;
    };
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            names.insert(line.to_string());
        }
    }
    names
}

/// Restore one `.linkslot-backup-*` directory into the unit directory.
///
/// The journal written by [`BackupDir::write_journal`] lists the files this pull
/// may have installed in the unit directory. Only backed-up originals are
/// restored, and only journal-listed files that never reached the backup (new
/// names) are removed; whitelist files that a crash left in the unit directory
/// before they could be moved into the backup are untouched. Without a journal
/// (crash during the move-in loop) nothing is removed: the originals are simply
/// left where they are.
fn restore_dir(unit_dir: &Path, backup: &Path) -> Result<()> {
    let mut backed_up: HashSet<String> = HashSet::new();
    let entries = match std::fs::read_dir(backup) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(fs_error(format!("无法读取 {}", backup.display()), err)),
    };
    for entry in entries {
        let entry = entry.map_err(|err| fs_error(format!("无法读取 {}", backup.display()), err))?;
        if let Some(name) = entry.file_name().to_str() {
            if name != JOURNAL_NAME {
                backed_up.insert(name.to_string());
            }
        }
    }
    let installed = read_journal(backup);

    // Remove only half-installed new content: a journal name with no backed-up
    // original (a file that did not exist locally before this pull). A same-name
    // replacement is handled by the restore loop below; a name that is neither
    // backed up nor journaled is an original the crash left in place.
    for (name, path) in crate::unit::local_replaceable_files(unit_dir)? {
        if !backed_up.contains(&name) && installed.contains(&name) {
            let _ = std::fs::remove_file(&path);
        }
    }

    for name in &backed_up {
        let from = backup.join(name);
        let to = unit_dir.join(name);
        if to.exists() {
            let _ = std::fs::remove_file(&to);
        }
        move_file(&from, &to)?;
    }

    match std::fs::remove_dir_all(backup) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(fs_error(format!("无法删除 {}", backup.display()), err)),
    }
}

/// Recover every leftover `.linkslot-backup-*` in `unit_dir` (interrupted
/// pull). Returns the number of recovered directories.
pub fn recover_residual_backups(unit_dir: &Path) -> Result<usize> {
    let entries = match std::fs::read_dir(unit_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(fs_error(
                format!("无法读取目录 {}", unit_dir.display()),
                err,
            ));
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| fs_error(format!("无法读取 {}", unit_dir.display()), err))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(BACKUP_PREFIX) && entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
        {
            dirs.push(entry.path());
        }
    }
    for dir in &dirs {
        restore_dir(unit_dir, dir)?;
    }
    Ok(dirs.len())
}

/// Remove stale `.linkslot-tmp-*` directories. Returns the number removed.
pub fn cleanup_residual_temps(unit_dir: &Path) -> Result<usize> {
    let entries = match std::fs::read_dir(unit_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(fs_error(
                format!("无法读取目录 {}", unit_dir.display()),
                err,
            ));
        }
    };
    let mut count = 0usize;
    for entry in entries {
        let entry =
            entry.map_err(|err| fs_error(format!("无法读取 {}", unit_dir.display()), err))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(TMP_PREFIX)
            && entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && std::fs::remove_dir_all(entry.path()).is_ok()
        {
            count += 1;
        }
    }
    Ok(count)
}

/// Copy the local whitelist into `<parent>/<game>.conflict.<stamp>/` so a pull
/// never silently discards unsynced local saves.
pub fn conflict_copy(
    unit_dir: &Path,
    parent: &Path,
    game: &str,
    names: &[String],
) -> Result<PathBuf> {
    conflict_copy_stamped(unit_dir, parent, game, names, &util::stamp_now())
}

/// The same copy, with the timestamp supplied by the caller.  Tests pin it so
/// the `-2`, `-3` suffix rule does not depend on where a second boundary falls.
fn conflict_copy_stamped(
    unit_dir: &Path,
    parent: &Path,
    game: &str,
    names: &[String],
    stamp: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(parent)
        .map_err(|err| fs_error(format!("无法创建目录 {}", parent.display()), err))?;
    let mut target = parent.join(format!("{game}{CONFLICT_MARKER}{stamp}"));
    for index in 2u32.. {
        if !target.exists() {
            break;
        }
        target = parent.join(format!("{game}{CONFLICT_MARKER}{stamp}-{index}"));
    }
    std::fs::create_dir(&target)
        .map_err(|err| fs_error(format!("无法创建冲突副本 {}", target.display()), err))?;
    let result = (|| -> Result<()> {
        for name in names {
            let from = unit_dir.join(name);
            if !from.is_file() {
                continue;
            }
            copy_file_sync(&from, &target.join(name))?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_dir_all(&target);
        return Err(err);
    }
    sync_dir(&target);
    Ok(target)
}

/// Verify the card is writable before a pull starts, and give an actionable
/// error instead of failing halfway through the download.
pub fn probe_writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|err| fs_error(format!("存储卡只读或不可写入（{}）", dir.display()), err))?;
    let probe = dir.join(format!(".linkslot-write-test-{}", util::unique_suffix()));
    let result = (|| -> std::io::Result<()> {
        let mut file = File::create(&probe)?;
        file.write_all(b"linkslot")?;
        file.sync_all()?;
        drop(file);
        std::fs::remove_file(&probe)
    })();
    result.map_err(|err| fs_error("存储卡只读或不可写入", err))
}

/// One linkSlot sync at a time (global, not per unit).
#[derive(Debug)]
pub struct UnitLock {
    path: PathBuf,
    active: bool,
}

impl UnitLock {
    pub fn acquire(root: &Path) -> Result<UnitLock> {
        let path = root.join(LOCK_REL);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| fs_error(format!("无法创建目录 {}", dir.display()), err))?;
        }
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(
                        file,
                        "pid={}\nstarted={}",
                        std::process::id(),
                        util::now_iso8601()
                    );
                    let _ = file.sync_all();
                    return Ok(UnitLock { path, active: true });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    // The Slot UI cancels a sync with SIGTERM, which does not
                    // run destructors: a lock left by a dead process must not
                    // block the next attempt for 10 minutes. On Linux check
                    // /proc/<pid>; otherwise fall back to file age.
                    let owner_dead = lock_owner(&path)
                        .map(|pid| pid != std::process::id() && !process_alive(pid))
                        .unwrap_or(false);
                    let age = util::mtime_age_secs(&path).unwrap_or(0);
                    if attempt == 0 && (owner_dead || age > LOCK_STALE_SECS) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return Err(LinkError::new(
                        ErrorKind::Busy,
                        format!("另一个同步任务正在进行（锁文件 {}）", path.display()),
                    ));
                }
                Err(err) => {
                    return Err(fs_error(format!("无法创建锁文件 {}", path.display()), err));
                }
            }
        }
        Err(LinkError::new(ErrorKind::Busy, "另一个同步任务正在进行"))
    }
}

/// PID recorded in an existing lock file, if parseable.
fn lock_owner(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|pid| pid.trim().parse().ok())
}

/// Whether a PID is still alive. Only Linux exposes /proc reliably; on other
/// hosts we conservatively treat the owner as alive and rely on file age.
fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

impl Drop for UnitLock {
    fn drop(&mut self) {
        if self.active {
            let _ = std::fs::remove_file(&self.path);
            self.active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Battery save: private work directory and single-file install
// ---------------------------------------------------------------------------
//
// `Saves/<platform>/` is shared by every game, so the directory-replacement
// protocol above must never be used there: it would move or delete other
// games' save files. The battery unit is a single file, downloaded into a
// private work directory and installed with one rename.

/// `<root>/linkSlot/Config/work/sav-<rom_sha256>/`.
///
/// The ROM digest keeps different games apart and keeps Chinese names out of
/// the path; the directory is only ever touched by that one unit.
pub fn sav_work_dir(root: &Path, digest: &str) -> PathBuf {
    root.join(SAV_WORK_DIR_REL)
        .join(format!("{SAV_WORK_PREFIX}{digest}"))
}

/// Create the unit's work directory and drop leftovers of an interrupted
/// download (`battery.sav.new`, `*.tmp`, stale temp directories).
/// Returns the directory path.
pub fn prepare_sav_work(root: &Path, digest: &str) -> Result<PathBuf> {
    let dir = sav_work_dir(root, digest);
    std::fs::create_dir_all(&dir)
        .map_err(|err| fs_error(format!("无法创建工作目录 {}", dir.display()), err))?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) => return Err(fs_error(format!("无法读取目录 {}", dir.display()), err)),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let stale = name == SAV_INCOMING
            || name.ends_with(".tmp")
            || name.starts_with(TMP_PREFIX)
            || name.starts_with('.');
        if !stale {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let result = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(err) = result {
            return Err(fs_error(format!("无法清理残留 {}", path.display()), err));
        }
    }
    Ok(dir)
}

/// `<game>.conflict.<stamp>.sav` next to `target` (with a `-2`, `-3` … suffix
/// when that name is taken).
pub fn sav_conflict_path(target: &Path, game: &str) -> PathBuf {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let stamp = util::stamp_now();
    let mut path = dir.join(format!("{game}{CONFLICT_MARKER}{stamp}.sav"));
    for index in 2u32.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("{game}{CONFLICT_MARKER}{stamp}-{index}.sav"));
    }
    path
}

/// Copy the current local battery save to its conflict name before it is
/// replaced. Returns the backup path.
pub fn backup_local_sav(target: &Path, game: &str) -> Result<PathBuf> {
    let backup = sav_conflict_path(target, game);
    copy_file_sync(target, &backup)?;
    Ok(backup)
}

/// Move the verified download onto `target` with a single rename.
///
/// Deliberately *no* copy fallback: a copy would truncate the destination
/// before its content is safe. `Config/` and `Saves/` are on the same mount
/// (documented assumption), so a failing rename is reported as an `Fs` error
/// and both the old `target` and the work file stay as they were.
pub fn install_sav(incoming: &Path, target: &Path) -> Result<()> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| fs_error(format!("无法创建目录 {}", dir.display()), err))?;
    }
    std::fs::rename(incoming, target)
        .map_err(|err| fs_error(format!("无法落位 {}", target.display()), err))?;
    sync_dir(target);
    Ok(())
}

/// Rename with a copy+delete fallback (FAT32 rename is same-directory only in
/// practice; the fallback also covers cross-device surprises).
pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => {
            sync_dir(to);
            Ok(())
        }
        Err(_) => {
            // Cross-device or unusual FAT behaviour: fall back to copy+delete.
            copy_file_sync(from, to)?;
            std::fs::remove_file(from)
                .map_err(|err| fs_error(format!("无法删除 {}", from.display()), err))?;
            Ok(())
        }
    }
}

/// Copy a file with a flush, creating the destination parent if needed.
pub fn copy_file_sync(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| fs_error(format!("无法创建目录 {}", dir.display()), err))?;
    }
    let mut src =
        File::open(from).map_err(|err| fs_error(format!("无法读取 {}", from.display()), err))?;
    let mut dst =
        File::create(to).map_err(|err| fs_error(format!("无法写入 {}", to.display()), err))?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let read = src
            .read(&mut buf)
            .map_err(|err| fs_error(format!("无法读取 {}", from.display()), err))?;
        if read == 0 {
            break;
        }
        dst.write_all(&buf[..read])
            .map_err(|err| fs_error(format!("无法写入 {}", to.display()), err))?;
    }
    dst.sync_all()
        .map_err(|err| fs_error(format!("无法同步 {}", to.display()), err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("linkslot-store-{tag}-{}", util::unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let root = temp_root("atomic");
        let path = root.join("config.toml");
        atomic_write(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        atomic_write(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let root = temp_root("tmp");
        let path;
        {
            let tmp = TempDir::create(&root, TMP_PREFIX).unwrap();
            tmp.write("a.state", b"abc").unwrap();
            path = tmp.path().to_path_buf();
            assert_eq!(std::fs::read(path.join("a.state")).unwrap(), b"abc");
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(TMP_PREFIX));
        }
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_restores_on_drop_after_partial_replace() {
        let root = temp_root("backup");
        let unit = root.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"old-resume").unwrap();
        std::fs::write(unit.join("2026-10-20_10-00-00.state"), b"old-ring").unwrap();
        std::fs::write(unit.join("notes.txt"), b"keep-me").unwrap();
        let new_state = "2026-10-21_10-00-00.state".to_string();
        {
            let mut backup = BackupDir::create(&unit).unwrap();
            backup.move_in("resume.state").unwrap();
            backup.move_in("2026-10-20_10-00-00.state").unwrap();
            backup
                .write_journal(&["resume.state".to_string(), new_state.clone()])
                .unwrap();
            // Simulate a half-finished pull.
            std::fs::write(unit.join("resume.state"), b"new-resume").unwrap();
            std::fs::write(unit.join(&new_state), b"partial").unwrap();
            // Dropped without commit -> restore.
        }
        assert_eq!(
            std::fs::read(unit.join("resume.state")).unwrap(),
            b"old-resume"
        );
        assert_eq!(
            std::fs::read(unit.join("2026-10-20_10-00-00.state")).unwrap(),
            b"old-ring"
        );
        assert!(!unit.join(&new_state).exists());
        assert_eq!(std::fs::read(unit.join("notes.txt")).unwrap(), b"keep-me");
        assert_eq!(recover_residual_backups(&unit).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_keeps_originals_left_in_unit_by_a_crash_during_move_in() {
        // A crash while moving the old whitelist into the backup (before the
        // journal exists) must not delete the files that were not moved yet.
        let root = temp_root("partial-move");
        let unit = root.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"old-resume").unwrap();
        std::fs::write(unit.join("2026-10-20_10-00-00.state"), b"old-ring").unwrap();
        std::fs::write(unit.join("notes.txt"), b"keep-me").unwrap();
        let mut backup = BackupDir::create(&unit).unwrap();
        backup.move_in("resume.state").unwrap();
        // Crash here: no journal yet, the ring file never made it to backup.
        std::mem::forget(backup);

        assert_eq!(recover_residual_backups(&unit).unwrap(), 1);
        assert_eq!(
            std::fs::read(unit.join("resume.state")).unwrap(),
            b"old-resume"
        );
        assert_eq!(
            std::fs::read(unit.join("2026-10-20_10-00-00.state")).unwrap(),
            b"old-ring",
            "a whitelist file left in the unit by a crash during the move-in loop must be kept"
        );
        assert_eq!(std::fs::read(unit.join("notes.txt")).unwrap(), b"keep-me");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_after_diff_pull_move_back_keeps_unchanged_files() {
        // A file-level diff pull moves unchanged files straight back out of the
        // backup. A later restore must not treat them as half-pulled new files.
        let root = temp_root("diff-move-back");
        let unit = root.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"old-resume").unwrap();
        std::fs::write(unit.join("2026-10-20_10-00-00.state"), b"unchanged").unwrap();
        let unchanged = "2026-10-20_10-00-00.state".to_string();

        let mut backup = BackupDir::create(&unit).unwrap();
        backup.move_in("resume.state").unwrap();
        backup.move_in(&unchanged).unwrap();
        backup.write_journal(&["resume.state".to_string()]).unwrap();
        // The pull installs the changed resume and moves the unchanged ring
        // file straight back, then the device loses power before commit.
        std::fs::write(unit.join("resume.state"), b"new-resume").unwrap();
        std::fs::rename(backup.path().join(&unchanged), unit.join(&unchanged)).unwrap();
        std::mem::forget(backup);

        assert_eq!(recover_residual_backups(&unit).unwrap(), 1);
        assert_eq!(
            std::fs::read(unit.join("resume.state")).unwrap(),
            b"old-resume"
        );
        assert_eq!(
            std::fs::read(unit.join(&unchanged)).unwrap(),
            b"unchanged",
            "the moved-back unchanged file must survive recovery"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_after_diff_pull_before_move_back_restores_originals() {
        let root = temp_root("diff-before-move-back");
        let unit = root.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"old-resume").unwrap();
        std::fs::write(unit.join("2026-10-20_10-00-00.state"), b"unchanged").unwrap();
        let unchanged = "2026-10-20_10-00-00.state".to_string();

        let mut backup = BackupDir::create(&unit).unwrap();
        backup.move_in("resume.state").unwrap();
        backup.move_in(&unchanged).unwrap();
        backup.write_journal(&["resume.state".to_string()]).unwrap();
        std::fs::write(unit.join("resume.state"), b"new-resume").unwrap();
        // Crash before the unchanged file was moved back.
        std::mem::forget(backup);

        assert_eq!(recover_residual_backups(&unit).unwrap(), 1);
        assert_eq!(
            std::fs::read(unit.join("resume.state")).unwrap(),
            b"old-resume"
        );
        assert_eq!(std::fs::read(unit.join(&unchanged)).unwrap(), b"unchanged");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_commit_discards_originals() {
        let root = temp_root("commit");
        let unit = root.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"old").unwrap();
        {
            let mut backup = BackupDir::create(&unit).unwrap();
            backup.move_in("resume.state").unwrap();
            std::fs::write(unit.join("resume.state"), b"new").unwrap();
            backup.commit().unwrap();
        }
        assert_eq!(std::fs::read(unit.join("resume.state")).unwrap(), b"new");
        assert_eq!(recover_residual_backups(&unit).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recovers_interrupted_pull() {
        let root = temp_root("recover");
        let unit = root.join("unit");
        std::fs::create_dir_all(unit.join(format!("{BACKUP_PREFIX}dead"))).unwrap();
        std::fs::write(
            unit.join(format!("{BACKUP_PREFIX}dead/resume.state")),
            b"old",
        )
        .unwrap();
        std::fs::write(unit.join("resume.state"), b"new").unwrap();
        std::fs::write(
            unit.join("resume-refused-2026-10-20_10-00-00.state"),
            b"keep",
        )
        .unwrap();
        assert_eq!(recover_residual_backups(&unit).unwrap(), 1);
        assert_eq!(std::fs::read(unit.join("resume.state")).unwrap(), b"old");
        assert!(unit
            .join("resume-refused-2026-10-20_10-00-00.state")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cleans_residual_temp_dirs() {
        let root = temp_root("clean");
        std::fs::create_dir_all(root.join(format!("{TMP_PREFIX}x"))).unwrap();
        std::fs::create_dir_all(root.join("keep")).unwrap();
        assert_eq!(cleanup_residual_temps(&root).unwrap(), 1);
        assert!(!root.join(format!("{TMP_PREFIX}x")).exists());
        assert!(root.join("keep").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn conflict_copy_is_unique_and_complete() {
        let root = temp_root("conflict");
        let unit = root.join("unit");
        let parent = root.join("parent");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("resume.state"), b"r").unwrap();
        std::fs::write(unit.join("2026-10-20_10-00-00.state"), b"s").unwrap();
        std::fs::write(unit.join("resume-refused-2026-10-20_10-00-00.state"), b"x").unwrap();
        let names = vec![
            "resume.state".to_string(),
            "2026-10-20_10-00-00.state".to_string(),
        ];
        // One fixed stamp: two copies made in the same second must not collide,
        // and the second has to pick up the `-2` suffix.
        let stamp = "2026-10-20_10-00-00";
        let first = conflict_copy_stamped(&unit, &parent, "合金弹头", &names, stamp).unwrap();
        assert!(first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("合金弹头.conflict."));
        assert_eq!(std::fs::read(first.join("resume.state")).unwrap(), b"r");
        assert!(!first
            .join("resume-refused-2026-10-20_10-00-00.state")
            .exists());
        let second = conflict_copy_stamped(&unit, &parent, "合金弹头", &names, stamp).unwrap();
        assert_ne!(first, second);
        assert!(second.file_name().unwrap().to_string_lossy().contains("-2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn probe_writable_detects_ok_dir() {
        let root = temp_root("probe");
        probe_writable(&root.join("sub")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_is_exclusive_and_drop_releases() {
        let root = temp_root("lock");
        {
            let _lock = UnitLock::acquire(&root).unwrap();
            let err = UnitLock::acquire(&root).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Busy);
        }
        UnitLock::acquire(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let root = temp_root("stale");
        let path = root.join(LOCK_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"pid=1").unwrap();
        let old =
            std::time::SystemTime::now() - std::time::Duration::from_secs(LOCK_STALE_SECS + 60);
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(old).unwrap();
        drop(file);
        let _lock = UnitLock::acquire(&root).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_owner_is_parsed_from_content() {
        let root = temp_root("owner");
        let path = root.join("linkSlot.lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"pid=4242\nstarted=2026-10-20T10:00:00Z\n").unwrap();
        assert_eq!(lock_owner(&path), Some(4242));
        std::fs::write(&path, b"pid=notanumber\n").unwrap();
        assert_eq!(lock_owner(&path), None);
        std::fs::write(&path, b"garbage\n").unwrap();
        assert_eq!(lock_owner(&path), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_pid_lock_is_reclaimed_immediately() {
        let root = temp_root("deadowner");
        let path = root.join(LOCK_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // 999999 is not a valid live PID on the device.
        std::fs::write(&path, b"pid=999999\nstarted=2026-10-20T10:00:00Z\n").unwrap();
        let _lock = UnitLock::acquire(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- battery save install --------------------------------------------

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn sav_work_dir_is_digest_scoped_and_outside_saves() {
        let root = Path::new("/mnt/sdcard");
        let dir = sav_work_dir(root, DIGEST);
        assert_eq!(
            dir,
            PathBuf::from(format!("/mnt/sdcard/linkSlot/Config/work/sav-{DIGEST}"))
        );
        assert!(!dir.to_string_lossy().contains("合金弹头"));
        // Another ROM gets its own directory.
        let other = sav_work_dir(root, &"b".repeat(64));
        assert_ne!(dir, other);
    }

    #[test]
    fn prepare_sav_work_creates_and_cleans_leftovers() {
        let root = temp_root("sav-work");
        let dir = prepare_sav_work(&root, DIGEST).unwrap();
        assert!(dir.is_dir());
        // Leftovers from an interrupted download.
        std::fs::write(dir.join(SAV_INCOMING), b"half").unwrap();
        std::fs::write(dir.join("battery.sav.tmp"), b"tmp").unwrap();
        std::fs::create_dir_all(dir.join(format!("{TMP_PREFIX}old"))).unwrap();
        // Something a human put there must survive.
        std::fs::write(dir.join("notes.txt"), b"keep").unwrap();

        let dir = prepare_sav_work(&root, DIGEST).unwrap();
        assert!(!dir.join(SAV_INCOMING).exists());
        assert!(!dir.join("battery.sav.tmp").exists());
        assert!(!dir.join(format!("{TMP_PREFIX}old")).exists());
        assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_local_sav_copies_next_to_the_target_and_uniquifies() {
        let root = temp_root("sav-backup");
        let saves = root.join("Saves/GBA");
        std::fs::create_dir_all(&saves).unwrap();
        let target = saves.join("合金弹头.sav");
        std::fs::write(&target, b"old-sram").unwrap();

        let first = backup_local_sav(&target, "合金弹头").unwrap();
        assert_eq!(first.parent().unwrap(), saves);
        let name = first.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("合金弹头.conflict."), "{name}");
        assert!(name.ends_with(".sav"), "{name}");
        assert_eq!(std::fs::read(&first).unwrap(), b"old-sram");
        // The original is untouched by the backup step.
        assert_eq!(std::fs::read(&target).unwrap(), b"old-sram");

        let second = backup_local_sav(&target, "合金弹头").unwrap();
        assert_ne!(first, second);
        assert!(second.file_name().unwrap().to_string_lossy().contains("-2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_sav_replaces_the_target_and_consumes_the_incoming_file() {
        let root = temp_root("sav-install");
        let saves = root.join("Saves/GBA");
        std::fs::create_dir_all(&saves).unwrap();
        let target = saves.join("合金弹头.sav");
        std::fs::write(&target, b"old-sram").unwrap();
        // Another game's save in the same shared directory.
        let other = saves.join("Other.sav");
        std::fs::write(&other, b"other-sram").unwrap();

        let incoming = root.join("work/battery.sav.new");
        std::fs::create_dir_all(incoming.parent().unwrap()).unwrap();
        std::fs::write(&incoming, b"new-sram").unwrap();

        install_sav(&incoming, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-sram");
        assert!(!incoming.exists(), "the work file must be consumed");
        assert_eq!(std::fs::read(&other).unwrap(), b"other-sram");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_sav_failure_leaves_the_old_file_untouched() {
        let root = temp_root("sav-install-fail");
        let saves = root.join("Saves/GBA");
        std::fs::create_dir_all(&saves).unwrap();
        let target = saves.join("合金弹头.sav");
        std::fs::write(&target, b"old-sram").unwrap();

        // Missing download: the rename fails and the target keeps its bytes.
        let incoming = root.join("work/battery.sav.new");
        let err = install_sav(&incoming, &target).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Fs);
        assert_eq!(std::fs::read(&target).unwrap(), b"old-sram");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_sav_creates_a_missing_saves_directory() {
        let root = temp_root("sav-install-new");
        let target = root.join("Saves/GBA/合金弹头.sav");
        let incoming = root.join("work/battery.sav.new");
        std::fs::create_dir_all(incoming.parent().unwrap()).unwrap();
        std::fs::write(&incoming, b"first-sram").unwrap();
        install_sav(&incoming, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first-sram");
        let _ = std::fs::remove_dir_all(&root);
    }
}
