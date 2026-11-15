//! Runtime translations for slot / linkSlot.
//!
//! The tables live in `translations/` as one JSON file per domain and language,
//! so a domain can be edited without touching the others.  Every file is
//! embedded with [`include_str!`]: a release card never depends on external
//! translation files, and a missing file is a compile error rather than a blank
//! label on the device.
//!
//! Lookup rules, in order:
//!
//! 1. the requested language;
//! 2. Simplified Chinese, the source language;
//! 3. the key itself.
//!
//! Rule 3 exists so a half-finished domain still renders something a developer
//! can recognise.  `tests/translations.rs` fails when a key is only known to one
//! language, so rule 3 never reaches a release build by accident.
//!
//! This crate deliberately depends on no other project crate: `slot-store`,
//! `slot-ui` and `slot` all depend on it, and a cycle would be fatal.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A language the frontend can render.
///
/// Not a boolean on purpose: adding `JaJp` later means one more variant, one
/// more table and one more code, not a rewrite of every call site.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    /// Simplified Chinese, the source language and the fallback.
    #[default]
    ZhCn,
    /// English (United States).
    EnUs,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::ZhCn, Language::EnUs];

    /// The IETF tag written to `linkSlot/Config/language.txt` and returned by the Web
    /// API.  This is the canonical spelling; [`Language::parse`] accepts more.
    pub fn code(self) -> &'static str {
        match self {
            Language::ZhCn => "zh-CN",
            Language::EnUs => "en-US",
        }
    }

    /// The language's own name, shown in the menu that selects it.
    ///
    /// Never translated: whoever is looking at the language row must be able to
    /// read the entry that gets them back to a language they understand.
    pub fn menu_label(self) -> &'static str {
        match self {
            Language::ZhCn => "中文",
            Language::EnUs => "English",
        }
    }

    /// Parses the codes the config file, the Web page or a shell may carry.
    ///
    /// Accepts `zh`, `zh-CN`, `zh_CN`, `zh-Hans`, `en`, `en-US`, `en_US`,
    /// case-insensitively, with surrounding whitespace.  Anything else is
    /// `None`; callers fall back to [`Language::default`].
    pub fn parse(raw: &str) -> Option<Self> {
        let normalised = raw.trim().to_ascii_lowercase().replace('_', "-");
        match normalised.as_str() {
            "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" | "cn" => Some(Language::ZhCn),
            "en" | "en-us" | "en-gb" | "en-latn-us" => Some(Language::EnUs),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Language::ZhCn => 0,
            Language::EnUs => 1,
        }
    }

    pub fn at(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    /// The neighbour in the menu.  Left and right both wrap, so a two-language
    /// list can never get stuck.
    pub fn next(self, right: bool) -> Self {
        let len = Self::ALL.len();
        if right {
            Self::at((self.index() + 1) % len)
        } else {
            Self::at((self.index() + len - 1) % len)
        }
    }
}

/// One translation domain: a name, plus one embedded file per language.
struct Domain {
    name: &'static str,
    files: &'static [(Language, &'static str)],
}

/// The files embedded into the binary.
///
/// 新增一种语言 = 加一个枚举变体 + 每域一个 `(Language, include_str!)` 条目；
/// 键集一致性由 `tests/translations.rs` 按 `DOMAINS × Language::ALL` 校验。
const DOMAINS: [Domain; 6] = [
    Domain {
        name: "device-quick",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/device-quick.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/device-quick.en-US.json"),
            ),
        ],
    },
    Domain {
        name: "device-app",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/device-app.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/device-app.en-US.json"),
            ),
        ],
    },
    Domain {
        name: "web",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/web.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/web.en-US.json"),
            ),
        ],
    },
    Domain {
        name: "play",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/play.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/play.en-US.json"),
            ),
        ],
    },
    Domain {
        name: "web-api",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/web-api.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/web-api.en-US.json"),
            ),
        ],
    },
    Domain {
        name: "mods",
        files: &[
            (
                Language::ZhCn,
                include_str!("../../../translations/mods.zh-CN.json"),
            ),
            (
                Language::EnUs,
                include_str!("../../../translations/mods.en-US.json"),
            ),
        ],
    },
];

