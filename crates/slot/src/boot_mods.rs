//! Device-startup scheduling for `after_boot` mod actions.
//!
//! The device app owns the timer; the CLI only executes one selected action.
//! Delays are measured from Linux boot (`/proc/uptime`), so an app restart can
//! catch up on actions that became due while it was not running.  A tiny state
//! file records actions already completed for the current boot id.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use slot_ui::Millis;

use crate::sync_job::SyncRun;
use slot_mods::{ModError, Timing};

/// Hidden under `Config/`, which the Web file browser does not list.
const STATE_REL: &str = "linkSlot/Config/boot-mods.json";
const SCHEMA: u32 = 1;
const MAX_ATTEMPTS: u8 = 3;
const RETRY_DELAY_MS: Millis = 60_000;

/// One due/after-boot action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootAction {
    pub key: String,
    pub mod_id: String,
    pub action_id: String,
    pub delay_minutes: u32,
    pub order: i32,
    /// Relative to app start: 0 means "run when the app becomes idle".
    pub due_ms: Millis,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BootState {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    boot_id: String,
    #[serde(default)]
    ran: Vec<String>,
}

/// The scheduler snapshot loaded once at app startup.
pub struct BootSchedule {
    root: PathBuf,
    boot_id: String,
    actions: Vec<BootAction>,
    ran: BTreeSet<String>,
    in_flight: Option<String>,
    attempts: HashMap<String, u8>,
    retry_at: HashMap<String, Millis>,
}

impl BootSchedule {
    /// Load enabled `after_boot` actions. Never fails: a broken card simply
    /// produces an empty schedule and a diagnostic line.
    pub fn load(root: &Path) -> BootSchedule {
        let (boot_id, uptime_ms) = read_boot_clock();
        Self::load_with(root, &boot_id, uptime_ms)
    }

    /// Testable loader with an injected boot id/uptime.
    pub fn load_with(root: &Path, boot_id: &str, uptime_ms: u64) -> BootSchedule {
        let mut schedule = BootSchedule {
            root: root.to_path_buf(),
            boot_id: boot_id.to_string(),
            actions: Vec::new(),
            ran: BTreeSet::new(),
            in_flight: None,
            attempts: HashMap::new(),
            retry_at: HashMap::new(),
        };
        schedule.load_state();
        schedule.scan_mods(uptime_ms);
        schedule
    }

    /// True when no enabled after_boot action exists at all.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Return the next action that is due, not running, not completed, and not
    /// waiting for a retry.
    pub fn next_due(&self, now: Millis) -> Option<BootAction> {
        if self.in_flight.is_some() {
            return None;
        }
        self.actions.iter().find_map(|action| {
            if self.ran.contains(&action.key) {
                return None;
            }
            if self.retry_at.get(&action.key).is_some_and(|at| now < *at) {
                return None;
            }
            (action.due_ms <= now).then(|| action.clone())
        })
    }

    pub fn start(&mut self, action: &BootAction) {
        self.in_flight = Some(action.key.clone());
    }

    /// Drop the in-flight marker without recording a result (shutdown/restart).
    pub fn abandon_in_flight(&mut self) {
        self.in_flight = None;
    }

    /// Record the result. Success marks the action done for this boot; failure
    /// retries up to `MAX_ATTEMPTS`, then marks it done to avoid a loop.
    pub fn finish(&mut self, action: &BootAction, success: bool, now: Millis) {
        self.in_flight = None;
        if success {
            self.ran.insert(action.key.clone());
            self.attempts.remove(&action.key);
            self.retry_at.remove(&action.key);
            self.save_state();
            return;
        }
        let attempts = self.attempts.entry(action.key.clone()).or_insert(0);
        *attempts = attempts.saturating_add(1);
        let exhausted = *attempts >= MAX_ATTEMPTS;
        if exhausted {
            self.ran.insert(action.key.clone());
            self.attempts.remove(&action.key);
            self.retry_at.remove(&action.key);
            eprintln!(
                "slot: boot mods: {}/{} failed {} times; giving up for this boot",
                action.mod_id, action.action_id, MAX_ATTEMPTS
            );
            self.save_state();
        } else {
            self.retry_at
                .insert(action.key.clone(), now.saturating_add(RETRY_DELAY_MS));
            eprintln!(
                "slot: boot mods: {}/{} failed; retry in {} s",
                action.mod_id,
                action.action_id,
                RETRY_DELAY_MS / 1000
            );
        }
    }

    fn load_state(&mut self) {
        let path = self.root.join(STATE_REL);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Ok(state) = serde_json::from_slice::<BootState>(&bytes) else {
            eprintln!("slot: boot mods: ignoring corrupt {}", path.display());
            return;
        };
        if state.boot_id == self.boot_id {
            self.ran = state.ran.into_iter().collect();
        }
    }

