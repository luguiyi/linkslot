mod common;

use std::path::Path;

use common::{boot, tmp_root_with_carts};
use slot_input::Action;

fn write_playtime(root: &Path, core: &str, game: &str, body: &str) {
    let dir = root.join("States").join("GBA").join(core).join(game);
    std::fs::create_dir_all(&dir).expect("create playtime unit");
    std::fs::write(dir.join("playtime.json"), body).expect("write playtime");
}

#[test]
fn the_shelf_shows_the_merged_total_for_the_selected_cart() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_playtime(
        d.path(),
        "mgba",
        "Emerald",
        r#"{"schema":1,"seconds":60,"total_seconds":3661}"#,
    );
    let mut a = boot(d.path());
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "游戏时长 1 小时 1 分");
}

#[test]
fn a_missing_file_hides_the_label() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = boot(d.path());
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "");
}

#[test]
fn a_zero_or_malformed_file_hides_the_label() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_playtime(d.path(), "mgba", "Emerald", r#"{"schema":1}"#);
    let mut a = boot(d.path());
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "");

    write_playtime(d.path(), "mgba", "Emerald", "{not json");
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "");
}

#[test]
fn the_label_refreshes_after_leaving_the_shelf() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_playtime(
        d.path(),
        "mgba",
        "Emerald",
        r#"{"schema":1,"seconds":60,"total_seconds":60}"#,
    );
    let mut a = boot(d.path());
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "游戏时长 1 分钟");

    write_playtime(
        d.path(),
        "mgba",
        "Emerald",
        r#"{"schema":1,"seconds":7200,"total_seconds":7200}"#,
    );
    a.apply(Action::LidClose);
    a.apply(Action::LidOpen);
    a.update(1.0 / 60.0);
    assert_eq!(a.shelf_playtime_text(), "游戏时长 2 小时");
}
