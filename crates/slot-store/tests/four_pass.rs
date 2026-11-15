use slot_store::{read_slot_state, write_slot_state, Shader, SlotState};

#[test]
fn four_pass_survives_restart_without_changing_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
    let mut state = SlotState::default();
    state.shader_gba = Shader::FourPass;
    state.shader_gb = Shader::FourPass;
    state.volume = 37;
    write_slot_state(dir.path(), &state).unwrap();
    assert_eq!(read_slot_state(dir.path()), state);
}

#[test]
fn old_shader_keys_remain_compatible() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
    // The parser deliberately reads a state file that misses any required key
    // as default (see `a_slot_state_missing_a_key_reads_as_default_not_half_populated`),
    // so this case has to spell out the rest of the file to reach the shaders.
    std::fs::write(dir.path().join("linkSlot/Config/slot.state"),
        "cart=\nbrightness=5\nblue_light=0\nvolume=37\nmuted=0\nclock_set=1\nutc_offset_min=0\nshader_gba=grid\nshader_gb=simpletex\n").unwrap();
    let state = read_slot_state(dir.path());
    assert_eq!(state.shader_gba, Shader::Grid);
    assert_eq!(state.shader_gb, Shader::Simpletex);
    assert_eq!(state.volume, 37);
}
