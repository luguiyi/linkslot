//! linkSlot 领域契约（L1）。
//!
//! 这里只放**纯数据与派生函数**：机种表、核心表、存档单元布局、协议常量。
//! 不读文件、不碰环境、不依赖任何 crate —— 这样 `slot-store`、`slot-ui`、
//! `linkslot` 都能共用同一份事实，而不必各自维护影子表。

pub mod core;
pub mod layout;
pub mod machine;
pub mod power;

pub use core::{core, core_ids, cores_for, ColourSpec, CoreId, CoreSpec, CORES};
pub use layout::{resolve_root, DEVICE_ROOT};
pub use power::{Battery, Charge};
pub use machine::{
    by_dir, by_id, machine, Machine, MachineArt, MachineId, Platform, QuickSwitch, MACHINES,
};
