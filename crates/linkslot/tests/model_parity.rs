//! 护栏：`linkslot` 的机种/核心清单必须与 `slot-model` 完全一致（P5-08）。
//!
//! 云端布局、缓存 key、`States/<platform>/<core>/` 都由这组清单派生；
//! 一旦两边漂移，某机种的存档就会"界面上正常、实际上不同步"。

use slot_model::{MACHINES, CORES};

#[test]
fn linkslot_platforms_are_the_model_machines() {
    let linked: Vec<&str> = linkslot::unit::platform_all().collect();
    let modelled: Vec<&str> = MACHINES.iter().map(|m| m.dir).collect();
    assert_eq!(linked, modelled, "linkslot 机种清单与 slot-model 漂移");
}

#[test]
fn linkslot_emulators_are_the_model_cores() {
    let linked: Vec<&str> = linkslot::unit::emulators().collect();
    let modelled: Vec<&str> = CORES.iter().map(|c| c.id.0).collect();
    assert_eq!(linked, modelled, "linkslot 核心清单与 slot-model 漂移");
}

#[test]
fn every_linkable_core_machine_pair_is_accepted_by_validation() {
    for spec in CORES {
        for machine in spec.machines {
            linkslot::unit::validate_emulator(machine.0, spec.id.0)
                .unwrap_or_else(|e| panic!("{}/{} 被拒绝：{e:?}", spec.id.0, machine.0));
        }
    }
}

#[test]
fn a_core_is_refused_on_a_machine_it_does_not_declare() {
    // mGBA 不能跑 FC；fceumm 不能跑 GBA。这两个断言钉住"跨机种误同步"。
    assert!(linkslot::unit::validate_emulator("FC", "mgba").is_err());
    assert!(linkslot::unit::validate_emulator("GBA", "fceumm").is_err());
}

#[test]
fn rom_extensions_come_from_the_machine_table() {
    for machine in MACHINES {
        assert_eq!(
            linkslot::cli::rom_extensions(machine.dir),
            machine.rom_ext,
            "{} 的扩展名与 slot-model 不一致",
            machine.dir
        );
    }
    // 未知（且大小写不符）的机种不再猜测扩展名。
    assert!(linkslot::cli::rom_extensions("NES").is_empty());
}
