use slot_model::{by_dir, by_id, machine, MachineId, Platform, MACHINES};

#[test]
fn machine_ids_and_dirs_are_unique() {
    for (i, a) in MACHINES.iter().enumerate() {
        for b in &MACHINES[i + 1..] {
            assert_ne!(a.id, b.id, "duplicate machine id {:?}", a.id);
            assert_ne!(a.dir, b.dir, "duplicate machine dir {}", a.dir);
        }
    }
}

#[test]
fn platform_all_matches_the_table_in_order() {
    assert_eq!(Platform::ALL.len(), MACHINES.len());
    for (i, platform) in Platform::ALL.iter().enumerate() {
        assert_eq!(platform.index(), i, "{platform:?} 槽位漂移");
        assert_eq!(
            platform.machine().id,
            MACHINES[i].id,
            "Platform::ALL 顺序必须与 MACHINES 一致"
        );
    }
    // 历史落盘槽位（slot.state 旧格式）不可重排。
    assert_eq!(Platform::Fc.index(), 3);
}

#[test]
fn legacy_index_values_are_pinned() {
    assert_eq!(Platform::Gba.index(), 0);
    assert_eq!(Platform::Gb.index(), 1);
    assert_eq!(Platform::Gbc.index(), 2);
    assert_eq!(Platform::Fc.index(), 3);
    assert_eq!(Platform::Sfc.index(), 4);
    assert_eq!(Platform::Md.index(), 5);
}

#[test]
fn every_machine_has_a_usable_default_core() {
    for m in MACHINES {
        assert!(!m.dir.is_empty(), "{} has empty dir", m.id.0);
        assert!(
            m.cores.contains(&m.default_core),
            "{} 的默认核心 {} 不在 cores 列表里",
            m.id.0,
            m.default_core
        );
        assert!(!m.rom_ext.is_empty(), "{} has no rom extension", m.id.0);
        assert!(m.canvas.0 > 0 && m.canvas.1 > 0, "{} canvas", m.id.0);
    }
}

#[test]
fn extensions_are_lowercase_and_never_contain_a_dot() {
    for m in MACHINES {
        for ext in m.rom_ext {
            assert_eq!(*ext, ext.to_ascii_lowercase(), "{ext} 不是小写");
            assert!(!ext.starts_with('.'), "{ext} 不应带点");
        }
    }
}

#[test]
fn lookup_helpers_round_trip() {
    for p in Platform::ALL {
        let m = machine(p);
        assert_eq!(by_id(m.id).map(|x| x.id), Some(m.id));
        assert_eq!(by_dir(m.dir).map(|x| x.id), Some(m.id));
        assert_eq!(by_dir(&m.dir.to_ascii_lowercase()).map(|x| x.id), Some(m.id));
    }
    assert!(by_id(MachineId("NOPE")).is_none());
    assert!(by_dir("nope").is_none());
}

#[test]
fn accepts_matches_extension_case_insensitively() {
    use std::path::Path;
    assert!(Platform::Gba.accepts(Path::new("Game.GBA")));
    assert!(Platform::Fc.accepts(Path::new("mario.nes")));
    assert!(Platform::Md.accepts(Path::new("sonic.bin")));
    assert!(!Platform::Gba.accepts(Path::new("mario.nes")));
    assert!(!Platform::Gba.accepts(Path::new("no_extension")));
}

#[test]
fn all_rom_ext_is_deduplicated_in_table_order() {
    let ext: Vec<&str> = slot_model::machine::all_rom_ext().collect();
    assert_eq!(ext.first(), Some(&"gba"));
    let mut sorted = ext.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ext.len(), "扩展名重复：{ext:?}");
}
