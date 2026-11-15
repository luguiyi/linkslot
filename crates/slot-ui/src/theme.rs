use std::sync::OnceLock;

use slot_store::Theme;

/// The active palette.  It is set once during boot, before any face is
/// rasterised; changing `linkSlot/Config/theme.txt` therefore takes effect after a
/// restart, which keeps every cached texture consistent.
static THEME: OnceLock<Theme> = OnceLock::new();

pub fn set_theme(theme: Theme) {
    let _ = THEME.set(theme);
}

pub fn current() -> Theme {
    THEME.get().copied().unwrap_or_default()
}