/// The parsed tables: one key/value map per language.
struct Tables(HashMap<Language, HashMap<&'static str, &'static str>>);

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut tables: HashMap<Language, HashMap<&'static str, &'static str>> = HashMap::new();
        for domain in &DOMAINS {
            for &(language, raw) in domain.files {
                let table = tables.entry(language).or_default();
                load(table, raw, domain.name, language);
            }
        }
        Tables(tables)
    })
}

/// 遍历所有「域 × 语言 × 内嵌原文」，供测试与诊断使用。
///
/// 新增语言时，测试会自动覆盖 `Language::ALL` 的每个组合，不需要手改列表。
pub fn domains() -> impl Iterator<Item = (&'static str, Language, &'static str)> {
    DOMAINS
        .iter()
        .flat_map(|domain| domain.files.iter().map(move |&(language, raw)| (domain.name, language, raw)))
}

/// Parses one embedded file into `into`.
///
/// The strings are leaked once: the tables live for the whole process, so
/// `&'static str` costs one allocation per key and saves every caller a
/// `String`.  A malformed file is reported and skipped rather than panicking in
/// front of the user — `tests/translations.rs` is what keeps them well-formed.
fn load(
    into: &mut HashMap<&'static str, &'static str>,
    raw: &'static str,
    domain: &str,
    language: Language,
) {
    let parsed: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("slot: i18n: {domain} {} is not valid JSON: {error}", language.code());
            return;
        }
    };
    let declared = parsed.get("language").and_then(|v| v.as_str());
    if declared != Some(language.code()) {
        eprintln!(
            "slot: i18n: {domain} {} declares language {declared:?}",
            language.code()
        );
    }
    let Some(strings) = parsed.get("strings").and_then(|v| v.as_object()) else {
        eprintln!("slot: i18n: {domain} {} has no strings", language.code());
        return;
    };
    for (key, value) in strings {
        let Some(value) = value.as_str() else {
            eprintln!("slot: i18n: {domain} {key} is not a string");
            continue;
        };
        let key: &'static str = Box::leak(key.clone().into_boxed_str());
        let value: &'static str = Box::leak(value.to_string().into_boxed_str());
        if into.insert(key, value).is_some() {
            eprintln!("slot: i18n: {key} is defined twice, keeping the first value");
        }
    }
}

/// Looks `key` up without the "return the key" last resort.
///
/// The requested language wins; Chinese is the fallback so an untranslated key
/// still reads as prose rather than as an identifier.
pub fn t_opt(lang: Language, key: &str) -> Option<&'static str> {
    let tables = tables();
    let own = tables.0.get(&lang).and_then(|table| table.get(key).copied());
    own.or_else(|| {
        tables
            .0
            .get(&Language::ZhCn)
            .and_then(|table| table.get(key).copied())
    })
}