    fn scan_mods(&mut self, uptime_ms: u64) {
        let entries = match slot_mods::list(&self.root) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("slot: boot mods: cannot read mods: {error}");
                return;
            }
        };
        for entry in entries {
            if !entry.enabled || entry.error.is_some() {
                continue;
            }
            let detail = match slot_mods::read_mod(&self.root, &entry.id) {
                Ok(detail) => detail,
                Err(error) => {
                    if !matches!(error, ModError::NotFound(_)) {
                        eprintln!("slot: boot mods: cannot read mod {}: {error}", entry.id);
                    }
                    continue;
                }
            };
            for action in detail.actions {
                if !action.enabled
                    || action.timing != Some(Timing::AfterBoot)
                    || action.error.is_some()
                {
                    continue;
                }
                let key = format!("{}/{}", entry.id, action.id);
                if self.ran.contains(&key) {
                    continue;
                }
                let delay_ms = u64::from(action.boot_delay_minutes) * 60_000;
                self.actions.push(BootAction {
                    key,
                    mod_id: entry.id.clone(),
                    action_id: action.id.clone(),
                    delay_minutes: action.boot_delay_minutes,
                    order: action.order,
                    due_ms: delay_ms.saturating_sub(uptime_ms),
                });
            }
        }
        self.actions.sort_by(|left, right| {
            left.due_ms
                .cmp(&right.due_ms)
                .then_with(|| left.order.cmp(&right.order))
                .then_with(|| left.mod_id.cmp(&right.mod_id))
                .then_with(|| left.action_id.cmp(&right.action_id))
        });
        if !self.actions.is_empty() {
            eprintln!(
                "slot: boot mods: {} action(s) scheduled (boot {})",
                self.actions.len(),
                self.boot_id
            );
        }
    }

    fn save_state(&self) {
        let path = self.root.join(STATE_REL);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "slot: boot mods: cannot create {}: {error}",
                    parent.display()
                );
                return;
            }
        }
        let state = BootState {
            schema: SCHEMA,
            boot_id: self.boot_id.clone(),
            ran: self.ran.iter().cloned().collect(),
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&state) else {
            return;
        };
        let temp = path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temp, bytes).and_then(|_| std::fs::rename(&temp, &path))
        {
            eprintln!("slot: boot mods: cannot save {}: {error}", path.display());
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// One in-flight `mods boot` child process.
pub struct BootJob {
    pub run: Box<dyn SyncRun>,
    pub action: BootAction,
    pub started: Millis,
}

/// Read Linux boot id/uptime; on other hosts fall back to app-start semantics.
fn read_boot_clock() -> (String, u64) {
    let uptime_ms = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|value| value.parse::<f64>().ok())
        })
        .map(|seconds| (seconds.max(0.0) * 1000.0) as u64)
        .unwrap_or(0);
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            format!("fallback-{secs}")
        });
    (boot_id, uptime_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mod(root: &Path, id: &str, timing: &str, delay: u32, enabled: bool) {
        let dir = root.join("linkSlot/mods").join(id);
        std::fs::create_dir_all(dir.join("actions").join("a")).unwrap();
        std::fs::write(
            dir.join("mod.json"),
            format!(r#"{{"schema":2,"name":"{id}","enabled":true}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("actions/a/action.json"),
            format!(
                r#"{{"schema":1,"name":"a","timing":"{timing}","enabled":{enabled},"order":100,"boot_delay_minutes":{delay}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("actions/a/main.lua"), "return").unwrap();
        std::fs::write(dir.join("web/index.html"), "").ok();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_mod(dir.path(), "m", "after_boot", 5, true);
        write_mod(dir.path(), "other", "after_sync", 0, true);
        dir
    }

    #[test]
    fn schedules_after_boot_with_uptime_relative_delay() {
        let dir = fixture();
        let schedule = BootSchedule::load_with(dir.path(), "boot-a", 60_000);
        assert_eq!(schedule.actions.len(), 1);
        let action = &schedule.actions[0];
        assert_eq!(action.key, "m/a");
        assert_eq!(action.due_ms, 4 * 60_000);
        assert!(schedule.next_due(4 * 60_000 - 1).is_none());
        assert_eq!(schedule.next_due(4 * 60_000).unwrap().key, "m/a");
    }

    #[test]
    fn late_app_start_catches_up_immediately() {
        let dir = fixture();
        let schedule = BootSchedule::load_with(dir.path(), "boot-a", 6 * 60_000);
        assert_eq!(schedule.actions[0].due_ms, 0);
        assert!(schedule.next_due(0).is_some());
    }

    #[test]
    fn state_skips_done_actions_only_for_the_same_boot() {
        let dir = fixture();
        let due = 300_000;
        let mut schedule = BootSchedule::load_with(dir.path(), "boot-a", 0);
        let action = schedule.next_due(due).unwrap();
        schedule.start(&action);
        schedule.finish(&action, true, due);
        assert!(schedule.next_due(due).is_none());

        let same = BootSchedule::load_with(dir.path(), "boot-a", 0);
        assert!(same.next_due(due).is_none());
        let next_boot = BootSchedule::load_with(dir.path(), "boot-b", 0);
        assert!(next_boot.next_due(due).is_some());
    }

    #[test]
    fn failed_actions_retry_then_exhaust() {
        let dir = fixture();
        let due = 300_000;
        let mut schedule = BootSchedule::load_with(dir.path(), "boot-a", 0);
        let action = schedule.next_due(due).unwrap();
        schedule.start(&action);
        schedule.finish(&action, false, due);
        assert!(schedule.next_due(due + 30_000).is_none());
        assert!(schedule.next_due(due + RETRY_DELAY_MS).is_some());
        for attempt in 1..MAX_ATTEMPTS {
            let at = due + RETRY_DELAY_MS * u64::from(attempt);
            let action = schedule.next_due(at).unwrap();
            schedule.start(&action);
            schedule.finish(&action, false, at);
        }
        assert!(schedule.next_due(due + RETRY_DELAY_MS * 10).is_none());
        assert_eq!(schedule.ran.len(), 1);
    }

    #[test]
    fn disabled_and_non_after_boot_actions_are_ignored() {
        let dir = fixture();
        write_mod(dir.path(), "off", "after_boot", 0, false);
        let schedule = BootSchedule::load_with(dir.path(), "boot-a", 0);
        assert_eq!(schedule.actions.len(), 1);
    }

    #[test]
    fn corrupt_state_is_ignored() {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        std::fs::write(dir.path().join(STATE_REL), b"{not json").unwrap();
        let schedule = BootSchedule::load_with(dir.path(), "boot-a", 0);
        assert_eq!(schedule.actions.len(), 1);
    }
}
