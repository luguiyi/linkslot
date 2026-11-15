use std::path::PathBuf;

use slot_model::{resolve_root, DEVICE_ROOT};

#[test]
fn explicit_root_wins_over_the_environment() {
    assert_eq!(
        resolve_root(Some(PathBuf::from("/explicit")), Some(PathBuf::from("/env"))),
        PathBuf::from("/explicit")
    );
}

#[test]
fn environment_is_used_when_no_explicit_root() {
    assert_eq!(
        resolve_root(None, Some(PathBuf::from("/env"))),
        PathBuf::from("/env")
    );
}

#[test]
fn the_device_root_is_the_last_resort() {
    assert_eq!(resolve_root(None, None), PathBuf::from(DEVICE_ROOT));
    assert_eq!(DEVICE_ROOT, "/mnt/sdcard");
}
