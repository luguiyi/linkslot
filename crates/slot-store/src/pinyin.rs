//! Canonical, tone-stripped pinyin readings for CJK characters.
//!
//! The table is generated from mozillazg/pinyin-data (MIT) by
//! `tools/gen_pinyin_full.py`; it covers U+3400..U+9FFF and stores all readings
//! for a character as a comma-separated ASCII string.

const START: u32 = 0x3400;
const END: u32 = 0xA000;
pub const CHARACTER_COUNT: usize = (END - START) as usize;
const DATA: &[u8] = include_bytes!("pinyin_full.bin");

fn count() -> usize {
    u32::from_le_bytes([DATA[0], DATA[1], DATA[2], DATA[3]]) as usize
}

fn blob_base() -> usize {
    4 + count() * 4
}

fn offset(i: usize) -> usize {
    let at = 4 + i * 4;
    u32::from_le_bytes([DATA[at], DATA[at + 1], DATA[at + 2], DATA[at + 3]]) as usize
}

/// All tone-stripped readings for `ch`, comma-separated. Empty when the
/// character is outside the table or has no recorded reading.
pub fn readings(ch: char) -> &'static str {
    let cp = ch as u32;
    if !(START..END).contains(&cp) {
        return "";
    }
    let i = (cp - START) as usize;
    let base = blob_base();
    let start = base + offset(i);
    let end = if i + 1 < count() {
        base + offset(i + 1)
    } else {
        DATA.len()
    };
    let end = end.saturating_sub(1); // trailing NUL
    std::str::from_utf8(&DATA[start..end]).unwrap_or("")
}

/// The first reading for `ch`, or `None` when the character has no reading.
pub fn first_reading(ch: char) -> Option<&'static str> {
    readings(ch).split(',').next().filter(|s| !s.is_empty())
}

/// All distinct readings for each CJK character in `text`. Non-CJK characters
/// are returned unchanged (lower-cased) so Latin title components still work.
pub fn readings_for_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase().to_string());
        } else if ch.is_ascii_whitespace() {
            continue;
        } else {
            for reading in readings(ch).split(',').filter(|r| !r.is_empty()) {
                out.push(reading.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_characters_have_their_full_readings() {
        assert!(readings('宝').split(',').any(|r| r == "bao"));
        assert!(readings('可').split(',').any(|r| r == "ke"));
        assert!(readings('梦').split(',').any(|r| r == "meng"));
    }

    #[test]
    fn polyphonic_characters_keep_all_readings() {
        let readings = readings('行');
        assert!(readings.split(',').any(|r| r == "xing"));
        assert!(readings.split(',').any(|r| r == "hang"));
    }

    #[test]
    fn latin_and_missing_characters_fall_back_safely() {
        assert_eq!(readings('A'), "");
        assert_eq!(first_reading('A'), None);
        assert_eq!(readings_for_text("A宝"), ["a", "bao"]);
    }

    #[test]
    fn the_table_covers_the_expected_cjk_range() {
        assert_eq!(count(), CHARACTER_COUNT);
        assert!(DATA.len() > blob_base());
        assert!(!first_reading('中').unwrap().is_empty());
    }
}
