//! Pinyin-to-Chinese candidates and cross-platform game lookup for the shelf
//! search overlay.  Candidates are built from the installed game library, so a
//! newly scanned game is searchable without shipping a word dictionary.

use slot_store::{pinyin, Cart, Platform};

/// One page of IME candidates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CandidatePage {
    pub items: Vec<String>,
    pub page: usize,
    pub pages: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub shelf: usize,
    pub index: usize,
    pub platform: Platform,
    pub label: String,
}

#[derive(Clone, Debug)]
struct Term {
    text: String,
    pinyin: String,
    initials: String,
    kind: u8, // 0 full title, 1 word, 2 single character
    count: u32,
}

#[derive(Clone, Debug)]
struct Char {
    text: String,
    readings: Vec<String>,
    count: u32,
}

#[derive(Clone, Debug, Default)]
pub struct GameSearchIndex {
    terms: Vec<Term>,
    chars: Vec<Char>,
    carts: Vec<(usize, usize, Platform, String, String)>,
}

pub const CANDIDATES_PER_PAGE: usize = 8;

impl GameSearchIndex {
    pub fn build<'a>(
        carts: impl Iterator<Item = (usize, usize, Platform, &'a Cart)>,
    ) -> GameSearchIndex {
        let mut index = GameSearchIndex::default();
        let mut seen_terms: Vec<(String, usize)> = Vec::new();
        let mut seen_chars: Vec<(String, usize)> = Vec::new();
        for (shelf, number, platform, cart) in carts {
            index.carts.push((
                shelf,
                number,
                platform,
                cart.stem.clone(),
                cart.title.clone(),
            ));
            let mut labels = vec![cart.stem.as_str()];
            if !cart.title.is_empty() && cart.title != cart.stem {
                labels.push(cart.title.as_str());
            }
            for label in labels {
                index.add_term(&mut seen_terms, label, 0);
                for word in split_words(label) {
                    index.add_term(&mut seen_terms, word, 1);
                }
                for ch in label.chars() {
                    index.add_char(&mut seen_chars, ch);
                }
            }
        }
        index
    }

    fn add_term(&mut self, seen: &mut Vec<(String, usize)>, text: &str, kind: u8) {
        let Some((full, initials)) = pinyin_pair(text) else {
            return;
        };
        if let Some((_, at)) = seen.iter().find(|(known, _)| known == text) {
            self.terms[*at].count += 1;
            self.terms[*at].kind = self.terms[*at].kind.min(kind);
            return;
        }
        seen.push((text.to_string(), self.terms.len()));
        self.terms.push(Term {
            text: text.to_string(),
            pinyin: full,
            initials,
            kind,
            count: 1,
        });
    }

    fn add_char(&mut self, seen: &mut Vec<(String, usize)>, ch: char) {
        if !is_cjk(ch) {
            return;
        }
        let readings: Vec<String> = pinyin::readings(ch)
            .split(',')
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect();
        if readings.is_empty() {
            return;
        }
        let text = ch.to_string();
        if let Some((_, at)) = seen.iter().find(|(known, _)| known == &text) {
            self.chars[*at].count += 1;
            return;
        }
        seen.push((text.clone(), self.chars.len()));
        self.chars.push(Char {
            text,
            readings,
            count: 1,
        });
    }

    pub fn candidates(&self, composition: &str, page: usize, per_page: usize) -> CandidatePage {
        let composition = normalise_composition(composition);
        if composition.is_empty() {
            return CandidatePage {
                items: Vec::new(),
                page: 0,
                pages: 1,
            };
        }
        let mut matches: Vec<(u8, u32, usize, String)> = Vec::new();
        for term in &self.terms {
            if term.pinyin.starts_with(&composition) || term.initials.starts_with(&composition) {
                matches.push((term.kind, term.count, term.text.len(), term.text.clone()));
            }
        }
        for ch in &self.chars {
            if ch.readings.iter().any(|r| r.starts_with(&composition)) {
                matches.push((2, ch.count, ch.text.len(), ch.text.clone()));
            }
        }
        matches.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(&b.3))
        });
        let mut items: Vec<String> = Vec::new();
        for (_, _, _, text) in matches {
            if !items.iter().any(|known| known == &text) {
                items.push(text);
            }
        }
        let per_page = per_page.max(1);
        let pages = items.len().div_ceil(per_page).max(1);
        let page = page.min(pages - 1);
        let start = page * per_page;
        let end = (start + per_page).min(items.len());
        CandidatePage {
            items: items[start..end].to_vec(),
            page,
            pages,
        }
    }

    pub fn hits(&self, query: &str) -> Vec<SearchHit> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut seen = Vec::new();
        for (shelf, index, platform, label, title) in &self.carts {
            let lower = label.to_lowercase();
            let title = title.to_lowercase();
            if (lower.contains(&query) || title.contains(&query)) && !seen.contains(&(shelf, index)) {
                seen.push((shelf, index));
                hits.push(SearchHit {
                    shelf: *shelf,
                    index: *index,
                    platform: *platform,
                    label: label.clone(),
                });
            }
        }
        hits
    }
}

