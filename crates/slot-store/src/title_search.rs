/// First significant character; Chinese uses the dictionary's primary reading.
pub fn initial(name: &str) -> Option<char> {
    const PINYIN: &[u8] = include_bytes!("pinyin_initials.bin");
    let ch = name.chars().find(|c| c.is_alphanumeric())?;
    if ch.is_ascii_alphabetic() { return Some(ch.to_ascii_uppercase()); }
    let cp = ch as usize;
    if (0x3400..0xa000).contains(&cp) {
        let value = PINYIN[cp - 0x3400];
        if value.is_ascii_uppercase() { return Some(value as char); }
    }
    None
}
