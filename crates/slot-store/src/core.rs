use std::collections::HashMap;
use std::path::Path;

use slot_model::{core as model_core, CoreSpec, Platform};

pub const SELECTED_CORE_FILE: &str = "linkSlot/Config/selected_core.txt";

/// 前端可以直接选择/加载的核心。
///
/// 具体的身份与能力（动态库名、显示名、可运行机种、固定选项、颜色校正）
/// 全部来自 `slot-model` 的 [`CORES`](slot_model::CORES) 表；本枚举只负责
/// "有哪些核心"以及让 `match` 保持穷尽检查。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Core {
    #[default]
    Mgba,
    Gpsp,
    /// Famicom/NES。Deliberately absent from [`Core::ALL`]; see its docs.
    Nes,
    /// Super Famicom / SNES. `supafaust` is the libretro core on the card.
    Snes,
    /// Mega Drive / Genesis. `genesis_plus_gx` is the libretro core on the card.
    Gpgx,
    /// Super Famicom / SNES 的官方随包核心（v1.0.0 起默认）；`Snes`（supafaust）
    /// 保留为可选核心，用户自备二进制。
    Snes9x,
}

impl Core {
    /// The cores the picker board has physical sockets for. Exactly two:
    /// the board lays them out from `SOCKET_U: [f32; 2]`, so a third entry
    /// would index past the drawn sockets. [`Core::EVERY`] is the list of
    /// every core we ship; use that when iterating cores in general.
    pub const ALL: [Core; 2] = [Core::Mgba, Core::Gpsp];

    /// Every core the app can load, including ones with no picker socket
    /// (the Famicom core is chosen by the cart's platform, not by hand).
    pub const EVERY: [Core; 6] = [
        Core::Mgba,
        Core::Gpsp,
        Core::Nes,
        Core::Snes,
        Core::Gpgx,
        Core::Snes9x,
    ];

    /// 核心 id（与 `slot-model` 表一致）。
    pub fn id(self) -> &'static str {
        match self {
            Core::Mgba => "mgba",
            Core::Gpsp => "gpsp",
            Core::Nes => "fceumm",
            Core::Snes => "supafaust",
            Core::Gpgx => "genesis_plus_gx",
            Core::Snes9x => "snes9x",
        }
    }

    /// 表项；表不变量由 `tests/core.rs` 守住。
    pub fn spec(self) -> &'static CoreSpec {
        model_core(self.id()).expect("every Core variant must exist in slot_model::CORES")
    }

    /// The cores offered by the manual core picker for a platform.
    ///
    /// 这是**选择器策略**（只有 GBA 有两槽），不是核心能力；能力见
    /// [`Core::spec().machines`](slot_model::CoreSpec::machines)。
    pub fn options_for(platform: Platform) -> &'static [Core] {
        const GBA: [Core; 2] = [Core::Mgba, Core::Gpsp];
        const MGBA: [Core; 1] = [Core::Mgba];
        const FCEUMM: [Core; 1] = [Core::Nes];
        /// SFC 默认 snes9x；supafaust 为可选核心（用户自备，见 README）。
        const SFC: [Core; 2] = [Core::Snes9x, Core::Snes];
        const GENESIS: [Core; 1] = [Core::Gpgx];
        match platform {
            Platform::Gba => &GBA,
            Platform::Gb | Platform::Gbc => &MGBA,
            Platform::Fc => &FCEUMM,
            Platform::Sfc => &SFC,
            Platform::Md => &GENESIS,
        }
    }

    /// 核心二进制是否在卡上（`System/linkSlot/cores/<核心>/<dylib>_libretro.so`）。
    pub fn is_available(self, root: &Path) -> bool {
        root.join(slot_model::layout::SYSTEM_CORES)
            .join(self.as_str())
            .join(format!("{}_libretro.{}", self.as_str(), std::env::consts::DLL_EXTENSION))
            .is_file()
    }

    /// 动态库文件名前缀（`System/<as_str>_libretro.so`）。
    pub fn as_str(self) -> &'static str {
        self.spec().dylib
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn text(self) -> &'static str {
        self.spec().display
    }

    /// Short name for the PCB socket/chip, where the full display name does
    /// not fit inside the cavity.
    pub fn chip_text(self) -> &'static str {
        self.spec().chip
    }

    pub fn parse(s: &str) -> Option<Core> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mgba" => Some(Core::Mgba),
            "gpsp" => Some(Core::Gpsp),
            "fceumm" => Some(Core::Nes),
            "supafaust" | "snes" => Some(Core::Snes),
            "snes9x" | "snes9x_libretro" => Some(Core::Snes9x),
            "genesis_plus_gx" | "genesisplusgx" | "gpgx" | "genplusgx" => Some(Core::Gpgx),
            _ => None,
        }
    }

    /// Whether this core can run a cart of `platform`（数据来自 `slot-model`）。
    pub fn runs(self, platform: Platform) -> bool {
        self.spec()
            .machines
            .iter()
            .any(|m| *m == platform.machine().id)
    }

    pub fn default_for(platform: Platform) -> Core {
        Core::parse(platform.machine().default_core).unwrap_or_default()
    }
}

pub fn read_selected_cores(root: &Path) -> HashMap<String, Core> {
    crate::ini::read(root, SELECTED_CORE_FILE)
        .into_iter()
        .filter_map(|(stem, name)| Core::parse(&name).map(|core| (stem, core)))
        .collect()
}

pub fn core_for(root: &Path, stem: &str) -> Core {
    crate::ini::value(root, SELECTED_CORE_FILE, stem)
        .as_deref()
        .and_then(Core::parse)
        .unwrap_or_default()
}

pub fn core_for_platform(root: &Path, stem: &str, platform: Platform) -> Core {
    let recorded = crate::ini::value(root, SELECTED_CORE_FILE, stem)
        .as_deref()
        .and_then(Core::parse)
        .filter(|core| core.runs(platform));
    // 记录的核心可能已不在卡上（例如老游戏记录着 supafaust，而包只带 snes9x）：
    // 这时回退到机种默认核心，避免"一启动就报缺核心"。
    match recorded {
        Some(core) if core.is_available(root) => core,
        _ => Core::default_for(platform),
    }
}

/// 选择器里实际可用的核心：只保留 `System/<dylib>_libretro.so` 存在的条目。
///
/// 这样"把核心丢进 System/"就等于把它加进了选择器，不需要改配置；全部缺失时
/// 仍返回默认核心，保证 UI 不为空。
pub fn available_options(root: &Path, platform: Platform) -> Vec<Core> {
    let mut out: Vec<Core> = Core::options_for(platform)
        .iter()
        .copied()
        .filter(|core| core.is_available(root))
        .collect();
    if out.is_empty() {
        out.push(Core::default_for(platform));
    }
    out
}

pub fn write_selected_core(root: &Path, stem: &str, core: Core) -> std::io::Result<()> {
    crate::ini::write(root, SELECTED_CORE_FILE, stem, core.as_str())
}
