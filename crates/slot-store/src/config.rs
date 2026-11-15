use std::io;
use std::path::Path;

pub const CONFIG_DIR: &str = "Config";
pub const LINKSLOT_DIR: &str = "linkSlot/Config";

const MOVED: [&str; 5] = [
    "slot.state",
    "selected_core.ini",
    "video_mode.ini",
    "cart_shell.ini",
    "theme.txt",
];

const RENAMED: [(&str, &str); 7] = [
    ("Config/selected_core.ini", crate::SELECTED_CORE_FILE),
    ("Config/video_mode.ini", "linkSlot/Config/video_mode.txt"),
    ("Config/cart_shell.ini", crate::CART_SHELL_FILE),
    ("linkSlot/Config/selected_core.ini", crate::SELECTED_CORE_FILE),
    ("linkSlot/Config/video_mode.ini", "linkSlot/Config/video_mode.txt"),
    ("linkSlot/Config/cart_shell.ini", crate::CART_SHELL_FILE),
    ("Labels/cart_shell.ini", crate::LABELS_SHELL_FILE),
];

pub fn move_config(root: &Path) -> io::Result<()> {
    let own = root.join(LINKSLOT_DIR);
    std::fs::create_dir_all(&own)?;
    for name in MOVED {
        let old = root.join("System").join(name);
        let new = own.join(name);
        if old.is_file() && !new.exists() {
            std::fs::rename(&old, &new)?;
        }
    }
    for (old, new) in RENAMED {
        let old = root.join(old);
        let new = root.join(new);
        if old.is_file() && !new.exists() {
            std::fs::rename(&old, &new)?;
        }
    }
    Ok(())
}
