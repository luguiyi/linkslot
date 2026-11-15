//! 骨架一致性：`build/skeleton.txt` 必须与运行时 `root::DIRS` 完全一致。
//!
//! 打包（`build/dist.sh`）用 skeleton.txt 建目录，运行时用 DIRS；
//! 两份定义一旦分叉，空卡第一次开机就会缺少某个目录。这里把它钉死。

#[test]
fn skeleton_file_matches_runtime_dirs() {
    let raw = include_str!("../../../build/skeleton.txt");
    let actual: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let expected: Vec<&str> = slot::root::DIRS.to_vec();
    assert_eq!(
        actual, expected,
        "build/skeleton.txt 与 crates/slot/src/root.rs 的 DIRS 不一致；\
         运行 python build/gen-skeleton.py 重新生成"
    );
}