/// The translation of `key`, or `key` itself when no table knows it.
///
/// `key` is `&'static str` on purpose: keys are written at the call site, and
/// returning the key lets the third rule stay allocation-free.
pub fn t(lang: Language, key: &'static str) -> &'static str {
    t_opt(lang, key).unwrap_or(key)
}

/// [`t`] with `{name}` placeholders replaced.
///
/// Placeholders are named rather than positional so the parity test can compare
/// the two languages' sets without caring about order.
pub fn t_args(lang: Language, key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = t(lang, key).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// Every `{name}` a template uses, deduplicated and sorted.
///
/// Shared with the parity test, which requires both languages of a key to use
/// exactly the same placeholders.
pub fn placeholders(template: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        let name = &after[..close];
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !found.iter().any(|seen| seen == name)
        {
            found.push(name.to_string());
        }
        rest = &after[close + 1..];
    }
    found.sort();
    found
}

/// Every key/value pair known for `lang`, sorted by key.
///
/// The Web API ships this to the browser, so its order must be stable across
/// builds.
pub fn entries(lang: Language) -> Vec<(&'static str, &'static str)> {
    let tables = tables();
    let mut out: Vec<(&'static str, &'static str)> = tables
        .0
        .get(&lang)
        .map(|table| table.iter().map(|(key, value)| (*key, *value)).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

/// How many strings `lang` has.  Used by the API status line and tests.
pub fn len(lang: Language) -> usize {
    entries(lang).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_round_trip() {
        for lang in Language::ALL {
            assert_eq!(Language::parse(lang.code()), Some(lang), "{lang:?}");
        }
    }

    #[test]
    fn every_spelling_of_a_language_parses() {
        for raw in ["zh", "zh-CN", "zh_CN", "ZH-cn", " zh ", "zh-Hans"] {
            assert_eq!(Language::parse(raw), Some(Language::ZhCn), "{raw:?}");
        }
        for raw in ["en", "en-US", "en_US", "EN-us", " en"] {
            assert_eq!(Language::parse(raw), Some(Language::EnUs), "{raw:?}");
        }
    }

    #[test]
    fn unknown_and_empty_values_do_not_parse() {
        for raw in ["", "  ", "fr-FR", "zh-Hant", "english", "en-US-x"] {
            assert_eq!(Language::parse(raw), None, "{raw:?}");
        }
        assert_eq!(Language::parse("nonsense").unwrap_or_default(), Language::ZhCn);
    }

    #[test]
    fn next_wraps_in_both_directions() {
        assert_eq!(Language::ZhCn.next(true), Language::EnUs);
        assert_eq!(Language::EnUs.next(true), Language::ZhCn);
        assert_eq!(Language::ZhCn.next(false), Language::EnUs);
        assert_eq!(Language::EnUs.next(false), Language::ZhCn);
        assert_eq!(Language::default(), Language::ZhCn);
    }

    #[test]
    fn a_missing_key_falls_back_to_the_key_itself() {
        assert_eq!(t(Language::EnUs, "no.such.key"), "no.such.key");
        assert_eq!(t(Language::ZhCn, "no.such.key"), "no.such.key");
        assert_eq!(t_opt(Language::EnUs, "no.such.key"), None);
    }

    #[test]
    fn chinese_is_the_fallback_when_a_translation_is_missing() {
        // `device.quick.label.language` exists in both tables today, so this
        // exercises the lookup order rather than a real gap.
        assert_eq!(t(Language::ZhCn, "device.quick.label.language"), "语言");
        assert_eq!(t(Language::EnUs, "device.quick.label.language"), "Language");
    }

    #[test]
    fn placeholders_are_found_once_and_sorted() {
        assert_eq!(placeholders("no names here"), Vec::<String>::new());
        assert_eq!(
            placeholders("{version} then {name} then {version}"),
            vec!["name".to_string(), "version".to_string()]
        );
        // Not placeholders: empty, spaced, or unterminated.
        assert_eq!(placeholders("{} {a b} {open"), Vec::<String>::new());
    }

    #[test]
    fn args_replace_named_placeholders() {
        let text = t_args(
            Language::EnUs,
            "device.app.language.switched",
            &[("name", "English")],
        );
        assert_eq!(text, "Switched to English");
        let text = t_args(
            Language::ZhCn,
            "device.app.language.switched",
            &[("name", "English")],
        );
        assert_eq!(text, "已切换为 English");
    }

    #[test]
    fn entries_are_sorted_and_cover_every_domain() {
        let zh = entries(Language::ZhCn);
        assert!(!zh.is_empty());
        let mut sorted = zh.clone();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(zh, sorted);
        assert_eq!(len(Language::ZhCn), zh.len());
        // At least one key from each domain is present, so a domain that is not
        // embedded at all cannot pass unnoticed.
        for prefix in [
            "device.quick.",
            "device.app.",
            "web.",
            "play.",
            "web_api.",
            "mods.",
        ] {
            assert!(
                zh.iter().any(|(key, _)| key.starts_with(prefix)),
                "no {prefix} key in the Chinese tables"
            );
        }
    }
}
