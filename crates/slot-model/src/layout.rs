//! 内容根目录解析（纯函数）。
//!
//! 前端与 CLI 共用同一套优先级：**显式参数 > `SLOT_ROOT` > 设备默认**。
//! 这里不读环境变量，由调用方把值传进来，保持 `slot-model` 无 I/O。

use std::path::PathBuf;

/// 设备上的卡根目录（BaseOS 挂载点）。
pub const DEVICE_ROOT: &str = "/mnt/sdcard";

/// 本前端在卡上的私有命名空间。
pub const APP_ID: &str = "linkSlot";
/// 卡根下的私有数据目录（Config、Fronts、Metadata、mods、Logs）。
pub const APP_ROOT: &str = "linkSlot";
/// `System/` 下的前端运行目录。
pub const SYSTEM_APP: &str = "System/linkSlot";

/// 运行目录的固定分组。前端、CLI 与打包脚本都从这里取名，
/// 避免路径散落在各处。
pub const SYSTEM_BIN: &str = "System/linkSlot/bin";
pub const SYSTEM_SHELL: &str = "System/linkSlot/shell";
pub const SYSTEM_CORES: &str = "System/linkSlot/cores";
pub const SYSTEM_ASSETS: &str = "System/linkSlot/assets";

/// 公共区里由本前端负责创建的目录（其余公共目录由用户自行放置）。
pub const PUBLIC_DIRS: [&str; 12] = [
    "BIOS",
    "Cheats",
    "Games",
    "Guides",
    "Info",
    "Labels",
    "Overlay",
    "Saves",
    "States",
    "Wallpapers",
    "Bootlogos",
    "Music",
];

/// 解析内容根目录。
///
/// - `explicit`：命令行 `--root` 之类的显式值；
/// - `env_root`：调用方读取的 `SLOT_ROOT`。
pub fn resolve_root(explicit: Option<PathBuf>, env_root: Option<PathBuf>) -> PathBuf {
    explicit
        .or(env_root)
        .unwrap_or_else(|| PathBuf::from(DEVICE_ROOT))
}
