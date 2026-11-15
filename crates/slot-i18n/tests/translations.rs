//! Shape and parity checks for `translations/`.
//!
//! 这些测试按 `DOMAINS × Language::ALL` 展开，不再手写 12 个文件条目；
//! 新增语言或域时，测试自动覆盖，不必改这个文件。

use std::collections::{BTreeMap, BTreeSet};

use slot_i18n::{placeholders, Language};

struct File {
    domain: &'static str,
    language: Language,
    raw: &'static str,
}

fn files() -> Vec<File> {
    slot_i18n::domains()
        .map(|(domain, language, raw)| File {
            domain,
            language,
            raw,
        })
        .collect()
}

/// domain → 该域的每个语言文件（按 `Language::ALL` 顺序）。
fn by_domain() -> BTreeMap<&'static str, Vec<File>> {
    let mut out: BTreeMap<&'static str, Vec<File>> = BTreeMap::new();
    for file in files() {
        out.entry(file.domain).or_default().push(file);
    }
    for (domain, list) in &mut out {
        list.sort_by_key(|f| f.language);
        let langs: Vec<Language> = list.iter().map(|f| f.language).collect();
        let mut want = Language::ALL.to_vec();
        want.sort();
        assert_eq!(
            langs, want,
            "{domain} 没有覆盖每一种语言（新增语言时漏了文件）"
        );
    }
    out
}

fn strings(file: &File) -> BTreeMap<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(file.raw)
        .unwrap_or_else(|error| panic!("{} {}: {error}", file.domain, file.language.code()));
    let schema = parsed["schema"].as_u64();
    assert_eq!(
        schema,
        Some(1),
        "{} {} must declare schema 1",
        file.domain,
        file.language.code()
    );
    assert_eq!(
        parsed["language"].as_str(),
        Some(file.language.code()),
        "{} declares the wrong language",
        file.domain
    );
    let map = parsed["strings"]
        .as_object()
        .unwrap_or_else(|| panic!("{} has no strings object", file.domain));
    assert!(!map.is_empty(), "{} has no strings", file.domain);
    map.iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("{} {key} is not a string", file.domain));
            assert!(!value.trim().is_empty(), "{} {key} is empty", file.domain);
            (key.clone(), value.to_string())
        })
        .collect()
}

#[test]
fn files_are_utf8_without_a_bom_and_use_lf() {
    for file in files() {
        assert!(
            !file.raw.starts_with('\u{feff}'),
            "{} {} starts with a BOM",
            file.domain,
            file.language.code()
        );
        assert!(
            !file.raw.contains('\r'),
            "{} {} uses CRLF",
            file.domain,
            file.language.code()
        );
        assert!(
            file.raw.ends_with('\n'),
            "{} {} has no final newline",
            file.domain,
            file.language.code()
        );
    }
}

#[test]
fn every_language_of_a_domain_carries_the_same_keys() {
    for (domain, list) in by_domain() {
        let reference = strings(&list[0]);
        for file in &list[1..] {
            let other = strings(file);
            let reference_keys: BTreeSet<&String> = reference.keys().collect();
            let other_keys: BTreeSet<&String> = other.keys().collect();
            let missing: Vec<&&String> = reference_keys.difference(&other_keys).collect();
            let extra: Vec<&&String> = other_keys.difference(&reference_keys).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{domain}: {} 缺 {:?}，多 {:?}",
                file.language.code(),
                missing,
                extra
            );
        }
    }
}

#[test]
fn placeholders_match_across_languages() {
    for (domain, list) in by_domain() {
        let reference = strings(&list[0]);
        for file in &list[1..] {
            let other = strings(file);
            for (key, text) in &reference {
                let Some(other_text) = other.get(key) else {
                    continue; // covered by the key-set test
                };
                assert_eq!(
                    placeholders(text),
                    placeholders(other_text),
                    "{domain} {key} 在 {} 的占位符不一致",
                    file.language.code()
                );
            }
        }
    }
}

#[test]
fn no_key_is_defined_in_two_domains() {
    let mut seen: BTreeMap<String, &'static str> = BTreeMap::new();
    // 每个域只看第一个语言：键集一致已由上一个测试保证。
    for (domain, list) in by_domain() {
        for key in strings(&list[0]).keys() {
            if let Some(previous) = seen.insert(key.clone(), domain) {
                panic!("{key} is defined in both {previous} and {domain}");
            }
        }
    }
}

#[test]
fn every_key_is_prefixed_with_its_domain() {
    fn prefixes(domain: &'static str) -> &'static [&'static str] {
        match domain {
            "device-quick" => &["device.quick.", "device.toast."],
            "device-app" => &["device.app."],
            "web-api" => &["web_api."],
            "web" => &["web."],
            "play" => &["play."],
            "mods" => &["mods."],
            other => panic!("no prefix rule for {other}"),
        }
    }
    for file in files() {
        let want = prefixes(file.domain);
        for key in strings(&file).keys() {
            assert!(
                want.iter().any(|prefix| key.starts_with(prefix)),
                "{}: {key} is not under {want:?}",
                file.domain
            );
        }
    }
}

#[test]
fn english_tables_do_not_leak_chinese() {
    fn is_cjk(c: char) -> bool {
        ('\u{3000}'..='\u{303f}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
    }
    for file in files().iter().filter(|f| f.language == Language::EnUs) {
        for (key, value) in strings(file) {
            if key == "device.quick.value.language_zh" {
                continue;
            }
            assert!(
                !value.chars().any(is_cjk),
                "{} {key} still contains Chinese: {value}",
                file.domain
            );
        }
    }
}

#[test]
fn the_runtime_tables_agree_with_the_files() {
    for file in files() {
        for (key, value) in strings(&file) {
            let key: &'static str = Box::leak(key.into_boxed_str());
            assert_eq!(
                slot_i18n::t_opt(file.language, key),
                Some(value.as_str()),
                "{key} differs at runtime for {}",
                file.language.code()
            );
        }
    }
}

#[test]
fn the_bilingual_language_row_renders_in_both_tables() {
    for lang in Language::ALL {
        assert_eq!(
            slot_i18n::t(lang, "device.quick.value.language_zh"),
            "中文",
            "the Chinese entry keeps its own name in every language"
        );
        assert_eq!(slot_i18n::t(lang, "device.quick.value.language_en"), "English");
    }
    assert_eq!(slot_i18n::t(Language::ZhCn, "device.quick.label.language"), "语言");
    assert_eq!(
        slot_i18n::t(Language::EnUs, "device.quick.label.language"),
        "Language"
    );
}

/// 加一种语言只需要「一个枚举变体 + 每域一个文件」：这里把这条契约钉死。
#[test]
fn every_domain_covers_every_language() {
    let domains = by_domain();
    assert!(!domains.is_empty());
    for (domain, list) in domains {
        assert_eq!(
            list.len(),
            Language::ALL.len(),
            "{domain} 的语言文件数与 Language::ALL 不一致"
        );
    }
}
