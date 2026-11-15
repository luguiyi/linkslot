//! Debug logging for the link feature.
//!
//! Link logs use the existing `Logs/linkSlot.log` channel and the
//! normal debug-level switch.  Detailed lines are only written while the user
//! has Debug enabled; warnings still reach stderr (and therefore `slot.log`)
//! so a failed link is not silent at the default level.

use std::path::{Path, PathBuf};

use crate::linkcfg::LogLevel;

const CATEGORY: &str = "link";
#[cfg(any(feature = "device", test))]
const TRACE_REL: &str = "linkSlot/Logs/linkSlot.log";

fn root() -> Option<PathBuf> {
    std::env::var_os("SLOT_ROOT").map(PathBuf::from)
}

fn text(message: impl AsRef<str>) -> String {
    message.as_ref().to_owned()
}

pub(crate) fn debug(message: impl AsRef<str>) {
    if let Some(root) = root() {
        debug_to(&root, message);
    }
}

pub(crate) fn debug_to(root: &Path, message: impl AsRef<str>) {
    if !crate::diag::enabled(root, LogLevel::Debug) {
        return;
    }
    let message = text(message);
    eprintln!("slot: link: {message}");
    crate::diag::write(root, LogLevel::Debug, CATEGORY, &message);
}

/// Warn about an explicit root: used at boot, before SLOT_ROOT is set.
pub(crate) fn warn_to(root: &Path, message: impl AsRef<str>) {
    let message = text(message);
    eprintln!("slot: link: {message}");
    crate::diag::write(root, LogLevel::Warn, CATEGORY, &message);
}
pub(crate) fn warn(message: impl AsRef<str>) {
    let message = text(message);
    eprintln!("slot: link: {message}");
    if let Some(root) = root() {
        crate::diag::write(&root, LogLevel::Warn, CATEGORY, &message);
    }
}

/// Path for `slotlink.sh`'s own trace, but only while Debug is enabled.
#[cfg(any(feature = "device", test))]
pub(crate) fn trace_file(root: &Path) -> Option<PathBuf> {
    crate::diag::enabled(root, LogLevel::Debug).then(|| root.join(TRACE_REL))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_written_only_at_debug_level() {
        let root = std::env::temp_dir().join(format!("slot-link-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        crate::linkcfg::write_log_level(&root, LogLevel::Info).unwrap();
        debug_to(&root, "hidden detail");
        let log = root.join(TRACE_REL);
        assert!(!log.exists(), "an Info card got a debug link line");

        crate::linkcfg::write_log_level(&root, LogLevel::Debug).unwrap();
        debug_to(&root, "peer connected");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("[debug] [link] peer connected"), "{text}");
        assert!(trace_file(&root).is_some());

        crate::linkcfg::write_log_level(&root, LogLevel::Info).unwrap();
        assert!(trace_file(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
