//! `Core` 与 `slot-model::CORES` 的一致性护栏（P3-08）。
//!
//! 枚举保留是为了 `match` 的穷尽检查；真正的身份/能力数据在核心表里。
//! 这些测试保证两边不会漂移。

use slot_model::{core, CORES, MACHINES};
use slot_store::{Core, Platform};

#[test]
fn every_variant_resolves_to_a_spec() {
    for c in Core::EVERY {
        let spec = core(c.id()).unwrap_or_else(|| panic!("{c:?} 不在 CORES 里"));
        assert_eq!(c.as_str(), spec.dylib, "{c:?} 的 dylib 与表不一致");
        assert_eq!(c.text(), spec.display, "{c:?} 的显示名与表不一致");
        assert_eq!(c.chip_text(), spec.chip, "{c:?} 的芯片名与表不一致");
    }
}

#[test]
fn the_table_has_no_orphan_cores() {
    for spec in CORES {
        assert!(
            Core::EVERY.iter().any(|c| c.id() == spec.id.0),
            "核心表里的 {} 没有对应的 Core 变体",
            spec.id.0
        );
    }
    assert_eq!(CORES.len(), Core::EVERY.len());
}

#[test]
fn runs_matches_the_table() {
    for c in Core::EVERY {
        for p in Platform::ALL {
            let expected = core(c.id())
                .expect("spec")
                .machines
                .iter()
                .any(|m| *m == p.machine().id);
            assert_eq!(c.runs(p), expected, "{c:?} × {p:?}");
        }
    }
}

#[test]
fn every_machine_has_a_runnable_default_core() {
    for p in Platform::ALL {
        let default = Core::default_for(p);
        assert!(default.runs(p), "{p:?} 的默认核心 {default:?} 跑不了它");
        assert!(
            core(default.id()).is_some(),
            "{p:?} 的默认核心不在核心表里"
        );
    }
}

#[test]
fn picker_options_are_runnable_and_never_empty() {
    for p in Platform::ALL {
        let options = Core::options_for(p);
        assert!(!options.is_empty(), "{p:?} 没有可选核心");
        for c in options {
            assert!(c.runs(p), "{p:?} 的选择器里出现了跑不了它的 {c:?}");
        }
    }
}

#[test]
fn every_shipped_machine_is_covered_by_some_core() {
    for m in MACHINES {
        let covered = CORES.iter().any(|c| c.machines.contains(&m.id));
        assert!(covered, "机种 {} 没有任何核心能跑", m.dir);
    }
}
