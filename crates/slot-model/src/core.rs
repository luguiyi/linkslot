//! 核心表：5 个 libretro 核心的元数据。
//!
//! 只放**静态事实**：id、动态库名、显示名、芯片短名、可运行的机种、固定选项、
//! 颜色校正键名、是否支持连机。依赖运行时上下文的部分（调色板、BIOS、序列号、
//! 连机玩家号）仍由 `slot/src/core.rs` 按上下文补齐。

use crate::machine::MachineId;

/// 核心 id：与 `System/<id>_libretro.so`、`States/<platform>/<id>/` 一致。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CoreId(pub &'static str);

impl CoreId {
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// libretro 的颜色校正选项（键名与开/关取值每个核心都不一样）。
#[derive(Clone, Copy, Debug)]
pub struct ColourSpec {
    pub key: &'static str,
    pub on: &'static str,
    pub off: &'static str,
}

/// 一个核心的全部静态元数据。
#[derive(Debug)]
pub struct CoreSpec {
    pub id: CoreId,
    /// 动态库文件名前缀（`System/<dylib>_libretro.so`）。
    pub dylib: &'static str,
    /// 显示名（设置页 / PCB 大字号）。
    pub display: &'static str,
    /// PCB 插槽里的短名。
    pub chip: &'static str,
    /// 可运行的机种。
    pub machines: &'static [MachineId],
    /// 是否接受 `<dylib>_frameskip = auto`（未声明该选项的核心会告警）。
    pub frameskip: bool,
    /// 是否支持 mGBA 的连机选项。
    pub link: bool,
    /// 与上下文无关的固定选项（libretro 键名 + 值）。
    pub fixed_options: &'static [(&'static str, &'static str)],
    pub colour: Option<ColourSpec>,
}

pub const MGBA: CoreSpec = CoreSpec {
    id: CoreId("mgba"),
    dylib: "mgba",
    display: "mGBA",
    chip: "mGBA",
    machines: &[MachineId("GBA"), MachineId("GB"), MachineId("GBC")],
    frameskip: true,
    link: true,
    fixed_options: &[("mgba_sgb_borders", "OFF"), ("mgba_force_gbp", "ON")],
    colour: Some(ColourSpec {
        key: "mgba_color_correction",
        on: "Auto",
        off: "OFF",
    }),
};

pub const GPSP: CoreSpec = CoreSpec {
    id: CoreId("gpsp"),
    dylib: "gpsp",
    display: "gpSP",
    chip: "gpSP",
    machines: &[MachineId("GBA")],
    frameskip: true,
    link: false,
    // 序列号与 BIOS 启动模式依赖运行时上下文，见 slot/src/core.rs。
    fixed_options: &[("gpsp_rtc_time_source", "system")],
    colour: Some(ColourSpec {
        key: "gpsp_color_correction",
        on: "enabled",
        off: "disabled",
    }),
};

pub const FCEUMM: CoreSpec = CoreSpec {
    id: CoreId("fceumm"),
    dylib: "fceumm",
    display: "fceumm",
    chip: "fceumm",
    machines: &[MachineId("FC")],
    frameskip: false,
    link: false,
    // overscan 的值是像素数：fceumm 用 atoi，写 "enabled" 会被当成 0。
    fixed_options: &[
        ("fceumm_overscan_v_top", "8"),
        ("fceumm_overscan_v_bottom", "8"),
        ("fceumm_turbo_enable", "Player 1"),
    ],
    colour: None,
};

/// Super Famicom / SNES 的官方随包核心：GPL-2.0，构建时自编译（glibc ≤ 2.31）。
pub const SNES9X: CoreSpec = CoreSpec {
    id: CoreId("snes9x"),
    dylib: "snes9x",
    display: "Snes9x",
    chip: "snes9x",
    machines: &[MachineId("SFC")],
    frameskip: false,
    link: false,
    fixed_options: &[],
    colour: None,
};

pub const SUPAFAUST: CoreSpec = CoreSpec {
    id: CoreId("supafaust"),
    dylib: "supafaust",
    display: "supafaust",
    chip: "supafaust",
    machines: &[MachineId("SFC")],
    frameskip: false,
    link: false,
    fixed_options: &[],
    colour: None,
};

pub const GENESIS: CoreSpec = CoreSpec {
    id: CoreId("genesis_plus_gx"),
    dylib: "genesis_plus_gx",
    display: "Genesis Plus GX",
    chip: "GPGX",
    machines: &[MachineId("MD")],
    frameskip: false,
    link: false,
    fixed_options: &[
        ("genesis_plus_gx_overscan", "disabled"),
        ("genesis_plus_gx_render", "single field"),
    ],
    colour: None,
};

/// 核心表。顺序用于稳定输出（不是落盘契约）。
pub const CORES: &[CoreSpec] = &[MGBA, GPSP, FCEUMM, SNES9X, SUPAFAUST, GENESIS];

pub fn core(id: &str) -> Option<&'static CoreSpec> {
    CORES.iter().find(|c| c.id.0 == id)
}

/// 能运行某机种的核心（表顺序）。
pub fn cores_for(machine: MachineId) -> impl Iterator<Item = &'static CoreSpec> {
    CORES
        .iter()
        .filter(move |c| c.machines.contains(&machine))
}

/// 所有核心 id（表顺序）。
pub fn core_ids() -> impl Iterator<Item = &'static str> {
    CORES.iter().map(|c| c.id.0)
}
