//! Theme packs: an enabled mod can restyle the device without touching
//! `linkSlot/Config/theme.txt`.
//!
//! A pack is any enabled mod whose directory carries at least one of
//! `theme.txt`, `wallpaper.png`, or a `font.*` face. The lowest `order` wins;
//! when no pack applies, the built-in presets and `linkSlot/Config/theme.txt` behave
//! exactly as before. Everything here is resolved once at boot because the
//! palette and the label font are baked into cached textures.

use std::path::{Path, PathBuf};

use slot_store::Theme;

/// Font faces a pack may ship, in the order they are tried.
const FONT_FILES: [&str; 3] = ["font.otf", "font.ttf", "font.ttc"];

pub struct ThemePack {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
}

impl ThemePack {
    pub fn theme_file(&self) -> Option<PathBuf> {
        let path = self.dir.join("theme.txt");
        path.is_file().then_some(path)
    }

    pub fn wallpaper(&self) -> Option<PathBuf> {
        let path = self.dir.join("wallpaper.png");
        path.is_file().then_some(path)
    }

    pub fn font(&self) -> Option<PathBuf> {
        FONT_FILES
            .iter()
            .map(|name| self.dir.join(name))
            .find(|path| path.is_file())
    }

    /// Pack palette, falling back to the card's own `linkSlot/Config/theme.txt` when the
    /// pack ships none (a wallpaper-only or font-only pack).
    pub fn theme(&self, root: &Path) -> Theme {
        match self.theme_file().and_then(|path| std::fs::read_to_string(path).ok()) {
            Some(text) => Theme::parse(&text),
            None => Theme::read(root),
        }
    }
}

/// The pack that should style this boot, if any.
pub fn active(root: &Path) -> Option<ThemePack> {
    let mut mods = slot_mods::list(root).unwrap_or_default();
    mods.retain(|entry| entry.enabled && entry.error.is_none());
    mods.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    mods.into_iter().find_map(|entry| {
        let dir = root.join("linkSlot/mods").join(&entry.id);
        let pack = ThemePack {
            id: entry.id,
            name: entry.name,
            dir,
        };
        let themed = pack.theme_file().is_some()
            || pack.wallpaper().is_some()
            || pack.font().is_some();
        themed.then_some(pack)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("slot-theme-pack-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("linkSlot/mods")).unwrap();
        dir
    }

    fn write_mod(root: &Path, id: &str, order: i32, enabled: bool, files: &[(&str, &str)]) {
        let dir = root.join("linkSlot/mods").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mod.json"),
            format!(
                "{{\"schema\":2,\"id\":\"{id}\",\"name\":\"{id}\",\"order\":{order},\"enabled\":{enabled}}}"
            ),
        )
        .unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
    }

    #[test]
    fn lowest_order_enabled_pack_wins() {
        let root = temp_root("order");
        write_mod(&root, "theme-low", 10, true, &[("theme.txt", "accent #00ff00\n")]);
        write_mod(&root, "theme-high", 20, true, &[("theme.txt", "accent #ff0000\n")]);
        let pack = active(&root).expect("a pack");
        assert_eq!(pack.id, "theme-low");
        assert_eq!(pack.theme(&root).accent, [0x00, 0xff, 0x00]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_and_unrelated_mods_are_skipped() {
        let root = temp_root("skip");
        write_mod(&root, "theme-off", 1, false, &[("theme.txt", "accent #00ff00\n")]);
        write_mod(&root, "plain", 2, true, &[("README.md", "hello")]);
        assert!(active(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wallpaper_and_font_alone_make_a_pack() {
        let root = temp_root("assets");
        write_mod(&root, "art", 5, true, &[("wallpaper.png", "not-a-real-png")]);
        let pack = active(&root).expect("a pack");
        assert!(pack.wallpaper().is_some());
        assert!(pack.theme_file().is_none());
        // Without its own palette the pack defers to linkSlot/Config/theme.txt.
        assert_eq!(pack.theme(&root).preset, slot_store::ThemePreset::Default);
        let _ = std::fs::remove_dir_all(&root);
    }
}
