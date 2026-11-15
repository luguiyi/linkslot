use std::collections::BTreeMap;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cheat { pub description: String, pub code: String, pub enabled: bool }
pub fn parse(text: &str) -> Vec<Cheat> {
    let mut fields: BTreeMap<usize, (Cheat,bool)> = BTreeMap::new();
    for line in text.lines() {
        let Some((key,value))=line.split_once('=') else {continue};
        let Some(key)=key.trim().strip_prefix("cheat") else {continue};
        let Some((number,field))=key.split_once('_') else {continue};
        let Ok(index)=number.parse::<usize>() else {continue};
        if index>=4096 {continue;}
        let value=value.trim().trim_matches('"');
        let entry=fields.entry(index).or_default();
        match field {
            "desc" => entry.0.description=value.to_string(),
            "code" => entry.0.code=value.to_string(),
            "handler" if value!="0" => entry.1=true,
            _ => {}
        }
    }
    fields.into_values().filter_map(|(mut c,unsupported)| {
        if unsupported || c.code.is_empty() || c.code.len()>1048576 || !c.code.chars().all(|ch| ch.is_ascii_hexdigit() || ch.is_ascii_whitespace() || "+-:".contains(ch)) {return None;}
        if c.description.is_empty() {c.description="未命名金手指".into();}
        Some(c)
    }).collect()
}
pub fn lid_expired(minutes: u8, elapsed_ms: u64) -> bool {
    minutes != 0 && elapsed_ms >= u64::from(minutes)*60_000
}
