use std::path::{Path, PathBuf};

pub use slot_model::layout::{SYSTEM_ASSETS, SYSTEM_BIN, SYSTEM_CORES, SYSTEM_SHELL};

/// 每张卡在首次开机时补齐的目录。
///
/// 公共区（Games/Saves/States/Labels/...）按机种建子目录；本前端私有区统一
/// 放在 `linkSlot/` 下；运行文件放在 `System/linkSlot/` 下。
pub const DIRS: [&str; 51] = [
    // 公共区
    "BIOS",
    "Bootlogos",
    "Cheats",
    "Games",
    "Games/GBA",
    "Games/GB",
    "Games/GBC",
    "Games/FC",
    "Games/SFC",
    "Games/MD",
    "Guides",
    "Info",
    "Labels",
    "Labels/GBA",
    "Labels/GB",
    "Labels/GBC",
    "Labels/FC",
    "Labels/SFC",
    "Labels/MD",
    "Music",
    "Overlay",
    "Saves",
    "Saves/GBA",
    "Saves/GB",
    "Saves/GBC",
    "Saves/FC",
    "Saves/SFC",
    "Saves/MD",
    "States",
    "States/GBA",
    "States/GB",
    "States/GBC",
    "States/FC",
    "States/SFC",
    "States/MD",
    "Wallpapers",
    // 本前端私有区
    "linkSlot",
    "linkSlot/Config",
    "linkSlot/Fronts",
    "linkSlot/Logs",
    "linkSlot/Metadata",
    "linkSlot/mods",
    // 运行目录
    "System",
    "System/linkSlot",
    "System/linkSlot/assets",
    "System/linkSlot/assets/bezels",
    "System/linkSlot/assets/carts",
    "System/linkSlot/bin",
    "System/linkSlot/cores",
    "System/linkSlot/licenses",
    "System/linkSlot/shell",
];

pub fn ensure(root: &Path) {
    for sub in DIRS {
        let _ = std::fs::create_dir_all(root.join(sub));
    }
    if let Err(e) = slot_store::move_config(root) {
        eprintln!("slot: config: {e}");
    }
    match slot_mods::migrate_legacy_mods(root) {
        Ok(0) => {}
        Ok(moved) => eprintln!("slot: mods: migrated {moved} legacy entr{}", if moved == 1 { "y" } else { "ies" }),
        Err(e) => eprintln!("slot: mods: {e}"),
    }
}
/// Old releases shipped factory nameplates and unused cart art in
/// `System/linkSlot/assets/carts`. The built-in plates are now the default, so those leftover
/// files would override them or linger in the web browser. Remove only files
/// whose size and SHA-256 are exactly the old stock art, so a user's own
/// `slot_back_<machine>.png` is never touched.
pub fn purge_legacy_carts(root: &Path) -> usize {
    const LEGACY: &[(&str, u64, &str)] = &[
        (
            "slot_back_gba.png",
            4362,
            "8deab7a9d619d28ad6dc7de7dac361ffbd5ee17cb0822ba21127c34cbb363c43",
        ),
        (
            "slot_back_gb.png",
            3143,
            "5ee457fc6382cfee27bdf68d94f4a60f56ac14f64834e29fc19553c8f123a109",
        ),
        (
            "slot_back_gbc.png",
            3744,
            "7bbdcb49c6bf0af14f53867a0f82c8f4a996d62500588e294d0d745e4137ff22",
        ),
        (
            "slot_back_fc.png",
            2348,
            "fb57c98fad139e2451afbb001cd69d2306553057d49a695ff717189b6e52bfff",
        ),
        (
            "slot_back_recents.png",
            6940,
            "0e20f94f0a0c76f9dbd6cb732b2c62179aece7db939d074e6112c2386c788beb",
        ),
        (
            "slot_back_favorites.png",
            4585,
            "6982a5030afa93283e0880cd87a82baff18be02a700bbcd0b9b5d43e731b672d",
        ),
        // v0.8.4/v0.8.5 carried these reference images under System/linkSlot/assets/carts.
        // The frontend embeds its cart art, so an upgraded card can lose them.
        (
            "cart_sfc.png",
            65443,
            "3579b1cee3db44e0a58d7f5341bbf5a902001afc7088f0b157fc473652b63154",
        ),
        (
            "cart_md.png",
            32997,
            "55488c9bfdbbaaebbfafcd73a0ffc1ca45f61407b4261792e9651388b36869ec",
        ),
        (
            "SLOT_SYS_SFC.png",
            3203,
            "327d9a8bddc828251cc322590ce6be75668812ebf82a030ff13d8d9983723228",
        ),
    ];
    let dir = root.join("System/linkSlot/assets/carts");
    let mut removed = 0;
    for (name, size, digest) in LEGACY {
        let path = dir.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() != *size {
            continue;
        }
        let Ok(actual) = crate::rom_id::sha256_file(&path) else {
            continue;
        };
        if actual.eq_ignore_ascii_case(digest) {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

pub fn bios_dir(root: &Path) -> PathBuf {
    root.join("BIOS")
}

const BIOS_FILE: &str = "gba_bios.bin";

const BIOS_BYTES: u64 = 16 * 1024;
const BIOS_FIRST_BYTE: u8 = 0x18;

pub fn has_real_bios(root: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(bios_dir(root).join(BIOS_FILE)) else {
        return false;
    };
    if !f.metadata().is_ok_and(|m| m.len() == BIOS_BYTES) {
        return false;
    }
    let mut first = [0u8; 1];
    std::io::Read::read_exact(&mut f, &mut first).is_ok() && first[0] == BIOS_FIRST_BYTE
}

pub fn saves_dir(root: &Path) -> PathBuf {
    root.join("Saves")
}
