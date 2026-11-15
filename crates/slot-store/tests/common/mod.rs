use tempfile::TempDir;

pub fn tmp_root() -> TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in [
        "Games",
        "Games/GBA",
        "Games/GB",
        "Games/GBC",
        "Games/FC",
        "Labels",
        "Labels/FC",
        "Saves",
        "Saves/FC",
        "States",
        "States/FC",
        "System",
        "Config",
        "linkSlot/Config",
    ] {
        std::fs::create_dir_all(d.path().join(sub)).expect("create content dir");
    }
    d
}
