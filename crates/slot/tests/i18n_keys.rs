//! 护栏：代码里引用的每个 i18n key 都必须在所有语言里存在。
//!
//! 设备上曾把 `device.app.common.delete` 原样显示成 `DEVICE.APP.COMMON.DE`
//! （存档相册的 Y 删除提示），因为代码引用了、翻译里没有。这个测试扫描
//! Rust 源码与 Web 静态文件里的 key 字面量，逐个查表，漏翻就失败。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use slot_i18n::Language;

/// 这些前缀后面还要拼变量（`device.app.{view}`），不是完整 key。
const DYNAMIC_PREFIXES: [&str; 3] = ["device.app.", "device.quick.", "device.toast."];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn candidate(text: &str) -> bool {
    let rest = match text {
        _ if text.starts_with("device.") => &text["device.".len()..],
        _ if text.starts_with("web_api.") => &text["web_api.".len()..],
        _ if text.starts_with("web.") => &text["web.".len()..],
        _ if text.starts_with("play.") => &text["play.".len()..],
        _ if text.starts_with("mods.") => &text["mods.".len()..],
        _ => return false,
    };
    if rest.is_empty() || DYNAMIC_PREFIXES.contains(&text) {
        return false;
    }
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn scan(text: &str, out: &mut BTreeSet<String>) {
    for quote in ['"', '\''] {
        for (index, piece) in text.split(quote).enumerate() {
            if index % 2 == 1 && candidate(piece) {
                out.insert(piece.to_string());
            }
        }
    }
}

fn collect(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect(&path, out);
            continue;
        }
        let relevant = path.extension().is_some_and(|ext| ext == "rs")
            || path
                .file_name()
                .is_some_and(|name| name == "app.js" || name == "index.html");
        if !relevant {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            scan(&text, out);
        }
    }
}

#[test]
fn every_referenced_key_exists_in_every_language() {
    let mut keys = BTreeSet::new();
    collect(&repo_root().join("crates"), &mut keys);
    assert!(keys.len() > 500, "扫描到的 key 太少，路径可能不对：{}", keys.len());

    let mut missing = Vec::new();
    for key in &keys {
        for lang in Language::ALL {
            if slot_i18n::t_opt(lang, key).is_none() {
                missing.push(format!("{key} ({})", lang.code()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "以下 key 被引用但没有翻译：\n{}",
        missing.join("\n")
    );
}