fn pinyin_pair(text: &str) -> Option<(String, String)> {
    let mut full = String::new();
    let mut initials = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            let lower = ch.to_ascii_lowercase();
            full.push(lower);
            initials.push(lower);
        } else if ch.is_ascii_whitespace() || ch.is_ascii_punctuation() || ch == '　' {
            continue;
        } else if is_cjk(ch) {
            let reading = pinyin::first_reading(ch)?;
            full.push_str(reading);
            initials.push(reading.chars().next()?);
        } else {
            continue;
        }
    }
    if full.is_empty() {
        None
    } else {
        Some((full, initials))
    }
}

fn split_words(label: &str) -> Vec<&str> {
    label
        .split(|c: char| c.is_whitespace() || is_punct(c))
        .filter(|word| {
            let mut chars = word.chars();
            chars.next().is_some() && chars.next().is_some()
        })
        .collect()
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '、' | '。' | '，' | '：' | '；' | '！' | '？' | '（' | '）' | '【' | '】' | '·'
        )
}

fn is_cjk(ch: char) -> bool {
    (0x3400..0xA000).contains(&(ch as u32))
}

fn normalise_composition(text: &str) -> String {
    text.chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphabetic() {
                Some(ch.to_ascii_lowercase())
            } else if ch == 'ü' || ch == 'v' {
                Some('v')
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cart(stem: &str, title: &str) -> Cart {
        Cart {
            platform: Platform::Gba,
            stem: stem.into(),
            rom: PathBuf::from(format!("Games/GBA/{stem}.gba")),
            label: None,
            title: title.into(),
            code: String::new(),
            shell: None,
        model: Default::default(),
        front: None,
}
    }

    fn built(names: &[(&str, &str)]) -> GameSearchIndex {
        let carts: Vec<Cart> = names.iter().map(|(stem, title)| cart(stem, title)).collect();
        GameSearchIndex::build(
            carts
                .iter()
                .enumerate()
                .map(|(i, cart)| (i / 2, i, cart.platform, cart)),
        )
    }

    #[test]
    fn full_pinyin_and_initials_find_chinese_titles() {
        let index = built(&[("宝可梦 红宝石", "POKEMON RUBY"), ("三国演义", "")]);
        let full = index.candidates("baokemeng", 0, 8);
        assert!(full.items.iter().any(|item| item.contains("宝可梦")), "{:?}", full.items);
        let initials = index.candidates("bkm", 0, 8);
        assert!(initials.items.iter().any(|item| item.contains("宝可梦")), "{:?}", initials.items);
        let sgyy = index.candidates("sgyy", 0, 8);
        assert!(sgyy.items.iter().any(|item| item.contains("三国演义")), "{:?}", sgyy.items);
    }

    #[test]
    fn chinese_and_english_queries_produce_cross_platform_hits() {
        let index = built(&[("宝可梦 红宝石", "POKEMON RUBY"), ("Metroid Fusion", "")]);
        let hits = index.hits("宝可梦");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "宝可梦 红宝石");
        let hits = index.hits("metroid");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn rebuilding_the_index_picks_up_a_new_game() {
        let old = built(&[("宝可梦", "")]);
        assert!(old.candidates("sanguo", 0, 8).items.is_empty());
        let new = built(&[("宝可梦", ""), ("三国演义", "")]);
        assert!(new
            .candidates("sanguo", 0, 8)
            .items
            .iter()
            .any(|item| item.contains("三国")));
    }

    #[test]
    fn candidate_pages_are_bounded_and_stable() {
        let index = built(&[("宝可梦", ""), ("宝可梦 红宝石", ""), ("宝可梦 绿宝石", "")]);
        let page = index.candidates("bao", 0, 2);
        assert_eq!(page.items.len(), 2);
        assert!(page.pages >= 1);
        let second = index.candidates("bao", 1, 2);
        assert!(second.page <= page.pages - 1);
    }
}

