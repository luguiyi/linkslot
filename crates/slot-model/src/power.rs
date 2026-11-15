//! 电池与充电状态（纯数据）。
//!
//! 这些类型原先定义在 `slot-power`，但表现层也要画电池图标；把它下沉到契约层，
//! 让 `slot-ui` 不必依赖硬件电源实现。

/// 充电状态。
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Charge {
    Unknown,
    Discharging,
    Charging,
    Full,
}

/// 一次电量读数。
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Battery {
    pub percent: u8,
    pub charge: Charge,
}

impl Battery {
    /// 是否正在充电（`Charging` 与 `Full` 都算插着电）。
    pub fn on_charger(self) -> bool {
        matches!(self.charge, Charge::Charging | Charge::Full)
    }
}
